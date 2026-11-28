package io.dagaz.player.downloads

import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.Downloader
import io.dagaz.player.playback.RetrySchedule
import java.io.IOException

@UnstableApi
internal class RetryingDownloader(private val delegate:Downloader):Downloader {
    @Volatile private var cancelled=false
    @Volatile private var thread:Thread?=null
    override fun download(listener:Downloader.ProgressListener?) {
        thread=Thread.currentThread();var attempt=0
        try{while(true){
            if(cancelled)throw InterruptedException()
            try{delegate.download(listener);return}catch(error:IOException){
                if(!RetrySchedule.retryable(error))throw error
                Thread.sleep(RetrySchedule.delayMs(++attempt))
            }
        }}finally{thread=null}
    }
    override fun cancel(){cancelled=true;delegate.cancel();thread?.interrupt()}
    override fun remove()=delegate.remove()
}
