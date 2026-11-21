package io.dagaz.player.playback

import android.os.Bundle
import androidx.media3.common.*
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.*
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import io.dagaz.player.api.*
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

private const val DESCRIPTOR="dagaz.descriptor"
@UnstableApi
fun playbackItem(media:MediaDescriptor,title:String="Трек",artist:String=""):MediaItem = MediaItem.Builder()
    .setMediaId(media.trackId.toString()).setRequestMetadata(MediaItem.RequestMetadata.Builder().setExtras(Bundle().apply{putString(DESCRIPTOR,Wire.json.encodeToString(MediaDescriptor.serializer(),media))}).build())
    .setMediaMetadata(MediaMetadata.Builder().setTitle(title.take(256)).setArtist(artist.take(256)).build()).build()

@UnstableApi
fun queueItem(track:Track,quality:Quality,baseUrl:String):MediaItem {
    val url=ServerAddress.parse(baseUrl).newBuilder().addPathSegments("api/v1/tracks/${track.id}/media").addQueryParameter("revision",track.revision.toString()).addQueryParameter("quality",quality.name).build()
    return MediaItem.Builder().setMediaId(track.id.toString()).setRequestMetadata(MediaItem.RequestMetadata.Builder().setMediaUri(android.net.Uri.parse(url.toString())).setExtras(Bundle().apply{
        putString("dagaz.id",track.id.toString());putLong("dagaz.revision",track.revision);putString("dagaz.quality",quality.name)
    }).build()).setMediaMetadata(MediaMetadata.Builder().setTitle((track.title?.takeUnless{it.isBlank()}?:track.filename).take(256)).setArtist(track.artist?.take(256)).build()).build()
}

@UnstableApi
class PlaybackService:MediaSessionService() {
    companion object { internal val activePlayers=AtomicInteger() }
    private lateinit var session:MediaSession
    private val descriptors=ConcurrentHashMap<String,MediaDescriptor>()
    private val pending=ConcurrentHashMap<String,PendingMedia>()
    override fun onCreate() {
        super.onCreate()
        val player=ExoPlayer.Builder(this)
            .setLoadControl(DefaultLoadControl.Builder().setBufferDurationsMs(5000,120000,1000,2000).setTargetBufferBytes(64*1024*1024).setPrioritizeTimeOverSizeThresholds(false).build())
            .setMediaSourceFactory(DefaultMediaSourceFactory(MediaTransport(this,descriptors,pending)).setLoadErrorHandlingPolicy(MediaRetryPolicy()))
            .setAudioAttributes(AudioAttributes.DEFAULT,true).setHandleAudioBecomingNoisy(true).setWakeMode(C.WAKE_MODE_LOCAL).build()
        activePlayers.incrementAndGet()
        val activity=android.app.PendingIntent.getActivity(this,0,android.content.Intent(this,io.dagaz.player.MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP or android.content.Intent.FLAG_ACTIVITY_CLEAR_TOP),android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT)
        session=MediaSession.Builder(this,player).setSessionActivity(activity).setCallback(object:MediaSession.Callback {
            override fun onAddMediaItems(session:MediaSession,controller:MediaSession.ControllerInfo,mediaItems:List<MediaItem>):ListenableFuture<List<MediaItem>> {
                return try {Futures.immediateFuture(mediaItems.map { item ->
                    val extras=item.requestMetadata.extras?:throw MediaValidationException()
                    val json=extras.getString(DESCRIPTOR)
                    val url=if(json!=null) {
                        val media=Wire.json.decodeFromString<MediaDescriptor>(json);require(item.mediaId==media.trackId.toString());descriptors[media.url]=media;media.url
                    } else {
                        val id=UuidSerializer.parse(extras.getString("dagaz.id")?:throw MediaValidationException());val revision=extras.getLong("dagaz.revision");require(revision>0&&item.mediaId==id.toString())
                        val url=item.requestMetadata.mediaUri?.toString()?:throw MediaValidationException()
                        pending[url]=PendingMedia(id,revision,Quality.valueOf(extras.getString("dagaz.quality")?:throw MediaValidationException()));url
                    }
                    item.buildUpon().setUri(url).setCustomCacheKey(url).build()
                })}catch(error:Exception){Futures.immediateFailedFuture(error)}
            }
        }).build()
        player.addListener(object:Player.Listener {
            override fun onTimelineChanged(timeline:Timeline,reason:Int) {
                val active=(0 until player.mediaItemCount).mapNotNull{player.getMediaItemAt(it).localConfiguration?.uri?.toString()}.toSet()
                descriptors.keys.retainAll(active)
                pending.keys.retainAll(active)
            }
        })
    }
    override fun onGetSession(controllerInfo:MediaSession.ControllerInfo):MediaSession? = if(controllerInfo.packageName==packageName||controllerInfo.isTrusted)session else null
    override fun onDestroy(){session.player.release();session.release();activePlayers.decrementAndGet();descriptors.clear();super.onDestroy()}
}
