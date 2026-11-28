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
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*
import java.io.File
import java.io.IOException
import java.io.RandomAccessFile
import java.security.MessageDigest
import java.util.UUID

internal fun textPatch(fields: Map<String, String?>) = buildJsonObject {
    fields.forEach { (name, value) ->
        if (value != null) put(name, if (value.isBlank()) JsonNull else JsonPrimitive(value.trim()))
    }
}

internal fun chunkOperation(id: UUID, offset: Long): UUID =
    UUID.nameUUIDFromBytes("$id:chunk:$offset".toByteArray())

private fun ByteArray.hex() = joinToString("") { "%02x".format(it) }

@Serializable
internal data class StagedAudio(val operation: String, val filename: String, val length: Long, val sha: String) {
    val id get() = UUID.fromString(operation)
}

internal class LibraryEditor(private val context: Context, private val session: Session,
    private val resolver: android.content.ContentResolver = context.contentResolver) {
    private val api = DagazApi(session.baseUrl)
    private val folder = File(context.noBackupFilesDir, "uploads-${LocalDatabase.scope(session)}")

    suspend fun online() = api.checkAccess(session)

    suspend fun staged(): List<StagedAudio> = withContext(Dispatchers.IO) {
        folder.listFiles().orEmpty().filter { it.extension == "json" }.mapNotNull {
            runCatching { Wire.json.decodeFromString<StagedAudio>(AtomicFile(it).readFully().decodeToString()) }.getOrNull()
        }
    }

    suspend fun stage(uri: Uri): StagedAudio = withContext(Dispatchers.IO) {
        val filename = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        } ?: throw IOException("Не удалось прочитать имя файла.")
        if (filename.substringAfterLast('.').lowercase() !in setOf("wav", "mp3", "flac")) throw ClientException(Failure.InvalidFile)
        folder.mkdirs()
        val id = UUID.randomUUID()
        val audio = File(folder, "$id.audio")
        val record = AtomicFile(File(folder, "$id.json"))
        val digest = MessageDigest.getInstance("SHA-256")
        val limit = 8L * 1024 * 1024 * 1024
        try {
            var size = 0L
            val input = resolver.openInputStream(uri) ?: throw IOException("Выбери файл снова.")
            input.use { source ->
                audio.outputStream().use { out ->
                    val buffer = ByteArray(64 * 1024)
                    while (true) {
                        ensureActive()
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
            val out = record.startWrite()
            try {
                out.write(Wire.json.encodeToString(StagedAudio.serializer(), item).encodeToByteArray())
                record.finishWrite(out)
            } catch (error: Exception) {
                record.failWrite(out)
                throw error
            }
            item
        } catch (error: Exception) {
            audio.delete()
            throw error
        }
    }

    suspend fun discard(item: StagedAudio) = withContext(Dispatchers.IO) {
        AtomicFile(File(folder, "${item.id}.json")).delete()
        File(folder, "${item.id}.audio").delete()
    }

    suspend fun upload(item: StagedAudio, progress: (Long) -> Unit): Job = withContext(Dispatchers.IO) {
        val audio = File(folder, "${item.id}.audio")
        if (audio.length() != item.length) throw IOException("Локальная копия недоступна. Выбери файл снова.")
        val create = buildJsonObject {
            put("filename", item.filename); put("total_bytes", item.length); put("sha256", item.sha)
        }
        var upload = retryUpload { api.write(session, "uploads", "POST", item.id, 201, UploadState.serializer(), create) }
        val path = "uploads/${upload.id}"
        if (upload.totalBytes != item.length || upload.offset > item.length) throw ClientException(Failure.InvalidResponse)
        val offset = upload.offset
        progress(offset)
        RandomAccessFile(audio, "r").use { source ->
            source.seek(offset)
            while (upload.offset < item.length) {
                ensureActive()
                val start = upload.offset
                val bytes = ByteArray(minOf(1024L * 1024, item.length - start).toInt())
                source.readFully(bytes)
                val updated = retryUpload { api.write(session, path, "PATCH", chunkOperation(item.id, start), 200,
                    UploadState.serializer(), bytes = bytes, headers = mapOf(
                        "Upload-Offset" to start.toString(),
                        "X-Chunk-SHA256" to MessageDigest.getInstance("SHA-256").digest(bytes).hex())) }
                if (updated.id != upload.id || updated.totalBytes != item.length || updated.offset != start + bytes.size)
                    throw ClientException(Failure.InvalidResponse)
                upload = updated
                progress(upload.offset)
            }
        }
        val complete = UUID.nameUUIDFromBytes("${item.id}:complete".toByteArray())
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

    suspend fun edit(tracks: List<Track>, patch: JsonObject): List<Job> {
        MetadataPatch(patch)
        val single = tracks.singleOrNull()
        if (single != null) {
            val key = UUID.randomUUID()
            val payload = buildJsonObject { put("expected_revision", single.revision); put("patch", patch) }
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
            val key = UUID.randomUUID()
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
        Playlist.serializer(), buildJsonObject { put("name", name) })
    suspend fun renamePlaylist(playlist: Playlist, name: String, key: UUID) = api.write(session, "playlists/${playlist.id}", "PATCH", key, 200,
        Playlist.serializer(), buildJsonObject { put("name", name); put("expected_revision", playlist.revision) })
    suspend fun savePlaylist(playlist: Playlist, entries: List<UUID>, key: UUID) = api.write(session, "playlists/${playlist.id}/tracks", "PUT", key, 200,
        Playlist.serializer(), buildJsonObject {
            put("expected_revision", playlist.revision)
            putJsonArray("track_ids") { entries.forEach { add(it.toString()) } }
        })
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
