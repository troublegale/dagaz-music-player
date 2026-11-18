package io.dagaz.probe.media

import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.PlaybackException
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

private fun <T> main(block: () -> T): T {
    val result = AtomicReference<T>()
    InstrumentationRegistry.getInstrumentation().runOnMainSync { result.set(block()) }
    return result.get()
}

@UnstableApi
private fun connect(context: Context): MediaController = main {
    MediaController.Builder(context,SessionToken(context,ComponentName(context,ProbePlayerService::class.java))).buildAsync()
}.get(10,TimeUnit.SECONDS)

@UnstableApi
private fun play(controller: MediaController, uri: String, key: String) {
    val ready = CountDownLatch(1)
    val error = AtomicReference<PlaybackException>()
    val listener = object: Player.Listener {
        override fun onIsPlayingChanged(playing: Boolean) { if (playing) ready.countDown() }
        override fun onPlayerError(exception: PlaybackException) { error.set(exception); ready.countDown() }
    }
    main {
        controller.pause()
        controller.addListener(listener)
        controller.setMediaItem(MediaItem.Builder().setMediaId(key).setUri(uri).setCustomCacheKey(key).build())
        controller.prepare()
        controller.play()
    }
    try {
        assertTrue("player did not become ready",ready.await(15,TimeUnit.SECONDS))
        assertNull("playback error: ${error.get()}",error.get())
        assertTrue(main { controller.isPlaying })
        assertEquals(key,main { controller.currentMediaItem?.mediaId })
    } finally { main { controller.removeListener(listener) } }
}

@RunWith(AndroidJUnit4::class)
@UnstableApi
class MediaProbeTest {
    @Test fun formatsStreamDownloadAndBackground() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        val controller = connect(context)
        try {
            for (name in listOf("pcm.wav","vbr.mp3","tone.flac")) {
                play(controller,"asset:///audio/$name",name)
            }
            main { controller.stop(); controller.clearMediaItems() }
            ProbeRuntime.cache(context).removeResource(ProbeRuntime.KEY)
            val networkUri = ProbeRuntime.URL + "&run=" + android.os.SystemClock.elapsedRealtime()
            play(controller,networkUri,ProbeRuntime.KEY)
            val initialBytes = ProbeRuntime.cache(context).getCachedBytes(ProbeRuntime.KEY,0,ProbeRuntime.LONG_BYTES)
            assertTrue("stream waited for full download",initialBytes < ProbeRuntime.LONG_BYTES)
            main { controller.stop(); controller.clearMediaItems() }
            val complete = CountDownLatch(1)
            val failure = AtomicReference<Exception>()
            val manager = main { ProbeRuntime.downloads(context) }
            val listener = object: DownloadManager.Listener {
                override fun onDownloadChanged(manager: DownloadManager,download: Download,finalException: Exception?) {
                    if (download.request.id != ProbeRuntime.KEY) return
                    if (download.state == Download.STATE_COMPLETED) complete.countDown()
                    if (download.state == Download.STATE_FAILED) { failure.set(finalException ?: Exception("download failed")); complete.countDown() }
                }
            }
            main {
                manager.addListener(listener)
                val request = DownloadRequest.Builder(ProbeRuntime.KEY,Uri.parse(networkUri)).setCustomCacheKey(ProbeRuntime.KEY).build()
                DownloadService.sendAddDownload(context,ProbeDownloadService::class.java,request,true)
            }
            try {
                assertTrue("download timed out",complete.await(45,TimeUnit.SECONDS))
                assertNull("download error: ${failure.get()}",failure.get())
            } finally { main { manager.removeListener(listener) } }
            assertEquals(ProbeRuntime.LONG_BYTES,ProbeRuntime.cache(context).getCachedBytes(ProbeRuntime.KEY,0,ProbeRuntime.LONG_BYTES))
            play(controller,networkUri,ProbeRuntime.KEY)
            main { controller.seekTo(5000) }
            scenario.recreate()
            val before = main { controller.currentPosition }
            scenario.moveToState(androidx.lifecycle.Lifecycle.State.CREATED)
            val observed = CountDownLatch(1)
            Handler(Looper.getMainLooper()).postDelayed({ observed.countDown() },1500)
            assertTrue(observed.await(5,TimeUnit.SECONDS))
            assertTrue("background playback stopped",main { controller.isPlaying })
            assertTrue("background position did not advance",main { controller.currentPosition } > before+300)
        } finally {
            main { controller.pause(); controller.release() }
            scenario.close()
        }
    }
}

@RunWith(AndroidJUnit4::class)
@UnstableApi
class OfflineProbeTest {
    @Test fun completedDownloadPlaysAfterProcessRestartWithoutServer() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val connection = java.net.URL(ProbeRuntime.URL).openConnection() as java.net.HttpURLConnection
        connection.connectTimeout = 500
        connection.readTimeout = 500
        try { connection.inputStream.use { it.read() }; fail("test server must be unavailable") }
        catch (_: java.io.IOException) { /* Expected: only cached bytes may satisfy playback. */ }
        finally { connection.disconnect() }
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        val controller = connect(context)
        try {
            assertEquals(ProbeRuntime.LONG_BYTES,ProbeRuntime.cache(context).getCachedBytes(ProbeRuntime.KEY,0,ProbeRuntime.LONG_BYTES))
            play(controller,ProbeRuntime.URL,ProbeRuntime.KEY)
            main { controller.seekTo(25000) }
            val observed = CountDownLatch(1)
            Handler(Looper.getMainLooper()).postDelayed({ observed.countDown() },1500)
            assertTrue(observed.await(5,TimeUnit.SECONDS))
            assertNull(main { controller.playerError })
            assertTrue(main { controller.isPlaying })
            assertTrue(main { controller.currentPosition } >= 25000)
        } finally {
            main { controller.pause(); controller.release() }
            scenario.close()
        }
    }
}

@RunWith(AndroidJUnit4::class)
@UnstableApi
class QualityProbeTest {
    @Test fun encodedProfilesPlayAndSeek() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        val controller = connect(context)
        try {
            for (name in listOf("high.mp3","low.mp3")) {
                play(controller,"asset:///audio/profiles/$name",name)
                assertTrue("unexpected duration for $name",main { kotlin.math.abs(controller.duration-30_000) <= 500 })
                main { controller.seekTo(25_000) }
                val observed = CountDownLatch(1)
                Handler(Looper.getMainLooper()).postDelayed({ observed.countDown() },1500)
                assertTrue(observed.await(5,TimeUnit.SECONDS))
                assertNull(main { controller.playerError })
                assertTrue("profile stopped after seek: $name",main { controller.isPlaying })
                assertTrue("position did not advance: $name",main { controller.currentPosition >= 25_300 })
            }
        } finally {
            main { controller.pause(); controller.release() }
            scenario.close()
        }
    }
}

@RunWith(AndroidJUnit4::class)
@UnstableApi
class LockedScreenProbeTest {
    @Test fun cachedPlaybackContinuesWhileScreenIsOff() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val power = context.getSystemService(android.os.PowerManager::class.java)
        assertTrue("unlock the phone before starting the screen-off probe",power.isInteractive)
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        val controller = connect(context)
        try {
            play(controller,ProbeRuntime.URL,ProbeRuntime.KEY)
            val deadline = android.os.SystemClock.elapsedRealtime()+60_000
            while (power.isInteractive && android.os.SystemClock.elapsedRealtime()<deadline) Thread.sleep(200)
            assertFalse("press the physical power button during the tone",power.isInteractive)
            val before = main { controller.currentPosition }
            val observed = CountDownLatch(1)
            Handler(Looper.getMainLooper()).postDelayed({ observed.countDown() },15000)
            assertTrue(observed.await(20,TimeUnit.SECONDS))
            assertFalse("keep screen off for at least 16 seconds",power.isInteractive)
            assertTrue("screen-off playback stopped",main { controller.isPlaying })
            assertTrue("position stopped advancing",main { controller.currentPosition } > before+300)
        } finally {
            main { controller.pause(); controller.release() }
            scenario.close()
        }
    }
}
