package com.vcp.mobile.voice

import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.roundToInt

/** A 32-second ring at the same 16 kHz sample clock reported to the VAD consumer. */
internal class VoiceSegmentStore(initialEpoch: Long) {
    private val pcm = ShortArray(32 * 16_000)
    private val pcmLock = Any()
    private val fileLock = Any()
    private val files = LinkedHashSet<File>()
    private var epoch = initialEpoch
    private var endSample = 0L
    @Volatile private var closed = false
    private var sealed = false

    fun reset(nextEpoch: Long) = synchronized(pcmLock) {
        if (!closed && !sealed) { epoch = nextEpoch; endSample = 0 }
    }

    fun append(frameEpoch: Long, frameEndSample: Long, frame: FloatArray): Boolean = synchronized(pcmLock) {
        if (closed || sealed || frameEpoch != epoch) return@synchronized false
        check(frameEndSample - frame.size == endSample) { "voice_segment_discontinuity" }
        frame.forEachIndexed { index, value ->
            pcm[((endSample + index) % pcm.size).toInt()] = (value.coerceIn(-1f, 1f) * 32768)
                .roundToInt().coerceIn(-32768, 32767).toShort()
        }
        endSample = frameEndSample
        true
    }

    fun copy(requestEpoch: Long, start: Long, end: Long): ShortArray = synchronized(pcmLock) {
        check(!closed) { "cancelled:voice_segment" }
        require(requestEpoch == epoch) { "voice_segment_epoch_expired" }
        require(start >= 0 && end > start && end - start <= 30L * 16_000) { "invalid_voice_segment_range" }
        require(start >= maxOf(0, endSample - pcm.size) && end <= endSample) { "voice_segment_expired_or_unavailable" }
        val result = ShortArray((end - start).toInt())
        val position = (start % pcm.size).toInt()
        val first = minOf(result.size, pcm.size - position)
        pcm.copyInto(result, 0, position, position + first)
        if (first < result.size) pcm.copyInto(result, first, 0, result.size - first)
        result
    }

    /** Runs only on the bounded file worker, never on main or the capture worker. */
    fun writeWav(directory: File, sessionId: String, samples: ShortArray): File {
        require(samples.isNotEmpty() && samples.size <= 30 * 16_000) { "invalid_voice_segment_range" }
        check(!closed) { "cancelled:voice_segment" }
        val expired = synchronized(fileLock) { files.toList() }.filter { !it.exists() }
        synchronized(fileLock) { files.removeAll(expired.toSet()) }
        check(synchronized(fileLock) { files.size < 8 }) { "voice_segment_file_limit" }
        check(directory.isDirectory || directory.mkdirs()) { "voice_segment_cache_unavailable" }
        val file = File.createTempFile("voice-$sessionId-", ".wav", directory)
        val registered = synchronized(fileLock) { if (closed) false else { files.add(file); true } }
        if (!registered) { file.delete(); throw IllegalStateException("cancelled:voice_segment") }
        try {
            BufferedOutputStream(FileOutputStream(file)).use { output ->
                val dataBytes = samples.size * 2
                val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)
                header.put("RIFF".toByteArray(Charsets.US_ASCII)).putInt(36 + dataBytes)
                    .put("WAVEfmt ".toByteArray(Charsets.US_ASCII)).putInt(16)
                    .putShort(1).putShort(1).putInt(16_000).putInt(32_000).putShort(2).putShort(16)
                    .put("data".toByteArray(Charsets.US_ASCII)).putInt(dataBytes)
                output.write(header.array())
                val chunk = ByteArray(8192)
                var position = 0
                while (position < samples.size) {
                    check(!closed) { "cancelled:voice_segment" }
                    val count = minOf(samples.size - position, chunk.size / 2)
                    repeat(count) { index ->
                        val value = samples[position + index].toInt()
                        chunk[index * 2] = value.toByte()
                        chunk[index * 2 + 1] = (value shr 8).toByte()
                    }
                    output.write(chunk, 0, count * 2)
                    position += count
                }
            }
            check(!closed) { "cancelled:voice_segment" }
            return file
        } catch (error: Throwable) {
            discard(file)
            throw error
        }
    }

    fun discard(file: File) {
        synchronized(fileLock) { files.remove(file) }
        file.delete()
    }

    fun close() {
        closed = true
        synchronized(pcmLock) { sealed = true; endSample = 0; pcm.fill(0) }
    }

    fun seal() = synchronized(pcmLock) { sealed = true }

    /** A session with outstanding exports is retained only until this worker cleanup runs. */
    fun cleanupIfClosed() {
        if (!closed) return
        val expired = synchronized(fileLock) { files.toList().also { files.clear() } }
        expired.forEach { it.delete() }
    }

    fun hasFiles(): Boolean = synchronized(fileLock) { files.isNotEmpty() }
}
