package io.dagaz.player.playback

import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.upstream.DefaultLoadErrorHandlingPolicy
import androidx.media3.exoplayer.upstream.LoadErrorHandlingPolicy
import java.io.IOException
import kotlin.random.Random

object RetrySchedule {
    fun delayMs(attempt: Int): Long {
        val base=longArrayOf(1000,2000,4000,8000,16000,30000)[(attempt-1).coerceIn(0,5)]
        return Random.nextLong(base*9/10,base*11/10+1)
    }
    fun retryable(status:Int)=status==408||status==429||status in 500..599
    @androidx.media3.common.util.UnstableApi
    fun retryable(error:IOException):Boolean {
        val causes=generateSequence<Throwable>(error){it.cause}.take(16).toList()
        if(causes.any{it is MediaValidationException||it is androidx.media3.datasource.cache.Cache.CacheException||it is java.io.FileNotFoundException||it is androidx.media3.common.ParserException})return false
        return causes.filterIsInstance<HttpDataSource.InvalidResponseCodeException>().firstOrNull()?.let{retryable(it.responseCode)}?:true
    }
}
class MediaValidationException:IOException("Неверная или недоступная версия аудио")
@UnstableApi
class MediaRetryPolicy:DefaultLoadErrorHandlingPolicy() {
    override fun getMinimumLoadableRetryCount(dataType:Int)=Int.MAX_VALUE
    override fun getRetryDelayMsFor(info:LoadErrorHandlingPolicy.LoadErrorInfo):Long {
        if(!RetrySchedule.retryable(info.exception))return C.TIME_UNSET
        return RetrySchedule.delayMs(info.errorCount)
    }
}
