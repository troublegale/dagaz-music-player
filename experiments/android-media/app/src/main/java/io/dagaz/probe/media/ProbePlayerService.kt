package io.dagaz.probe.media

import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService

@UnstableApi
class ProbePlayerService: MediaSessionService() {
    private lateinit var session: MediaSession
    override fun onCreate() {
        super.onCreate()
        val player = ExoPlayer.Builder(this)
            .setMediaSourceFactory(DefaultMediaSourceFactory(ProbeRuntime.dataSource(this)))
            .setAudioAttributes(AudioAttributes.DEFAULT,true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_LOCAL)
            .build()
        player.repeatMode = Player.REPEAT_MODE_ONE
        player.addListener(object: Player.Listener {
            override fun onIsPlayingChanged(playing: Boolean) {
                ProbeRuntime.status.value = if (playing) "Воспроизведение" else "Пауза / загрузка"
                android.util.Log.i("DagazProbe","playing=$playing track=${player.currentMediaItem?.mediaId}")
            }
        })
        session = MediaSession.Builder(this,player).build()
    }
    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo) = session
    override fun onDestroy() { session.player.release(); session.release(); super.onDestroy() }
}
