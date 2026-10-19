package com.vcp.mobile.voice

import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class VoiceSegmentStoreTest {
    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })

    private fun value(index: Long): Short = ((index % 20_000) - 10_000).toShort()

    @Test fun wrappedRingReturnsTheExactHalfOpenIntervalAndNeverClipsExpiredAudio() {
        val store = VoiceSegmentStore(7)
        repeat(1_002) { index ->
            val start = index * 512L
            assertTrue(store.append(7, start + 512, FloatArray(512) { value(start + it) / 32768f }))
        }
        val start = 32L * 16_000 - 32
        assertArrayEquals(ShortArray(64) { value(start + it) }, store.copy(7, start, start + 64))
        assertTrue(runCatching { store.copy(7, 0, 512) }.exceptionOrNull()?.message == "voice_segment_expired_or_unavailable")
        assertTrue(runCatching { store.copy(7, 2_000, 2_000 + 30L * 16_000 + 1) }.exceptionOrNull()?.message == "invalid_voice_segment_range")
    }

    @Test fun epochResetAndStopInvalidateOldReadsAndLateFrames() {
        val store = VoiceSegmentStore(1)
        store.append(1, 512, FloatArray(512) { 0.25f })
        store.reset(2)
        assertFalse(store.append(1, 1_024, FloatArray(512)))
        assertTrue(runCatching { store.copy(1, 0, 512) }.exceptionOrNull()?.message == "voice_segment_epoch_expired")
        assertTrue(runCatching { store.copy(2, 0, 1) }.isFailure)
        store.append(2, 512, FloatArray(512) { -0.5f })
        assertArrayEquals(ShortArray(2) { -16_384 }, store.copy(2, 10, 12))
        store.close()
        assertFalse(store.append(2, 1_024, FloatArray(512)))
        assertTrue(runCatching { store.copy(2, 10, 12) }.isFailure)
    }

    @Test fun sealedCapturePreservesRecordedAudioUntilExplicitRelease() {
        val store = VoiceSegmentStore(4)
        store.append(4, 200, FloatArray(200) { 0.25f })
        val accepted = store.copy(4, 10, 100)
        store.seal()
        assertFalse(store.append(4, 201, floatArrayOf(0.5f)))
        assertArrayEquals(ShortArray(90) { 8_192 }, store.copy(4, 10, 100))
        val file = store.writeWav(temporary.newFolder(), "sealed", accepted)
        assertTrue(file.exists())
        store.close()
        store.cleanupIfClosed()
        assertFalse(file.exists())
        assertTrue(runCatching { store.copy(4, 10, 100) }.isFailure)
    }

    @Test fun exportedWavContainsOnlyTheSelectedPcmAndIsRemovedOnSessionClose() {
        val store = VoiceSegmentStore(1)
        store.append(1, 512, FloatArray(512) { value(it.toLong()) / 32768f })
        val selected = store.copy(1, 10, 200)
        val file = store.writeWav(temporary.newFolder(), "session-one", selected)
        val bytes = file.readBytes()
        val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(44 + 190 * 2, bytes.size)
        assertEquals("RIFF", String(bytes, 0, 4, Charsets.US_ASCII))
        assertEquals(16_000, buffer.getInt(24))
        assertEquals(380, buffer.getInt(40))
        val actual = ShortArray(190)
        buffer.position(44)
        buffer.asShortBuffer().get(actual)
        assertArrayEquals(selected, actual)
        store.close()
        store.cleanupIfClosed()
        assertFalse(file.exists())
        assertTrue(runCatching { store.writeWav(file.parentFile!!, "late", selected) }.isFailure)
    }

    @Test fun temporaryFileBudgetIsReclaimedWhenRustDeletesAnExport() {
        val store = VoiceSegmentStore(1)
        val directory = temporary.newFolder()
        val files = List(8) { store.writeWav(directory, "bounded", shortArrayOf(1, 2)) }
        assertTrue(runCatching { store.writeWav(directory, "too-many", shortArrayOf(3)) }.exceptionOrNull()?.message == "voice_segment_file_limit")
        assertTrue(files.first().delete())
        val replacement = store.writeWav(directory, "replacement", shortArrayOf(4))
        assertTrue(replacement.exists())
        store.close()
        store.cleanupIfClosed()
        assertTrue(directory.listFiles().orEmpty().isEmpty())
    }
}
