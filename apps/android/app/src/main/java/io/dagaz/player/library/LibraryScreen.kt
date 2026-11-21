package io.dagaz.player.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.api.Quality
import io.dagaz.player.playback.NowPlayingScreen

fun timeText(ms:Long)="${ms/60000}:${((ms/1000)%60).toString().padStart(2,'0')}"
fun qualityText(quality:Quality)=when(quality){Quality.original->"Оригинал";Quality.high->"Высокое";Quality.low->"Низкое"}
@OptIn(ExperimentalMaterial3Api::class)
@UnstableApi
@Composable
fun LibraryScreen(model:LibraryViewModel,settings:()->Unit) {
    val state by model.state.collectAsStateWithLifecycle()
    val audio by model.player.state.collectAsStateWithLifecycle()
    var tab by rememberSaveable { mutableStateOf("Библиотека") }
    var expanded by rememberSaveable { mutableStateOf(false) }
    var qualityMenu by remember { mutableStateOf(false) }
    val list=remember(state.kind,state.detail,state.query){LazyListState()}
    val latest by rememberUpdatedState(state)
    LaunchedEffect(list){snapshotFlow{Triple(list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?:-1,latest.tracks.size+latest.albums.size+latest.artists.size,latest.nextCursor)}.collect{(last,count,cursor)->if(count>0&&cursor!=null&&last>=count-8)model.more()}}
    if(expanded){NowPlayingScreen(audio,model.player,model::stop){expanded=false};return}
    Scaffold(
        topBar={Row(Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal=20.dp,vertical=12.dp),horizontalArrangement=Arrangement.SpaceBetween){Text("◇ dagaz",style=MaterialTheme.typography.headlineSmall,color=MaterialTheme.colorScheme.primary);TextButton(onClick=settings){Text("Сервер")}}},
        bottomBar={Column {
            if(audio.queue.isNotEmpty()) Surface(Modifier.fillMaxWidth().padding(horizontal=8.dp).clickable{expanded=true},color=MaterialTheme.colorScheme.surfaceContainerHigh,shape=MaterialTheme.shapes.medium) {
                Row(Modifier.padding(12.dp),horizontalArrangement=Arrangement.spacedBy(12.dp)) {
                    Column(Modifier.weight(1f)){Text(audio.title,maxLines=1,overflow=TextOverflow.Ellipsis);Text(if(audio.waiting)"Ожидание данных…" else audio.artist,style=MaterialTheme.typography.bodySmall,color=MaterialTheme.colorScheme.onSurfaceVariant,maxLines=1)}
                    TextButton(onClick=model.player::toggle){Text(if(audio.playRequested)"Пауза" else "Играть")}
                    TextButton(onClick={expanded=true}){Text("Очередь")}
                }
            }
            NavigationBar {listOf("Библиотека" to "♫","Поиск" to "⌕","Скачанное" to "↓").forEach{(name,icon)->NavigationBarItem(selected=tab==name,onClick={tab=name;if(name!="Скачанное")model.query("")},icon={Text(icon)},label={Text(name)})}}
        }}
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if(tab=="Скачанное") {Text("Скачанных треков нет",Modifier.padding(24.dp));return@Column}
            if(tab=="Поиск")OutlinedTextField(state.query,model::query,label={Text("Поиск по библиотеке")},singleLine=true,modifier=Modifier.fillMaxWidth().padding(horizontal=20.dp,vertical=8.dp))
            else Row(Modifier.padding(horizontal=12.dp),horizontalArrangement=Arrangement.spacedBy(4.dp)){listOf("Песни","Альбомы","Исполнители").forEach{name->FilterChip(selected=state.kind==name,onClick={model.kind(name)},label={Text(name)})}}
            Row(Modifier.fillMaxWidth().padding(horizontal=16.dp),horizontalArrangement=Arrangement.SpaceBetween) {
                if(state.detail!=null)TextButton(onClick={model.kind("Песни")}){Text("‹ ${state.detail}",maxLines=1,overflow=TextOverflow.Ellipsis)}else TextButton(onClick=model::refresh){Text("Обновить")}
                Box {TextButton(onClick={qualityMenu=true}){Text(qualityText(state.quality))};DropdownMenu(expanded=qualityMenu,onDismissRequest={qualityMenu=false}){Quality.entries.forEach{quality->DropdownMenuItem(text={Text(qualityText(quality))},onClick={model.quality(quality);qualityMenu=false})}}}
            }
            if(state.busy)LinearProgressIndicator(Modifier.fillMaxWidth())
            state.error?.let{Text(it,Modifier.padding(16.dp),color=MaterialTheme.colorScheme.error)}
            audio.error?.let{Text(it,Modifier.padding(horizontal=16.dp),color=MaterialTheme.colorScheme.error)}
            LazyColumn(Modifier.fillMaxSize(),state=list,contentPadding=PaddingValues(bottom=12.dp)) {
                when(state.kind) {
                    "Альбомы"->itemsIndexed(state.albums,key={_,album->album.id.toString()}){_,album->ListItem(headlineContent={Text(album.name)},supportingContent={Text(album.albumArtist.orEmpty())},leadingContent={Text("▣")},modifier=Modifier.clickable{model.album(album)})}
                    "Исполнители"->itemsIndexed(state.artists,key={_,artist->artist.id.toString()}){_,artist->ListItem(headlineContent={Text(artist.name)},leadingContent={Text("♫")},modifier=Modifier.clickable{model.artist(artist)})}
                    else->itemsIndexed(state.tracks,key={index,track->"$index:${track.id}"}){index,track->ListItem(headlineContent={Text(track.title?.takeUnless{it.isBlank()}?:track.filename,maxLines=1,overflow=TextOverflow.Ellipsis)},supportingContent={Text(track.artist?.takeUnless{it.isBlank()}?:"Неизвестный исполнитель",maxLines=1,overflow=TextOverflow.Ellipsis)},leadingContent={Text(if(audio.queue.getOrNull(audio.queueIndex)?.id==track.id.toString()&&audio.playing)"Ⅱ" else "♫",color=MaterialTheme.colorScheme.primary)},trailingContent={Text(timeText(track.durationMs),style=MaterialTheme.typography.bodySmall)},modifier=Modifier.clickable{model.play(index)})}
                }
                if(!state.busy&&state.tracks.isEmpty()&&state.albums.isEmpty()&&state.artists.isEmpty()&&state.error==null)item{Text(if(state.query.isEmpty())"В библиотеке пока нет музыки" else "Ничего не найдено",Modifier.padding(24.dp))}
                if(state.nextCursor!=null)item{TextButton(onClick=model::more,enabled=!state.busy,modifier=Modifier.fillMaxWidth()){Text("Ещё")}}
            }
        }
    }
}
