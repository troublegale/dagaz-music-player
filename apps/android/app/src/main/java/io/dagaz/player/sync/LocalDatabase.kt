package io.dagaz.player.sync

import android.content.Context
import androidx.room.*
import kotlinx.coroutines.flow.Flow
import io.dagaz.player.api.Session
import java.security.MessageDigest

@Entity(tableName="catalog",primaryKeys=["entity","id"])
data class CatalogRow(val entity:String,val id:String,val payload:String,val deleted:Boolean=false,val sequence:Long=0,val search:String="",val album:String?=null,val artist:String?=null)
@Entity(tableName="sync_state")
data class SyncState(@PrimaryKey val id:Int=1,val generation:String,val cursor:String)
@Entity(tableName="selections")
data class LocalSelection(@PrimaryKey val id:String,val kind:String,val target:String,val quality:String,val label:String,val force:Boolean=false,val operation:String=id,val manifest:String?=null,val cursor:String?=null,val revision:Long?=null,val complete:Boolean=false,val error:String?=null,val position:Int=0,val refreshRevision:Boolean=false)
@Entity(tableName="pins",primaryKeys=["selection","position"],indices=[Index("track")])
data class LocalPin(val selection:String,val position:Int,val track:String)
@Entity(tableName="pending_pins",primaryKeys=["selection","position"],indices=[Index("track")])
data class PendingPin(val selection:String,val position:Int,val track:String)
@Entity(tableName="local_media")
data class LocalMedia(@PrimaryKey val track:String,val ready:String?=null,val candidate:String?=null,val error:String?=null,val readyGeneration:String?=null,val candidateGeneration:String?=null)
@Entity(tableName="members",primaryKeys=["playlist","staged","position"],indices=[Index("track")])
data class Membership(val playlist:String,val staged:Boolean,val position:Int,val track:String)

@Dao
interface LocalDao {
    @Query("DELETE FROM members WHERE staged=1") fun clearMemberStage()
    @Query("DELETE FROM members WHERE playlist=:id AND staged=1") fun clearMemberStage(id:String)
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun member(row:Membership)
    @Query("DELETE FROM members WHERE playlist=:id AND staged=0") fun deleteMembers(id:String)
    @Query("DELETE FROM members WHERE staged=0") fun deleteMembers()
    @Query("INSERT OR REPLACE INTO members SELECT playlist,0,position,track FROM members WHERE staged=1") fun promoteMembers()
    @Query("INSERT OR REPLACE INTO members SELECT playlist,0,position,track FROM members WHERE staged=1 AND playlist=:id") fun promoteMembers(id:String)
    @Query("SELECT c.* FROM members p JOIN catalog c ON c.entity='track' AND c.id=p.track JOIN local_media m ON m.track=c.id AND m.ready IS NOT NULL WHERE p.playlist=:id AND p.staged=0 AND instr(c.search,:query)>0 ORDER BY p.position LIMIT :limit OFFSET :offset") fun playlistTracks(id:String,query:String,limit:Int=100,offset:Int=0):List<CatalogRow>
    @Query("SELECT c.* FROM catalog c WHERE c.entity=:entity AND c.deleted=0 AND ((:entity='album' AND EXISTS(SELECT 1 FROM catalog t JOIN local_media m ON m.track=t.id AND m.ready IS NOT NULL WHERE t.entity='track' AND t.album=c.id)) OR (:entity='artist' AND EXISTS(SELECT 1 FROM catalog t JOIN local_media m ON m.track=t.id AND m.ready IS NOT NULL WHERE t.entity='track' AND t.artist=c.id)) OR (:entity='playlist' AND EXISTS(SELECT 1 FROM members p JOIN local_media m ON m.track=p.track AND m.ready IS NOT NULL WHERE p.playlist=c.id AND p.staged=0))) ORDER BY c.id LIMIT :limit OFFSET :offset") fun localGroups(entity:String,limit:Int=100,offset:Int=0):List<CatalogRow>
    @Query("SELECT c.* FROM catalog c WHERE c.entity='track' AND (:offline=1 OR c.deleted=0) AND (:offline=0 OR EXISTS(SELECT 1 FROM local_media m WHERE m.track=c.id AND m.ready IS NOT NULL)) AND instr(c.search,:query)>0 AND (:album IS NULL OR c.album=:album) AND (:artist IS NULL OR c.artist=:artist) ORDER BY c.search,c.id LIMIT :limit OFFSET :offset") fun tracks(query:String,album:String?=null,artist:String?=null,offline:Boolean=false,limit:Int=100,offset:Int=0):List<CatalogRow>
    @Query("DELETE FROM catalog WHERE entity LIKE 'stage_%'") fun clearStage()
    @Query("INSERT OR REPLACE INTO catalog(entity,id,payload,deleted,sequence,search,album,artist) SELECT substr(entity,7),id,payload,deleted,sequence,search,album,artist FROM catalog WHERE entity LIKE 'stage_%'") fun promoteStage()
    @Query("DELETE FROM sync_state") fun clearSync()
    @Query("SELECT * FROM catalog WHERE entity=:entity AND deleted=0 ORDER BY id LIMIT :limit OFFSET :offset") fun page(entity:String,limit:Int=100,offset:Int=0):List<CatalogRow>
    @Query("SELECT * FROM catalog WHERE entity=:entity AND id=:id") fun record(entity:String,id:String):CatalogRow?
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun record(row:CatalogRow)
    @Query("UPDATE catalog SET deleted=1 WHERE entity=:entity AND id=:id") fun tombstone(entity:String,id:String)
    @Query("UPDATE catalog SET deleted=1 WHERE entity NOT LIKE 'stage_%'") fun markCatalogDeleted()
    @Query("SELECT * FROM sync_state WHERE id=1") fun sync():SyncState?
    @Query("SELECT * FROM sync_state WHERE id=1") fun watchSync():Flow<SyncState?>
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun sync(row:SyncState)
    @Query("SELECT * FROM selections ORDER BY rowid") fun selections():List<LocalSelection>
    @Query("SELECT * FROM selections ORDER BY rowid") fun watchSelections():Flow<List<LocalSelection>>
    @Query("SELECT * FROM selections WHERE id=:id") fun selection(id:String):LocalSelection?
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun selection(row:LocalSelection)
    @Query("DELETE FROM selections WHERE id=:id") fun deleteSelection(id:String)
    @Query("DELETE FROM pins WHERE selection=:id") fun deletePins(id:String)
    @Query("DELETE FROM pending_pins WHERE selection=:id") fun deletePendingPins(id:String)
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun pendingPin(row:PendingPin)
    @Query("INSERT INTO pins SELECT selection,position,track FROM pending_pins WHERE selection=:id") fun commitPins(id:String)
    @Query("SELECT track FROM pins WHERE selection=:id UNION SELECT track FROM pending_pins WHERE selection=:id") fun selectionTracks(id:String):List<String>
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun pin(row:LocalPin)
    @Query("SELECT * FROM pins WHERE selection=:id ORDER BY position") fun pins(id:String):List<LocalPin>
    @Query("SELECT (SELECT COUNT(*) FROM pins WHERE track=:id)+(SELECT COUNT(*) FROM pending_pins WHERE track=:id)") fun pinCount(id:String):Int
    @Query("UPDATE local_media SET candidate=NULL,error=CASE WHEN ready IS NULL THEN 'Трек удалён на сервере' ELSE NULL END WHERE track IN(SELECT id FROM catalog WHERE entity='track' AND deleted=1)") fun cancelDeletedCandidates()
    @Query("SELECT * FROM local_media WHERE track=:id") fun media(id:String):LocalMedia?
    @Query("SELECT * FROM local_media") fun media():List<LocalMedia>
    @Query("SELECT * FROM local_media WHERE candidate IS NOT NULL") fun candidates():List<LocalMedia>
    @Query("SELECT * FROM local_media") fun watchMedia():Flow<List<LocalMedia>>
    @Insert(onConflict=OnConflictStrategy.REPLACE) fun media(row:LocalMedia)
    @Query("DELETE FROM local_media WHERE track=:id") fun deleteMedia(id:String)
}

@Database(entities=[CatalogRow::class,SyncState::class,LocalSelection::class,LocalPin::class,PendingPin::class,LocalMedia::class,Membership::class],version=1,exportSchema=true)
abstract class LocalDatabase:RoomDatabase() {
    abstract fun dao():LocalDao
    companion object {
        fun scope(session:Session)=MessageDigest.getInstance("SHA-256").digest(session.baseUrl.toByteArray()).joinToString(""){"%02x".format(it)}.take(32)
        fun open(context:Context,scope:String)=Room.databaseBuilder(context.applicationContext,LocalDatabase::class.java,"catalog-$scope.db").build()
    }
}
