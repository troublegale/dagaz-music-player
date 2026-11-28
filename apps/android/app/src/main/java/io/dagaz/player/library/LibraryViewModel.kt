package io.dagaz.player.library

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.playback.*
import io.dagaz.player.downloads.*
import io.dagaz.player.sync.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import java.util.UUID
import java.util.Locale

data class LibraryState(val tracks:List<Track> = emptyList(),val albums:List<Album> = emptyList(),val artists:List<Artist> = emptyList(),val playlists:List<Playlist> = emptyList(),val query:String="",val kind:String="Песни",val detail:String?=null,val busy:Boolean=false,val nextCursor:String?=null,val quality:Quality=Quality.high,val downloadQuality:Quality=Quality.high,val error:String?=null)
@UnstableApi
class LibraryViewModel(app:Application):AndroidViewModel(app) {
    // shortcut: visited pages/download summaries stay in memory; use Paging before R3 large-library validation.
    private val mutable=MutableStateFlow(LibraryState());val state=mutable.asStateFlow()
    private val stored=MutableStateFlow(DownloadsState());val downloads=stored.asStateFlow()
    val player=PlayerConnection(app)
    private var api:DagazApi?=null;private var session:Session?=null;private var repository:DownloadRepository?=null
    internal fun editor() = session?.let { io.dagaz.player.editor.LibraryEditor(getApplication(), it) }
    private var albumId:UUID?=null;private var artistId:UUID?=null;private var playlistId:UUID?=null
    private var loading:kotlinx.coroutines.Job?=null;private var preparing:kotlinx.coroutines.Job?=null;private var syncing:kotlinx.coroutines.Job?=null
    init {
        viewModelScope.launch {
            try {
                val connection=withContext(Dispatchers.IO){Credentials(getApplication()).load()}?:throw ClientException(Failure.PairingRequired)
                session=connection;api=DagazApi(connection.baseUrl)
                val repo=withContext(Dispatchers.IO){DownloadRepository.get(getApplication(),connection)};repository=repo
                repo.reconcile()
                launch {combine(repo.dao.watchSelections(),repo.dao.watchMedia(),repo.dao.watchSync()){selections,media,sync->
                    DownloadsState(media.mapNotNull{row->val record=repo.dao.record("track",row.track)?:return@mapNotNull null
                        val track=Wire.json.decodeFromString<Track>(record.payload);val ready=row.ready?.let{Wire.json.decodeFromString<MediaDescriptor>(it)}
                        DownloadRow(track,ready,row.candidate!=null,record.deleted,row.error,ready!=null&&(ready.revision!=track.revision||row.readyGeneration!=sync?.generation))},selections)
                }.flowOn(Dispatchers.IO).collect{stored.value=it}}
                if(withContext(Dispatchers.IO){repo.dao.selections().any{!it.complete}||repo.dao.media().any{it.candidate!=null}})repo.schedule()
                load(false);sync()
            }catch(error:ClientException){mutable.update{it.copy(error=error.failure.label)}}
        }
    }
    private fun reset(){albumId=null;artistId=null;playlistId=null}
    fun query(value:String){mutable.update{it.copy(query=value,kind="Песни",detail=null)};reset();load(false,true)}
    fun kind(value:String){reset();mutable.update{it.copy(kind=value,query="",detail=null)};load(false)}
    fun album(value:Album){reset();albumId=value.id;mutable.update{it.copy(kind="Песни",query="",detail=value.name)};load(false)}
    fun artist(value:Artist){reset();artistId=value.id;mutable.update{it.copy(kind="Песни",query="",detail=value.name)};load(false)}
    fun playlist(value:Playlist){reset();playlistId=value.id;mutable.update{it.copy(kind="Песни",query="",detail=value.name)};load(false)}
    fun quality(value:Quality){mutable.update{it.copy(quality=value,error=null)}}
    fun downloadQuality(value:Quality){mutable.update{it.copy(downloadQuality=value,error=null)}}
    fun refresh(){load(false);sync()}
    private fun sync(){
        val repo=repository?:return;if(syncing?.isActive==true)return
        syncing=viewModelScope.launch {try{CatalogSync(getApplication(),repo.db,repo.session).refresh();repo.cleanup()}catch(_:ClientException){/* The current page and completed downloads remain usable. */}}
    }
    fun more(){if(!state.value.busy&&state.value.nextCursor!=null)load(true)}
    private fun load(append:Boolean,debounce:Boolean=false) {
        val connection=session?:return;val client=api?:return;val repo=repository?:return
        loading?.cancel()
        val requested=state.value;val album=albumId;val artist=artistId;val playlist=playlistId
        mutable.update{if(append)it.copy(busy=true,error=null)else it.copy(tracks=emptyList(),albums=emptyList(),artists=emptyList(),playlists=emptyList(),busy=true,nextCursor=null,error=null)}
        loading=viewModelScope.launch {
            try {
                if(debounce)delay(250)
                val cursor=if(append)requested.nextCursor else null
                if(cursor?.startsWith("local:")==true){localPage(requested,append,cursor,album,artist,playlist);return@launch}
                if(!append){localPage(requested,false,null,album,artist,playlist);mutable.update{it.copy(error=null,busy=true)}}
                when(requested.kind) {
                    "Альбомы"->{val page=client.albums(connection,cursor);mutable.update{it.copy(albums=(if(append)it.albums else emptyList())+page.items,nextCursor=page.nextCursor)}}
                    "Исполнители"->{val page=client.artists(connection,cursor);mutable.update{it.copy(artists=(if(append)it.artists else emptyList())+page.items,nextCursor=page.nextCursor)}}
                    "Плейлисты"->{val page=client.playlists(connection,cursor);mutable.update{it.copy(playlists=(if(append)it.playlists else emptyList())+page.items,nextCursor=page.nextCursor)}}
                    else->{
                        val page=if(playlist==null)client.tracks(connection,requested.query,cursor,album,artist)else {
                            val detail=client.playlist(connection,playlist,cursor)
                            val tracks=withContext(Dispatchers.IO){detail.trackIds.map{id->repo.dao.record("track",id.toString())?.takeUnless{it.deleted}?.let{Wire.json.decodeFromString<Track>(it.payload)}?:client.track(connection,id)}}
                            Page(tracks,detail.nextCursor)
                        }
                        withContext(Dispatchers.IO){page.items.forEach{track->if(repo.dao.record("track",track.id.toString())==null)repo.dao.record(CatalogRow("track",track.id.toString(),Wire.json.encodeToString(Track.serializer(),track),search=listOfNotNull(track.title,track.filename,track.artist,track.album).joinToString(" ").lowercase(Locale.ROOT),album=track.albumId?.toString(),artist=track.artistId?.toString()))}}
                        mutable.update{it.copy(tracks=(if(append)it.tracks else emptyList())+page.items,nextCursor=page.nextCursor)}
                    }
                }
            }catch(error:ClientException){if(error.failure in listOf(Failure.Unavailable,Failure.RateLimited))localPage(requested,append,null,album,artist,playlist)else mutable.update{it.copy(error=error.failure.label)}}
            finally{if(isActive)mutable.update{it.copy(busy=false)}}
        }
    }
    private suspend fun localPage(request:LibraryState,append:Boolean,cursor:String?,album:UUID?,artist:UUID?,playlist:UUID?) {
        val dao=repository?.dao?:return
        val current=state.value
        val offset=cursor?.substringAfter("local:")?.toIntOrNull()?:if(append)current.tracks.size+current.albums.size+current.artists.size+current.playlists.size else 0
        val group=when(request.kind){"Альбомы"->"album";"Исполнители"->"artist";"Плейлисты"->"playlist";else->null}
        val rows=withContext(Dispatchers.IO){if(group!=null)dao.localGroups(group,offset=offset)else if(playlist!=null)dao.playlistTracks(playlist.toString(),request.query.lowercase(Locale.ROOT),offset=offset)else dao.tracks(request.query.lowercase(Locale.ROOT),album?.toString(),artist?.toString(),offline=true,offset=offset)}
        mutable.update{it.copy(tracks=(if(append)it.tracks else emptyList())+if(group==null)rows.map{r->Wire.json.decodeFromString<Track>(r.payload)}else emptyList(),albums=(if(append)it.albums else emptyList())+if(group=="album")rows.map{r->Wire.json.decodeFromString<Album>(r.payload)}else emptyList(),artists=(if(append)it.artists else emptyList())+if(group=="artist")rows.map{r->Wire.json.decodeFromString<Artist>(r.payload)}else emptyList(),playlists=(if(append)it.playlists else emptyList())+if(group=="playlist")rows.map{r->Wire.json.decodeFromString<Playlist>(r.payload)}else emptyList(),nextCursor=if(rows.size==100)"local:${offset+100}"else null,error="Нет связи с сервером · доступно скачанное")}
    }
    fun download(track:Track)=download(SelectionKind.track,track.id,track.title?:track.filename)
    fun downloadAlbum(album:Album)=download(SelectionKind.album,album.id,album.name)
    fun downloadPlaylist(playlist:Playlist)=download(SelectionKind.playlist,playlist.id,playlist.name)
    private fun download(kind:SelectionKind,id:UUID,label:String){viewModelScope.launch{try{repository?.enqueue(kind,id,state.value.downloadQuality,label)}catch(error:ClientException){mutable.update{it.copy(error=error.failure.label)}}}}
    fun removeDownload(id:String){viewModelScope.launch{repository?.removeSelection(id)}}
    fun refreshDownload(id:String,quality:Quality?=null){viewModelScope.launch{repository?.refresh(id,quality)}}
    fun playSelection(id:String){preparing?.cancel();preparing=viewModelScope.launch{
        val items=withContext(Dispatchers.IO){val repo=repository?:return@withContext emptyList();repo.dao.pins(id).mapNotNull{pin->val media=repo.ready(pin.track)?:return@mapNotNull null;val track=repo.dao.record("track",pin.track)?.let{Wire.json.decodeFromString<Track>(it.payload)};playbackItem(media,track?.title?:track?.filename?:"Трек",track?.artist.orEmpty())}}
        if(items.isNotEmpty())player.setQueue(items,0)
    }}
    fun playDownloaded(track:Track){preparing?.cancel();preparing=viewModelScope.launch{val media=withContext(Dispatchers.IO){repository?.ready(track.id.toString())}?:return@launch;player.setQueue(listOf(playbackItem(media,track.title?:track.filename,track.artist.orEmpty())),0)}}
    fun play(index:Int) {
        preparing?.cancel();val snapshot=state.value;val connection=session?:return;val client=api?:return
        preparing=viewModelScope.launch {
            try {
                mutable.update{it.copy(error=null)}
                val track=snapshot.tracks[index];val local=withContext(Dispatchers.IO){repository?.ready(track.id.toString())}
                val media=local?:client.media(connection,track,snapshot.quality)
                val items=snapshot.tracks.mapIndexed{i,item->if(i==index)playbackItem(media,item.title?.takeUnless{it.isBlank()}?:item.filename,item.artist.orEmpty())else queueItem(item,snapshot.quality,connection.baseUrl)}
                player.setQueue(items,index)
            }catch(error:ClientException){mutable.update{it.copy(error=error.failure.label)}}
        }
    }
    fun stop(){preparing?.cancel();player.stop()}
    fun deactivate(){loading?.cancel();syncing?.cancel();stop();player.release()}
    override fun onCleared(){player.release();super.onCleared()}
}
