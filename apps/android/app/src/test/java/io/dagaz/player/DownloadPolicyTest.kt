package io.dagaz.player

import io.dagaz.player.api.*
import io.dagaz.player.downloads.reuseDownload
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test
import androidx.media3.exoplayer.offline.Downloader
import io.dagaz.player.downloads.RetryingDownloader
import io.dagaz.player.playback.MediaValidationException
import java.io.IOException

class DownloadPolicyTest {
    @Test fun rebindingTheSameLibraryKeepsItsLocalScope() {
        val first=Session("https://music.example/",java.util.UUID.randomUUID(),"a".repeat(64))
        val rebound=Session(first.baseUrl,java.util.UUID.randomUUID(),"b".repeat(64))
        assertEquals(io.dagaz.player.sync.LocalDatabase.scope(first),io.dagaz.player.sync.LocalDatabase.scope(rebound))
        assertNotEquals(io.dagaz.player.sync.LocalDatabase.scope(first),io.dagaz.player.sync.LocalDatabase.scope(Session("https://other.example/",first.deviceId,first.token)))
    }
    @Test fun outagesRetryButInvalidBytesStop() {
        var attempts=0
        val source=object:Downloader {
            override fun download(listener:Downloader.ProgressListener?){if(++attempts<3)throw IOException("offline")}
            override fun cancel(){}
            override fun remove(){}
        }
        RetryingDownloader(source).download(null);assertEquals(3,attempts)
        val invalid=object:Downloader {
            override fun download(listener:Downloader.ProgressListener?){throw MediaValidationException()}
            override fun cancel(){}
            override fun remove(){}
        }
        assertThrows(MediaValidationException::class.java){RetryingDownloader(invalid).download(null)}
        var diskAttempts=0
        val disk=object:Downloader {
            override fun download(listener:Downloader.ProgressListener?){diskAttempts++;throw androidx.media3.datasource.cache.Cache.CacheException(IOException("ENOSPC"))}
            override fun cancel(){}
            override fun remove(){}
        }
        assertThrows(androidx.media3.datasource.cache.Cache.CacheException::class.java){RetryingDownloader(disk).download(null)}
        assertEquals(1,diskAttempts)
    }
    @Test fun oneTrackOneRepresentation() {
        val cases=Wire.json.parseToJsonElement(javaClass.classLoader!!.getResource("download-policy.json")!!.readText()).jsonArray
        for(case in cases){val c=case.jsonObject
            assertEquals(c.toString(),c.getValue("reuse").jsonPrimitive.boolean,reuseDownload(Quality.valueOf(c.getValue("existing").jsonPrimitive.content),Quality.valueOf(c.getValue("requested").jsonPrimitive.content),c.getValue("force").jsonPrimitive.boolean))
        }
    }
}
