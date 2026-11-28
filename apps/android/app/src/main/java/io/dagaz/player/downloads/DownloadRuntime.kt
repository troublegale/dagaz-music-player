package io.dagaz.player.downloads

import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.*
import androidx.media3.datasource.cache.*
import androidx.media3.exoplayer.offline.*
import androidx.media3.exoplayer.scheduler.Requirements
import io.dagaz.player.api.*
import io.dagaz.player.playback.MediaTransport
import io.dagaz.player.playback.MediaValidationException
import kotlinx.coroutines.*
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

// DownloadService retains one manager per service class, including across server changes.
@UnstableApi
internal object DownloadRuntime {
    private var provider:StandaloneDatabaseProvider?=null
    private var stored:SimpleCache?=null
    private var manager:DownloadManager?=null
    private val io=CoroutineScope(SupervisorJob()+Dispatchers.IO+CoroutineExceptionHandler{_,error->android.util.Log.e("DagazDownloads","Download state update failed; candidate retained for recovery",error)})
    internal fun verifyCompleted(repo:DownloadRepository,media:MediaDescriptor){io.launch{repo.verify(media)}}
    @Synchronized fun cache(context:Context):SimpleCache {
        if(stored==null){provider=StandaloneDatabaseProvider(context.applicationContext);stored=SimpleCache(File(context.noBackupFilesDir,"explicit-cache"),NoOpCacheEvictor(),provider!!)}
        return stored!!
    }
    fun downloads(context:Context):DownloadManager {
        check(android.os.Looper.myLooper()==android.os.Looper.getMainLooper())
        manager?.let{return it}
        val cache=cache(context);val index=DefaultDownloadIndex(provider!!,"shared")
        val upstream=DataSource.Factory { object:DataSource {
            private var source:DataSource?=null;private val listeners=mutableListOf<TransferListener>()
            override fun addTransferListener(listener:TransferListener){listeners.add(listener);source?.addTransferListener(listener)}
            override fun open(spec:DataSpec):Long {
                val request=index.getDownload(spec.key?:throw MediaValidationException())?.request?:throw MediaValidationException()
                val media=Wire.json.decodeFromString<MediaDescriptor>(request.data.toString(Charsets.UTF_8))
                if(media.cacheKey!=spec.key||media.url!=spec.uri.toString())throw MediaValidationException()
                val descriptors=ConcurrentHashMap<String,MediaDescriptor>().apply{put(media.url,media)}
                val selected=MediaTransport(context,descriptors,ConcurrentHashMap()).networkFactory().createDataSource();source=selected;listeners.forEach(selected::addTransferListener)
                return selected.open(spec)
            }
            override fun read(buffer:ByteArray,offset:Int,length:Int)=source?.read(buffer,offset,length)?:throw MediaValidationException()
            override fun getUri()=source?.uri
            override fun getResponseHeaders()=source?.responseHeaders?:emptyMap()
            override fun close(){source?.close();source=null}
        }}
        val factory=DefaultDownloaderFactory(CacheDataSource.Factory().setCache(cache).setUpstreamDataSourceFactory(upstream),Executors.newFixedThreadPool(2))
        return DownloadManager(context.applicationContext,index,DownloaderFactory{request->RetryingDownloader(factory.createDownloader(request))}).also{downloads->
            manager=downloads;downloads.setRequirements(Requirements(Requirements.NETWORK));downloads.maxParallelDownloads=2;downloads.minRetryCount=0
            downloads.addListener(object:DownloadManager.Listener {
                override fun onDownloadChanged(manager:DownloadManager,download:Download,finalException:Exception?) {
                    if(download.state!=Download.STATE_COMPLETED&&download.state!=Download.STATE_FAILED)return
                    val media=Wire.json.decodeFromString<MediaDescriptor>(download.request.data.toString(Charsets.UTF_8))
                    DownloadRepository.forMedia(media)?.let{repo->io.launch{if(download.state==Download.STATE_COMPLETED)repo.verify(media)else repo.failed(media)}}
                }
                override fun onInitialized(manager:DownloadManager){io.launch{DownloadRepository.openRepositories().forEach{it.reconcile()}}}
            })
        }
    }
}
