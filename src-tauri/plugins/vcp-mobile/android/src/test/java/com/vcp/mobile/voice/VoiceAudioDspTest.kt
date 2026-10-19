package com.vcp.mobile.voice

import java.io.ByteArrayInputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.sin
import kotlin.math.sqrt
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceAudioDspTest {
    private fun resample(input: ShortArray, rate: Int, chunk: Int): FloatArray {
        val converter = VoiceFrameResampler(rate)
        val frames = mutableListOf<FloatArray>()
        var offset = 0
        while (offset < input.size) {
            val end = minOf(input.size, offset + chunk)
            converter.push(input.copyOfRange(offset, end), end - offset) { frames.add(it.copyOf()) }
            offset = end
        }
        return frames.flatMap { it.asIterable() }.toFloatArray()
    }

    @Test fun resamplingIsIndependentOfAudioRecordReadBoundaries() {
        val signal = ShortArray(44_100) { (12_000 * sin(2 * PI * 997 * it / 44_100)).toInt().toShort() }
        val contiguous = resample(signal, 44_100, signal.size)
        val fragmented = resample(signal, 44_100, 137)
        assertArrayEquals(contiguous, fragmented, 0f)
        assertEquals(31 * 512, fragmented.size)
        assertTrue(fragmented.all { it.isFinite() && it in -1f..1f })
    }

    @Test fun native16kSamplesAreNotFilteredOrDuplicatedAcrossPartialReads() {
        val input = ShortArray(1_537) { (it - 700).toShort() }
        val output = resample(input, 16_000, 97)
        assertEquals(1_536, output.size)
        output.forEachIndexed { index, sample -> assertEquals(input[index] / 32768f, sample, 0f) }
    }

    @Test fun sealIncludesThePartialVadWindowAndAlreadyReadResamplingTail() {
        for (rate in listOf(8_000, 16_000, 44_100, 48_000)) {
            val input = ShortArray(rate / 3 + 7) { (it % 8_000 - 4_000).toShort() }
            val converter = VoiceFrameResampler(rate)
            val output = mutableListOf<Float>()
            converter.push(input, input.size) { output.addAll(it.asIterable()) }
            val partial = converter.finish { output.addAll(it.asIterable()) }
            output.addAll(partial.asIterable())
            assertEquals(input.size.toLong() * 16_000 / rate, output.size.toLong())
            assertTrue(partial.size < 512)
            assertTrue(converter.finish {}.isEmpty())
        }
    }

    @Test fun downsamplingRejectsOutOfBandEnergyInsteadOfAliasingItIntoSpeech() {
        fun tone(frequency: Int) = ShortArray(48_000) { (20_000 * sin(2 * PI * frequency * it / 48_000)).toInt().toShort() }
        fun rms(samples: FloatArray): Double = sqrt(samples.drop(512).sumOf { it.toDouble() * it } / (samples.size - 512))
        val speechBand = rms(resample(tone(1_000), 48_000, 480))
        val outsideBand = rms(resample(tone(12_000), 48_000, 480))
        assertTrue(speechBand > 0.35)
        assertTrue(outsideBand < speechBand * 0.05)
    }

    @Test fun meterIsSilentForZeroPcmAndTracksDifferentRealFrequencyBands() {
        val meter = VoiceAudioMeter()
        val silence = meter.measure(FloatArray(512))
        assertEquals(0.0, silence.level, 0.0)
        assertTrue(silence.bands.all { it == 0.0 })
        fun peak(frequency: Int): Int {
            val reading = meter.measure(FloatArray(512) { (0.3 * sin(2 * PI * frequency * it / 16_000)).toFloat() })
            assertTrue(reading.level > 0)
            return reading.bands.indices.maxBy { reading.bands[it] }
        }
        assertTrue(peak(1_000) in 16..18)
        assertTrue(peak(3_000) in 24..26)
    }

    @Test fun probeParserAcceptsOnlyBoundedMonoPcmAndRejectsTruncatedData() {
        val payload = ByteBuffer.allocate(44 + 16).order(ByteOrder.LITTLE_ENDIAN)
        payload.put("RIFF".toByteArray()).putInt(52).put("WAVEfmt ".toByteArray()).putInt(16)
            .putShort(1).putShort(1).putInt(16_000).putInt(32_000).putShort(2).putShort(16)
            .put("data".toByteArray()).putInt(16)
        repeat(8) { payload.putShort((it * 100).toShort()) }
        val decoded = readVoiceProbe(ByteArrayInputStream(payload.array()))
        assertEquals(16_000, decoded.sampleRate)
        assertArrayEquals(ShortArray(8) { (it * 100).toShort() }, decoded.samples)
        assertTrue(runCatching { readVoiceProbe(ByteArrayInputStream(payload.array().copyOf(48))) }.isFailure)
        assertTrue(runCatching { readBoundedVoiceAsset(ByteArrayInputStream(ByteArray(65)), 64) }.isFailure)
        payload.putShort(22, 2)
        assertTrue(runCatching { readVoiceProbe(ByteArrayInputStream(payload.array())) }.isFailure)
    }
}
