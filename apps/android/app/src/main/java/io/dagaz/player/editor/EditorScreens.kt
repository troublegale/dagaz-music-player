package io.dagaz.player.editor

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import io.dagaz.player.api.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*
import java.util.UUID

internal fun Throwable.editorMessage() = when (this) {
    is ClientException -> failure.label
    is SecurityException -> "Доступ к файлу отозван. Выбери его снова."
    is java.io.IOException -> message ?: "Не удалось прочитать или сохранить файл."
    else -> "Не удалось выполнить действие."
}

@Composable
internal fun UploadScreen(editor: LibraryEditor, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    var queue by remember { mutableStateOf(emptyList<StagedAudio>()) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf("") }
    suspend fun upload(items: List<StagedAudio>) {
        val results = mutableListOf<String>()
        for (item in items) {
            try {
                val job = editor.upload(item) { sent ->
                    message = "${item.filename} · ${sent * 100 / item.length}%"
                }
                results += "${item.filename} · " + if (job.status == JobStatus.succeeded) "добавлен" else "импорт не удался"
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { results += "${item.filename} · ${error.editorMessage()}" }
        }
        queue = editor.staged()
        message = results.joinToString("\n")
    }
    LaunchedEffect(editor) { queue = editor.staged() }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) scope.launch {
            busy = true
            try {
                val items = mutableListOf<StagedAudio>()
                val errors = mutableListOf<String>()
                for (uri in uris) {
                    try { message = "Читаем файл…"; items += editor.stage(uri) }
                    catch (error: CancellationException) { throw error }
                    catch (error: Exception) { errors += error.editorMessage() }
                }
                queue = editor.staged()
                upload(items)
                if (errors.isNotEmpty()) message = (errors + message).joinToString("\n")
            } finally { busy = false }
        }
    }
    BackHandler(onBack = back)
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            TextButton(onClick = back) { Text("К библиотеке") }
            Button(enabled = !busy, onClick = { picker.launch(arrayOf("audio/*", "application/octet-stream")) }) { Text("Выбрать файлы") }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (message.isNotEmpty()) Text(message)
        }
        items(queue, key = { it.operation }) { item ->
            ListItem(headlineContent = { Text(item.filename) }, supportingContent = {
                TextButton(enabled = !busy, onClick = { scope.launch { busy = true; try { upload(listOf(item)) } finally { busy = false } } }) { Text("Продолжить загрузку") }
            }, trailingContent = {
                TextButton(enabled = !busy, onClick = { scope.launch { editor.discard(item); queue = editor.staged() } }) { Text("Убрать") }
            })
        }
    }
}

@Composable
internal fun MetadataEditorScreen(editor: LibraryEditor, tracks: List<Track>, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val single = tracks.singleOrNull()
    val original = remember(tracks) { mapOf(
        "title" to single?.title.orEmpty(), "artist" to single?.artist.orEmpty(),
        "album" to single?.album.orEmpty(), "album_artist" to single?.albumArtist.orEmpty()) }
    val fields = remember(tracks) { mutableStateMapOf<String, String?>() }
    var cover by remember { mutableStateOf<Uri?>(null) }
    var clearCover by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var online by remember { mutableStateOf(false) }
    var finished by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf("") }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) { cover = uri; clearCover = false }
    }
    LaunchedEffect(editor) { try { editor.online(); online = true } catch (error: ClientException) { message = error.failure.label } }
    BackHandler(onBack = back)
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item { TextButton(onClick = back) { Text("К библиотеке") }; Text(single?.title ?: single?.filename ?: "Выбрано треков · ${tracks.size}") }
        for ((key, label) in mapOf("title" to "Название", "artist" to "Исполнитель", "album" to "Альбом", "album_artist" to "Исполнитель альбома")) item(key) {
            OutlinedTextField(fields[key] ?: original.getValue(key), { fields[key] = it }, label = { Text(label) },
                modifier = Modifier.fillMaxWidth(), enabled = !busy && !finished && online, singleLine = true)
            if (single == null) TextButton(enabled = !busy && !finished && online, onClick = { fields[key] = "" }) { Text("Очистить поле «$label»") }
        }
        item {
            Row {
                TextButton(enabled = !busy && !finished && online, onClick = { picker.launch(arrayOf("image/jpeg","image/png")) }) { Text(if (cover == null) "Выбрать обложку" else "Обложка выбрана") }
                TextButton(enabled = !busy && !finished && online, onClick = { cover = null; clearCover = !clearCover }) { Text(if (clearCover) "Отменить удаление" else "Убрать обложку") }
            }
            Button(enabled = online && !busy && !finished && (fields.isNotEmpty() || cover != null || clearCover), onClick = {
                scope.launch {
                    busy = true
                    try {
                        val edits = textPatch(fields)
                        val artwork = cover?.let { editor.artwork(it) }
                        val patch = buildJsonObject {
                            edits.forEach { (key, value) -> put(key, value) }
                            if (artwork != null) put("artwork_id", artwork.toString()) else if (clearCover) put("artwork_id", JsonNull)
                        }
                        val results = editor.edit(tracks, patch)
                        val failures = results.sumOf { it.errors.size }
                        val conflicts = results.sumOf { job -> job.errors.count { it.code == "revision_conflict" } }
                        finished = true
                        message = if (results.all { it.status == JobStatus.succeeded }) "Изменения сохранены."
                        else "Не обновлено: $failures. Изменились на сервере: $conflicts. Обнови библиотеку перед повторной правкой."
                    } catch (error: CancellationException) { throw error }
                    catch (error: Exception) { message = error.editorMessage() }
                    finally { busy = false }
                }
            }) { Text("Сохранить") }
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (message.isNotEmpty()) Text(message)
        }
    }
}
