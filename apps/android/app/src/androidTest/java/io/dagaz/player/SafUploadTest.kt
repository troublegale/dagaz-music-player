package io.dagaz.player

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.MatrixCursor
import android.graphics.Bitmap
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.dagaz.player.api.*
import io.dagaz.player.editor.LibraryEditor
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Assume.assumeNotNull
import org.junit.Test
import org.junit.runner.RunWith
import java.io.IOException
import java.util.UUID
import java.io.File
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.MockResponse
import io.dagaz.player.sync.LocalDatabase
import kotlin.concurrent.thread

private class UploadFixtureProvider(private val assets: android.content.res.AssetManager) : ContentProvider() {
    private val files = mapOf("high.mp3" to "profiles/high.mp3", "tone.flac" to "tone.flac", "cover.png" to "")
    override fun onCreate() = true
    override fun getType(uri: Uri) = "application/octet-stream"
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?) =
        MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME)).apply {
            if (uri.lastPathSegment in files) addRow(arrayOf(uri.lastPathSegment))
        }
    override fun insert(uri: Uri, values: ContentValues?): Uri? = throw UnsupportedOperationException()
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?): Int = throw UnsupportedOperationException()
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?): Int = throw UnsupportedOperationException()
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val asset = files[uri.lastPathSegment] ?: throw java.io.FileNotFoundException()
        if (mode != "r") throw java.io.FileNotFoundException()
        val (reader, writer) = ParcelFileDescriptor.createPipe()
        thread(isDaemon = true) {
            ParcelFileDescriptor.AutoCloseOutputStream(writer).use { out ->
                if (asset.isNotEmpty()) assets.open(asset).use { it.copyTo(out) }
                else Bitmap.createBitmap(8, 8, Bitmap.Config.ARGB_8888).let { image ->
                    image.compress(Bitmap.CompressFormat.PNG, 100, out); image.recycle()
                }
            }
        }
        return reader
    }
}

@RunWith(AndroidJUnit4::class)
class SafUploadTest {
    @Test fun expiredUploadCanRestartAfterRebindingWithoutSelectingFileAgain() = runBlocking {
        org.junit.Assume.assumeTrue(android.os.Build.VERSION.SDK_INT >= 29)
        val context = ApplicationProvider.getApplicationContext<Context>()
        val resolver = android.content.ContentResolver.wrap(UploadFixtureProvider(InstrumentationRegistry.getInstrumentation().context.assets))
        MockWebServer().use { server ->
            server.start(java.net.InetAddress.getByName("127.0.0.1"), 0)
            val url = server.url("/").newBuilder().host("127.0.0.1").build().toString()
            val first = Session(url, UUID.randomUUID(), "a".repeat(64))
            val rebound = Session(url, UUID.randomUUID(), "b".repeat(64))
            val initial = LibraryEditor(context, first, resolver)
            val item = initial.stage(Uri.parse("content://fixture/tone.flac"))
            val old = UUID.randomUUID()
            val new = UUID.randomUUID()
            fun upload(code: Int, id: UUID, status: UploadStatus, offset: Long = 0) = MockResponse()
                .setResponseCode(code).setBody(Wire.json.encodeToString(UploadState.serializer(),
                    UploadState(id, offset, item.length, "2026-11-29T00:00:00Z", status)))
            try {
                server.enqueue(upload(201, old, UploadStatus.uploading))
                try {
                    initial.upload(item) { throw IOException("Paused before sending bytes") }
                    fail("Expected injected pause")
                } catch (error: IOException) { assertEquals("Paused before sending bytes", error.message) }
                val originalRequest = server.takeRequest(2, java.util.concurrent.TimeUnit.SECONDS)!!
                val reopened = LibraryEditor(context, rebound, resolver)
                val orphan = File(context.noBackupFilesDir, "uploads-${LocalDatabase.scope(rebound)}/${UUID.randomUUID()}.audio")
                orphan.writeBytes(byteArrayOf(1))
                val saved = reopened.staged().single()
                assertFalse(orphan.exists())
                assertEquals(old.toString(), saved.uploadId)
                server.enqueue(upload(200, old, UploadStatus.expired))
                server.enqueue(upload(201, new, UploadStatus.uploading))
                server.enqueue(upload(200, new, UploadStatus.uploading, item.length))
                server.enqueue(MockResponse().setResponseCode(202).setBody(
                    Wire.json.encodeToString(Job.serializer(), Job(UUID.randomUUID(), JobKind.`import`, JobStatus.succeeded, errors = emptyList()))))
                assertEquals(JobStatus.succeeded, reopened.upload(saved) {}.status)
                val resumed = server.takeRequest(2, java.util.concurrent.TimeUnit.SECONDS)!!
                assertEquals("GET", resumed.method)
                assertEquals("/api/v1/uploads/$old", resumed.path)
                assertEquals("Bearer ${rebound.token}", resumed.getHeader("Authorization"))
                val restarted = server.takeRequest(2, java.util.concurrent.TimeUnit.SECONDS)!!
                assertNotEquals(originalRequest.getHeader("Idempotency-Key"), restarted.getHeader("Idempotency-Key"))
                assertTrue(reopened.staged().isEmpty())
            } finally { initial.discard(item) }
        }
    }

    @Test fun multipleUrisResumeWithoutFilesystemPath() = runBlocking {
        val args = InstrumentationRegistry.getArguments()
        val address = args.getString("server_url")
        val code = args.getString("pairing_code")
        assumeNotNull(address, code)
        val context = ApplicationProvider.getApplicationContext<Context>()
        val api = DagazApi(address!!)
        val paired = api.pair(code!!, "Android library editor fixture")
        val session = Session(api.baseUrl, paired.deviceId, paired.token)
        org.junit.Assume.assumeTrue(android.os.Build.VERSION.SDK_INT >= 29)
        val resolver = android.content.ContentResolver.wrap(UploadFixtureProvider(InstrumentationRegistry.getInstrumentation().context.assets))
        val editor = LibraryEditor(context, session, resolver)
        fun uri(file: String) = Uri.parse("content://io.dagaz.player.test.upload-fixture/$file")
        val high = editor.stage(uri("high.mp3"))
        val flac = editor.stage(uri("tone.flac"))
        try {
            try {
                editor.upload(high) { if (it == 1024L * 1024) throw IOException("Interrupted after first acknowledged chunk") }
                fail("Expected simulated interruption")
            } catch (error: IOException) { assertEquals("Interrupted after first acknowledged chunk", error.message) }
            val reopened = LibraryEditor(context, session, resolver)
            assertEquals(setOf(high.operation, flac.operation), reopened.staged().map { it.operation }.toSet())
            val resumed = mutableListOf<Long>()
            val first = reopened.upload(high) { resumed.add(it) }
            val second = reopened.upload(flac) {}
            assertEquals(1024L * 1024, resumed.first())
            assertEquals(high.length, resumed.last())
            assertEquals(JobStatus.succeeded, first.status)
            assertEquals(JobStatus.succeeded, second.status)
            assertTrue(reopened.staged().isEmpty())
            val imported = api.tracks(session).items
            assertEquals(2, imported.size)
            for (stage in listOf(high, flac)) {
                val track = imported.single { it.filename == stage.filename }
                assertEquals(stage.sha, api.media(session, track, Quality.original).sha256)
            }
            val cover = reopened.artwork(uri("cover.png"))
            val patch = buildJsonObject { put("artist", "Проверка метаданных"); put("artwork_id", cover.toString()) }
            val operation = UUID.randomUUID()
            val jobs = reopened.edit(imported, patch, operation)
            assertEquals(JobStatus.succeeded, jobs.single().status)
            assertEquals(jobs.single().id, reopened.edit(imported, patch, operation).single().id)
            val updated = api.track(session, imported.first().id)
            assertEquals("Проверка метаданных", updated.artist)
            assertEquals(cover, updated.artworkId)
            try {
                reopened.edit(listOf(imported.first()), buildJsonObject { put("artist", JsonNull) })
                fail("Expected a stale revision conflict")
            } catch (error: ClientException) { assertEquals(Failure.Conflict, error.failure) }
            val fresh = api.track(session, imported.last().id)
            val partial = reopened.edit(listOf(imported.first(), fresh), buildJsonObject { put("artist", JsonNull) }).single()
            assertEquals(JobStatus.partial, partial.status)
            assertEquals("revision_conflict", partial.errors.single().code)
            assertNull(api.track(session, fresh.id).artist)
            assertEquals(updated.artist, api.track(session, updated.id).artist)
            val createKey = UUID.randomUUID()
            val playlist = reopened.createPlaylist("Метро", createKey)
            val order = listOf(imported[1].id, imported[0].id, imported[1].id)
            val saved = reopened.savePlaylist(playlist, order, UUID.randomUUID())
            assertEquals(order, reopened.playlist(saved.id).trackIds)
            expectConflict { reopened.createPlaylist("Метро", createKey) }
            val renameKey = UUID.randomUUID()
            val renamed = reopened.renamePlaylist(saved, "Утро", renameKey)
            assertEquals("Утро", reopened.playlist(renamed.id).name)
            val concurrent = reopened.savePlaylist(renamed, order.reversed().drop(1), UUID.randomUUID())
            expectConflict { reopened.renamePlaylist(saved, "Утро", renameKey) }
            assertEquals(concurrent.revision, reopened.playlist(saved.id).revision)
            reopened.deletePlaylist(concurrent, UUID.randomUUID())
            assertTrue(api.playlists(session).items.isEmpty())
        } finally {
            editor.discard(high)
            editor.discard(flac)
        }
    }

    private suspend fun expectConflict(action: suspend () -> Unit) {
        try {
            action()
            fail("Replayed response must not authorize overwriting a newer playlist")
        } catch (error: ClientException) {
            assertEquals(Failure.Conflict, error.failure)
        }
    }
}
