package io.dagaz.player.api

import io.dagaz.player.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit

enum class Failure(val label: String) {
    InvalidAddress("Укажи адрес HTTPS без логина, пути и параметров."),
    InvalidInput("Проверь одноразовый код и имя устройства."),
    PairingRequired("Нужна новая привязка. Получи одноразовый код на сервере."),
    Unavailable("Сервер недоступен. Проверь адрес и подключение к сети."),
    InvalidResponse("Сервер вернул неожиданный ответ."),
    RateLimited("Слишком много запросов. Попробуй немного позже."),
    Storage("Не удалось сохранить подключение на устройстве.")
}
class ClientException(val failure: Failure) : Exception(failure.label)
object ServerAddress {
    fun parse(value: String, allowLoopback: Boolean = BuildConfig.DEBUG): HttpUrl {
        val url = value.toHttpUrlOrNull() ?: throw ClientException(Failure.InvalidAddress)
        val loopback = url.host == "127.0.0.1" || url.host == "::1"
        if ((!url.isHttps && !(allowLoopback && loopback)) || url.username.isNotEmpty() || url.password.isNotEmpty() || url.encodedPath != "/" || url.query != null || url.fragment != null) throw ClientException(Failure.InvalidAddress)
        return url
    }
}
class DagazApi(address: String) {
    private val origin = ServerAddress.parse(address)
    val baseUrl: String = origin.toString()
    private val http = OkHttpClient.Builder().followRedirects(false).followSslRedirects(false)
        .callTimeout(10, TimeUnit.SECONDS).connectTimeout(5, TimeUnit.SECONDS).build()

    suspend fun pair(code: String, deviceName: String): PairResponse {
        if (code.isEmpty() || code.toByteArray().size > 128 || code.any(Char::isISOControl) || deviceName.isBlank() || deviceName.toByteArray().size > 128 || deviceName.any(Char::isISOControl)) throw ClientException(Failure.InvalidInput)
        val body = Wire.json.encodeToString(PairRequest.serializer(), PairRequest(code, deviceName.trim()))
        val request = Request.Builder().url(origin.newBuilder().addPathSegments("api/v1/pair").build()).post(body.toRequestBody("application/json".toMediaType())).build()
        val result = execute(request, 201, PairResponse.serializer())
        try { Session(baseUrl, result.deviceId, result.token) } catch (_: IllegalArgumentException) { throw ClientException(Failure.InvalidResponse) }
        return result
    }
    suspend fun checkAccess(session: Session) {
        if (ServerAddress.parse(session.baseUrl) != origin) throw ClientException(Failure.InvalidAddress)
        val request = Request.Builder().url(origin.newBuilder().addPathSegments("api/v1/devices").addQueryParameter("limit", "1").build()).header("Authorization", "Bearer ${session.token}").build()
        execute(request, 200, Page.serializer(Device.serializer()))
    }
    private suspend fun <T> execute(request: Request, expected: Int, serializer: KSerializer<T>): T = withContext(Dispatchers.IO) {
        try {
            http.newCall(request).execute().use { response ->
                when (response.code) {
                    401, 403 -> throw ClientException(Failure.PairingRequired)
                    429 -> throw ClientException(Failure.RateLimited)
                    in 500..599 -> throw ClientException(Failure.Unavailable)
                }
                if (response.code != expected) throw ClientException(Failure.InvalidResponse)
                val source = response.body?.source() ?: throw ClientException(Failure.InvalidResponse)
                if (source.request(65537)) throw ClientException(Failure.InvalidResponse)
                try { Wire.json.decodeFromString(serializer, source.readUtf8()) } catch (_: IllegalArgumentException) { throw ClientException(Failure.InvalidResponse) }
            }
        } catch (_: IOException) { throw ClientException(Failure.Unavailable) }
    }
}
