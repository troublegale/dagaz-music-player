package io.dagaz.player.editor

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.AtomicFile
import io.dagaz.player.api.*
import io.dagaz.player.api.Job
import io.dagaz.player.sync.LocalDatabase
import io.dagaz.player.playback.RetrySchedule
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*
import java.io.File
import java.io.IOException
import java.io.RandomAccessFile
import java.security.MessageDigest
import java.util.UUID

internal fun textPatch(fields: Map<String, String?>) = buildJsonObject {
    fields.toSortedMap().forEach { (name, value) ->
        if (value != null) put(name, if (value.isBlank()) JsonNull else JsonPrimitive(value.trim()))
    }
}

internal fun chunkOperation(id: UUID, offset: Long): UUID =
    UUID.nameUUIDFromBytes("$id:chunk:$offset".toByteArray())

private fun ByteArray.hex() = joinToString("") { "%02x".format(it) }

@Serializable
internal data class StagedAudio(
    val operation: String,
    val filename: String,
    val length: Long,
    val sha: String,
    val attempt: String = operation,
    val uploadId: String? = null,
) {
    val id get() = UUID.fromString(operation)
}

internal class LibraryEditor(private val context: Context, private val session: Session,
    private val resolver: android.content.ContentResolver = context.contentResolver) {
    private val api = DagazApi(session.baseUrl)
    private val folder = File(context.noBackupFilesDir, "uploads-${LocalDatabase.scope(session)}")
    private companion object { val stagingLock = Mutex() }

    suspend fun online() = api.checkAccess(session)

    suspend fun staged(): List<StagedAudio> = withContext(Dispatchers.IO) {
        stagingLock.withLock {
            val files = folder.listFiles().orEmpty()
            val records = files.filter { it.name.endsWith(".json") || it.name.endsWith(".json.bak") }
                .map { it.name.removeSuffix(".bak") }.toSet()
            for (file in files) {
                if (file.extension == "audio" && "${file.nameWithoutExtension}.json" !in records) file.delete()
            }
            records.mapNotNull { runCatching { readRecord(File(folder, it)) }.getOrNull() }
        }
    }

    suspend fun stage(uri: Uri): StagedAudio = withContext(Dispatchers.IO) {
        stagingLock.withLock { copyAudio(uri) }
    }

    private suspend fun copyAudio(uri: Uri): StagedAudio {
        val filename = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        } ?: throw IOException("Не удалось прочитать имя файла.")
        if (filename.substringAfterLast('.').lowercase() !in setOf("wav", "mp3", "flac")) throw ClientException(Failure.InvalidFile)
        folder.mkdirs()
        val id = UUID.randomUUID()
        val audio = File(folder, "$id.audio")
        val digest = MessageDigest.getInstance("SHA-256")
        val limit = 8L * 1024 * 1024 * 1024
        try {
            var size = 0L
            val input = resolver.openInputStream(uri) ?: throw IOException("Выбери файл снова.")
            input.use { source ->
                audio.outputStream().use { out ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        currentCoroutineContext().ensureActive()
                        val n = source.read(buffer)
                        if (n < 0) break
                        size += n
                        if (size > limit) throw ClientException(Failure.TooLarge)
                        if (folder.usableSpace < n + 16L * 1024 * 1024) throw IOException("На устройстве недостаточно места.")
                        digest.update(buffer, 0, n)
                        out.write(buffer, 0, n)
                    }
                    out.fd.sync()
                }
            }
            if (size == 0L) throw ClientException(Failure.InvalidFile)
            val item = StagedAudio(id.toString(), filename, size, digest.digest().hex())
            saveRecord(item)
            return item
        } catch (error: Exception) {
            audio.delete()
            throw error
        }
    }

    private fun readRecord(file: File) =
        Wire.json.decodeFromString<StagedAudio>(AtomicFile(file).readFully().decodeToString())

    private fun saveRecord(item: StagedAudio) {
        val record = AtomicFile(File(folder, "${item.id}.json"))
        val output = record.startWrite()
        try {
            output.write(Wire.json.encodeToString(StagedAudio.serializer(), item).encodeToByteArray())
            record.finishWrite(output)
        } catch (error: Exception) {
            record.failWrite(output)
            throw error
        }
    }

    suspend fun discard(item: StagedAudio) = withContext(Dispatchers.IO) {
        stagingLock.withLock {
            AtomicFile(File(folder, "${item.id}.json")).delete()
            File(folder, "${item.id}.audio").delete()
        }
    }

    suspend fun upload(item: StagedAudio, progress: (Long) -> Unit): Job = withContext(Dispatchers.IO) {
        var draft = readRecord(File(folder, "${item.id}.json"))
        val audio = File(folder, "${item.id}.audio")
        if (audio.length() != draft.length) throw IOException("Локальная копия недоступна. Выбери файл снова.")
        val create = buildJsonObject {
            put("filename", draft.filename); put("total_bytes", draft.length); put("sha256", draft.sha)
        }
        suspend fun start() = retryUpload {
            api.write(session, "uploads", "POST", UUID.fromString(draft.attempt), 201, UploadState.serializer(), create)
        }
        val savedId = draft.uploadId?.let(UuidSerializer::parse)
        var upload = if (savedId == null) start() else retryUpload {
            api.read(session, "uploads/$savedId", UploadState.serializer())
        }
        if (savedId != null && upload.id != savedId) throw ClientException(Failure.InvalidResponse)
        if (upload.status == UploadStatus.expired) {
            draft = draft.copy(attempt = UUID.randomUUID().toString(), uploadId = null)
            saveRecord(draft)
            upload = start()
        }
        val path = "uploads/${upload.id}"
        if (upload.totalBytes != draft.length || upload.offset > draft.length) throw ClientException(Failure.InvalidResponse)
        saveRecord(draft.copy(uploadId = upload.id.toString()))
        val attempt = UUID.fromString(draft.attempt)
        val offset = upload.offset
        progress(offset)
        RandomAccessFile(audio, "r").use { source ->
            source.seek(offset)
            while (upload.offset < draft.length) {
                ensureActive()
                val start = upload.offset
                val bytes = ByteArray(minOf(1024L * 1024, draft.length - start).toInt())
                source.readFully(bytes)
                val updated = retryUpload { api.write(session, path, "PATCH", chunkOperation(attempt, start), 200,
                    UploadState.serializer(), bytes = bytes, headers = mapOf(
                        "Upload-Offset" to start.toString(),
                        "X-Chunk-SHA256" to MessageDigest.getInstance("SHA-256").digest(bytes).hex())) }
                if (updated.id != upload.id || updated.totalBytes != draft.length || updated.offset != start + bytes.size)
                    throw ClientException(Failure.InvalidResponse)
                upload = updated
                progress(upload.offset)
            }
        }
        val complete = UUID.nameUUIDFromBytes("$attempt:complete".toByteArray())
        val result = await(retryUpload { api.write(session, "$path/complete", "POST", complete, 202, Job.serializer()) })
        if (result.status == JobStatus.succeeded) discard(item)
        result
    }

    suspend fun artwork(uri: Uri): UUID = withContext(Dispatchers.IO) {
        val limit = 10 * 1024 * 1024
        val bytes = resolver.openInputStream(uri)?.use { input ->
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val count = input.read(buffer)
                if (count < 0) break
                if (output.size() + count > limit) throw ClientException(Failure.TooLarge)
                output.write(buffer, 0, count)
            }
            output.toByteArray()
        }
            ?: throw IOException("Выбери обложку снова.")
        val options = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
        android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        val type = options.outMimeType
        if (type !in listOf("image/png", "image/jpeg") || options.outWidth <= 0 || options.outHeight <= 0 ||
            options.outWidth.toLong() * options.outHeight > 25_000_000) throw ClientException(Failure.InvalidFile)
        val result = api.write(session, "artworks", "POST", UUID.randomUUID(), 201, JsonObject.serializer(), bytes = bytes, contentType = type!!)
        UuidSerializer.parse(result.getValue("id").jsonPrimitive.content)
    }

    suspend fun edit(tracks: List<Track>, patch: JsonObject, operation: UUID = UUID.randomUUID()): List<Job> {
        MetadataPatch(patch)
        val single = tracks.singleOrNull()
        if (single != null) {
            val payload = buildJsonObject { put("expected_revision", single.revision); put("patch", patch) }
            val key = UUID.nameUUIDFromBytes("$operation:${single.id}:$payload".toByteArray())
            return listOf(await(retryUpload {
                api.write(session, "tracks/${single.id}/metadata", "PATCH", key, 202, Job.serializer(), payload)
            }))
        }
        return tracks.distinctBy { it.id }.chunked(100).map { batch ->
            val payload = buildJsonObject {
                put("patch", patch)
                putJsonArray("items") { batch.forEach { track ->
                    add(buildJsonObject { put("track_id", track.id.toString()); put("expected_revision", track.revision) })
                } }
            }
            val key = UUID.nameUUIDFromBytes("$operation:$payload".toByteArray())
            await(retryUpload { api.write(session, "tracks/metadata-batch", "POST", key, 202, Job.serializer(), payload) })
        }
    }

    suspend fun playlist(id: UUID): Playlist {
        val first = api.playlist(session, id)
        var cursor = first.nextCursor
        val entries = first.trackIds.toMutableList()
        while (cursor != null) {
            val page = api.playlist(session, id, cursor)
            if (page.id != id || page.revision != first.revision || cursor == page.nextCursor) throw ClientException(Failure.Conflict)
            entries.addAll(page.trackIds)
            cursor = page.nextCursor
        }
        if (entries.size.toLong() != first.totalCount) throw ClientException(Failure.InvalidResponse)
        return first.copy(trackIds = entries, nextCursor = null)
    }

    suspend fun search(query: String) = api.tracks(session, query).items
    suspend fun track(id: UUID) = api.track(session, id)
    suspend fun createPlaylist(name: String, key: UUID) = api.write(session, "playlists", "POST", key, 201,
        Playlist.serializer(), buildJsonObject { put("name", name) }).also {
            if (it.revision != 1L || it.totalCount != 0L || it.name != name.trim()) throw ClientException(Failure.Conflict)
        }
    suspend fun renamePlaylist(playlist: Playlist, name: String, key: UUID) = api.write(session, "playlists/${playlist.id}", "PATCH", key, 200,
        Playlist.serializer(), buildJsonObject { put("name", name); put("expected_revision", playlist.revision) }).also {
            if (it.id != playlist.id || it.revision != playlist.revision + 1 || it.name != name.trim())
                throw ClientException(Failure.Conflict)
        }
    suspend fun savePlaylist(playlist: Playlist, entries: List<UUID>, key: UUID) = api.write(session, "playlists/${playlist.id}/tracks", "PUT", key, 200,
        Playlist.serializer(), buildJsonObject {
            put("expected_revision", playlist.revision)
            putJsonArray("track_ids") { entries.forEach { add(it.toString()) } }
        }).also {
            if (it.id != playlist.id || it.revision != playlist.revision + 1) throw ClientException(Failure.Conflict)
        }
    suspend fun deletePlaylist(playlist: Playlist, key: UUID) = api.write(session, "playlists/${playlist.id}", "DELETE", key, 204,
        JsonObject.serializer(), buildJsonObject { put("expected_revision", playlist.revision) })

    private suspend fun await(initial: Job): Job {
        var job = initial
        while (job.status == JobStatus.queued || job.status == JobStatus.running) {
            delay(750)
            job = retryUpload { api.read(session, "jobs/${initial.id}", Job.serializer()) }
        }
        return job
    }

    private suspend fun <T> retryUpload(request: suspend () -> T): T {
        repeat(3) { attempt ->
            try { return request() }
            catch (error: ClientException) {
                if (error.failure != Failure.Unavailable && error.failure != Failure.RateLimited) throw error
                delay(RetrySchedule.delayMs(attempt + 1))
            }
        }
        return request()
    }
}
