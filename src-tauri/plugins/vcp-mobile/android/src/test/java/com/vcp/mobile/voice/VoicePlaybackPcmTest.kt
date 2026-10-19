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
        val queue = VoicePlaybackQueue(1)
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
        val queue = VoicePlaybackQueue(1)
        val pending = mutableListOf<String?>()
        queue.append(shortArrayOf(1)) {}
        queue.append(shortArrayOf(2)) { pending.add(it) }
        queue.cancel("cancelled:test")
        queue.cancel("cancelled:again")
        assertEquals(listOf("cancelled:test"), pending)
        assertTrue(runCatching { queue.take() }.exceptionOrNull()?.message == "cancelled:test")
        val empty = VoicePlaybackQueue()
        val worker = Executors.newSingleThreadExecutor()
        try {
            val read = worker.submit<String> { runCatching { empty.take() }.exceptionOrNull()?.message.orEmpty() }
            empty.cancel("cancelled:empty")
            assertEquals("cancelled:empty", read.get(2, TimeUnit.SECONDS))
        } finally { worker.shutdownNow() }
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

    @Test fun legacyScoDownsamplingFiltersFrequenciesAboveItsNyquistLimit() {
        fun tone(frequency: Int) = ShortArray(24_000) { (20_000 * sin(2 * PI * frequency * it / 24_000)).toInt().toShort() }
        fun rms(samples: ShortArray) = sqrt(samples.drop(512).sumOf { it.toDouble() * it } / (samples.size - 512))
        val low = rms(convert(tone(1_000), 511))
        val high = rms(convert(tone(10_000), 511))
        assertTrue(low > 12_000)
        assertTrue(high < low * 0.05)
    }
}
