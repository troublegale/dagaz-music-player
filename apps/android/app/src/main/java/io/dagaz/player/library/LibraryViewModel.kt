package io.dagaz.player.library

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.playback.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import java.util.UUID

data class LibraryState(val tracks:List<Track> = emptyList(),val albums:List<Album> = emptyList(),val artists:List<Artist> = emptyList(),val query:String="",val kind:String="Песни",val detail:String?=null,val busy:Boolean=false,val nextCursor:String?=null,val quality:Quality=Quality.high,val error:String?=null)
@UnstableApi
class LibraryViewModel(app:Application):AndroidViewModel(app) {
    // shortcut: visited pages stay in memory in A2; move them to Room/Paging in A3 before large-library validation.
    private val mutable=MutableStateFlow(LibraryState());val state=mutable.asStateFlow()
    val player=PlayerConnection(app)
    private var api:DagazApi?=null;private var session:Session?=null
    private var albumId:UUID?=null;private var artistId:UUID?=null
    private var loading:kotlinx.coroutines.Job?=null;private var preparing:kotlinx.coroutines.Job?=null
    init {refresh()}
    fun query(value:String){mutable.update{it.copy(query=value,kind="Песни",detail=null)};albumId=null;artistId=null;load(false,true)}
    fun kind(value:String){albumId=null;artistId=null;mutable.update{it.copy(kind=value,query="",detail=null)};load(false)}
    fun album(value:Album){albumId=value.id;artistId=null;mutable.update{it.copy(kind="Песни",query="",detail=value.name)};load(false)}
    fun artist(value:Artist){artistId=value.id;albumId=null;mutable.update{it.copy(kind="Песни",query="",detail=value.name)};load(false)}
    fun quality(value:Quality){mutable.update{it.copy(quality=value,error=null)}}
    fun refresh(){load(false)}
    fun more(){if(!state.value.busy&&state.value.nextCursor!=null)load(true)}
    private fun load(append:Boolean,debounce:Boolean=false) {
        loading?.cancel()
        val requested=state.value;val album=albumId;val artist=artistId
        mutable.update{if(append)it.copy(busy=true,error=null)else it.copy(tracks=emptyList(),albums=emptyList(),artists=emptyList(),busy=true,nextCursor=null,error=null)}
        loading=viewModelScope.launch {
            try {
                if(debounce)delay(250)
                if(session==null){session=withContext(Dispatchers.IO){Credentials(getApplication()).load()}?:throw ClientException(Failure.PairingRequired);api=DagazApi(session!!.baseUrl)}
                val cursor=if(append)requested.nextCursor else null
                when(requested.kind) {
                    "Альбомы"->{val page=api!!.albums(session!!,cursor);mutable.update{it.copy(albums=(if(append)it.albums else emptyList())+page.items,nextCursor=page.nextCursor)}}
                    "Исполнители"->{val page=api!!.artists(session!!,cursor);mutable.update{it.copy(artists=(if(append)it.artists else emptyList())+page.items,nextCursor=page.nextCursor)}}
                    else->{val page=api!!.tracks(session!!,requested.query,cursor,album,artist);mutable.update{it.copy(tracks=(if(append)it.tracks else emptyList())+page.items,nextCursor=page.nextCursor)}}
                }
            }catch(error:ClientException){mutable.update{it.copy(error=error.failure.label)}}
            finally{if(isActive)mutable.update{it.copy(busy=false)}}
        }
    }
    fun play(index:Int) {
        preparing?.cancel();val snapshot=state.value;val connection=session?:return;val client=api?:return
        preparing=viewModelScope.launch {
            try {
                mutable.update{it.copy(error=null)}
                val track=snapshot.tracks[index];val media=client.media(connection,track,snapshot.quality)
                val items=snapshot.tracks.mapIndexed{i,item->if(i==index)playbackItem(media,item.title?.takeUnless{it.isBlank()}?:item.filename,item.artist.orEmpty())else queueItem(item,snapshot.quality,connection.baseUrl)}
                player.setQueue(items,index)
            }catch(error:ClientException){mutable.update{it.copy(error=error.failure.label)}}
        }
    }
    fun stop(){preparing?.cancel();player.stop()}
    fun deactivate(){loading?.cancel();stop();player.release()}
    override fun onCleared(){player.release();super.onCleared()}
}
