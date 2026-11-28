package io.dagaz.player.downloads

import android.content.Context
import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.cache.*
import androidx.media3.exoplayer.offline.*
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.playback.StreamCache
import io.dagaz.player.sync.*
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.encodeToString
import java.security.MessageDigest
import java.util.UUID

@UnstableApi
class DownloadRepository private constructor(private val context:Context,initialSession:Session) {
    @Volatile var session:Session=initialSession
        private set
    val scope=LocalDatabase.scope(session)
    val db=LocalDatabase.open(context,scope)
    val dao=db.dao()
    val cache=DownloadRuntime.cache(context)
    private val lock=Mutex()
    companion object {
        private val stores=mutableMapOf<String,DownloadRepository>()
        @Synchronized internal fun openRepositories()=stores.values.toList()
        @Synchronized internal fun forMedia(media:MediaDescriptor)=stores.values.firstOrNull{media.url.startsWith(it.session.baseUrl.trimEnd('/')+"/api/v1/")}
        @Synchronized fun get(context:Context,session:Session):DownloadRepository=stores.getOrPut(LocalDatabase.scope(session)){DownloadRepository(context.applicationContext,session)}.also{it.session=session}
        suspend fun current(context:Context):DownloadRepository?=withContext(Dispatchers.IO){Credentials(context).load()?.let{get(context,it)}}
    }
    private fun decode(json:String)=Wire.json.decodeFromString<MediaDescriptor>(json)
    fun ready(track:String):MediaDescriptor?=dao.media(track)?.ready?.let(::decode)
    fun candidate(track:String):MediaDescriptor?=dao.media(track)?.candidate?.let(::decode)
    suspend fun enqueue(kind:SelectionKind,id:UUID,quality:Quality,label:String,force:Boolean=false):String=withContext(Dispatchers.IO) {
        val local=LocalSelection(UUID.randomUUID().toString(),kind.name,id.toString(),quality.name,label,force)
        dao.selection(local);schedule();local.id
    }
    fun schedule() {
        val work=androidx.work.OneTimeWorkRequestBuilder<ManifestWorker>()
            .setInputData(androidx.work.workDataOf("scope" to scope))
            .setConstraints(androidx.work.Constraints.Builder().setRequiredNetworkType(androidx.work.NetworkType.CONNECTED).build()).build()
        androidx.work.WorkManager.getInstance(context).enqueueUniqueWork("downloads-$scope",androidx.work.ExistingWorkPolicy.APPEND_OR_REPLACE,work)
    }
    suspend fun prepare()=withContext(Dispatchers.IO){lock.withLock {
        val api=DagazApi(session.baseUrl)
        for(initial in dao.selections().filter{!it.complete}) {
            var selection=initial
            if(selection.manifest==null){val created=api.manifest(session,ManifestCreate(SelectionKind.valueOf(selection.kind),UUID.fromString(selection.target),Quality.valueOf(selection.quality)),selection.operation);selection=selection.copy(manifest=created.id.toString());dao.selection(selection)}
            var position=selection.position
            do {
                val page=api.manifestPage(session,UUID.fromString(selection.manifest),selection.cursor)
                val resolved=page.items.map{media->val existing=ready(media.trackId.toString())
                    if(selection.refreshRevision&&!selection.force&&existing!=null&&reuseDownload(existing.effectiveQuality,media.requestedQuality)&&existing.effectiveQuality!=media.effectiveQuality)api.media(session,media.trackId,media.revision,existing.effectiveQuality)else media
                }
                val tracks=resolved.map{it.trackId.toString()}.distinct().mapNotNull{id->dao.record("track",id)?.let{Wire.json.decodeFromString<Track>(it.payload)}}
                CatalogSync(context,db,session).covers(tracks)
                require(selection.revision==null||selection.revision==page.selectionRevision)
                // Pins and candidates commit together, so a restart can replay the same manifest page.
                db.runInTransaction {
                    for(media in resolved) {
                        validate(media)
                        val track=media.trackId.toString();dao.pendingPin(PendingPin(selection.id,position++,track))
                        val row=dao.media(track)?:LocalMedia(track)
                        val existing=row.ready?.let(::decode)
                        val pending=row.candidate?.let(::decode)
                        val reuse=existing!=null&&(!selection.refreshRevision||(existing.revision==media.revision&&existing.sha256==media.sha256&&row.readyGeneration==dao.sync()?.generation))&&reuseDownload(existing.effectiveQuality,media.requestedQuality,selection.force)
                        if(!reuse&&(pending==null||selection.force||selection.refreshRevision||!reuseDownload(pending.effectiveQuality,media.requestedQuality)))dao.media(row.copy(candidate=Wire.json.encodeToString(media),candidateGeneration=dao.sync()?.generation,error=null))
                    }
                    selection=selection.copy(cursor=page.nextCursor,revision=page.selectionRevision,complete=page.nextCursor==null,error=null,position=position);dao.selection(selection)
                    if(selection.complete){dao.deletePins(selection.id);dao.commitPins(selection.id);dao.deletePendingPins(selection.id)}
                }
                queueCandidates()
            }while(!selection.complete)
        }
        queueCandidates()
        collectUnused()
    }}
    private fun validate(media:MediaDescriptor) {
        val expected=ServerAddress.parse(session.baseUrl).newBuilder().addPathSegments("api/v1/tracks/${media.trackId}/media").addQueryParameter("revision",media.revision.toString()).addQueryParameter("quality",media.effectiveQuality.name).build().toString()
        if(media.url!=expected||media.byteLength<=0||!media.sha256.matches(Regex("[a-f0-9]{64}"))||!media.etag.startsWith('"')||!media.etag.endsWith('"')||(media.effectiveQuality!=Quality.original&&media.effectiveQuality!=media.requestedQuality))throw ClientException(Failure.InvalidResponse)
    }
    private suspend fun queueCandidates() {
        for(row in dao.candidates())row.candidate?.let { json ->
            val media=decode(json);validate(media)
            val available=context.noBackupFilesDir.usableSpace
            val missing=(media.byteLength-cache.getCachedBytes(media.cacheKey,0,media.byteLength)).coerceAtLeast(0)
            if(missing+16L*1024*1024>available){dao.media(row.copy(error="Недостаточно места. Освободи память и повтори скачивание."));return@let}
            withContext(Dispatchers.Main.immediate){
                val downloads=downloads()
                val stored=downloads.downloadIndex.getDownload(media.cacheKey)
                if(stored?.state==Download.STATE_COMPLETED)DownloadRuntime.verifyCompleted(this@DownloadRepository,media)
                else if(stored==null||stored.state==Download.STATE_FAILED||stored.state==Download.STATE_STOPPED)downloads.addDownload(DownloadRequest.Builder(media.cacheKey,android.net.Uri.parse(media.url)).setCustomCacheKey(media.cacheKey).setData(json.toByteArray()).build())
                downloads.resumeDownloads()
            }
        }
    }
    fun downloads():DownloadManager=DownloadRuntime.downloads(context)
    suspend fun reconcile() {
        val candidates=withContext(Dispatchers.IO){dao.candidates().mapNotNull{it.candidate?.let(::decode)}}
        if(candidates.isEmpty())return
        val completed=withContext(Dispatchers.Main.immediate){candidates.filter{downloads().downloadIndex.getDownload(it.cacheKey)?.state==Download.STATE_COMPLETED}}
        completed.forEach{verify(it)}
    }
    internal suspend fun failed(media:MediaDescriptor)=withContext(Dispatchers.IO){lock.withLock {
        dao.media(media.trackId.toString())?.let{if(it.candidate?.let(::decode)?.cacheKey==media.cacheKey)dao.media(it.copy(error="Скачивание прервано. Повторить"))}
    }}
    suspend fun verify(media:MediaDescriptor)=withContext(Dispatchers.IO){lock.withLock {
        val row=dao.media(media.trackId.toString())?:return@withLock
        if(row.candidate?.let(::decode)?.cacheKey!=media.cacheKey)return@withLock
        val verified=try {
            val source=CacheDataSource.Factory().setCache(cache).setUpstreamDataSourceFactory(null).createDataSource()
            val hash=MessageDigest.getInstance("SHA-256");var length=0L
            try{source.open(DataSpec.Builder().setUri(media.url).setKey(media.cacheKey).setLength(media.byteLength).build());val bytes=ByteArray(65536)
                while(true){val count=source.read(bytes,0,bytes.size);if(count==C.RESULT_END_OF_INPUT)break;hash.update(bytes,0,count);length+=count}
            }finally{source.close()}
            length==media.byteLength&&hash.digest().joinToString(""){"%02x".format(it)}==media.sha256
        }catch(error:CancellationException){throw error}catch(_:Exception){false}
        if(!verified){dao.media(row.copy(candidate=null,error="Файл не прошёл проверку. Повтори скачивание."));removeTransfer(media.cacheKey);return@withLock}
        // Publish only the verified representation; the previous one survives every failed update.
        var published=false
        db.runInTransaction{val current=dao.media(row.track)
            if(current?.candidate?.let(::decode)?.cacheKey==media.cacheKey){dao.media(current.copy(ready=Wire.json.encodeToString(media),readyGeneration=current.candidateGeneration,candidate=null,candidateGeneration=null,error=null));published=true}
        }
        if(!published){if(ready(row.track)?.cacheKey!=media.cacheKey)removeTransfer(media.cacheKey);return@withLock}
        StreamCache.markExplicit(context,media.cacheKey)
        row.ready?.let(::decode)?.takeIf{it.cacheKey!=media.cacheKey}?.let{StreamCache.unmarkExplicit(context,it.cacheKey);removeTransfer(it.cacheKey)}
    }}
    private suspend fun removeTransfer(key:String)=withContext(Dispatchers.Main.immediate){downloads().removeDownload(key)}
    suspend fun removeSelection(id:String)=withContext(Dispatchers.IO){lock.withLock {
        val tracks=dao.selectionTracks(id)
        db.runInTransaction{dao.deletePins(id);dao.deletePendingPins(id);dao.deleteSelection(id)}
        for(track in tracks)if(dao.pinCount(track)==0){val row=dao.media(track);dao.deleteMedia(track);listOfNotNull(row?.ready,row?.candidate).map(::decode).map{it.cacheKey}.distinct().forEach{StreamCache.unmarkExplicit(context,it);removeTransfer(it)}}
    }}
    suspend fun refresh(id:String,quality:Quality?=null)=withContext(Dispatchers.IO){lock.withLock {
        val old=dao.selection(id)?:return@withLock
        // A fresh selection keeps the old pins until the replacement snapshot has been acquired.
        db.runInTransaction{dao.deletePendingPins(id);dao.selection(old.copy(quality=quality?.name?:old.quality,force=quality!=null,operation=UUID.randomUUID().toString(),manifest=null,cursor=null,revision=null,complete=false,error=null,position=0,refreshRevision=true))}
        schedule()
    }}
    private suspend fun collectUnused() {
        for(row in dao.media())if(dao.pinCount(row.track)==0){row.ready?.let(::decode)?.let{StreamCache.unmarkExplicit(context,it.cacheKey)};dao.deleteMedia(row.track)}
        val keep=dao.media().flatMap{listOfNotNull(it.ready,it.candidate)}.map{decode(it).cacheKey}.toSet()
        withContext(Dispatchers.Main.immediate){val unused=mutableListOf<String>();downloads().downloadIndex.getDownloads().use{cursor->while(cursor.moveToNext()){val key=cursor.download.request.id;if(key.startsWith(session.baseUrl.trimEnd('/')+"/api/v1/")&&key !in keep)unused.add(key)}};unused.forEach{downloads().removeDownload(it)}}
    }
    suspend fun cleanup()=withContext(Dispatchers.IO){lock.withLock{collectUnused()}}
}
