package io.dagaz.player

import android.util.AtomicFile
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import okhttp3.Request
import org.junit.Assert.*
import org.junit.Assume.assumeNotNull
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class ServerPairingTest {
    @Test fun pairsWithRealServerAndHandlesRevocation() = runBlocking {
        val args = InstrumentationRegistry.getArguments()
        val address = args.getString("server_url"); val code = args.getString("pairing_code")
        assumeNotNull(address, code)
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val namespace = "test-${UUID.randomUUID()}"
        val credentials = Credentials(context, namespace)
        try {
            val api = DagazApi(address!!)
            val paired = api.pair(code!!, "Poco Android integration")
            val session = Session(api.baseUrl, paired.deviceId, paired.token)
            credentials.save(session)
            val restored = Credentials(context, namespace).load()!!
            assertEquals(paired.deviceId, restored.deviceId)
            api.checkAccess(restored)
            try { api.pair(code, "Poco Android integration"); fail() } catch (error: ClientException) { assertEquals(Failure.PairingRequired, error.failure) }
            val request = Request.Builder().url("${api.baseUrl}api/v1/devices/${paired.deviceId}")
                .header("Authorization", "Bearer ${restored.token}").header("Idempotency-Key", UUID.randomUUID().toString()).delete().build()
            OkHttpClient.Builder().followRedirects(false).callTimeout(10, java.util.concurrent.TimeUnit.SECONDS).build().newCall(request).execute().use { assertEquals(204, it.code) }
            try { api.checkAccess(restored); fail() } catch (error: ClientException) { assertEquals(Failure.PairingRequired, error.failure) }
        } finally {
            AtomicFile(java.io.File(context.noBackupFilesDir, "$namespace.json")).delete()
            KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry("dagaz.$namespace") }
        }
    }
}
