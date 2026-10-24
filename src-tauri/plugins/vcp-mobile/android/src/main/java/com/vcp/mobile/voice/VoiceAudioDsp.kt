package com.vcp.mobile.voice

import java.io.InputStream
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/** Streaming, windowed-sinc conversion; retained PCM is bounded independently of call length. */
internal class VoiceFrameResampler(private val sourceRate: Int) {
    private val history = FloatArray(64)
    private val frame = FloatArray(512)
    private var inputIndex = -1L
    private var outputIndex = 0L
    private var frameSize = 0
    private var receivedSamples = 0L
    private var finished = false
    val hasPartialFrame: Boolean get() = frameSize != 0
    private val cutoff = min(1.0, 16_000.0 / sourceRate) * 0.90
    init { require(sourceRate in setOf(8_000, 16_000, 44_100, 48_000)) { "Unsupported microphone sample rate" } }
    private val divisor = gcd(sourceRate, 16_000)
    private val phaseCount = 16_000 / divisor
    private val coefficients = Array(phaseCount) { phase ->
        val fraction = phase.toDouble() / phaseCount
        val weights = DoubleArray(32) { index ->
            val distance = fraction - (index - 15)
            val x = PI * distance * cutoff
            val sinc = if (kotlin.math.abs(x) < 1e-9) 1.0 else sin(x) / x
            cutoff * sinc * (0.5 + 0.5 * cos(PI * distance / 16))
        }
        val sum = weights.sum()
        weights.indices.forEach { weights[it] /= sum }
        weights
    }

    fun push(samples: ShortArray, count: Int, consume: (FloatArray) -> Unit) {
        require(count in 0..samples.size)
        check(!finished) { "voice_resampler_finished" }
        receivedSamples += count
        for (index in 0 until count) {
            val sample = samples[index] / 32768f
            if (sourceRate == 16_000) {
                append(sample, consume)
                continue
            }
            pushFiltered(sample, Long.MAX_VALUE, consume)
        }
    }

    private fun pushFiltered(sample: Float, limit: Long, consume: (FloatArray) -> Unit) {
        inputIndex += 1
        history[(inputIndex % history.size).toInt()] = sample
        var numerator = outputIndex * sourceRate
        while (numerator / 16_000 + 16 <= inputIndex && outputIndex < limit) {
                val center = numerator / 16_000
                val weights = coefficients[(numerator % 16_000 / divisor).toInt()]
                var weighted = 0.0
                for (tap in -15..16) {
                    val sampleIndex = center + tap
                    val value = if (sampleIndex < 0) 0f else history[(sampleIndex % history.size).toInt()]
                    weighted += value * weights[tap + 15]
                }
                append(weighted.toFloat().coerceIn(-1f, 1f), consume)
                outputIndex += 1
                numerator = outputIndex * sourceRate
        }
    }

    /** Flush already-read input only; no further microphone acquisition is performed. */
    fun finish(consume: (FloatArray) -> Unit): FloatArray {
        if (finished) return FloatArray(0)
        finished = true
        if (sourceRate != 16_000) {
            val limit = receivedSamples * 16_000 / sourceRate
            repeat(16) { pushFiltered(0f, limit, consume) }
        }
        return frame.copyOf(frameSize).also { frameSize = 0 }
    }

    private fun append(sample: Float, consume: (FloatArray) -> Unit) {
        frame[frameSize++] = sample
        if (frameSize == frame.size) {
            frameSize = 0
            consume(frame) // Borrowed only until this synchronous callback returns.
        }
    }

    private fun gcd(first: Int, second: Int): Int {
        var a = first
        var b = second
        while (b != 0) { val remainder = a % b; a = b; b = remainder }
        return a
    }
}

/** One real 512-sample spectrum; no random animation or speech decision lives here. */
internal class VoiceAudioMeter {
    private val real = DoubleArray(512)
    private val imaginary = DoubleArray(512)
    private val window = DoubleArray(512) { 0.5 - 0.5 * cos(2 * PI * it / 511) }
    private val ranges = Array(32) { index ->
        val low = floor(80.0 * 100.0.pow(index / 32.0) / 31.25).toInt().coerceIn(1, 255)
        val high = kotlin.math.ceil(80.0 * 100.0.pow((index + 1) / 32.0) / 31.25).toInt().coerceIn(low + 1, 256)
        low until high
    }

    data class Reading(val level: Double, val bands: DoubleArray)

    fun measure(samples: FloatArray): Reading {
        require(samples.size == 512)
        var energy = 0.0
        for (i in samples.indices) {
            val value = samples[i].toDouble()
            energy += value * value
            real[i] = value * window[i]
            imaginary[i] = 0.0
        }
        var j = 0
        for (i in 1 until 512) {
            var bit = 256
            while (j and bit != 0) { j = j xor bit; bit = bit shr 1 }
            j = j xor bit
            if (i < j) { val saved = real[i]; real[i] = real[j]; real[j] = saved }
        }
        var length = 2
        while (length <= 512) {
            val angle = -2 * PI / length
            val stepReal = cos(angle)
            val stepImaginary = sin(angle)
            var base = 0
            while (base < 512) {
                var twiddleReal = 1.0
                var twiddleImaginary = 0.0
                for (offset in 0 until length / 2) {
                    val left = base + offset
                    val right = left + length / 2
                    val oddReal = real[right] * twiddleReal - imaginary[right] * twiddleImaginary
                    val oddImaginary = real[right] * twiddleImaginary + imaginary[right] * twiddleReal
                    real[right] = real[left] - oddReal
                    imaginary[right] = imaginary[left] - oddImaginary
                    real[left] += oddReal
                    imaginary[left] += oddImaginary
                    val nextReal = twiddleReal * stepReal - twiddleImaginary * stepImaginary
                    twiddleImaginary = twiddleReal * stepImaginary + twiddleImaginary * stepReal
                    twiddleReal = nextReal
                }
                base += length
            }
            length *= 2
        }
        val rms = sqrt(energy / samples.size)
        val level = ((rms - 0.002).coerceAtLeast(0.0) / 0.06).pow(0.65).coerceAtMost(1.0)
        val bands = DoubleArray(32) { index ->
            var peak = 0.0
            for (bin in ranges[index]) peak = max(peak, sqrt(real[bin] * real[bin] + imaginary[bin] * imaginary[bin]) / 128)
            // -70..-15 dB display range, separate from the untouched model input.
            val decibels = 20 * ln(max(peak, 1e-7)) / ln(10.0)
            ((decibels + 70) / 55).coerceIn(0.0, 1.0) * sqrt(level)
        }
        return Reading(level, bands)
    }
}

internal fun readBoundedVoiceAsset(input: InputStream, maximum: Int): ByteArray {
    val output = java.io.ByteArrayOutputStream()
    val chunk = ByteArray(8192)
    while (true) {
        val count = input.read(chunk)
        if (count < 0) return output.toByteArray()
        if (count == 0) continue
        require(output.size() <= maximum - count) { "Voice asset exceeds its size limit" }
        output.write(chunk, 0, count)
    }
}
