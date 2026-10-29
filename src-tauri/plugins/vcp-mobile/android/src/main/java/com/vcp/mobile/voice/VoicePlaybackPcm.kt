package com.vcp.mobile.voice

import java.util.ArrayDeque
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin

/** A two-second input-domain ring and one bounded pending append. Tiny packets consume frames, not slots. */
internal class VoicePlaybackQueue(
    private val capacityFrames: Int = 48_000,
    private val startupFrames: Int,
) {
    data class Read(val samples: ShortArray?, val finished: Boolean, val segmentEnd: Boolean = false)
    private data class Waiting(val samples: ShortArray, val admitted: (String?) -> Unit)
    private val lock = Object()
    private val samples = ShortArray(capacityFrames)
    private val segmentEnds = ArrayDeque<Long>()
    private var readIndex = 0
    private var bufferedFrames = 0
    private var acceptedFrames = 0L
    private var consumedFrames = 0L
    private var lastSegmentEnd = 0L
    private var startupOpen = false
    private var waiting: Waiting? = null
    private var sealed = false
    private var cancelled: String? = null

    init { require(startupFrames in 1..capacityFrames) }

    private fun accept(input: ShortArray) {
        val offset = (readIndex + bufferedFrames) % capacityFrames
        val first = minOf(input.size, capacityFrames - offset)
        input.copyInto(samples, offset, 0, first)
        input.copyInto(samples, 0, first)
        bufferedFrames += input.size
        acceptedFrames += input.size
        if (bufferedFrames >= startupFrames) startupOpen = true
        lock.notifyAll()
    }

    fun append(samples: ShortArray, admitted: (String?) -> Unit) {
        val result = synchronized(lock) {
            when {
                samples.isEmpty() || samples.size > minOf(16_384, capacityFrames) -> "voice_playback_invalid_pcm"
                cancelled != null -> cancelled
                sealed -> "voice_playback_already_finished"
                waiting != null -> "voice_playback_backpressure"
                samples.size <= capacityFrames - bufferedFrames -> { accept(samples); "" }
                waiting == null -> { waiting = Waiting(samples, admitted); null }
                else -> "voice_playback_backpressure"
            }
        }
        if (result != null) admitted(result.ifEmpty { null })
    }

    fun take(waitMillis: Long = 0): Read {
        var promoted: Waiting? = null
        val result = synchronized(lock) {
            fun available() = (startupOpen && bufferedFrames > 0) || segmentEnds.firstOrNull() == consumedFrames || sealed
            if (waitMillis > 0) {
                if (!available() && cancelled == null) lock.wait(waitMillis)
            } else while (!available() && cancelled == null) lock.wait()
            cancelled?.let { throw IllegalStateException(it) }
            var count = if (startupOpen || sealed) minOf(bufferedFrames, 16_384) else 0
            segmentEnds.firstOrNull()?.let { count = minOf(count.toLong(), it - consumedFrames).toInt() }
            val chunk = if (count > 0) ShortArray(count).also { output ->
                val first = minOf(count, capacityFrames - readIndex)
                samples.copyInto(output, 0, readIndex, readIndex + first)
                samples.copyInto(output, first, 0, count - first)
                readIndex = (readIndex + count) % capacityFrames
                bufferedFrames -= count
                consumedFrames += count
            } else null
            val segmentEnd = segmentEnds.firstOrNull() == consumedFrames
            if (segmentEnd) segmentEnds.removeFirst()
            waiting?.takeIf { it.samples.size <= capacityFrames - bufferedFrames }?.let {
                waiting = null; accept(it.samples); promoted = it
            }
            Read(chunk, sealed && bufferedFrames == 0 && waiting == null && segmentEnds.isEmpty(), segmentEnd)
        }
        promoted?.admitted?.invoke(null)
        return result
    }

    fun endSegment() = synchronized(lock) {
        cancelled?.let { throw IllegalStateException(it) }
        check(!sealed) { "voice_playback_already_finished" }
        check(waiting == null) { "voice_playback_append_pending" }
        if (acceptedFrames > lastSegmentEnd) {
            segmentEnds.addLast(acceptedFrames)
            lastSegmentEnd = acceptedFrames
            startupOpen = true
            lock.notifyAll()
        }
    }

    fun hasBufferedAudio(): Boolean = synchronized(lock) { bufferedFrames > 0 || waiting != null }

    fun seal() = synchronized(lock) {
        cancelled?.let { throw IllegalStateException(it) }
        check(!sealed) { "voice_playback_already_finished" }
        sealed = true
        lock.notifyAll()
    }

    fun cancel(reason: String) {
        val pending = synchronized(lock) {
            if (cancelled != null) return
            cancelled = reason
            bufferedFrames = 0
            segmentEnds.clear()
            val pending = waiting
            waiting = null
            lock.notifyAll()
            pending
        }
        pending?.admitted?.invoke(reason)
    }
}

/** 24 kHz API PCM -> 16 kHz legacy SCO, including the final partial filter window. */
internal class VoiceLegacyScoResampler {
    private val history = ShortArray(64)
    private var inputIndex = -1L
    private var inputCount = 0L
    private var outputCount = 0L
    private var finished = false
    private val phases = Array(2) { phase ->
        DoubleArray(32) { index ->
            val distance = phase / 2.0 - (index - 15)
            val x = PI * distance * 0.6
            0.6 * (if (kotlin.math.abs(x) < 1e-9) 1.0 else sin(x) / x) * (0.5 + 0.5 * cos(PI * distance / 16))
        }.also { weights -> val sum = weights.sum(); weights.indices.forEach { weights[it] /= sum } }
    }

    fun process(input: ShortArray): ShortArray {
        check(!finished) { "voice_resampler_finished" }
        inputCount += input.size
        return convert(input, input.size)
    }

    fun finish(): ShortArray {
        check(!finished) { "voice_resampler_finished" }
        finished = true
        return convert(ShortArray(16), 16)
    }

    private fun convert(input: ShortArray, count: Int): ShortArray {
        val result = ShortArray(count * 2 / 3 + 32)
        var written = 0
        repeat(count) { index ->
            inputIndex += 1
            history[(inputIndex % history.size).toInt()] = input[index]
            while (outputCount * 3 / 2 + 16 <= inputIndex && outputCount < inputCount * 2 / 3) {
                val center = outputCount * 3 / 2
                val weights = phases[(outputCount % 2).toInt()]
                var value = 0.0
                for (tap in -15..16) {
                    val sample = center + tap
                    if (sample >= 0) value += history[(sample % history.size).toInt()] * weights[tap + 15]
                }
                result[written++] = value.roundToInt().coerceIn(-32768, 32767).toShort()
                outputCount += 1
            }
        }
        return result.copyOf(written)
    }
}
