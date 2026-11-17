package io.dagaz.probe.media

import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import kotlinx.coroutines.flow.MutableStateFlow
import java.io.File
import java.util.concurrent.Executors

@UnstableApi
object ProbeRuntime {
    const val URL = "http://127.0.0.1:18888/long.wav?drop=1"
    const val KEY = "long-wav-original"
    const val LONG_BYTES = 2646044L
    val status = MutableStateFlow("Готово")
    private var database: StandaloneDatabaseProvider? = null
    private var storedCache: SimpleCache? = null
    private var manager: DownloadManager? = null

    @Synchronized fun cache(context: Context): SimpleCache {
        if (storedCache == null) {
            database = StandaloneDatabaseProvider(context.applicationContext)
            storedCache = SimpleCache(File(context.filesDir,"downloads"),NoOpCacheEvictor(),database!!)
        }
        return storedCache!!
    }
    fun dataSource(context: Context): CacheDataSource.Factory = CacheDataSource.Factory()
        .setCache(cache(context))
        .setCacheKeyFactory { if (it.uri.path == "/long.wav") KEY else it.uri.toString() }
        .setUpstreamDataSourceFactory(DefaultDataSource.Factory(context,DefaultHttpDataSource.Factory()))

    @Synchronized fun downloads(context: Context): DownloadManager {
        if (manager == null) {
            val cache = cache(context)
            manager = DownloadManager(context.applicationContext,database!!,cache,DefaultHttpDataSource.Factory(),Executors.newFixedThreadPool(2))
            manager!!.addListener(object: DownloadManager.Listener {
                override fun onDownloadChanged(manager: DownloadManager, download: Download, finalException: Exception?) {
                    status.value = "Скачивание: ${download.state}, байт: ${download.bytesDownloaded}"
                    android.util.Log.i("DagazProbe","download state=${download.state} bytes=${download.bytesDownloaded}")
                }
            })
        }
        return manager!!
    }
}
