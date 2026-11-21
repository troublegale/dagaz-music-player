package io.dagaz.player

import android.content.ComponentName
import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.media3.common.*
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.*
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.playback.*
import okhttp3.mockwebserver.*
import okio.Buffer
import org.junit.Assert.*
import org.junit.Assume.assumeFalse
import org.junit.Test
import org.junit.runner.RunWith
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.*
import java.util.concurrent.atomic.AtomicReference

private fun <T> onMain(block: () -> T): T {
    val result=AtomicReference<T>(); androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync { result.set(block()) }; return result.get()
}
@UnstableApi
@RunWith(AndroidJUnit4::class)
class PlaybackServiceTest {
    @Test fun libraryAndBackgroundPlayback() = verifyPlayback(wave(), "audio/wav", true)
    @Test fun mp3Playback() = verifyPlayback(audioFixture("vbr.mp3"), "audio/mpeg", false)
    @Test fun flacPlayback() = verifyPlayback(audioFixture("tone.flac"), "audio/flac", false)
    @Test fun retryKeepsUserPause() = verifyPlayback(wave(), "audio/wav", true, retry = true)

    private fun audioFixture(name: String): ByteArray =
        androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().context.assets.open(name).use { it.readBytes() }

    private fun verifyPlayback(bytes: ByteArray, mime: String, longFixture: Boolean, retry: Boolean = false) {
        val context=ApplicationProvider.getApplicationContext<Context>()
        val file=java.io.File(context.noBackupFilesDir,"connection.json"); assumeFalse(file.exists())
        val hash=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        val id=UUID.randomUUID(); val etag="\"$hash\""; val requests=java.util.concurrent.atomic.AtomicInteger()
        val ranges=ConcurrentLinkedQueue<String>()
        val interrupted=CountDownLatch(1)
        MockWebServer().use { server ->
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse {
                if(request.getHeader("Authorization")!="Bearer ${"e".repeat(64)}") return MockResponse().setResponseCode(401)
                if(request.path?.contains("/media")!=true) return MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")
                if(requests.incrementAndGet()==1&&retry){interrupted.countDown();return MockResponse().setResponseCode(503)}
                ranges.add("${request.method} ${request.getHeader("Range")}")
                val range=request.getHeader("Range")
                val start=range?.substringAfter("bytes=")?.substringBefore('-')?.toInt()?:0
                val end=range?.substringAfter('-')?.toIntOrNull()?.coerceAtMost(bytes.lastIndex)?:bytes.lastIndex
                val response=MockResponse().setHeader("ETag",etag).setHeader("X-Content-SHA256",hash).setHeader("Accept-Ranges","bytes").setHeader("Content-Type",mime)
                if(range!=null) response.setResponseCode(206).setHeader("Content-Range","bytes $start-$end/${bytes.size}")
                return response.setBody(Buffer().write(bytes,start,end-start+1)).throttleBody(16384,25,TimeUnit.MILLISECONDS)
            }}
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val base=server.url("/").newBuilder().host("127.0.0.1").build().toString()
            Credentials(context).save(Session(base,UUID.randomUUID(),"e".repeat(64)))
            val scenario=ActivityScenario.launch(MainActivity::class.java)
            val future=onMain { MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync() }
            val controller=future.get(10,TimeUnit.SECONDS)
            try {
                val media=MediaDescriptor(id,1,Quality.original,Quality.original,bytes.size.toLong(),hash,etag,"${base}api/v1/tracks/$id/media?revision=1&quality=original")
                val item=playbackItem(media,"Тестовая песня","Dagaz")
                val playing=CountDownLatch(1);val ready=CountDownLatch(1);val error=AtomicReference<PlaybackException>()
                onMain {
                    controller.addListener(object:Player.Listener {
                        override fun onIsPlayingChanged(value:Boolean){if(value)playing.countDown()}
                        override fun onPlaybackStateChanged(value:Int){if(value==Player.STATE_READY)ready.countDown()}
                        override fun onPlayerError(value:PlaybackException){error.set(value);playing.countDown()}
                    })
                    controller.setMediaItems(listOf(item,item));controller.repeatMode=Player.REPEAT_MODE_ALL;controller.prepare();controller.play()
                }
                if(retry){
                    assertTrue(interrupted.await(10,TimeUnit.SECONDS));onMain{controller.pause()}
                    assertTrue("retry did not recover",ready.await(15,TimeUnit.SECONDS))
                    assertFalse("retry overrode user pause",onMain{controller.playWhenReady})
                    assertFalse(onMain{controller.isPlaying});onMain{controller.play()}
                }
                assertTrue(playing.await(15,TimeUnit.SECONDS));assertNull(error.get())
                assertEquals(1,PlaybackService.activePlayers.get());assertEquals(id.toString(),onMain {controller.currentMediaItem!!.mediaId})
                if(longFixture)assertTrue("stream waited for full body",onMain { controller.bufferedPosition }<29000)
                val second=onMain {MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync()}.get(10,TimeUnit.SECONDS)
                assertEquals(1,PlaybackService.activePlayers.get());onMain {second.release()}
                onMain {controller.seekTo(1,if(longFixture)1000 else 0)};assertEquals(1,onMain {controller.currentMediaItemIndex})
                val readyAgain=CountDownLatch(1)
                val check=object:Runnable {override fun run(){if(controller.isPlaying||error.get()!=null)readyAgain.countDown() else Handler(Looper.getMainLooper()).postDelayed(this,100)}}
                Handler(Looper.getMainLooper()).post(check)
                assertTrue("seek timeout; ranges=$ranges",readyAgain.await(15,TimeUnit.SECONDS))
                assertNull("seek error; ranges=$ranges",error.get())
                onMain {controller.repeatMode=Player.REPEAT_MODE_ALL;controller.shuffleModeEnabled=true}
                assertEquals(Player.REPEAT_MODE_ALL,onMain {controller.repeatMode});assertTrue(onMain {controller.shuffleModeEnabled})
                scenario.recreate();scenario.moveToState(androidx.lifecycle.Lifecycle.State.CREATED)
                val elapsed=CountDownLatch(1);Handler(Looper.getMainLooper()).postDelayed({elapsed.countDown()},1200);assertTrue(elapsed.await(5,TimeUnit.SECONDS))
                assertTrue("background state=${onMain{controller.playbackState}} requested=${onMain{controller.playWhenReady}} error=${error.get()} ranges=$ranges",onMain {controller.isPlaying});assertEquals(1,PlaybackService.activePlayers.get())
                onMain {controller.pause()};assertFalse(onMain {controller.playWhenReady})
                assertTrue(requests.get()>0)
            } finally {
                onMain {controller.stop();controller.clearMediaItems();controller.release()};scenario.close()
                context.stopService(android.content.Intent(context,PlaybackService::class.java))
                android.util.AtomicFile(file).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}
            }
        }
    }
    private fun wave():ByteArray {
        val samples=44100*30;val data=samples*2
        val buffer=ByteBuffer.allocate(44+data).order(ByteOrder.LITTLE_ENDIAN)
        buffer.put("RIFF".toByteArray()).putInt(36+data).put("WAVEfmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(44100).putInt(88200).putShort(2).putShort(16).put("data".toByteArray()).putInt(data)
        repeat(samples){buffer.putShort((kotlin.math.sin(it*2*Math.PI*440/44100)*3000).toInt().toShort())};return buffer.array()
    }
}
