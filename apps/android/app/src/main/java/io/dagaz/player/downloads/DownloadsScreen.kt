package io.dagaz.player.downloads

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.media3.common.util.UnstableApi
import io.dagaz.player.api.*
import io.dagaz.player.library.*
import io.dagaz.player.sync.LocalSelection

data class DownloadRow(val track:Track,val ready:MediaDescriptor?,val pending:Boolean,val deleted:Boolean,val error:String?,val outdated:Boolean=false)
data class DownloadsState(val rows:List<DownloadRow> = emptyList(),val selections:List<LocalSelection> = emptyList())

@UnstableApi
@Composable
fun DownloadsScreen(model:LibraryViewModel) {
    val downloads by model.downloads.collectAsState()
    var query by remember{mutableStateOf("")}
    var change by remember{mutableStateOf<Pair<LocalSelection,Quality>?>(null)}
    Column {
        OutlinedTextField(query,{query=it},label={Text("Поиск в скачанном")},singleLine=true,modifier=Modifier.fillMaxWidth().padding(16.dp))
        LazyColumn {
            items(downloads.selections,key={it.id}) { selection ->
                var menu by remember{mutableStateOf(false)}
                ListItem(modifier=Modifier.clickable{model.playSelection(selection.id)},headlineContent={Text(selection.label)},supportingContent={Text(selection.error?:if(!selection.complete)"Подготовка подборки…" else "Снимок · ${qualityText(Quality.valueOf(selection.quality))}")},trailingContent={Box {
                    TextButton(onClick={menu=true}){Text("Действия")}
                    DropdownMenu(menu,{menu=false}) {
                        DropdownMenuItem(text={Text("Обновить снимок и файлы")},onClick={menu=false;model.refreshDownload(selection.id)})
                        Quality.entries.forEach{q->DropdownMenuItem(text={Text("Скачать: ${qualityText(q)}")},onClick={menu=false;change=selection to q})}
                        DropdownMenuItem(text={Text("Убрать из скачанного")},onClick={menu=false;model.removeDownload(selection.id)})
                    }
                }})
            }
            val rows=downloads.rows.filter{listOfNotNull(it.track.title,it.track.filename,it.track.artist,it.track.album).any{name->name.contains(query,true)}}
            items(rows,key={it.track.id.toString()}){row->
                ListItem(leadingContent={LocalCover(row.track)},headlineContent={Text(row.track.title?:row.track.filename)},supportingContent={Text(listOfNotNull(row.track.artist,row.ready?.let{qualityText(it.effectiveQuality)},if(row.deleted)"Удалён на сервере" else if(row.outdated)"Есть обновление" else null,if(row.pending)"Скачивается…" else null,row.error).joinToString(" · "))},modifier=Modifier.clickable(enabled=row.ready!=null){model.playDownloaded(row.track)})
            }
            if(downloads.rows.isEmpty())item{Text("Скачай трек, альбом или плейлист из библиотеки",Modifier.padding(24.dp))}
        }
    }
    change?.let{(selection,quality)->AlertDialog(onDismissRequest={change=null},title={Text("Сменить качество?")},text={Text("Новая общая копия заменит файл во всех подборках после проверки. Старая останется доступна до завершения скачивания.")},confirmButton={TextButton(onClick={change=null;model.refreshDownload(selection.id,quality)}){Text("Скачать")}},dismissButton={TextButton(onClick={change=null}){Text("Отмена")}})}
}
