package io.dagaz.player

import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.downloads.*
import io.dagaz.player.playback.MediaTransport
import io.dagaz.player.sync.*
import kotlinx.coroutines.*
import okhttp3.mockwebserver.*
import okio.Buffer
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

@UnstableApi
@RunWith(AndroidJUnit4::class)
class OfflineTest {
    @Test fun syncCoalescesRepeatedPlaylistUpdates()=runBlocking {
        val context=ApplicationProvider.getApplicationContext<Context>();val id=UUID.randomUUID();val track=UUID.randomUUID();val generation=UUID.randomUUID()
        val latest=Playlist(id,3,"Последняя версия",listOf(track),totalCount=1);val malformed=java.util.concurrent.atomic.AtomicBoolean(false);val artist=UUID.randomUUID()
        val changes=listOf(Change(1,Entity.playlist,id,2,false,Wire.json.encodeToJsonElement(Playlist.serializer(),latest.copy(revision=2))),Change(2,Entity.playlist,id,3,false,Wire.json.encodeToJsonElement(Playlist.serializer(),latest)))
        MockWebServer().use{server->
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse=when {
                request.path?.startsWith("/api/v1/snapshot")==true->MockResponse().setBody(Wire.json.encodeToString(SnapshotPage.serializer(),SnapshotPage(generation,0,emptyList())))
                request.path?.startsWith("/api/v1/changes")==true->MockResponse().setBody(Wire.json.encodeToString(SyncPage.serializer(),if(malformed.get())SyncPage(generation,listOf(Change(3,Entity.artist,artist,1,false,Wire.json.encodeToJsonElement(Artist.serializer(),Artist(UUID.randomUUID(),"Wrong ID")))),"$generation:3",false)else SyncPage(generation,changes,"$generation:2",false)))
                request.path?.startsWith("/api/v1/playlists/")==true->MockResponse().setBody(Wire.json.encodeToString(Playlist.serializer(),latest))
                else->MockResponse().setResponseCode(404)
            }}
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val session=Session(server.url("/").newBuilder().host("127.0.0.1").build().toString(),UUID.randomUUID(),"a".repeat(64));val db=LocalDatabase.open(context,LocalDatabase.scope(session))
            try{val sync=CatalogSync(context,db,session);sync.refresh();sync.refresh()
                withContext(Dispatchers.IO){assertEquals(3L,Wire.json.decodeFromString<Playlist>(db.dao().record("playlist",id.toString())!!.payload).revision)
                    db.openHelper.readableDatabase.query("SELECT track FROM members WHERE playlist=? AND staged=0",arrayOf(id.toString())).use{cursor->assertEquals(1,cursor.count);assertTrue(cursor.moveToFirst());assertEquals(track.toString(),cursor.getString(0))}
                }
                malformed.set(true)
                try{sync.refresh();fail("Mismatched payload identity accepted")}catch(error:ClientException){assertEquals(Failure.InvalidResponse,error.failure)}
                withContext(Dispatchers.IO){assertNull(db.dao().record("artist",artist.toString()));assertEquals("$generation:2",db.dao().sync()!!.cursor)}
            }finally{db.close();context.deleteDatabase("catalog-${LocalDatabase.scope(session)}.db")}
        }
    }
    @Test fun foregroundDownloadIndexIncludesTheNewServer()=runBlocking {
        val context=ApplicationProvider.getApplicationContext<Context>()
        val one=DownloadRepository.get(context,Session("https://one-${UUID.randomUUID()}.invalid/",UUID.randomUUID(),"a".repeat(64)))
        val two=DownloadRepository.get(context,Session("https://two-${UUID.randomUUID()}.invalid/",UUID.randomUUID(),"b".repeat(64)))
        val id=UUID.randomUUID();val media=MediaDescriptor(id,1,Quality.original,Quality.original,100,"a".repeat(64),"\"tag\"",two.session.baseUrl+"api/v1/tracks/$id/media?revision=1&quality=original")
        try {
            withContext(Dispatchers.Main){two.downloads().addDownload(androidx.media3.exoplayer.offline.DownloadRequest.Builder(media.cacheKey,android.net.Uri.parse(media.url)).setCustomCacheKey(media.cacheKey).setData(Wire.json.encodeToString(MediaDescriptor.serializer(),media).toByteArray()).build(),1)}
            withTimeout(3000){while(withContext(Dispatchers.Main){two.downloads().downloadIndex.getDownload(media.cacheKey)}==null)delay(25)}
            assertNotNull("The foreground service must retain the new server's transfer index",withContext(Dispatchers.Main){one.downloads().downloadIndex.getDownload(media.cacheKey)})
        }finally{withContext(Dispatchers.Main){two.downloads().removeDownload(media.cacheKey)}}
    }
    @Test fun manifestPageRetryKeepsOldSnapshotAndDuplicates()=runBlocking {
        val context=ApplicationProvider.getApplicationContext<Context>();val file=File(context.noBackupFilesDir,"connection.json");requireEmptyConnectionFixture(file)
        val bytes=androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().context.assets.open("vbr.mp3").use{it.readBytes()}
        val sha=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString(""){"%02x".format(it)}
        val id=UUID.randomUUID();val old=UUID.randomUUID();val local=UUID.randomUUID().toString();val manifest=UUID.randomUUID();val creates=AtomicInteger();val pending=java.util.concurrent.atomic.AtomicBoolean(true)
        MockWebServer().use{server->
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0);val base=server.url("/").newBuilder().host("127.0.0.1").build()
            val media=MediaDescriptor(id,1,Quality.original,Quality.original,bytes.size.toLong(),sha,"\"$sha\"",base.resolve("/api/v1/tracks/$id/media?revision=1&quality=original")!!.toString())
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse=when {
                request.method=="POST"->{creates.incrementAndGet();MockResponse().setResponseCode(201).setBody("{\"id\":\"$manifest\"}")}
                request.path?.contains("download-manifests")==true->{val second=request.requestUrl?.queryParameter("cursor")!=null
                    if(second&&pending.get())MockResponse().setResponseCode(425)else MockResponse().setBody(Wire.json.encodeToString(ManifestPage.serializer(),ManifestPage(if(second)listOf(media,media)else listOf(media),if(second)null else "page2",7)))
                }
                request.path?.contains("/media")==true->{val start=request.getHeader("Range")?.substringAfter("bytes=")?.substringBefore('-')?.toInt()?:0;val response=MockResponse().setHeader("ETag",media.etag).setHeader("X-Content-SHA256",sha)
                    if(request.getHeader("Range")!=null)response.setResponseCode(206).setHeader("Content-Range","bytes $start-${bytes.lastIndex}/${bytes.size}")
                    response.setBody(Buffer().write(bytes,start,bytes.size-start))
                }
                else->MockResponse().setResponseCode(404)
            }}
            val session=Session(base.toString(),UUID.randomUUID(),"b".repeat(64));Credentials(context).save(session);val repo=DownloadRepository.get(context,session)
            try {
                withContext(Dispatchers.IO){repo.dao.selection(LocalSelection(local,"playlist",UUID.randomUUID().toString(),"original","Снимок",refreshRevision=true));repo.dao.pin(LocalPin(local,0,old.toString()))}
                try{repo.prepare();fail("The second page is not ready")}catch(error:ClientException){assertEquals(Failure.Pending,error.failure)}
                withContext(Dispatchers.IO){assertEquals(listOf(old.toString()),repo.dao.pins(local).map{it.track});assertEquals("page2",repo.dao.selection(local)!!.cursor)}
                pending.set(false);repo.prepare()
                withTimeout(15000){while(withContext(Dispatchers.IO){repo.ready(id.toString())}==null)delay(50)}
                withContext(Dispatchers.IO){assertEquals(listOf(id.toString(),id.toString(),id.toString()),repo.dao.pins(local).map{it.track});assertTrue(repo.dao.selection(local)!!.complete);assertEquals(1,repo.dao.media().count{it.ready!=null})}
                assertEquals(1,creates.get())
            }finally{repo.removeSelection(local);android.util.AtomicFile(file).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}}
        }
    }
    @Test fun catalogSnapshotAndTombstoneAtomic()=runBlocking {
        val context=ApplicationProvider.getApplicationContext<Context>();val id=UUID.randomUUID();val generation=UUID.randomUUID()
        val track=Track(id,1,"snapshot.mp3",title="Снимок",durationMs=1000,format=AudioFormat.mp3,originalBytes=100,readyQualities=listOf(Quality.original))
        MockWebServer().use{server->
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse=when {
                request.path?.startsWith("/api/v1/snapshot")==true->MockResponse().setBody(Wire.json.encodeToString(SnapshotPage.serializer(),SnapshotPage(generation,1,listOf(Change(1,Entity.track,id,1,false,Wire.json.encodeToJsonElement(Track.serializer(),track))))))
                request.path?.startsWith("/api/v1/changes")==true->MockResponse().setBody(Wire.json.encodeToString(SyncPage.serializer(),SyncPage(generation,listOf(Change(2,Entity.track,id,2,true)),"$generation:2",false)))
                else->MockResponse().setResponseCode(404)
            }}
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val session=Session(server.url("/").newBuilder().host("127.0.0.1").build().toString(),UUID.randomUUID(),"a".repeat(64))
            val db=LocalDatabase.open(context,LocalDatabase.scope(session))
            try{val sync=CatalogSync(context,db,session);sync.refresh()
                withContext(Dispatchers.IO){assertFalse("A promoted snapshot record must remain live",db.dao().record("track",id.toString())!!.deleted)}
                sync.refresh()
                withContext(Dispatchers.IO){assertTrue(db.dao().record("track",id.toString())!!.deleted);assertEquals("$generation:2",db.dao().sync()!!.cursor)}
            }finally{db.close();context.deleteDatabase("catalog-${LocalDatabase.scope(session)}.db")}
        }
    }
    @Test fun restartRevisionAndSharedPins()=runBlocking {
        val context=ApplicationProvider.getApplicationContext<Context>()
        requireEmptyConnectionFixture(File(context.noBackupFilesDir,"connection.json"))
        val bytes=androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().context.assets.open("vbr.mp3").use{it.readBytes()}
        val sha=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString(""){"%02x".format(it)}
        val id=UUID.randomUUID();val calls=AtomicInteger();val broken=java.util.concurrent.atomic.AtomicBoolean(false);val resumed=java.util.concurrent.atomic.AtomicBoolean(false)
        MockWebServer().use { server ->
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse {
                val attempt=calls.incrementAndGet()
                if(request.path?.contains("/media")!=true)return MockResponse().setResponseCode(404)
                val start=request.getHeader("Range")?.substringAfter("bytes=")?.substringBefore('-')?.toInt()?:0
                if(start>0)resumed.set(true)
                val end=request.getHeader("Range")?.substringAfter('-')?.toIntOrNull()?.coerceAtMost(bytes.lastIndex)?:bytes.lastIndex
                val content=if(broken.get())ByteArray(bytes.size){42}else bytes
                val response=MockResponse().setHeader("ETag","\"$sha\"").setHeader("X-Content-SHA256",sha).setHeader("Content-Type","audio/mpeg")
                if(request.getHeader("Range")!=null)response.setResponseCode(206).setHeader("Content-Range","bytes $start-$end/${bytes.size}")
                if(attempt==1)response.setSocketPolicy(SocketPolicy.DISCONNECT_DURING_RESPONSE_BODY)
                return response.setBody(Buffer().write(content,start,end-start+1))
            }}
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val base=server.url("/").newBuilder().host("127.0.0.1").build()
            val session=Session(base.toString(),UUID.randomUUID(),"a".repeat(64));val credentials=Credentials(context);credentials.save(session)
            val repository=DownloadRepository.get(context,session)
            val original=MediaDescriptor(id,1,Quality.original,Quality.original,bytes.size.toLong(),sha,"\"$sha\"",base.resolve("/api/v1/tracks/$id/media?revision=1&quality=original")!!.toString())
            val track=Track(id,1,"tone.mp3",title="Метро",durationMs=1000,format=AudioFormat.mp3,originalBytes=bytes.size.toLong(),readyQualities=listOf(Quality.original))
            try {
                withContext(Dispatchers.IO){
                    repository.dao.record(CatalogRow("track",id.toString(),Wire.json.encodeToString(Track.serializer(),track),search="метро"))
                    repository.dao.selection(LocalSelection("one","album",UUID.randomUUID().toString(),"original","Альбом",complete=true))
                    repository.dao.selection(LocalSelection("two","playlist",UUID.randomUUID().toString(),"original","Плейлист",complete=true))
                    repository.dao.pin(LocalPin("one",0,id.toString()));repository.dao.pin(LocalPin("two",0,id.toString()))
                    repository.dao.media(LocalMedia(id.toString(),candidate=Wire.json.encodeToString(MediaDescriptor.serializer(),original)))
                }
                repository.prepare()
                withTimeout(15000){while(withContext(Dispatchers.IO){repository.ready(id.toString())}==null)delay(50)}
                assertTrue("The interrupted download must resume from cached bytes",resumed.get())
                assertEquals("A late streaming write must not duplicate explicit bytes",0L,lateStreamSpan(context,original,bytes))
                repository.removeSelection("one")
                withContext(Dispatchers.IO){assertNotNull(repository.ready(id.toString()));assertEquals(1,repository.dao.pinCount(id.toString()))}
                val replacement=original.copy(revision=2,url=original.url.replace("revision=1","revision=2"));broken.set(true)
                withContext(Dispatchers.IO){repository.dao.media(repository.dao.media(id.toString())!!.copy(candidate=Wire.json.encodeToString(MediaDescriptor.serializer(),replacement)))}
                repository.prepare()
                withTimeout(15000){while(withContext(Dispatchers.IO){repository.candidate(id.toString())}!=null)delay(50)}
                val collision=original.copy(sha256="f".repeat(64),etag="\"changed\"")
                withContext(Dispatchers.IO){repository.dao.media(repository.dao.media(id.toString())!!.copy(candidate=Wire.json.encodeToString(MediaDescriptor.serializer(),collision)))}
                repository.verify(collision)
                delay(200)
                assertTrue("A failed replacement with the same URL must preserve the ready bytes",repository.cache.isCached(original.cacheKey,0,original.byteLength))
                withContext(Dispatchers.IO){assertEquals(1L,repository.ready(id.toString())!!.revision);repository.dao.tombstone("track",id.toString())}
                val before=calls.get()
                withContext(Dispatchers.IO){
                    assertEquals(1,repository.dao.tracks("метро",offline=true).size)
                    val reopened=LocalDatabase.open(context,repository.scope)
                    try{assertEquals(1L,Wire.json.decodeFromString<MediaDescriptor>(reopened.dao().media(id.toString())!!.ready!!).revision)}finally{reopened.close()}
                    val source=MediaTransport(context,ConcurrentHashMap<String,MediaDescriptor>().apply{put(replacement.url,replacement)},ConcurrentHashMap()).createDataSource()
                    val output=java.io.ByteArrayOutputStream()
                    try{source.open(DataSpec.Builder().setUri(replacement.url).build());val b=ByteArray(4096);while(true){val n=source.read(b,0,b.size);if(n<0)break;output.write(b,0,n)}}finally{source.close()}
                    assertArrayEquals(bytes,output.toByteArray())
                }
                assertEquals("Offline reads must make no HTTP requests",before,calls.get())
                repository.removeSelection("two")
                assertEquals("Removing the last pin must restore ordinary stream caching",original.byteLength,lateStreamSpan(context,original,bytes))
                io.dagaz.player.playback.StreamCache.get(context).removeResource(original.cacheKey)
            }finally{repository.removeSelection("two");android.util.AtomicFile(File(context.noBackupFilesDir,"connection.json")).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}}
        }
    }
    private suspend fun lateStreamSpan(context:Context,media:MediaDescriptor,bytes:ByteArray)=withContext(Dispatchers.IO){
        val cache=io.dagaz.player.playback.StreamCache.get(context)
        val source=androidx.media3.datasource.cache.CacheDataSource.Factory().setCache(cache).setUpstreamDataSourceFactory(androidx.media3.datasource.DataSource.Factory{androidx.media3.datasource.ByteArrayDataSource(bytes)}).createDataSource()
        try{source.open(DataSpec.Builder().setUri(media.url).setKey(media.cacheKey).setLength(bytes.size.toLong()).build());val buffer=ByteArray(4096);while(source.read(buffer,0,buffer.size)>=0){}}finally{source.close()}
        cache.getCachedBytes(media.cacheKey,0,media.byteLength)
    }
}
