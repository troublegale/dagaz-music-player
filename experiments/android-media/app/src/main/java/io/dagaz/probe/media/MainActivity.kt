package io.dagaz.probe.media

import android.content.ComponentName
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken

@UnstableApi
class MainActivity: ComponentActivity() {
    private var controller: MediaController? = null
    private val future by lazy { MediaController.Builder(this,SessionToken(this,ComponentName(this,ProbePlayerService::class.java))).buildAsync() }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        future.addListener({ controller = future.get() },ContextCompat.getMainExecutor(this))
        setContent {
            val status by ProbeRuntime.status.collectAsState()
            MaterialTheme(colorScheme=darkColorScheme()) {
                Surface(Modifier.fillMaxSize()) {
                    Column(Modifier.padding(24.dp).statusBarsPadding(),verticalArrangement=Arrangement.spacedBy(12.dp)) {
                        Text("Проверка Media3",style=MaterialTheme.typography.headlineMedium)
                        Text(status)
                        for ((name,uri) in listOf("WAV" to "asset:///audio/pcm.wav","MP3" to "asset:///audio/vbr.mp3","FLAC" to "asset:///audio/tone.flac","Сетевой WAV / кеш" to ProbeRuntime.URL)) {
                            Button(onClick={ play(name,uri) }) { Text(name) }
                        }
                        Button(onClick={ controller?.pause() }) { Text("Пауза") }
                        Button(onClick={ controller?.seekTo(10_000) }) { Text("Перемотать на 10 секунд") }
                        Button(onClick={
                            val request = DownloadRequest.Builder(ProbeRuntime.KEY,Uri.parse(ProbeRuntime.URL)).setCustomCacheKey(ProbeRuntime.KEY).build()
                            DownloadService.sendAddDownload(this@MainActivity,ProbeDownloadService::class.java,request,true)
                        }) { Text("Скачать WAV для офлайна") }
                    }
                }
            }
        }
    }
    private fun play(name: String,uri: String) {
        controller?.apply {
            setMediaItem(MediaItem.Builder().setMediaId(name).setUri(uri).setCustomCacheKey(if (uri==ProbeRuntime.URL) ProbeRuntime.KEY else uri).build())
            prepare(); play()
        }
    }
    override fun onDestroy() { MediaController.releaseFuture(future); super.onDestroy() }
}
