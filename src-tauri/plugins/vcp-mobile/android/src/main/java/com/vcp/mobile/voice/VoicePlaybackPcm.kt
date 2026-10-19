package com.vcp.mobile.voice

import java.util.ArrayDeque
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin

/** Four admitted chunks and one awaiting admission; producers never wait on the UI thread. */
internal class VoicePlaybackQueue(private val capacity: Int = 4) {
    data class Read(val samples: ShortArray?, val finished: Boolean)
    private data class Waiting(val samples: ShortArray, val admitted: (String?) -> Unit)
    private val lock = Object()
    private val chunks = ArrayDeque<ShortArray>()
    private var waiting: Waiting? = null
    private var sealed = false
    private var cancelled: String? = null

    init { require(capacity > 0) }

    fun append(samples: ShortArray, admitted: (String?) -> Unit) {
        val result = synchronized(lock) {
            when {
                cancelled != null -> cancelled
                sealed -> "voice_playback_already_finished"
                chunks.size < capacity -> { chunks.addLast(samples); lock.notifyAll(); "" }
                waiting == null -> { waiting = Waiting(samples, admitted); null }
                else -> "voice_playback_backpressure"
            }
        }
        if (result != null) admitted(result.ifEmpty { null })
    }

    fun take(waitMillis: Long = 0): Read {
        var promoted: Waiting? = null
        val result = synchronized(lock) {
            if (waitMillis > 0) {
                if (chunks.isEmpty() && !sealed && cancelled == null) lock.wait(waitMillis)
            } else while (chunks.isEmpty() && !sealed && cancelled == null) lock.wait()
            cancelled?.let { throw IllegalStateException(it) }
            val chunk = chunks.pollFirst()
            if (chunk != null) {
                promoted = waiting
                waiting = null
                promoted?.let { chunks.addLast(it.samples) }
            }
            Read(chunk, chunk == null && sealed)
        }
        promoted?.admitted?.invoke(null)
        return result
    }

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
            chunks.clear()
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
