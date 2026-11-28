package io.dagaz.player.playback

import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.*
import androidx.media3.datasource.cache.*
import androidx.media3.datasource.okhttp.OkHttpDataSource
import io.dagaz.player.api.*
import io.dagaz.player.auth.Credentials
import io.dagaz.player.downloads.DownloadRepository
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
    private val explicit=mutableMapOf<String,Cache.Listener>()
    @Synchronized fun get(context:Context):SimpleCache {
        if(cache==null)cache=SimpleCache(File(context.noBackupFilesDir,"stream-cache"),LeastRecentlyUsedCacheEvictor(1024L*1024*1024),StandaloneDatabaseProvider(context))
        return cache!!
    }
    @Synchronized fun markExplicit(context:Context,key:String) {
        val stored=get(context)
        if(key !in explicit){val listener=object:Cache.Listener {
            override fun onSpanAdded(cache:Cache,span:CacheSpan){cache.removeSpan(span)}
            override fun onSpanRemoved(cache:Cache,span:CacheSpan){}
            override fun onSpanTouched(cache:Cache,oldSpan:CacheSpan,newSpan:CacheSpan){}
        };explicit[key]=listener;stored.addListener(key,listener)}
        stored.removeResource(key)
    }
    @Synchronized fun unmarkExplicit(context:Context,key:String){explicit.remove(key)?.let{get(context).removeListener(key,it)}}
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
        val streamed=CacheDataSource.Factory().setCache(StreamCache.get(context)).setUpstreamDataSourceFactory(upstream)
        val cached=DataSource.Factory { object:DataSource {
            private var source:DataSource?=null
            private val listeners=mutableListOf<TransferListener>()
            override fun addTransferListener(listener:TransferListener){listeners.add(listener);source?.addTransferListener(listener)}
            override fun open(spec:DataSpec):Long {
                val session=Credentials(context).load()?:throw MediaValidationException()
                val stored=DownloadRepository.get(context,session)
                val media=descriptors[spec.uri.toString()]
                val ready=media?.let{stored.ready(it.trackId.toString())}
                val factory=if(ready?.cacheKey==spec.key)CacheDataSource.Factory().setCache(stored.cache).setUpstreamDataSourceFactory(null)else streamed
                val selected=factory.createDataSource();source=selected;listeners.forEach(selected::addTransferListener);return selected.open(spec)
            }
            override fun read(buffer:ByteArray,offset:Int,length:Int)=source?.read(buffer,offset,length)?:throw MediaValidationException()
            override fun getUri()=source?.uri
            override fun getResponseHeaders()=source?.responseHeaders?:emptyMap()
            override fun close(){source?.close();source=null}
        }}
        return ResolvingDataSource.Factory(cached,object:ResolvingDataSource.Resolver{
            override fun resolveDataSpec(spec:DataSpec):DataSpec {
                val session=try{Credentials(context).load()}catch(_:ClientException){throw MediaValidationException()}?:throw MediaValidationException()
                val id=descriptors[spec.uri.toString()]?.trackId?:pending[spec.uri.toString()]?.id?:throw MediaValidationException()
                val local=DownloadRepository.get(context,session).ready(id.toString())
                val media=local?:descriptors[spec.uri.toString()]?:run {
                    val item=pending[spec.uri.toString()]?:throw MediaValidationException()
                    val expected=ServerAddress.parse(session.baseUrl).newBuilder().addPathSegments("api/v1/tracks/${item.id}/media").addQueryParameter("revision",item.revision.toString()).addQueryParameter("quality",item.quality.name).build()
                    if(expected.toString()!=spec.uri.toString())throw MediaValidationException()
                    try{runBlocking{DagazApi(session.baseUrl).media(session,item.id,item.revision,item.quality)}}catch(error:ClientException){if(error.failure==Failure.Unavailable||error.failure==Failure.RateLimited||error.failure==Failure.Pending)throw java.io.IOException(error.failure.label);throw MediaValidationException()}
                }
                validateOrigin(session,media)
                descriptors[spec.uri.toString()]=media;descriptors[media.url]=media
                return spec.buildUpon().setUri(media.url).setKey(media.cacheKey).build()
            }
        }).createDataSource()
    }
    internal fun networkFactory()=DataSource.Factory { RevisionSource() }
    private fun validateOrigin(session:Session,media:MediaDescriptor){
        val origin=ServerAddress.parse(session.baseUrl)
        val target=media.url.toHttpUrlOrNull()?:throw MediaValidationException()
        if(target.scheme!=origin.scheme||target.host!=origin.host||target.port!=origin.port||target.username.isNotEmpty()||target.password.isNotEmpty()||target.fragment!=null||target.encodedPath!="/api/v1/tracks/${media.trackId}/media"||target.queryParameterValues("revision")!=listOf(media.revision.toString())||target.queryParameterValues("quality")!=listOf(media.effectiveQuality.name)||target.queryParameterNames!=setOf("revision","quality"))throw MediaValidationException()
    }
    private inner class RevisionSource:DataSource {
        private var source:HttpDataSource?=null
        private val listeners=mutableListOf<TransferListener>()
        override fun addTransferListener(listener:TransferListener){listeners.add(listener);source?.addTransferListener(listener)}
        override fun open(spec:DataSpec):Long {
            val session=try{Credentials(context).load()}catch(_:ClientException){throw MediaValidationException()}?:throw MediaValidationException()
            val media=descriptors[spec.uri.toString()]?:throw MediaValidationException()
            validateOrigin(session,media)
            val data=OkHttpDataSource.Factory(http).createDataSource();source=data;listeners.forEach(data::addTransferListener)
            return data.open(spec.buildUpon().setHttpRequestHeaders(spec.httpRequestHeaders+mapOf("Authorization" to "Bearer ${session.token}","If-Match" to media.etag,"Accept-Encoding" to "identity")).build())
        }
        override fun read(buffer:ByteArray,offset:Int,length:Int)=source?.read(buffer,offset,length)?:throw MediaValidationException()
        override fun getUri()=source?.uri
        override fun getResponseHeaders()=source?.responseHeaders?:emptyMap()
        override fun close(){source?.close();source=null}
    }
}
