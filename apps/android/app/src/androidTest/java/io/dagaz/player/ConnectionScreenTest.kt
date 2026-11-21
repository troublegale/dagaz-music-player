package io.dagaz.player

import android.util.AtomicFile
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import okhttp3.mockwebserver.*
import org.junit.Assume.assumeFalse
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(AndroidJUnit4::class)
class ConnectionScreenTest {
    @get:Rule val compose = createEmptyComposeRule()
    @Test fun pairingSurvivesRecreationAndHandlesRevocation() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val file = java.io.File(context.noBackupFilesDir, "connection.json")
        // Do not replace an existing personal connection when running instrumentation later.
        assumeFalse(file.exists())
        val revoked = AtomicBoolean(false)
        val used = AtomicBoolean(false)
        MockWebServer().use { server ->
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse = when {
                    request.path == "/api/v1/pair" && !used.getAndSet(true) -> MockResponse().setResponseCode(201).setBody("{\"device_id\":\"${UUID.randomUUID()}\",\"token\":\"${"c".repeat(64)}\"}")
                    request.path?.startsWith("/api/v1/devices") == true && !revoked.get() -> MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")
                    else -> MockResponse().setResponseCode(401)
                }
            }
            server.start(java.net.InetAddress.getByName("127.0.0.1"), 0)
            val address = server.url("/").newBuilder().host("127.0.0.1").build().toString()
            var scenario = ActivityScenario.launch(MainActivity::class.java)
            try {
                compose.waitUntil(10_000) { compose.onAllNodesWithText("Подключить").fetchSemanticsNodes().isNotEmpty() }
                compose.onNodeWithText("Адрес сервера").performTextInput(address)
                compose.onNodeWithText("Код привязки").performTextInput("one-time")
                compose.onNodeWithText("Подключить").performScrollTo().performClick()
                waitConnected()
                scenario.recreate()
                waitConnected()
                scenario.close()
                scenario = ActivityScenario.launch(MainActivity::class.java)
                waitConnected()
                revoked.set(true)
                compose.onNodeWithText("Проверить подключение").performClick()
                compose.waitUntil(10_000) { compose.onAllNodesWithText("Подключить").fetchSemanticsNodes().isNotEmpty() }
                compose.onNodeWithText("Адрес сервера").assertTextContains(address)
                compose.onNodeWithText("Нужна новая привязка. Получи одноразовый код на сервере.").assertIsDisplayed()
            } catch (error: Throwable) {
                throw AssertionError("Mock requests=${server.requestCount}; UI=${compose.onRoot().printToString()}", error)
            } finally {
                scenario.close()
                AtomicFile(file).delete()
                KeyStore.getInstance("AndroidKeyStore").apply { load(null); deleteEntry("dagaz.connection") }
            }
        }
    }
    private fun waitConnected() {
        compose.waitUntil(10_000) { compose.onAllNodesWithText("Подключено").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Мой сервер").assertIsDisplayed()
    }
}
