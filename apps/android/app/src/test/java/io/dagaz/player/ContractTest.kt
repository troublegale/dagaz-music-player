package io.dagaz.player

import io.dagaz.player.api.*
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

class ContractTest {
    private fun fixture(name: String) = javaClass.classLoader!!.getResource(name)!!.readText()
    private fun <T> roundTrip(serializer: KSerializer<T>, value: JsonElement) {
        assertEquals(value, Wire.json.encodeToJsonElement(serializer, Wire.json.decodeFromJsonElement(serializer, value)))
    }
    @Test fun fixtureRoundTrip() {
        val messages = Wire.json.parseToJsonElement(fixture("messages.json")).jsonObject
        val serializers = mapOf(
            "PairRequest" to PairRequest.serializer(), "PairResponse" to PairResponse.serializer(),
            "Playlist" to Playlist.serializer(), "MediaDescriptor" to MediaDescriptor.serializer(),
            "UploadState" to UploadState.serializer(), "Job" to Job.serializer(), "Change" to Change.serializer(),
            "SyncPage" to SyncPage.serializer(), "MetadataEdit" to MetadataEdit.serializer(),
            "PlaybackCommand" to PlaybackCommand.serializer(), "QueueUpdate" to QueueUpdate.serializer(),
            "PlaybackReport" to PlaybackReport.serializer(), "PlaybackAck" to PlaybackAck.serializer()
        )
        assertEquals(messages.keys, serializers.keys)
        serializers.forEach { (name, serializer) -> roundTrip(serializer, messages.getValue(name)) }
        roundTrip(Track.serializer(), Wire.json.parseToJsonElement(fixture("track-null-tags.json")))
        roundTrip(ApiError.serializer(), Wire.json.parseToJsonElement(fixture("errors.json")))
        val playback = Wire.json.decodeFromString<PlaybackState>(fixture("playback-duplicates.json"))
        assertEquals(1, playback.queueIndex)
        assertEquals(listOf(1, 0), playback.playOrder)
        roundTrip(PlaybackState.serializer(), Wire.json.parseToJsonElement(fixture("playback-duplicates.json")))
        val patch = Wire.json.decodeFromString<MetadataPatch>(fixture("metadata-clear.json"))
        val encoded = Wire.json.encodeToJsonElement(patch).jsonObject
        assertEquals(JsonNull, encoded["album"])
        assertFalse(encoded.containsKey("artist"))
    }
    @Test fun rejectsInvalidWireValues() {
        for (value in listOf("{\"album\":1}", "{\"album\":false}", "{\"typo\":null}", "{\"track_number\":-1}", "{\"artwork_id\":\"invalid\"}")) {
            assertThrows(Exception::class.java) { Wire.json.decodeFromString<MetadataPatch>(value) }
        }
        assertThrows(Exception::class.java) { Wire.json.decodeFromString<Quality>("\"ultra\"") }
        val state = Wire.json.parseToJsonElement(fixture("playback-duplicates.json")).jsonObject
        for ((key, value) in listOf("epoch" to JsonPrimitive(0), "position_ms" to JsonPrimitive(-1), "play_order" to JsonArray(listOf(JsonPrimitive(1), JsonPrimitive(1))), "queue_index" to JsonPrimitive(2))) {
            assertThrows(Exception::class.java) { Wire.json.decodeFromJsonElement<PlaybackState>(JsonObject(state + (key to value))) }
        }
    }
}
