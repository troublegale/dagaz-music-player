package io.dagaz.player.playback

import android.content.ComponentName
import android.content.Context
import androidx.media3.common.*
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.*
import io.dagaz.player.api.MediaDescriptor
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

data class QueueEntry(val id:String,val title:String,val artist:String)
data class AudioUiState(val connected:Boolean=false,val playing:Boolean=false,val playRequested:Boolean=false,val waiting:Boolean=false,val title:String="",val artist:String="",val positionMs:Long=0,val durationMs:Long=0,val queue:List<QueueEntry> = emptyList(),val playOrder:List<Int> = emptyList(),val queueIndex:Int=0,val repeat:Int=Player.REPEAT_MODE_OFF,val shuffle:Boolean=false,val error:String?=null)
@UnstableApi
class PlayerConnection(private val context:Context) {
    private val scope=CoroutineScope(SupervisorJob()+Dispatchers.Main.immediate)
    private val mutable=MutableStateFlow(AudioUiState());val state=mutable.asStateFlow()
    private var controller:MediaController?=null
    private val future=MediaController.Builder(context,SessionToken(context,ComponentName(context,PlaybackService::class.java))).buildAsync()
    init {
        future.addListener({try {
            val player=future.get();controller=player
            player.addListener(object:Player.Listener {
                override fun onEvents(player:Player,events:Player.Events){publish()}
                override fun onPlayerError(error:PlaybackException){mutable.update{it.copy(error="Не удалось воспроизвести трек. Проверь доступ и сеть.")}}
            });publish()
        }catch(_:Exception){mutable.update{it.copy(error="Не удалось подключиться к плееру.")}}},context.mainExecutor)
        scope.launch {while(isActive){delay(500);controller?.let{player->mutable.update{it.copy(positionMs=player.currentPosition.coerceAtLeast(0),durationMs=player.duration.coerceAtLeast(0))}}}}
    }
    private fun publish() {
        val player=controller?:return
        val queue=(0 until player.mediaItemCount).map{index->val item=player.getMediaItemAt(index);QueueEntry(item.mediaId,item.mediaMetadata.title?.toString()?:"Трек",item.mediaMetadata.artist?.toString().orEmpty())}
        val order=mutableListOf<Int>();val timeline=player.currentTimeline;var index=timeline.getFirstWindowIndex(player.shuffleModeEnabled)
        while(index!=C.INDEX_UNSET&&order.size<queue.size){order.add(index);index=timeline.getNextWindowIndex(index,Player.REPEAT_MODE_OFF,player.shuffleModeEnabled)}
        mutable.update{it.copy(connected=true,playing=player.isPlaying,playRequested=player.playWhenReady,waiting=player.playbackState==Player.STATE_BUFFERING,title=player.currentMediaItem?.mediaMetadata?.title?.toString().orEmpty(),artist=player.currentMediaItem?.mediaMetadata?.artist?.toString().orEmpty(),positionMs=player.currentPosition.coerceAtLeast(0),durationMs=player.duration.coerceAtLeast(0),queue=queue,playOrder=if(order.size==queue.size)order else queue.indices.toList(),queueIndex=player.currentMediaItemIndex.coerceAtLeast(0),repeat=player.repeatMode,shuffle=player.shuffleModeEnabled,error=if(player.playerError==null)null else it.error)}
    }
    private suspend fun ready():MediaController = controller?:suspendCancellableCoroutine { continuation ->
        future.addListener({if(continuation.isActive)try{continuation.resume(future.get())}catch(error:Exception){continuation.resumeWithException(error)}},context.mainExecutor)
    }
    suspend fun prepare(media:MediaDescriptor,positionMs:Long)=withContext(Dispatchers.Main.immediate){val player=ready();player.pause();player.setMediaItem(playbackItem(media));player.seekTo(positionMs);player.prepare()}
    suspend fun setQueue(items:List<MediaItem>,index:Int)=withContext(Dispatchers.Main.immediate){val player=ready();player.setMediaItems(items,index,0);player.prepare();player.play()}
    fun toggle(){controller?.let{if(it.playWhenReady)it.pause()else{if(it.playerError!=null)it.prepare();it.play()}}}
    fun pause(){controller?.pause()}
    fun play(){controller?.let{if(it.playerError!=null)it.prepare();it.play()}}
    fun stop(){controller?.let{it.stop();it.clearMediaItems()}}
    fun seek(positionMs:Long){controller?.seekTo(positionMs.coerceAtLeast(0))}
    fun select(index:Int){controller?.seekToDefaultPosition(index)}
    fun next(){controller?.seekToNextMediaItem()}
    fun previous(){controller?.seekToPreviousMediaItem()}
    fun repeat(){controller?.let{it.repeatMode=when(it.repeatMode){Player.REPEAT_MODE_OFF->Player.REPEAT_MODE_ALL;Player.REPEAT_MODE_ALL->Player.REPEAT_MODE_ONE;else->Player.REPEAT_MODE_OFF}}}
    fun shuffle(){controller?.let{it.shuffleModeEnabled=!it.shuffleModeEnabled}}
    fun release(){scope.cancel();val idle=controller?.mediaItemCount==0;controller?.release();if(controller==null)MediaController.releaseFuture(future);if(idle)context.stopService(android.content.Intent(context,PlaybackService::class.java));controller=null}
}
