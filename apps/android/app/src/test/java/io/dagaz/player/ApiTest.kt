package io.dagaz.player

import io.dagaz.player.api.*
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID
import java.util.concurrent.TimeUnit

class ApiTest {
    private val token = "a".repeat(64)
    @Test fun pairingAndOriginSafety() = runBlocking {
        MockWebServer().use { server ->
            server.start()
            val api = DagazApi(server.url("/").toString())
            server.enqueue(MockResponse().setResponseCode(401))
            assertEquals(Failure.PairingRequired, failure { api.pair("wrong", "Телефон") })
            val id = UUID.randomUUID()
            server.enqueue(MockResponse().setResponseCode(201).setBody("{\"device_id\":\"$id\",\"token\":\"$token\"}"))
            val paired = api.pair("test-code", "Телефон")
            assertEquals(id, paired.deviceId)
            server.takeRequest()
            val request = server.takeRequest()
            assertNull(request.getHeader("Authorization"))
            assertTrue(request.body.readUtf8().contains("android"))
            val session = Session(api.baseUrl, paired.deviceId, paired.token)
            server.enqueue(MockResponse().setBody("{\"items\":[],\"next_cursor\":null}"))
            api.checkAccess(session)
            assertEquals("Bearer $token", server.takeRequest().getHeader("Authorization"))
            assertEquals(Failure.InvalidAddress, failure { DagazApi("https://other.example").checkAccess(session) })
            server.enqueue(MockResponse().setResponseCode(401))
            assertEquals(Failure.PairingRequired, failure { api.checkAccess(session) })
        }
        for (value in listOf("http://music.example", "https://user:password@music.example", "https://music.example/path", "https://music.example/?token=secret", "https://music.example/#fragment")) {
            assertThrows(ClientException::class.java) { DagazApi(value) }
        }
        assertThrows(ClientException::class.java) { ServerAddress.parse("http://127.0.0.1", allowLoopback = false) }
        Unit
    }
    @Test fun refusesRedirectsAndOversizedResponses() = runBlocking {
        MockWebServer().use { source -> MockWebServer().use { sink ->
            source.start(); sink.start()
            source.enqueue(MockResponse().setResponseCode(307).setHeader("Location", sink.url("/")))
            val api = DagazApi(source.url("/").toString())
            assertEquals(Failure.InvalidResponse, failure { api.checkAccess(Session(api.baseUrl, UUID.randomUUID(), token)) })
            assertNull(sink.takeRequest(100, TimeUnit.MILLISECONDS))
            source.enqueue(MockResponse().setResponseCode(201).setBody("x".repeat(65537)))
            assertEquals(Failure.InvalidResponse, failure { api.pair("code", "Phone") })
        } }
    }
    private suspend fun failure(block: suspend () -> Unit): Failure {
        try { block(); fail("Expected failure") } catch (error: ClientException) { return error.failure }
        error("unreachable")
    }
}
