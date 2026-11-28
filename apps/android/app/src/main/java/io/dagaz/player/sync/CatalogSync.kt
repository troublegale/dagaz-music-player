package io.dagaz.player.sync

import android.content.Context
import android.util.AtomicFile
import io.dagaz.player.api.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*
import java.io.File
import java.util.Locale
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.concurrent.ConcurrentHashMap

class CatalogSync(private val context:Context,private val db:LocalDatabase,private val session:Session) {
    companion object {private val locks=ConcurrentHashMap<String,Mutex>()}
    private val api=DagazApi(session.baseUrl)
    private val dao=db.dao()
    suspend fun refresh()=withContext(Dispatchers.IO) {locks.getOrPut(LocalDatabase.scope(session)){Mutex()}.withLock {
        val state=dao.sync()
        if(state==null)snapshot() else try {
            var cursor=state.cursor
            do {
                val page=api.changes(session,cursor)
                if(page.generation.toString()!=state.generation)throw ClientException(Failure.InvalidResponse)
                val latest=page.changes.associateBy{it.entity to it.entityId}.values
                membership(latest.toList())
                db.runInTransaction{latest.forEach{apply(it);if(it.entity==Entity.playlist){dao.deleteMembers(it.entityId.toString());if(!it.deleted)dao.promoteMembers(it.entityId.toString());dao.clearMemberStage(it.entityId.toString())}};dao.cancelDeletedCandidates();dao.sync(SyncState(generation=state.generation,cursor=page.nextCursor))}
                cursor=page.nextCursor
            }while(page.hasMore)
        }catch(error:ClientException){if(error.failure==Failure.MediaGone||error.failure==Failure.Conflict)snapshot()else throw error}
    }}
    private suspend fun snapshot() {
        dao.clearStage();dao.clearMemberStage();var cursor:String?=null;var generation:String?=null;var high=0L
        do {
            val page=api.snapshot(session,cursor)
            if(generation!=null&&(generation!=page.generation.toString()||high!=page.highWatermark))throw ClientException(Failure.InvalidResponse)
            generation=page.generation.toString();high=page.highWatermark
            membership(page.records)
            db.runInTransaction{page.records.forEach{apply(it,"stage_")}}
            cursor=page.nextCursor
        }while(cursor!=null)
        db.runInTransaction{dao.markCatalogDeleted();dao.promoteStage();dao.clearStage();dao.deleteMembers();dao.promoteMembers();dao.clearMemberStage();dao.cancelDeletedCandidates();dao.sync(SyncState(generation=generation!!,cursor="$generation:$high"))}
    }
    private suspend fun membership(changes:List<Change>) {
        for(change in changes.filter{it.entity==Entity.playlist&&!it.deleted}) {
            val id=change.entityId.toString();dao.clearMemberStage(id);var cursor:String?=null;var position=0
            do {
                val page=api.playlist(session,change.entityId,cursor)
                if(page.id!=change.entityId||page.revision!=change.revision)throw ClientException(Failure.Conflict)
                db.runInTransaction{page.trackIds.forEach{dao.member(Membership(id,true,position++,it.toString()))}}
                cursor=page.nextCursor
            }while(cursor!=null)
        }
    }
    private fun apply(change:Change,prefix:String="") {
        val entity=prefix+change.entity.name;val id=change.entityId.toString()
        if(change.deleted){dao.tombstone(entity,id);return}
        val payload=change.payload?:throw ClientException(Failure.InvalidResponse)
        val track=try{when(change.entity){
            Entity.track->Wire.json.decodeFromJsonElement<Track>(payload)
            Entity.album->{if(Wire.json.decodeFromJsonElement<Album>(payload).id!=change.entityId)throw ClientException(Failure.InvalidResponse);null}
            Entity.artist->{if(Wire.json.decodeFromJsonElement<Artist>(payload).id!=change.entityId)throw ClientException(Failure.InvalidResponse);null}
            Entity.playlist->{val value=Wire.json.decodeFromJsonElement<Playlist>(payload);if(value.id!=change.entityId||value.revision!=change.revision)throw ClientException(Failure.InvalidResponse);null}
            else->null
        }}catch(_:IllegalArgumentException){throw ClientException(Failure.InvalidResponse)}
        if(track!=null&&(track.id!=change.entityId||track.revision!=change.revision))throw ClientException(Failure.InvalidResponse)
        dao.record(CatalogRow(entity,id,payload.toString(),sequence=change.sequence,
            search=track?.let{listOfNotNull(it.title,it.filename,it.artist,it.album).joinToString(" ").lowercase(Locale.ROOT)}.orEmpty(),album=track?.albumId?.toString(),artist=track?.artistId?.toString()))
    }
    suspend fun covers(tracks:List<Track>)=withContext(Dispatchers.IO) {
        val folder=File(context.noBackupFilesDir,"covers-${LocalDatabase.scope(session)}");folder.mkdirs()
        for(id in tracks.mapNotNull{it.artworkId}.distinct()) {
            val file=File(folder,"$id.png")
            if(file.exists())continue
            val bytes=api.artwork(session,id)
            val bounds=android.graphics.BitmapFactory.Options().apply{inJustDecodeBounds=true}
            android.graphics.BitmapFactory.decodeByteArray(bytes,0,bytes.size,bounds)
            if(bounds.outWidth !in 1..256||bounds.outHeight !in 1..256)throw ClientException(Failure.InvalidResponse)
            val decoded=android.graphics.BitmapFactory.decodeByteArray(bytes,0,bytes.size)?:throw ClientException(Failure.InvalidResponse)
            decoded.recycle()
            val atomic=AtomicFile(file);val stream=atomic.startWrite()
            try{stream.write(bytes);atomic.finishWrite(stream)}catch(error:Exception){atomic.failWrite(stream);throw error}
        }
    }
}
