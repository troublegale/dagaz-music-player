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
}
class MediaValidationException:IOException("Неверная или недоступная версия аудио")
@UnstableApi
class MediaRetryPolicy:DefaultLoadErrorHandlingPolicy() {
    override fun getMinimumLoadableRetryCount(dataType:Int)=Int.MAX_VALUE
    override fun getRetryDelayMsFor(info:LoadErrorHandlingPolicy.LoadErrorInfo):Long {
        val causes=generateSequence<Throwable>(info.exception){it.cause}.take(16).toList()
        if(causes.any{it is MediaValidationException})return C.TIME_UNSET
        if(causes.any{it is androidx.media3.datasource.cache.Cache.CacheException||it is java.io.FileNotFoundException})return C.TIME_UNSET
        val http=causes.filterIsInstance<HttpDataSource.InvalidResponseCodeException>().firstOrNull()
        if(http!=null&&!RetrySchedule.retryable(http.responseCode))return C.TIME_UNSET
        if(causes.any{it is androidx.media3.common.ParserException})return C.TIME_UNSET
        return RetrySchedule.delayMs(info.errorCount)
    }
}
