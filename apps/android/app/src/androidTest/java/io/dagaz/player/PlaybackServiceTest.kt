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
    @Test fun interruptedStreamResumesRange() = verifyPlayback(wave(), "audio/wav", true, disconnect = true)
    @Test fun audioFocusRespectsUserPause() = verifyPlayback(wave(), "audio/wav", true, focus = true)
    @Test fun nextTrackIsBuffered() = verifyPlayback(wave(), "audio/wav", true, prefetch = true)
    @Test fun prepareKeepsPaused() = verifyPlayback(wave(), "audio/wav", true, pausedPrepare = true)
    @Test fun aliasedQualityUsesCanonicalCache() = verifyPlayback(audioFixture("profiles/low.mp3"),"audio/mpeg",false,alias=true)

    private fun audioFixture(name: String): ByteArray =
        androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().context.assets.open(name).use { it.readBytes() }

    private fun verifyPlayback(bytes: ByteArray, mime: String, longFixture: Boolean, retry: Boolean = false, disconnect: Boolean = false, focus: Boolean = false, prefetch: Boolean = false, pausedPrepare: Boolean = false, alias: Boolean = false) {
        val context=ApplicationProvider.getApplicationContext<Context>()
        val file=java.io.File(context.noBackupFilesDir,"connection.json"); requireEmptyConnectionFixture(file)
        val power=context.getSystemService(android.os.PowerManager::class.java)
        assertTrue("unlock the phone before starting playback tests",power.isInteractive)
        val hash=MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        val id=UUID.randomUUID(); val etag="\"$hash\""; val requests=java.util.concurrent.atomic.AtomicInteger()
        val ranges=ConcurrentLinkedQueue<String>()
        val headRequests=java.util.concurrent.atomic.AtomicInteger()
        val interrupted=CountDownLatch(1)
        val resumed=CountDownLatch(1);val nextRequested=CountDownLatch(1);val nextId=UUID.randomUUID()
        MockWebServer().use { server ->
            server.dispatcher=object:Dispatcher(){override fun dispatch(request:RecordedRequest):MockResponse {
                if(request.getHeader("Authorization")!="Bearer ${"e".repeat(64)}") return MockResponse().setResponseCode(401)
                if(request.path?.contains("/media")!=true) return MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")
                if(request.method=="HEAD"){
                    if(alias&&headRequests.incrementAndGet()==1)return MockResponse().setResponseCode(425)
                    return MockResponse().setHeader("ETag",etag).setHeader("X-Content-SHA256",hash).setHeader("Content-Length",bytes.size).setHeader("X-Effective-Quality","original")
                }
                val attempt=requests.incrementAndGet()
                if(attempt==1&&retry){interrupted.countDown();return MockResponse().setResponseCode(503)}
                if(request.path?.contains(nextId.toString())==true)nextRequested.countDown()
                ranges.add("${request.method} ${request.getHeader("Range")}")
                val range=request.getHeader("Range")
                val start=range?.substringAfter("bytes=")?.substringBefore('-')?.toInt()?:0
                if(attempt>1&&start>0)resumed.countDown()
                val end=range?.substringAfter('-')?.toIntOrNull()?.coerceAtMost(bytes.lastIndex)?:bytes.lastIndex
                val response=MockResponse().setHeader("ETag",etag).setHeader("X-Content-SHA256",hash).setHeader("Accept-Ranges","bytes").setHeader("Content-Type",mime)
                if(range!=null) response.setResponseCode(206).setHeader("Content-Range","bytes $start-$end/${bytes.size}")
                if(disconnect&&attempt==1)response.setSocketPolicy(SocketPolicy.DISCONNECT_DURING_RESPONSE_BODY)
                return response.setBody(Buffer().write(bytes,start,end-start+1)).throttleBody(16384,25,TimeUnit.MILLISECONDS)
            }}
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val base=server.url("/").newBuilder().host("127.0.0.1").build().toString()
            Credentials(context).save(Session(base,UUID.randomUUID(),"e".repeat(64)))
            val scenario=ActivityScenario.launch(MainActivity::class.java)
            val future=onMain { MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync() }
            val controller=future.get(10,TimeUnit.SECONDS)
            @Suppress("DEPRECATION")
            val screenAwake=power.newWakeLock(android.os.PowerManager.SCREEN_BRIGHT_WAKE_LOCK,"dagaz:test-screen")
            if(androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("manual")!="lock")screenAwake.acquire(120000)
            try {
                val media=MediaDescriptor(id,1,Quality.original,Quality.original,bytes.size.toLong(),hash,etag,"${base}api/v1/tracks/$id/media?revision=1&quality=original")
                if(pausedPrepare){
                    val bridge=onMain{PlayerConnection(context)}
                    try{
                        kotlinx.coroutines.runBlocking{bridge.prepare(media,1000)}
                        awaitCondition("prepare did not finish"){onMain{controller.playbackState==Player.STATE_READY}}
                        assertFalse(onMain{controller.playWhenReady});assertFalse(onMain{controller.isPlaying})
                        assertEquals(1000L,onMain{controller.currentPosition})
                        assertEquals(1,PlaybackService.activePlayers.get())
                        return
                    }finally{onMain{bridge.release()}}
                }
                val item=playbackItem(media,"Тестовая песня","Dagaz")
                val next=if(alias)queueItem(Track(nextId,1,"low.mp3",durationMs=30000,format=AudioFormat.mp3,originalBytes=bytes.size.toLong(),readyQualities=listOf(Quality.original)),Quality.high,base)else if(prefetch)playbackItem(media.copy(trackId=nextId,url=media.url.replace(id.toString(),nextId.toString())),"Следующая песня","Dagaz")else item
                val playing=CountDownLatch(1);val ready=CountDownLatch(1);val error=AtomicReference<PlaybackException>()
                onMain {
                    controller.addListener(object:Player.Listener {
                        override fun onIsPlayingChanged(value:Boolean){if(value)playing.countDown()}
                        override fun onPlaybackStateChanged(value:Int){if(value==Player.STATE_READY)ready.countDown()}
                        override fun onPlayerError(value:PlaybackException){error.set(value);playing.countDown()}
                    })
                    controller.setMediaItems(listOf(item,next));controller.repeatMode=Player.REPEAT_MODE_ALL;controller.prepare();controller.play()
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
                when(androidx.test.platform.app.InstrumentationRegistry.getArguments().getString("manual")){
                    "lock"->{verifyScreenLock(context,controller);return}
                    "headset"->{verifyHeadsetDisconnect(context,controller);return}
                }
                if(disconnect){
                    assertTrue("stream did not resume from a byte offset; ranges=$ranges",resumed.await(15,TimeUnit.SECONDS))
                    awaitCondition("playback did not recover"){onMain{controller.isPlaying&&controller.currentPosition>1000}}
                    assertNull(error.get())
                }
                if(prefetch){
                    assertTrue("next track was not requested",nextRequested.await(15,TimeUnit.SECONDS))
                    assertEquals(0,onMain{controller.currentMediaItemIndex})
                    assertTrue(onMain{controller.currentPosition}<29000)
                }
                if(focus)verifyAudioFocus(context,controller)
                val second=onMain {MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync()}.get(10,TimeUnit.SECONDS)
                assertEquals(1,PlaybackService.activePlayers.get());onMain {second.release()}
                onMain {controller.seekTo(1,if(longFixture)1000 else 0)};assertEquals(1,onMain {controller.currentMediaItemIndex})
                val readyAgain=CountDownLatch(1)
                val check=object:Runnable {override fun run(){if(controller.isPlaying||error.get()!=null)readyAgain.countDown() else Handler(Looper.getMainLooper()).postDelayed(this,100)}}
                Handler(Looper.getMainLooper()).post(check)
                assertTrue("seek timeout; ranges=$ranges",readyAgain.await(15,TimeUnit.SECONDS))
                assertNull("seek error; ranges=$ranges",error.get())
                if(alias){val canonical=media.url.replace(id.toString(),nextId.toString())+"#sha256=$hash";assertTrue(headRequests.get()>=2);assertTrue(StreamCache.get(context).keys.contains(canonical));assertFalse(StreamCache.get(context).keys.contains(canonical.replace("quality=original","quality=high")))}
                onMain {controller.repeatMode=Player.REPEAT_MODE_ALL;controller.shuffleModeEnabled=true}
                assertEquals(Player.REPEAT_MODE_ALL,onMain {controller.repeatMode});assertTrue(onMain {controller.shuffleModeEnabled})
                scenario.recreate();scenario.moveToState(androidx.lifecycle.Lifecycle.State.CREATED)
                val elapsed=CountDownLatch(1);Handler(Looper.getMainLooper()).postDelayed({elapsed.countDown()},1200);assertTrue(elapsed.await(5,TimeUnit.SECONDS))
                assertTrue("background state=${onMain{controller.playbackState}} requested=${onMain{controller.playWhenReady}} error=${error.get()} ranges=$ranges",onMain {controller.isPlaying});assertEquals(1,PlaybackService.activePlayers.get())
                onMain {controller.pause()};assertFalse(onMain {controller.playWhenReady})
                assertTrue(requests.get()>0)
            } finally {
                try{
                    onMain {controller.stop();controller.clearMediaItems();controller.release()}
                    // Finish from the foreground: this Poco delays destroying a stopped Activity.
                    if(power.isInteractive)scenario.moveToState(androidx.lifecycle.Lifecycle.State.RESUMED)
                    scenario.close()
                    context.stopService(android.content.Intent(context,PlaybackService::class.java))
                    android.util.AtomicFile(file).delete();java.security.KeyStore.getInstance("AndroidKeyStore").apply{load(null);deleteEntry("dagaz.connection")}
                }finally{if(screenAwake.isHeld)screenAwake.release()}
            }
        }
    }
    private fun awaitCondition(message:String,condition:()->Boolean){
        val deadline=android.os.SystemClock.elapsedRealtime()+15000
        while(!condition()){assertTrue(message,android.os.SystemClock.elapsedRealtime()<deadline);Thread.sleep(50)}
    }
    private fun verifyScreenLock(context:Context,controller:MediaController){
        val power=context.getSystemService(android.os.PowerManager::class.java)
        val deadline=android.os.SystemClock.elapsedRealtime()+60000
        while(power.isInteractive&&android.os.SystemClock.elapsedRealtime()<deadline)Thread.sleep(100)
        assertFalse("phone was not physically locked within 60 seconds",power.isInteractive)
        val before=onMain{controller.currentPosition}
        repeat(15){Thread.sleep(1000);assertFalse("keep the screen off for 15 seconds",power.isInteractive);assertTrue("playback stopped while locked",onMain{controller.isPlaying})}
        val after=onMain{controller.currentPosition}
        val elapsed=if(after>=before)after-before else 30000-before+after
        assertTrue("playback position did not advance while locked",elapsed>12000)
        assertEquals(1,PlaybackService.activePlayers.get())
    }
    private fun verifyHeadsetDisconnect(context:Context,controller:MediaController){
        val audio=context.getSystemService(android.media.AudioManager::class.java)
        fun connected()=audio.getDevices(android.media.AudioManager.GET_DEVICES_OUTPUTS).any{it.type in listOf(android.media.AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,android.media.AudioDeviceInfo.TYPE_BLE_HEADSET)}
        assertTrue("connect Bluetooth headphones before this test",connected())
        val deadline=android.os.SystemClock.elapsedRealtime()+60000
        while(connected()&&android.os.SystemClock.elapsedRealtime()<deadline)Thread.sleep(100)
        assertFalse("Bluetooth headphones were not disconnected within 60 seconds",connected())
        awaitCondition("headset disconnect did not pause playback"){onMain{!controller.playWhenReady&&!controller.isPlaying}}
        Thread.sleep(1000);assertFalse("playback resumed on speaker",onMain{controller.isPlaying})
    }
    private fun verifyAudioFocus(context:Context,controller:MediaController){
        val audio=context.getSystemService(android.media.AudioManager::class.java)
        val request=android.media.AudioFocusRequest.Builder(android.media.AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(android.media.AudioAttributes.Builder().setUsage(android.media.AudioAttributes.USAGE_MEDIA).setContentType(android.media.AudioAttributes.CONTENT_TYPE_MUSIC).build())
            .setOnAudioFocusChangeListener({},Handler(Looper.getMainLooper())).build()
        try{
            assertEquals(android.media.AudioManager.AUDIOFOCUS_REQUEST_GRANTED,onMain{audio.requestAudioFocus(request)})
            awaitCondition("focus loss did not pause audio"){onMain{!controller.isPlaying}}
            onMain{audio.abandonAudioFocusRequest(request)}
            awaitCondition("focus gain did not resume audio"){onMain{controller.isPlaying}}
            assertEquals(android.media.AudioManager.AUDIOFOCUS_REQUEST_GRANTED,onMain{audio.requestAudioFocus(request)})
            awaitCondition("second focus loss did not pause audio"){onMain{!controller.isPlaying}}
            onMain{controller.pause();audio.abandonAudioFocusRequest(request)}
            Thread.sleep(1000)
            assertFalse("focus gain overrode user pause",onMain{controller.playWhenReady})
            assertFalse(onMain{controller.isPlaying});onMain{controller.play()}
        }finally{onMain{audio.abandonAudioFocusRequest(request)}}
    }
    private fun wave():ByteArray {
        val samples=44100*30;val data=samples*2
        val buffer=ByteBuffer.allocate(44+data).order(ByteOrder.LITTLE_ENDIAN)
        buffer.put("RIFF".toByteArray()).putInt(36+data).put("WAVEfmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(44100).putInt(88200).putShort(2).putShort(16).put("data".toByteArray()).putInt(data)
        repeat(samples){buffer.putShort((kotlin.math.sin(it*2*Math.PI*440/44100)*3000).toInt().toShort())};return buffer.array()
    }
}
