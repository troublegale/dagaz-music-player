package io.dagaz.player.downloads

import android.app.Notification
import android.content.Context
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.*
import androidx.media3.exoplayer.scheduler.PlatformScheduler
import androidx.media3.exoplayer.scheduler.Scheduler
import androidx.work.CoroutineWorker
import androidx.work.WorkerParameters
import io.dagaz.player.api.ClientException
import io.dagaz.player.api.Failure
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.Dispatchers
import io.dagaz.player.sync.CatalogSync

@UnstableApi
class DagazDownloadService:DownloadService(55,DEFAULT_FOREGROUND_NOTIFICATION_UPDATE_INTERVAL,"downloads",io.dagaz.player.R.string.app_name,0) {
    private val helper by lazy{DownloadNotificationHelper(this,"downloads")}
    override fun getDownloadManager():DownloadManager=DownloadRuntime.downloads(this)
    override fun getScheduler():Scheduler=PlatformScheduler(this,56)
    override fun getForegroundNotification(downloads:MutableList<Download>,notMetRequirements:Int):Notification = helper.buildProgressNotification(this,android.R.drawable.stat_sys_download,null,"Скачивание музыки",downloads,notMetRequirements)
    override fun onTimeout(startId:Int,fgsType:Int){downloadManager.pauseDownloads();stopSelf()}
}

@UnstableApi
class ManifestWorker(context:Context,parameters:WorkerParameters):CoroutineWorker(context,parameters) {
    override suspend fun doWork():Result {
        val repository=DownloadRepository.current(applicationContext)?:return Result.failure()
        if(repository.scope!=inputData.getString("scope"))return Result.failure()
        return try {
            withTimeout(240000){
                val sync=CatalogSync(applicationContext,repository.db,repository.session)
                sync.refresh();repository.prepare()
            }
            DownloadService.sendResumeDownloads(applicationContext,DagazDownloadService::class.java,true)
            Result.success()
        }catch(_:kotlinx.coroutines.TimeoutCancellationException){Result.retry()}
        catch(error:kotlinx.coroutines.CancellationException){throw error}
        catch(error:ClientException){withContext(Dispatchers.IO){repository.dao.selections().filter{!it.complete}.forEach{repository.dao.selection(it.copy(error=error.failure.label))}};if(error.failure in listOf(Failure.Unavailable,Failure.Pending,Failure.RateLimited,Failure.Conflict))Result.retry()else Result.failure()}
        catch(error:Exception){
            android.util.Log.e("DagazDownloads","Manifest preparation failed",error)
            withContext(Dispatchers.IO){repository.dao.selections().filter{!it.complete}.forEach{repository.dao.selection(it.copy(error="Не удалось подготовить скачивание. Повтори."))}}
            Result.failure()
        }
    }
}
