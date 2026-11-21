package io.dagaz.player.playback

import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.*
import androidx.media3.datasource.cache.*
import androidx.media3.datasource.okhttp.OkHttpDataSource
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking

internal data class PendingMedia(val id:java.util.UUID,val revision:Long,val quality:Quality)

@UnstableApi
internal object StreamCache {
    private var cache:SimpleCache?=null
    @Synchronized fun get(context:Context):SimpleCache {
        if(cache==null)cache=SimpleCache(File(context.noBackupFilesDir,"stream-cache"),LeastRecentlyUsedCacheEvictor(1024L*1024*1024),StandaloneDatabaseProvider(context))
        return cache!!
    }
}
@UnstableApi
internal class MediaTransport(private val context:Context,private val descriptors:ConcurrentHashMap<String,MediaDescriptor>,private val pending:ConcurrentHashMap<String,PendingMedia>):DataSource.Factory {
    private val http=OkHttpClient.Builder().followRedirects(false).followSslRedirects(false).retryOnConnectionFailure(false)
        .connectTimeout(5,TimeUnit.SECONDS).readTimeout(5,TimeUnit.SECONDS).addNetworkInterceptor { chain ->
            val request=chain.request();val media=descriptors[request.url.toString()]?:throw MediaValidationException()
            val response=chain.proceed(request)
            if(response.code==200||response.code==206) {
                try {
                    if(response.header("ETag")!=media.etag||!response.header("X-Content-SHA256").equals(media.sha256,true))throw MediaValidationException()
                    if(response.header("Content-Encoding")?.let{it!="identity"}==true)throw MediaValidationException()
                    val range=request.header("Range")
                    val length=response.header("Content-Length")?.toLongOrNull()?:throw MediaValidationException()
                    if(range==null) {if(response.code!=200||length!=media.byteLength)throw MediaValidationException()}
                    else {
                        val match=Regex("bytes=(\\d+)-(\\d*)").matchEntire(range)?:throw MediaValidationException()
                        val start=match.groupValues[1].toLong();val end=match.groupValues[2].toLongOrNull()?.coerceAtMost(media.byteLength-1)?:media.byteLength-1
                        if(response.code!=206||response.header("Content-Range")!="bytes $start-$end/${media.byteLength}"||length!=end-start+1)throw MediaValidationException()
                    }
                }catch(error:Exception){response.close();throw MediaValidationException()}
            }
            response
        }.build()
    override fun createDataSource():DataSource {
        val upstream=DataSource.Factory { RevisionSource() }
        return CacheDataSource.Factory().setCache(StreamCache.get(context)).setUpstreamDataSourceFactory(upstream).createDataSource()
    }
    private inner class RevisionSource:DataSource {
        private var source:HttpDataSource?=null
        private val listeners=mutableListOf<TransferListener>()
        override fun addTransferListener(listener:TransferListener){listeners.add(listener);source?.addTransferListener(listener)}
        override fun open(spec:DataSpec):Long {
            val session=try{Credentials(context).load()}catch(_:ClientException){throw MediaValidationException()}?:throw MediaValidationException()
            val media=descriptors[spec.uri.toString()]?:run {
                val item=pending[spec.uri.toString()]?:throw MediaValidationException()
                try {runBlocking{DagazApi(session.baseUrl).media(session,item.id,item.revision,item.quality)}}catch(error:ClientException){if(error.failure==Failure.Unavailable||error.failure==Failure.RateLimited)throw java.io.IOException(error.failure.label);throw MediaValidationException()}
            }.also{if(it.url!=spec.uri.toString())throw MediaValidationException();descriptors[it.url]=it}
            val origin=ServerAddress.parse(session.baseUrl)
            val target=media.url.toHttpUrlOrNull()?:throw MediaValidationException()
            if(target.scheme!=origin.scheme||target.host!=origin.host||target.port!=origin.port||target.username.isNotEmpty()||target.password.isNotEmpty()||target.fragment!=null||target.encodedPath!="/api/v1/tracks/${media.trackId}/media"||target.queryParameterValues("revision")!=listOf(media.revision.toString())||target.queryParameterValues("quality")!=listOf(media.effectiveQuality.name))throw MediaValidationException()
            val data=OkHttpDataSource.Factory(http).createDataSource();source=data;listeners.forEach(data::addTransferListener)
            return data.open(spec.buildUpon().setHttpRequestHeaders(spec.httpRequestHeaders+mapOf("Authorization" to "Bearer ${session.token}","If-Match" to media.etag,"Accept-Encoding" to "identity")).build())
        }
        override fun read(buffer:ByteArray,offset:Int,length:Int)=source?.read(buffer,offset,length)?:throw MediaValidationException()
        override fun getUri()=source?.uri
        override fun getResponseHeaders()=source?.responseHeaders?:emptyMap()
        override fun close(){source?.close();source=null}
    }
}
