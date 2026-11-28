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
            val jobs = reopened.edit(imported, buildJsonObject { put("artist", "Проверка метаданных"); put("artwork_id", cover.toString()) })
            assertEquals(JobStatus.succeeded, jobs.single().status)
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
            val playlist = reopened.createPlaylist("Метро", UUID.randomUUID())
            val order = listOf(imported[1].id, imported[0].id, imported[1].id)
            val saved = reopened.savePlaylist(playlist, order, UUID.randomUUID())
            assertEquals(order, reopened.playlist(saved.id).trackIds)
            val renamed = reopened.renamePlaylist(saved, "Утро", UUID.randomUUID())
            assertEquals("Утро", reopened.playlist(renamed.id).name)
            reopened.deletePlaylist(renamed, UUID.randomUUID())
            assertTrue(api.playlists(session).items.isEmpty())
        } finally {
            editor.discard(high)
            editor.discard(flac)
        }
    }
}
