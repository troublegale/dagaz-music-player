package io.dagaz.player

import io.dagaz.player.api.*
import io.dagaz.player.playback.RetrySchedule
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class LibraryApiTest {
    @Test fun manifestResolvesRelativeUrlsAndRejectsOtherOrigins()=kotlinx.coroutines.runBlocking {
        okhttp3.mockwebserver.MockWebServer().use{server->
            server.start(java.net.InetAddress.getByName("127.0.0.1"),0)
            val address=server.url("/").newBuilder().host("127.0.0.1").build().toString();val session=Session(address,java.util.UUID.randomUUID(),"a".repeat(64));val api=DagazApi(address)
            val id=java.util.UUID.randomUUID();val path="/api/v1/tracks/$id/media?revision=1&quality=original"
            val media=MediaDescriptor(id,1,Quality.high,Quality.original,100,"a".repeat(64),"\"tag\"",path)
            server.enqueue(okhttp3.mockwebserver.MockResponse().setBody(Wire.json.encodeToString(ManifestPage.serializer(),ManifestPage(listOf(media),selectionRevision=1))))
            assertEquals(address.trimEnd('/')+path,api.manifestPage(session,java.util.UUID.randomUUID()).items.single().url)
            server.enqueue(okhttp3.mockwebserver.MockResponse().setBody(Wire.json.encodeToString(ManifestPage.serializer(),ManifestPage(listOf(media.copy(url="https://elsewhere.example$path")),selectionRevision=1))))
            try{api.manifestPage(session,java.util.UUID.randomUUID());fail("Foreign origin accepted")}catch(error:ClientException){assertEquals(Failure.InvalidResponse,error.failure)}
        }
    }
    @Test fun reusedQualityHasCanonicalOriginalUrl() = runBlocking {
        MockWebServer().use {server->
            server.start();val api=DagazApi(server.url("/").toString());val session=Session(api.baseUrl,UUID.randomUUID(),"d".repeat(64));val id=UUID.randomUUID();val hash="a".repeat(64)
            server.enqueue(MockResponse().setHeader("Content-Length",123).setHeader("ETag","\"$hash\"").setHeader("X-Content-SHA256",hash).setHeader("X-Effective-Quality","original"))
            val media=api.media(session,id,1,Quality.high)
            assertEquals(Quality.high,media.requestedQuality);assertEquals(Quality.original,media.effectiveQuality);assertTrue(media.url.endsWith("quality=original"))
            server.enqueue(MockResponse().setHeader("Content-Length",123).setHeader("ETag","\"$hash\"").setHeader("X-Content-SHA256",hash).setHeader("X-Effective-Quality","high"))
            try{api.media(session,id,1,Quality.original);fail()}catch(error:ClientException){assertEquals(Failure.InvalidResponse,error.failure)}
            server.enqueue(MockResponse().setResponseCode(422))
            try{api.media(session,id,1,Quality.high);fail()}catch(error:ClientException){assertEquals("Выбранное качество недоступно. Попробуй оригинал.",error.failure.label)}
        }
    }
    @Test fun catalogAndPinnedMedia() = runBlocking {
        MockWebServer().use { server ->
            server.start()
            val api = DagazApi(server.url("/").toString())
            val session = Session(api.baseUrl, UUID.randomUUID(), "d".repeat(64))
            val track = Wire.json.decodeFromString<Track>(javaClass.classLoader!!.getResource("track-null-tags.json")!!.readText())
            val payload = Wire.json.encodeToString(Track.serializer(), track)
            server.enqueue(MockResponse().setBody("{\"items\":[$payload],\"next_cursor\":\"next\"}"))
            val page = api.tracks(session, "Ночь", "cursor")
            assertEquals(track.id, page.items.single().id)
            val request = server.takeRequest()
            assertEquals("Ночь", request.requestUrl!!.queryParameter("q"))
            assertEquals("cursor", request.requestUrl!!.queryParameter("cursor"))
            assertEquals("Bearer ${session.token}", request.getHeader("Authorization"))
            val hash = "a".repeat(64)
            server.enqueue(MockResponse().setHeader("Content-Length", 123).setHeader("ETag", "\"$hash\"").setHeader("X-Content-SHA256", hash))
            val media = api.media(session, track, Quality.original)
            assertEquals(123L, media.byteLength); assertEquals(track.revision, media.revision)
            assertEquals("HEAD", server.takeRequest().method)
            server.enqueue(MockResponse().setResponseCode(425))
            try { api.media(session, track, Quality.high); fail() } catch (error: ClientException) { assertEquals(Failure.Pending, error.failure) }
            server.enqueue(MockResponse().setHeader("Content-Length", 12).setHeader("ETag", "weak"))
            try { api.media(session, track, Quality.original); fail() } catch (error: ClientException) { assertEquals(Failure.InvalidResponse, error.failure) }
        }
    }
    @Test fun retryIsBoundedAndTerminalErrorsStop() {
        for (attempt in 1..15) {
            val base = listOf(1000L,2000L,4000L,8000L,16000L,30000L)[(attempt-1).coerceAtMost(5)]
            assertTrue(RetrySchedule.delayMs(attempt) in base*9/10..base*11/10)
        }
        for (status in listOf(401,403,404,410,412,416,425)) assertFalse(RetrySchedule.retryable(status))
        for (status in listOf(408,429,500,503)) assertTrue(RetrySchedule.retryable(status))
    }
}
