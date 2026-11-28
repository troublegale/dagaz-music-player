package io.dagaz.player

import android.util.AtomicFile
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.dagaz.player.api.*
import okhttp3.mockwebserver.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.security.KeyStore
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(AndroidJUnit4::class)
@androidx.media3.common.util.UnstableApi
class ConnectionScreenTest {
    @get:Rule val compose = createEmptyComposeRule()
    @Test fun pairingSurvivesRecreationAndHandlesRevocation() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val file = java.io.File(context.noBackupFilesDir, "connection.json")
        // Do not replace an existing personal connection when running instrumentation later.
        requireEmptyConnectionFixture(file)
        val revoked = AtomicBoolean(false)
        val used = AtomicBoolean(false)
        val search = java.util.concurrent.atomic.AtomicReference<String>()
        MockWebServer().use { server ->
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse = when {
                    request.path == "/api/v1/pair" && !used.getAndSet(true) -> MockResponse().setResponseCode(201).setBody("{\"device_id\":\"${UUID.randomUUID()}\",\"token\":\"${"c".repeat(64)}\"}")
                    request.path?.startsWith("/api/v1/devices") == true && !revoked.get() -> MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")
                    request.path?.startsWith("/api/v1/tracks") == true && !revoked.get() -> {
                        val query=request.requestUrl?.queryParameter("q").orEmpty();search.set(query)
                        val tracks=if(query=="Ночь")listOf(Track(UUID.randomUUID(),1,"test.flac",title="Ночная музыка",artist="Исполнитель",durationMs=1000,format=AudioFormat.flac,originalBytes=100,readyQualities=listOf(Quality.original)))else emptyList()
                        MockResponse().setBody(Wire.json.encodeToString(Page.serializer(Track.serializer()),Page(tracks)))
                    }
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
                compose.onNodeWithText("К библиотеке").performClick()
                compose.onNodeWithText("Поиск по библиотеке").assertDoesNotExist()
                compose.onNodeWithText("Поиск").performClick()
                compose.onNodeWithText("Поиск по библиотеке").performTextInput("Ночь")
                compose.waitUntil(10_000){search.get()=="Ночь"&&compose.onAllNodesWithText("Ночная музыка").fetchSemanticsNodes().isNotEmpty()}
                compose.onNodeWithText("Ночная музыка").assertIsDisplayed()
                compose.onNodeWithText("Сервер").performClick()
                scenario.recreate()
                waitConnected()
                scenario.close()
                scenario = ActivityScenario.launch(MainActivity::class.java)
                waitConnected()
                revoked.set(true)
                compose.onNodeWithText("Проверить подключение").performClick()
                compose.waitUntil(10_000) { compose.onAllNodesWithText("Нужна новая привязка. Получи одноразовый код на сервере.").fetchSemanticsNodes().isNotEmpty() }
                compose.onNodeWithText("К библиотеке").assertIsDisplayed()
                compose.onNodeWithText("Нужна новая привязка. Получи одноразовый код на сервере.").assertIsDisplayed()
                compose.onNodeWithText("К библиотеке").performClick()
                compose.onNodeWithText("Скачанное").assertIsDisplayed()
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
        compose.waitUntil(10_000) { compose.onAllNodesWithText("Сервер").fetchSemanticsNodes().isNotEmpty()||compose.onAllNodesWithText("Мой сервер").fetchSemanticsNodes().isNotEmpty() }
        if(compose.onAllNodesWithText("Сервер").fetchSemanticsNodes().isNotEmpty())compose.onNodeWithText("Сервер").performClick()
        compose.waitUntil(10_000) { compose.onAllNodesWithText("Подключено").fetchSemanticsNodes().isNotEmpty() }
        compose.onNodeWithText("Мой сервер").assertIsDisplayed()
    }
}
