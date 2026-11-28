package io.dagaz.player.editor

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.dagaz.player.api.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.util.UUID

@Composable
internal fun PlaylistEditorScreen(editor: LibraryEditor, initial: Playlist?, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    var playlist by remember { mutableStateOf<Playlist?>(null) }
    var name by remember { mutableStateOf(initial?.name.orEmpty()) }
    var entries by remember { mutableStateOf(emptyList<UUID>()) }
    val titles = remember { mutableStateMapOf<UUID, String>() }
    var query by remember { mutableStateOf("") }
    var found by remember { mutableStateOf(emptyList<Track>()) }
    var message by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var online by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    // Keep an operation key with its exact request until a reply is received.
    val keys = remember { mutableMapOf<Any, UUID>() }
    fun key(request: Any) = keys.getOrPut(request) { UUID.randomUUID() }
    fun perform(action: suspend () -> Unit) {
        scope.launch {
            busy = true
            message = ""
            try { action() }
            catch (error: CancellationException) { throw error }
            catch (error: Exception) { message = error.editorMessage() }
            finally { busy = false }
        }
    }
    LaunchedEffect(editor, initial?.id) {
        busy = true
        try {
            editor.online()
            if (initial != null) {
                val current = editor.playlist(initial.id)
                playlist = current
                name = current.name
                entries = current.trackIds
            }
            online = true
        } catch (error: ClientException) { message = error.failure.label }
        finally { busy = false }
    }
    BackHandler(onBack = back)
    val enabled = online && !busy
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            TextButton(onClick = back) { Text("К библиотеке") }
            OutlinedTextField(name, { name = it }, label = { Text("Название плейлиста") }, enabled = enabled, singleLine = true, modifier = Modifier.fillMaxWidth())
            Row {
                Button(enabled = enabled && name.isNotBlank(), onClick = {
                    perform {
                        val label = name.trim()
                        var current = playlist ?: editor.createPlaylist(label, key("create" to label))
                        playlist = current
                        if (current.name != label) {
                            current = editor.renamePlaylist(current, label, key(Triple(current.id, current.revision, label)))
                            playlist = current.copy(trackIds = playlist!!.trackIds)
                        }
                        if (playlist!!.trackIds != entries) {
                            val updated = editor.savePlaylist(current, entries, key(Triple(current.id, current.revision, entries)))
                            playlist = updated.copy(trackIds = entries)
                        }
                        back()
                    }
                }) { Text("Сохранить") }
                if (playlist != null) TextButton(enabled = enabled, onClick = { confirmDelete = true }) { Text("Удалить плейлист") }
            }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (message.isNotEmpty()) Text(message)
        }
        itemsIndexed(entries) { index, id ->
            LaunchedEffect(id) {
                if (id !in titles) try {
                    val track = editor.track(id)
                    titles[id] = track.title ?: track.filename
                } catch (_: ClientException) { titles[id] = "Трек недоступен" }
            }
            ListItem(headlineContent = { Text(titles[id] ?: "Загрузка названия…") }, supportingContent = {
                Row {
                    TextButton(enabled = enabled && index > 0, onClick = {
                        entries = entries.toMutableList().apply { add(index - 1, removeAt(index)) }
                    }) { Text("Выше") }
                    TextButton(enabled = enabled && index + 1 < entries.size, onClick = {
                        entries = entries.toMutableList().apply { add(index + 1, removeAt(index)) }
                    }) { Text("Ниже") }
                    TextButton(enabled = enabled, onClick = { entries = entries.toMutableList().apply { removeAt(index) } }) { Text("Убрать") }
                }
            })
        }
        item {
            OutlinedTextField(query, { query = it }, label = { Text("Найти треки для плейлиста") }, enabled = enabled, singleLine = true, modifier = Modifier.fillMaxWidth())
            TextButton(enabled = enabled, onClick = { perform { found = editor.search(query) } }) { Text("Найти") }
        }
        itemsIndexed(found) { _, track ->
            ListItem(headlineContent = { Text(track.title ?: track.filename) }, supportingContent = { Text(track.artist.orEmpty()) },
                trailingContent = { TextButton(enabled = enabled, onClick = {
                    titles[track.id] = track.title ?: track.filename
                    entries = entries + track.id
                }) { Text("Добавить") } })
        }
    }
    if (confirmDelete) AlertDialog(onDismissRequest = { confirmDelete = false }, title = { Text("Удалить плейлист?") },
        text = { Text("Его треки останутся в библиотеке.") },
        confirmButton = { TextButton(onClick = {
            confirmDelete = false
            val current = playlist ?: return@TextButton
            perform { editor.deletePlaylist(current, key("delete" to current.revision)); back() }
        }) { Text("Удалить") } }, dismissButton = {
            TextButton(onClick = { confirmDelete = false }) { Text("Отмена") }
        })
}
