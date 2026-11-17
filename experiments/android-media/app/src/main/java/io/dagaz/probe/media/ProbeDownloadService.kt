package io.dagaz.probe.media

import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.exoplayer.scheduler.Scheduler
import androidx.media3.exoplayer.offline.DownloadNotificationHelper

@UnstableApi
class ProbeDownloadService: DownloadService(42,DEFAULT_FOREGROUND_NOTIFICATION_UPDATE_INTERVAL,"downloads",R.string.app_name,0) {
    private val helper by lazy { DownloadNotificationHelper(this,"downloads") }
    override fun getDownloadManager() = ProbeRuntime.downloads(this)
    override fun getScheduler(): Scheduler? = null
    override fun getForegroundNotification(downloads: MutableList<Download>,notMetRequirements: Int) =
        helper.buildProgressNotification(this,android.R.drawable.stat_sys_download,null,"Скачивание тестового WAV",downloads,notMetRequirements)
}
