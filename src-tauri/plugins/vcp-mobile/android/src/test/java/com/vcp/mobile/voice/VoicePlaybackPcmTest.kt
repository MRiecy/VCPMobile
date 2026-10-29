package com.vcp.mobile.voice

import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.math.PI
import kotlin.math.sin
import kotlin.math.sqrt
import org.junit.Assert.*
import org.junit.Test

class VoicePlaybackPcmTest {
    @Test fun fullQueueDefersOneAppendUntilConsumptionAndRejectsAdditionalUnboundedWork() {
        val queue = VoicePlaybackQueue(capacityFrames = 1, startupFrames = 1)
        val accepted = mutableListOf<String>()
        queue.append(shortArrayOf(1)) { accepted.add("first:$it") }
        queue.append(shortArrayOf(2)) { accepted.add("second:$it") }
        queue.append(shortArrayOf(3)) { accepted.add("third:$it") }
        assertEquals(listOf("first:null", "third:voice_playback_backpressure"), accepted)
        assertArrayEquals(shortArrayOf(1), queue.take().samples)
        assertEquals(listOf("first:null", "third:voice_playback_backpressure", "second:null"), accepted)
        queue.seal()
        assertArrayEquals(shortArrayOf(2), queue.take().samples)
        assertTrue(queue.take().finished)
    }

    @Test fun cancellationSettlesThePendingAppendAndWakesAWaitingConsumer() {
        val queue = VoicePlaybackQueue(capacityFrames = 1, startupFrames = 1)
        val pending = mutableListOf<String?>()
        queue.append(shortArrayOf(1)) {}
        queue.append(shortArrayOf(2)) { pending.add(it) }
        queue.cancel("cancelled:test")
        queue.cancel("cancelled:again")
        assertEquals(listOf("cancelled:test"), pending)
        assertTrue(runCatching { queue.take() }.exceptionOrNull()?.message == "cancelled:test")
        val empty = VoicePlaybackQueue(startupFrames = 5_760)
        val worker = Executors.newSingleThreadExecutor()
        try {
            val read = worker.submit<String> { runCatching { empty.take() }.exceptionOrNull()?.message.orEmpty() }
            empty.cancel("cancelled:empty")
            assertEquals("cancelled:empty", read.get(2, TimeUnit.SECONDS))
        } finally { worker.shutdownNow() }
    }

    @Test fun thousandsOfSingleFramePacketsReachTheWatermarkWithoutExhaustingPacketSlots() {
        val queue = VoicePlaybackQueue(startupFrames = 5_760)
        val expected = ShortArray(5_760) { it.toShort() }
        expected.forEach { sample -> queue.append(shortArrayOf(sample)) { assertNull(it) } }
        assertArrayEquals(expected, queue.take().samples)
        queue.append(shortArrayOf(42)) { assertNull(it) }
        assertArrayEquals(shortArrayOf(42), queue.take().samples) // the startup gate opens once
    }

    @Test fun shortCompletedSegmentStartsBeforeTheJobEndsAndEmptySegmentsDoNotOpenTheGate() {
        val queue = VoicePlaybackQueue(startupFrames = 5_760)
        queue.endSegment()
        queue.append(shortArrayOf(11, 22, 33)) { assertNull(it) }
        assertNull(queue.take(1).samples)
        queue.endSegment()
        val first = queue.take()
        assertArrayEquals(shortArrayOf(11, 22, 33), first.samples)
        assertTrue(first.segmentEnd)
        assertFalse(first.finished)
        queue.append(shortArrayOf(44)) { assertNull(it) }
        assertArrayEquals(shortArrayOf(44), queue.take().samples)
    }

    @Test fun ringWrapAndPendingAdmissionPreserveEverySampleAndSegmentOrder() {
        val queue = VoicePlaybackQueue(capacityFrames = 8, startupFrames = 3)
        val admitted = mutableListOf<String>()
        queue.append(shortArrayOf(1, 2, 3, 4, 5, 6)) { admitted.add("a:$it") }
        queue.append(shortArrayOf(7, 8, 9, 10)) { admitted.add("b:$it") }
        assertEquals(listOf("a:null"), admitted)
        assertArrayEquals(shortArrayOf(1, 2, 3, 4, 5, 6), queue.take().samples)
        assertEquals(listOf("a:null", "b:null"), admitted)
        queue.endSegment()
        queue.append(shortArrayOf(11, 12)) { assertNull(it) }
        val boundary = queue.take()
        assertArrayEquals(shortArrayOf(7, 8, 9, 10), boundary.samples)
        assertTrue(boundary.segmentEnd)
        queue.seal()
        val last = queue.take()
        assertArrayEquals(shortArrayOf(11, 12), last.samples)
        assertTrue(last.finished)
    }

    @Test fun eosReleasesAShortFirstBufferAndCancellationReleasesAClosedStartupGate() {
        val queue = VoicePlaybackQueue(startupFrames = 5_760)
        queue.append(shortArrayOf(123)) { assertNull(it) }
        queue.seal()
        val result = queue.take()
        assertArrayEquals(shortArrayOf(123), result.samples)
        assertTrue(result.finished)
        val cancelled = VoicePlaybackQueue(startupFrames = 5_760)
        cancelled.append(shortArrayOf(456)) { assertNull(it) }
        cancelled.cancel("cancelled:before_start")
        assertEquals("cancelled:before_start", runCatching { cancelled.take() }.exceptionOrNull()?.message)
    }

    private fun convert(input: ShortArray, chunk: Int): ShortArray {
        val resampler = VoiceLegacyScoResampler()
        val output = mutableListOf<Short>()
        var position = 0
        while (position < input.size) {
            val end = minOf(position + chunk, input.size)
            output.addAll(resampler.process(input.copyOfRange(position, end)).asIterable())
            position = end
        }
        output.addAll(resampler.finish().asIterable())
        return output.toShortArray()
    }

    @Test fun legacyScoConversionKeepsChunkBoundariesAndFlushesTheFinalSamples() {
        val pcm = ShortArray(24_001) { (15_000 * sin(2 * PI * 997 * it / 24_000)).toInt().toShort() }
        val contiguous = convert(pcm, pcm.size)
        val fragmented = convert(pcm, 137)
        assertEquals(16_000, contiguous.size)
        assertArrayEquals(contiguous, fragmented)
    }

    @Test fun legacyScoRangeConversionSkipsSurroundingSamplesAndPreservesFilterContinuity() {
        val pcm = ShortArray(2_401) { (12_000 * sin(2 * PI * 997 * it / 24_000)).toInt().toShort() }
        val padded = ShortArray(pcm.size + 18) { Short.MAX_VALUE }
        pcm.copyInto(padded, 7)
        val resampler = VoiceLegacyScoResampler()
        val output = mutableListOf<Short>()
        var position = 0
        while (position < pcm.size) {
            val count = minOf(137, pcm.size - position)
            output.addAll(resampler.process(padded, 7 + position, count).asIterable())
            position += count
        }
        output.addAll(resampler.finish().asIterable())
        assertArrayEquals(convert(pcm, pcm.size), output.toShortArray())
    }

    @Test fun legacyScoDownsamplingFiltersFrequenciesAboveItsNyquistLimit() {
        fun tone(frequency: Int) = ShortArray(24_000) { (20_000 * sin(2 * PI * frequency * it / 24_000)).toInt().toShort() }
        fun rms(samples: ShortArray) = sqrt(samples.drop(512).sumOf { it.toDouble() * it } / (samples.size - 512))
        val low = rms(convert(tone(1_000), 511))
        val high = rms(convert(tone(10_000), 511))
        assertTrue(low > 12_000)
        assertTrue(high < low * 0.05)
    }
}
