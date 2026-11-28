package io.dagaz.player

import io.dagaz.player.editor.textPatch
import io.dagaz.player.editor.chunkOperation
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test
import java.util.UUID

class LibraryEditorTest {
    @Test fun unchangedFieldsAreOmittedAndEmptyEditsClearTags() {
        assertEquals(buildJsonObject { put("album", JsonNull); put("artist", "Новый") },
            textPatch(mapOf("album" to "", "artist" to "Новый", "title" to null)))
    }

    @Test fun uploadRetriesUseStableKeysWithoutReusingAnotherOffset() {
        val id = UUID.randomUUID()
        assertEquals(chunkOperation(id, 7), chunkOperation(id, 7))
        assertNotEquals(chunkOperation(id, 7), chunkOperation(id, 8))
        assertNotEquals(chunkOperation(id, 7), chunkOperation(UUID.randomUUID(), 7))
    }
}
