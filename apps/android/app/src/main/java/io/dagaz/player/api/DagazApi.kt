package io.dagaz.player.api

import io.dagaz.player.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.JsonObject
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit

enum class Failure(val label: String) {
    TooLarge("Файл превышает допустимый размер."),
    InvalidFile("Сервер отклонил формат или содержимое файла."),
    ServerStorage("На сервере недостаточно места."),
    InvalidAddress("Укажи адрес HTTPS без логина, пути и параметров."),
    InvalidInput("Проверь одноразовый код и имя устройства."),
    PairingRequired("Нужна новая привязка. Получи одноразовый код на сервере."),
    Unavailable("Сервер недоступен. Проверь адрес и подключение к сети."),
    InvalidResponse("Сервер вернул неожиданный ответ."),
    RateLimited("Слишком много запросов. Попробуй немного позже."),
    Storage("Не удалось сохранить подключение на устройстве."),
    Pending("Выбранное качество ещё готовится. Можно выбрать оригинал."),
    MediaGone("Эта версия трека недоступна. Обнови библиотеку."),
    Conflict("Подборка изменилась. Обнови её и попробуй снова."),
    QualityFailed("Выбранное качество недоступно. Попробуй оригинал.")
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
    private fun authenticated(session: Session, url: HttpUrl): Request.Builder {
        if (ServerAddress.parse(session.baseUrl) != origin) throw ClientException(Failure.InvalidAddress)
        return Request.Builder().url(url).header("Authorization", "Bearer ${session.token}")
    }
    suspend fun tracks(session: Session, query: String = "", cursor: String? = null, albumId: java.util.UUID? = null, artistId: java.util.UUID? = null): Page<Track> {
        val path=if(albumId==null)"api/v1/tracks" else "api/v1/albums/$albumId/tracks"
        val url=origin.newBuilder().addPathSegments(path).addQueryParameter("q",query).addQueryParameter("limit","100")
        if(cursor!=null)url.addQueryParameter("cursor",cursor)
        if(artistId!=null)url.addQueryParameter("artist_id",artistId.toString())
        return execute(authenticated(session,url.build()).build(),200,Page.serializer(Track.serializer()),4*1024*1024)
    }
    private fun listUrl(path:String,cursor:String?)=origin.newBuilder().addPathSegments("api/v1/$path").apply{if(cursor!=null)addQueryParameter("cursor",cursor)}.build()
    suspend fun albums(session: Session,cursor:String?=null): Page<Album> = execute(authenticated(session,listUrl("albums",cursor)).build(),200,Page.serializer(Album.serializer()),4*1024*1024)
    suspend fun artists(session: Session,cursor:String?=null): Page<Artist> = execute(authenticated(session,listUrl("artists",cursor)).build(),200,Page.serializer(Artist.serializer()),4*1024*1024)
    suspend fun media(session:Session,track:Track,quality:Quality)=media(session,track.id,track.revision,quality)
    suspend fun media(session: Session, id:java.util.UUID, revision:Long, quality: Quality): MediaDescriptor = withContext(Dispatchers.IO) {
        val url=origin.newBuilder().addPathSegments("api/v1/tracks/$id/media").addQueryParameter("revision",revision.toString()).addQueryParameter("quality",quality.name).build()
        try { http.newCall(authenticated(session,url).head().build()).execute().use { response ->
            if(response.code==422)throw ClientException(Failure.QualityFailed)
            checkStatus(response.code,200)
            val length=response.header("Content-Length")?.toLongOrNull()
            val sha=response.header("X-Content-SHA256")
            val etag=response.header("ETag")
            if(length==null||length<0||sha==null||!sha.matches(Regex("[a-fA-F0-9]{64}"))||etag==null||!etag.startsWith('"')||!etag.endsWith('"')||etag.length>256)throw ClientException(Failure.InvalidResponse)
            val effective=response.header("X-Effective-Quality")?.let{value->Quality.entries.find{it.name==value}?:throw ClientException(Failure.InvalidResponse)}?:quality
            if(effective!=Quality.original&&effective!=quality)throw ClientException(Failure.InvalidResponse)
            if(quality==Quality.original&&effective!=Quality.original)throw ClientException(Failure.InvalidResponse)
            val canonical=url.newBuilder().setQueryParameter("quality",effective.name).build()
            MediaDescriptor(id,revision,quality,effective,length,sha.lowercase(),etag,canonical.toString())
        }} catch (_:IOException){throw ClientException(Failure.Unavailable)}
    }
    suspend fun manifest(session:Session,request:ManifestCreate,operation:String):ManifestCreated {
        val body=Wire.json.encodeToString(ManifestCreate.serializer(),request).toRequestBody("application/json".toMediaType())
        return execute(authenticated(session,origin.newBuilder().addPathSegments("api/v1/download-manifests").build()).header("Idempotency-Key",operation).post(body).build(),201,ManifestCreated.serializer())
    }
    suspend fun manifestPage(session:Session,id:java.util.UUID,cursor:String?=null):ManifestPage {
        val page=execute(authenticated(session,listUrl("download-manifests/$id",cursor)).build(),200,ManifestPage.serializer(),4*1024*1024)
        return page.copy(items=page.items.map{media->
            val canonical=origin.newBuilder().addPathSegments("api/v1/tracks/${media.trackId}/media").addQueryParameter("revision",media.revision.toString()).addQueryParameter("quality",media.effectiveQuality.name).build()
            if(media.url!=canonical.toString()&&media.url!=canonical.encodedPath+"?"+canonical.encodedQuery)throw ClientException(Failure.InvalidResponse)
            media.copy(url=canonical.toString())
        })
    }
    suspend fun snapshot(session:Session,cursor:String?=null):SnapshotPage = execute(authenticated(session,listUrl("snapshot",cursor)).build(),200,SnapshotPage.serializer(),4*1024*1024)
    suspend fun changes(session:Session,cursor:String):SyncPage = execute(authenticated(session,listUrl("changes",cursor)).build(),200,SyncPage.serializer(),4*1024*1024)
    suspend fun playlists(session:Session,cursor:String?=null):Page<Playlist> = execute(authenticated(session,listUrl("playlists",cursor)).build(),200,Page.serializer(Playlist.serializer()),4*1024*1024)
    suspend fun track(session:Session,id:java.util.UUID):Track = execute(authenticated(session,listUrl("tracks/$id",null)).build(),200,Track.serializer())
    suspend fun playlist(session:Session,id:java.util.UUID,cursor:String?=null):Playlist = execute(authenticated(session,listUrl("playlists/$id",cursor)).build(),200,Playlist.serializer(),4*1024*1024)
    suspend fun artwork(session:Session,id:java.util.UUID):ByteArray = withContext(Dispatchers.IO) {
        try{http.newCall(authenticated(session,origin.newBuilder().addPathSegments("api/v1/artworks/$id").addQueryParameter("size","256").build()).build()).execute().use{response->
            checkStatus(response.code,200);val source=response.body?.source()?:throw ClientException(Failure.InvalidResponse)
            if(source.request(1024L*1024+1))throw ClientException(Failure.InvalidResponse)
            source.readByteArray()
        }}catch(_:IOException){throw ClientException(Failure.Unavailable)}
    }
    internal suspend fun <T> write(session: Session, path: String, method: String, key: java.util.UUID,
        status: Int, serializer: KSerializer<T>, payload: JsonObject? = null,
        bytes: ByteArray? = null, contentType: String = "application/octet-stream",
        headers: Map<String, String> = emptyMap()): T {
        val body = bytes?.toRequestBody(contentType.toMediaType())
            ?: payload?.toString().orEmpty().toRequestBody("application/json".toMediaType())
        val request = authenticated(session, listUrl(path, null)).header("Idempotency-Key", key.toString())
        headers.forEach { (name, value) -> request.header(name, value) }
        return execute(request.method(method, body).build(), status, serializer, 4 * 1024 * 1024)
    }
    internal suspend fun <T> read(session: Session, path: String, serializer: KSerializer<T>): T =
        execute(authenticated(session, listUrl(path, null)).build(), 200, serializer, 4 * 1024 * 1024)

    private fun checkStatus(code: Int, expected: Int) {
        when(code) {
            401,403 -> throw ClientException(Failure.PairingRequired)
            404,410,412 -> throw ClientException(Failure.MediaGone)
            409 -> throw ClientException(Failure.Conflict)
            425 -> throw ClientException(Failure.Pending)
            429 -> throw ClientException(Failure.RateLimited)
            in 500..599 -> throw ClientException(Failure.Unavailable)
        }
        if(code!=expected)throw ClientException(when(code) {
            413 -> Failure.TooLarge
            415,422 -> Failure.InvalidFile
            507 -> Failure.ServerStorage
            else -> Failure.InvalidResponse
        })
    }
    private suspend fun <T> execute(request: Request, expected: Int, serializer: KSerializer<T>, maxBytes: Int = 65536): T = withContext(Dispatchers.IO) {
        try {
            http.newCall(request).execute().use { response ->
                checkStatus(response.code,expected)
                if(expected == 204) return@withContext Wire.json.decodeFromString(serializer, "{}")
                val source = response.body?.source() ?: throw ClientException(Failure.InvalidResponse)
                if (source.request(maxBytes.toLong()+1)) throw ClientException(Failure.InvalidResponse)
                try { Wire.json.decodeFromString(serializer, source.readUtf8()) } catch (_: IllegalArgumentException) { throw ClientException(Failure.InvalidResponse) }
            }
        } catch (_: IOException) { throw ClientException(Failure.Unavailable) }
    }
}
