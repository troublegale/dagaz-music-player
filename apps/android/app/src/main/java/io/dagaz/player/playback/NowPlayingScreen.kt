package io.dagaz.player.playback

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.library.timeText

@UnstableApi
@Composable
fun NowPlayingScreen(state:AudioUiState,player:PlayerConnection,stop:()->Unit,back:()->Unit) {
    BackHandler(onBack=back)
    LaunchedEffect(state.queue.isEmpty()){if(state.queue.isEmpty())back()}
    var seeking by remember { mutableStateOf<Float?>(null) }
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().safeDrawingPadding().padding(20.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
            TextButton(onClick=back){Text("‹ Библиотека")}
            Text(state.title,style=MaterialTheme.typography.headlineSmall,maxLines=2,overflow=TextOverflow.Ellipsis)
            Text(state.artist,color=MaterialTheme.colorScheme.onSurfaceVariant,maxLines=1,overflow=TextOverflow.Ellipsis)
            Text(if(state.waiting)"Ожидание данных…" else if(state.playing)"Играет на этом телефоне" else "Пауза",style=MaterialTheme.typography.bodySmall)
            state.error?.let{Text(it,color=MaterialTheme.colorScheme.error)}
            Slider(value=seeking?:state.positionMs.toFloat().coerceIn(0f,state.durationMs.toFloat().coerceAtLeast(1f)),onValueChange={seeking=it},onValueChangeFinished={seeking?.let{player.seek(it.toLong())};seeking=null},valueRange=0f..state.durationMs.toFloat().coerceAtLeast(1f),enabled=state.durationMs>0)
            Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceBetween){Text(timeText(state.positionMs));Text(timeText(state.durationMs))}
            Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceEvenly){TextButton(onClick=player::previous){Text("Назад")};Button(onClick=player::toggle){Text(if(state.playRequested)"Пауза" else "Играть")};TextButton(onClick=player::next){Text("Далее")}}
            Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceEvenly){FilterChip(selected=state.shuffle,onClick=player::shuffle,label={Text("Перемешать")});FilterChip(selected=state.repeat!=Player.REPEAT_MODE_OFF,onClick=player::repeat,label={Text(when(state.repeat){Player.REPEAT_MODE_ONE->"Повтор: трек";Player.REPEAT_MODE_ALL->"Повтор: все";else->"Повтор"})});TextButton(onClick=stop){Text("Стоп")}}
            Text("Очередь",style=MaterialTheme.typography.titleMedium)
            LazyColumn(Modifier.weight(1f)){itemsIndexed(state.playOrder,key={_,index->index}){position,index->val item=state.queue[index];ListItem(headlineContent={Text(item.title,maxLines=1,overflow=TextOverflow.Ellipsis)},supportingContent={Text(item.artist,maxLines=1)},leadingContent={Text(if(index==state.queueIndex)"▶" else "${position+1}")},modifier=Modifier.clickable{player.select(index)})}}
        }
    }
}
