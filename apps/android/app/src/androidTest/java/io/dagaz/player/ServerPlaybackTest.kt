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
import io.dagaz.player.playback.*
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Assume.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID
import java.util.concurrent.TimeUnit

@UnstableApi
@RunWith(AndroidJUnit4::class)
class ServerPlaybackTest {
    @Test fun playsRealServerQualitiesAndOriginalAlias() = runBlocking {
        val args=InstrumentationRegistry.getArguments()
        val address=args.getString("server_url");val code=args.getString("pairing_code");val track=args.getString("track_id");val alias=args.getString("alias_track_id")
        assumeNotNull(address,code,track,alias)
        val context=ApplicationProvider.getApplicationContext<Context>()
        val file=java.io.File(context.noBackupFilesDir,"connection.json");requireEmptyConnectionFixture(file)
        val power=context.getSystemService(PowerManager::class.java);assertTrue("Unlock the phone before the real-server test",power.isInteractive)
        @Suppress("DEPRECATION") val awake=power.newWakeLock(PowerManager.SCREEN_BRIGHT_WAKE_LOCK,"dagaz:server-test")
        var scenario:ActivityScenario<MainActivity>?=null;var controller:MediaController?=null
        awake.acquire(120000)
        try{
            val api=DagazApi(address!!);val paired=api.pair(code!!,"Poco server playback test");val session=Session(api.baseUrl,paired.deviceId,paired.token);Credentials(context).save(session)
            scenario=ActivityScenario.launch(MainActivity::class.java)
            val future=withContext(Dispatchers.Main){MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync()}
            val player=future.get(10,TimeUnit.SECONDS);controller=player
            val lengths=mutableMapOf<Quality,Long>()
            for(quality in listOf(Quality.original,Quality.high,Quality.low)){
                val media=api.media(session,UUID.fromString(track),1,quality);assertEquals(quality,media.effectiveQuality);lengths[quality]=media.byteLength
                play(player,media)
            }
            assertTrue(lengths[Quality.low]!!<lengths[Quality.high]!!)
            val reused=api.media(session,UUID.fromString(alias),1,Quality.high)
            assertEquals(Quality.original,reused.effectiveQuality);assertTrue(reused.url.endsWith("quality=original"));play(player,reused)
            val cacheKey="${reused.url}#sha256=${reused.sha256}";assertTrue(StreamCache.get(context).keys.contains(cacheKey));assertFalse(StreamCache.get(context).keys.contains(cacheKey.replace("quality=original","quality=high")))
            assertEquals(1,PlaybackService.activePlayers.get())
        }finally{
            withContext(Dispatchers.Main){controller?.let{it.stop();it.clearMediaItems();it.release()}}
            scenario?.close();context.stopService(android.content.Intent(context,PlaybackService::class.java))
            android.util.AtomicFile(file).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}
            if(awake.isHeld)awake.release()
        }
    }
    private suspend fun play(player:MediaController,media:MediaDescriptor){
        withContext(Dispatchers.Main){player.setMediaItem(playbackItem(media,"Server quality test"),1000);player.prepare();player.play()}
        val deadline=android.os.SystemClock.elapsedRealtime()+15000
        while(!withContext(Dispatchers.Main){player.isPlaying}){
            assertNull(withContext(Dispatchers.Main){player.playerError});assertTrue("Server audio did not start",android.os.SystemClock.elapsedRealtime()<deadline);delay(50)
        }
        delay(400);assertTrue(withContext(Dispatchers.Main){player.currentPosition>1200});assertNull(withContext(Dispatchers.Main){player.playerError})
        withContext(Dispatchers.Main){player.pause()}
    }
}
