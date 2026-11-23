package io.dagaz.player

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class PairingTest {
    @Test fun credentialRestore() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val guard=java.io.File(context.cacheDir,"connection-guard-${UUID.randomUUID()}")
        for(suffix in listOf("",".bak",".new")){
            val candidate=java.io.File(guard.path+suffix);assertTrue(candidate.createNewFile())
            try{candidate.writeText("test-owned");try{requireEmptyConnectionFixture(guard);fail("Fixture must preserve AtomicFile state $suffix")}catch(_:org.junit.AssumptionViolatedException){};assertEquals("test-owned",candidate.readText())}finally{candidate.delete()}
        }
        val namespace = "test-${UUID.randomUUID()}"
        val credentials = Credentials(context, namespace)
        val file = java.io.File(context.noBackupFilesDir, "$namespace.json")
        val alias = "dagaz.$namespace"
        val token = "b".repeat(64)
        try { MockWebServer().use { server ->
            server.start(java.net.InetAddress.getByName("127.0.0.1"), 0)
            val api = DagazApi(server.url("/").newBuilder().host("127.0.0.1").build().toString())
            server.enqueue(MockResponse().setResponseCode(401))
            try { api.pair("wrong", "Poco"); fail() } catch (error: ClientException) { assertEquals(Failure.PairingRequired, error.failure) }
            val id = UUID.randomUUID()
            server.enqueue(MockResponse().setResponseCode(201).setBody("{\"device_id\":\"$id\",\"token\":\"$token\"}"))
            val paired = api.pair("test-code", "Poco")
            val session = Session(api.baseUrl, paired.deviceId, paired.token)
            credentials.save(session); credentials.save(session)
            assertNull(KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.getKey(alias, null).encoded)
            assertFalse(file.readText().contains(token))
            assertFalse(session.toString().contains(token))
            val restored = Credentials(context, namespace).load()!!
            assertEquals(id, restored.deviceId); assertEquals(api.baseUrl, restored.baseUrl)
            server.enqueue(MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")); api.checkAccess(restored)
            server.enqueue(MockResponse().setResponseCode(401))
            try { api.checkAccess(restored); fail() } catch (error: ClientException) { assertEquals(Failure.PairingRequired, error.failure) }
            val saved = file.readText()
            val envelope = org.json.JSONObject(saved)
            val encrypted = android.util.Base64.decode(envelope.getString("ciphertext"), android.util.Base64.NO_WRAP)
            encrypted[0] = (encrypted[0].toInt() xor 1).toByte()
            envelope.put("ciphertext", android.util.Base64.encodeToString(encrypted, android.util.Base64.NO_WRAP))
            file.writeText(envelope.toString())
            assertThrows(ClientException::class.java) { credentials.load() }
            file.writeText(saved)
            KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(alias) }
            assertThrows(ClientException::class.java) { credentials.load() }
            credentials.save(session); assertEquals(id, credentials.load()!!.deviceId)
        } } finally {
            android.util.AtomicFile(file).delete()
            KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry(alias) }
        }
    }
}
