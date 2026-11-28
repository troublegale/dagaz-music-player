@file:OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
@file:kotlinx.serialization.UseSerializers(io.dagaz.player.api.UuidSerializer::class)
package io.dagaz.player.api

import kotlinx.serialization.*
import kotlinx.serialization.descriptors.*
import kotlinx.serialization.encoding.*
import kotlinx.serialization.json.*
import java.util.UUID

object Wire {
    val json = Json {
        namingStrategy = JsonNamingStrategy.SnakeCase
        encodeDefaults = true
        explicitNulls = true
        ignoreUnknownKeys = true
    }
}
object UuidSerializer : KSerializer<UUID> {
    override val descriptor = PrimitiveSerialDescriptor("UUID", PrimitiveKind.STRING)
    override fun serialize(encoder: Encoder, value: UUID) = encoder.encodeString(value.toString())
    override fun deserialize(decoder: Decoder): UUID = parse(decoder.decodeString())
    fun parse(value: String): UUID = UUID.fromString(value).also { require(it.toString().equals(value, ignoreCase = true)) }
}
@Serializable enum class Quality { original, high, low }
@Serializable enum class SelectionKind { track, album, playlist }
@Serializable data class ManifestCreate(val kind:SelectionKind,val id:UUID,val quality:Quality)
@Serializable data class ManifestCreated(val id:UUID)
@Serializable data class ManifestPage(val items:List<MediaDescriptor>,val nextCursor:String?=null,val selectionRevision:Long)
@Serializable enum class Platform { windows, android }
@Serializable enum class AudioFormat { wav, mp3, flac }
@Serializable enum class Repeat { off, one, all }
@Serializable enum class UploadStatus { uploading, processing, completed, failed, expired }
@Serializable enum class JobKind { `import`, retag, transcode, cleanup }
@Serializable enum class JobStatus { queued, running, succeeded, failed, partial }
@Serializable enum class Entity { track, album, artist, playlist, artwork, device }
@Serializable enum class AckPhase { ready, stopped, started, failed }
@Serializable enum class PlaybackAction { play_here, pause, resume, seek, next, previous, replace_queue, set_modes, transfer }

@Serializable class PairRequest(val pairingCode: String, val deviceName: String, val platform: Platform = Platform.android) {
    override fun toString() = "PairRequest([redacted])"
}
@Serializable class PairResponse(val deviceId: UUID, val token: String) {
    override fun toString() = "PairResponse(deviceId=$deviceId, token=[redacted])"
}
@Serializable class Session(val baseUrl: String, val deviceId: UUID, val token: String) {
    init { require(token.length == 64 && token.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) }
    override fun toString() = "Session(deviceId=$deviceId, token=[redacted])"
}
@Serializable data class ApiError(val code: String, val message: String, val requestId: UUID, val jobId: UUID? = null)
@Serializable data class Track(
    val id: UUID, val revision: Long, val filename: String,
    val title: String? = null, val artist: String? = null, val artistId: UUID? = null,
    val albumId: UUID? = null, val album: String? = null, val albumArtist: String? = null,
    val trackNumber: Int? = null, val discNumber: Int? = null,
    val durationMs: Long, val format: AudioFormat, val artworkId: UUID? = null,
    val originalBytes: Long, val readyQualities: List<Quality>
) { init { require(revision > 0 && durationMs >= 0 && originalBytes >= 0); require(trackNumber == null || trackNumber in 0..65535); require(discNumber == null || discNumber in 0..65535) } }
@Serializable data class Album(val id: UUID, val name: String, val albumArtist: String? = null, val albumArtistId: UUID? = null, val artworkId: UUID? = null)
@Serializable data class Artist(val id: UUID, val name: String)
@Serializable data class Device(val id: UUID, val deviceName: String, val platform: Platform, val online: Boolean)
@Serializable data class Page<T>(val items: List<T>, val nextCursor: String? = null)
@Serializable data class Playlist(val id: UUID, val revision: Long, val name: String, val trackIds: List<UUID>, val nextCursor: String? = null, val totalCount: Long) {
    init { require(revision > 0 && totalCount >= 0) }
}
@Serializable data class MediaDescriptor(val trackId: UUID, val revision: Long, val requestedQuality: Quality, val effectiveQuality: Quality, val byteLength: Long, val sha256: String, val etag: String, val url: String) {
    init { require(revision > 0 && byteLength >= 0) }
    val cacheKey:String get()="$url#sha256=$sha256"
}
@Serializable data class UploadState(val id: UUID, val offset: Long, val totalBytes: Long, val expiresAt: String, val status: UploadStatus) {
    init { require(offset >= 0 && totalBytes > 0) }
}
@Serializable data class Job(val id: UUID, val kind: JobKind, val status: JobStatus, val result: JsonElement? = null, val errors: List<ApiError>)
@Serializable data class Change(val sequence: Long, val entity: Entity, val entityId: UUID, val revision: Long, val deleted: Boolean, val payload: JsonElement? = null) {
    init { require(sequence > 0 && revision > 0) }
}
@Serializable data class SyncPage(val generation: UUID, val changes: List<Change>, val nextCursor: String, val hasMore: Boolean)
@Serializable data class SnapshotPage(val generation: UUID, val highWatermark: Long, val records: List<Change>, val nextCursor: String? = null) {
    init { require(highWatermark >= 0) }
}
@Serializable(with = MetadataPatchSerializer::class)
class MetadataPatch(val fields: JsonObject) {
    init {
        fields.forEach { (name, value) ->
            require(name in setOf("title", "artist", "album", "album_artist", "track_number", "disc_number", "artwork_id"))
            if (value != JsonNull) {
                require(value is JsonPrimitive)
                when (name) {
                    "track_number", "disc_number" -> require(!value.isString && value.intOrNull?.let { it in 0..65535 } == true)
                    "artwork_id" -> { require(value.isString); UuidSerializer.parse(value.content) }
                    else -> require(value.isString)
                }
            }
        }
    }
}
object MetadataPatchSerializer : KSerializer<MetadataPatch> {
    override val descriptor = JsonObject.serializer().descriptor
    override fun deserialize(decoder: Decoder) = MetadataPatch(JsonObject.serializer().deserialize(decoder))
    override fun serialize(encoder: Encoder, value: MetadataPatch) = JsonObject.serializer().serialize(encoder, value.fields)
}
@Serializable data class MetadataEdit(val expectedRevision: Long, val patch: MetadataPatch) { init { require(expectedRevision > 0) } }
@Serializable data class QueueUpdate(val queue: List<UUID>, val playOrder: List<Int>, val queueIndex: Int? = null, val positionMs: Long, val repeat: Repeat, val shuffleSeed: Long? = null) {
    init { require(positionMs >= 0 && playOrder.all { it >= 0 } && (queueIndex == null || queueIndex >= 0)) }
}
@Serializable data class PlaybackState(
    val epoch: Long, val ownerDeviceId: UUID? = null, val trackId: UUID? = null,
    val queue: List<UUID>, val playOrder: List<Int>, val queueIndex: Int? = null,
    val positionMs: Long, val paused: Boolean, val repeat: Repeat,
    val shuffleSeed: Long? = null, val lastReportSequence: Long
) {
    init {
        require(epoch > 0 && positionMs >= 0 && lastReportSequence >= 0)
        require(playOrder.sorted() == queue.indices.toList())
        require(shuffleSeed != null || playOrder == queue.indices.toList())
        require(if (queueIndex == null || trackId == null) queueIndex == null && trackId == null && queue.isEmpty() && paused else queue.getOrNull(queueIndex) == trackId)
    }
}
@Serializable data class PlaybackCommand(
    val operationId: UUID, val expectedEpoch: Long, val action: PlaybackAction,
    @EncodeDefault(EncodeDefault.Mode.NEVER) val payload: JsonObject? = null
) {
    init {
        require(expectedEpoch > 0)
        when (action) {
            PlaybackAction.seek -> require(payload?.get("position_ms")?.jsonPrimitive?.longOrNull?.let { it >= 0 } == true)
            PlaybackAction.play_here, PlaybackAction.replace_queue -> Wire.json.decodeFromJsonElement<QueueUpdate>(requireNotNull(payload))
            PlaybackAction.transfer -> UuidSerializer.parse(requireNotNull(payload)["target_device_id"]!!.jsonPrimitive.content)
            PlaybackAction.set_modes -> { requireNotNull(payload); Wire.json.decodeFromJsonElement<Repeat>(payload.getValue("repeat")); require(payload.getValue("play_order").jsonArray.all { it.jsonPrimitive.int >= 0 }) }
            else -> require(payload == null)
        }
    }
}
@Serializable data class PlaybackReport(val epoch: Long, val sequence: Long, val trackId: UUID, val queueIndex: Int, val positionMs: Long, val paused: Boolean) {
    init { require(epoch > 0 && sequence > 0 && queueIndex >= 0 && positionMs >= 0) }
}
@Serializable data class PlaybackAck(val operationId: UUID, val epoch: Long, val phase: AckPhase, val positionMs: Long? = null, val error: ApiError? = null) {
    init { require(epoch > 0) }
}
