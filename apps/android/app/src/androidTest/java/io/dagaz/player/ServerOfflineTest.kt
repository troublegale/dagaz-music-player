package io.dagaz.player

import android.content.ComponentName
import android.content.Context
import android.os.PowerManager
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.downloads.*
import io.dagaz.player.playback.*
import kotlinx.coroutines.*
import kotlinx.serialization.Serializable
import org.junit.Assert.*
import org.junit.Assume.assumeNotNull
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID
import java.util.concurrent.TimeUnit

@Serializable private data class OfflineMarker(val device:String,val baseUrl:String,val selections:List<String>,val tracks:List<String>,val process:Int,val covers:List<String>)
@UnstableApi
@RunWith(AndroidJUnit4::class)
class ServerOfflineTest {
    private val context get()=ApplicationProvider.getApplicationContext<Context>()
    private val args get()=InstrumentationRegistry.getArguments()
    private fun marker():File {val nonce=UUID.fromString(args.getString("fixture_nonce"));return File(context.noBackupFilesDir,"offline-fixture-$nonce.json")}
    @Test fun downloadFixture()=runBlocking {
        val address=args.getString("server_url");val code=args.getString("pairing_code");val album=args.getString("album_id");val playlist=args.getString("playlist_id");assumeNotNull(address,code,album,playlist)
        requireEmptyConnectionFixture(File(context.noBackupFilesDir,"connection.json"));assertFalse(marker().exists())
        val api=DagazApi(address!!);val pair=api.pair(code!!,"Poco offline fixture");val session=Session(api.baseUrl,pair.deviceId,pair.token)
        Credentials(context).save(session);val repo=DownloadRepository.get(context,session)
        var scenario:ActivityScenario<MainActivity>?=null;var saved:OfflineMarker?=null;var passed=false
        val power=context.getSystemService(PowerManager::class.java)
        @Suppress("DEPRECATION") val awake=power.newWakeLock(PowerManager.SCREEN_BRIGHT_WAKE_LOCK,"dagaz:offline-download-test")
        try {
            assertTrue("Unlock the phone before the download test",power.isInteractive);awake.acquire(120000)
            scenario=ActivityScenario.launch(MainActivity::class.java)
            val one=repo.enqueue(SelectionKind.album,UUID.fromString(album),Quality.low,"Альбом офлайн")
            val two=repo.enqueue(SelectionKind.playlist,UUID.fromString(playlist),Quality.low,"Плейлист офлайн")
            try{withTimeout(60000){while(withContext(Dispatchers.IO){repo.dao.selections().any{!it.complete}||repo.dao.media().count{it.ready!=null}!=2})delay(100)}}catch(error:TimeoutCancellationException){
                val state=withContext(Dispatchers.IO){"selections="+repo.dao.selections().map{Triple(it.complete,it.position,it.error)}+"; media="+repo.dao.media().map{Triple(it.ready!=null,it.candidate!=null,it.error)}}
                throw AssertionError(state,error)
            }
            withTimeout(15000){while(withContext(Dispatchers.IO){androidx.work.WorkManager.getInstance(context).getWorkInfosForUniqueWork("downloads-${repo.scope}").get(5,TimeUnit.SECONDS).any{!it.state.isFinished}})delay(100)}
            val covers=mutableListOf<String>()
            val ids=withContext(Dispatchers.IO){
                assertEquals(3,repo.dao.pins(two).size)
                val track=repo.dao.pins(one).single().track;assertEquals(3,repo.dao.pinCount(track))
                repo.dao.media().map{row->val media=repo.ready(row.track)!!;assertTrue(repo.cache.isCached(media.cacheKey,0,media.byteLength));val track=Wire.json.decodeFromString<Track>(repo.dao.record("track",row.track)!!.payload);track.artworkId?.let{covers.add(it.toString())};row.track}
            }
            assertTrue("The real fixture must exercise cover persistence",covers.isNotEmpty());verifyCovers(repo,covers)
            saved=OfflineMarker(session.deviceId.toString(),session.baseUrl,listOf(one,two),ids,android.os.Process.myPid(),covers)
            marker().writeText(Wire.json.encodeToString(OfflineMarker.serializer(),saved))
            passed=true
        }finally{
            scenario?.close();context.stopService(android.content.Intent(context,DagazDownloadService::class.java));context.stopService(android.content.Intent(context,PlaybackService::class.java))
            if(!passed)cleanup(repo,saved?.selections?:withContext(Dispatchers.IO){repo.dao.selections().map{it.id}})
            if(awake.isHeld)awake.release()
        }
    }
    @Test fun coldStartWithoutServer()=runBlocking {
        val file=marker();assertTrue("Run the download phase first",file.exists())
        val saved=Wire.json.decodeFromString<OfflineMarker>(file.readText())
        val session=Credentials(context).load()!!
        assertEquals(saved.device,session.deviceId.toString());assertEquals(saved.baseUrl,session.baseUrl);assertEquals(args.getString("server_url"),session.baseUrl.trimEnd('/'))
        assertNotEquals("The fixture must reopen in a fresh process",saved.process,android.os.Process.myPid())
        val repo=DownloadRepository.get(context,session);var scenario:ActivityScenario<MainActivity>?=null;var player:MediaController?=null
        val power=context.getSystemService(PowerManager::class.java);assertTrue(power.isInteractive)
        @Suppress("DEPRECATION") val awake=power.newWakeLock(PowerManager.SCREEN_BRIGHT_WAKE_LOCK,"dagaz:offline-test");awake.acquire(120000)
        try {
            withContext(Dispatchers.IO){assertEquals(2,repo.dao.media().count{it.ready!=null});assertEquals(2,repo.dao.tracks("",offline=true).size)}
            verifyCovers(repo,saved.covers)
            try{DagazApi(session.baseUrl).checkAccess(session);fail("The USB server tunnel must be disconnected")}catch(error:ClientException){assertEquals(Failure.Unavailable,error.failure)}
            scenario=ActivityScenario.launch(MainActivity::class.java)
            val future=withContext(Dispatchers.Main){MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync()}
            val controller=future.get(10,TimeUnit.SECONDS);player=controller
            val items=withContext(Dispatchers.IO){repo.dao.pins(saved.selections[1]).map{pin->val row=repo.dao.record("track",pin.track)!!;queueItem(Wire.json.decodeFromString<Track>(row.payload),Quality.high,session.baseUrl)}}
            withContext(Dispatchers.Main){controller.setMediaItems(items,0,1000);controller.prepare();controller.play()}
            withTimeout(15000){while(!withContext(Dispatchers.Main){controller.isPlaying}){assertNull(withContext(Dispatchers.Main){controller.playerError});delay(50)}}
            val started=withContext(Dispatchers.Main){controller.currentPosition};assertTrue("Start position was lost: $started",started>=850)
            delay(500);val position=withContext(Dispatchers.Main){controller.currentPosition};assertTrue("Offline position did not advance: $started -> $position",position>=started+250);assertEquals(1,PlaybackService.activePlayers.get())
            repo.removeSelection(saved.selections[0])
            withContext(Dispatchers.IO){assertEquals(2,repo.dao.media().count{it.ready!=null})}
        }finally{
            withContext(Dispatchers.Main){player?.let{it.stop();it.clearMediaItems();it.release()}}
            scenario?.close();context.stopService(android.content.Intent(context,PlaybackService::class.java));cleanup(repo,saved.selections);file.delete()
            if(awake.isHeld)awake.release()
        }
    }
    private suspend fun cleanup(repo:DownloadRepository,selections:List<String>) {
        withContext(Dispatchers.IO){androidx.work.WorkManager.getInstance(context).cancelUniqueWork("downloads-${repo.scope}").result.get(10,TimeUnit.SECONDS)}
        selections.forEach{repo.removeSelection(it)}
        context.stopService(android.content.Intent(context,DagazDownloadService::class.java))
        if(Credentials(context).load()?.deviceId==repo.session.deviceId){android.util.AtomicFile(File(context.noBackupFilesDir,"connection.json")).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}}
    }
    private suspend fun verifyCovers(repo:DownloadRepository,ids:List<String>)=withContext(Dispatchers.IO){for(id in ids){
        val image=android.graphics.BitmapFactory.decodeFile(File(context.noBackupFilesDir,"covers-${repo.scope}/$id.png").path)
        assertNotNull("Cover must survive without server access",image);assertTrue(image!!.width in 1..256&&image.height in 1..256);image.recycle()
    }}
}
