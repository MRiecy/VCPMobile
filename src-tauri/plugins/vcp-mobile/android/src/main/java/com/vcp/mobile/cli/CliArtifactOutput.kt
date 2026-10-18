package com.vcp.mobile.cli

import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlin.math.min

// Output IO never decides whether a process is terminal; CliProcessHost owns that fence.

internal const val MAX_ARTIFACT_BYTES = 256L * 1024L * 1024L

internal enum class ArtifactStream {
    STDOUT,
    STDERR,
}

/**
 * A shared cap across stdout and stderr. Writes stop at the cap, but the caller
 * keeps draining the child pipe so a verbose process cannot deadlock or grow RAM.
 */
internal class ArtifactBudget(
    private val maxBytes: Long,
    stdoutFile: File,
    stderrFile: File,
) {
    private val lock = Any()
    private val stdout: FileOutputStream
    private val stderr: FileOutputStream
    private var stdoutOpen = true
    private var stderrOpen = true
    private var totalBytes = 0L

    val stdoutBytes = AtomicLong(0)
    val stderrBytes = AtomicLong(0)
    val stdoutTruncated = AtomicBoolean(false)
    val stderrTruncated = AtomicBoolean(false)

    init {
        require(maxBytes in 0..MAX_ARTIFACT_BYTES) { "artifactMaxBytes is outside the supported range" }
        val stdoutStream = FileOutputStream(stdoutFile, false)
        try {
            val stderrStream = FileOutputStream(stderrFile, false)
            stdout = stdoutStream
            stderr = stderrStream
        } catch (error: Throwable) {
            try {
                stdoutStream.close()
            } catch (_: Exception) {}
            throw error
        }
    }

    fun write(stream: ArtifactStream, bytes: ByteArray, count: Int) {
        if (count <= 0) return
        synchronized(lock) {
            val output = outputFor(stream)
            if (output == null) {
                markTruncated(stream)
                return
            }

            val allowed = min(count.toLong(), maxBytes - totalBytes).coerceAtLeast(0).toInt()
            if (allowed > 0) {
                try {
                    output.write(bytes, 0, allowed)
                    totalBytes += allowed
                    storedCounter(stream).addAndGet(allowed.toLong())
                } catch (_: Exception) {
                    markTruncated(stream)
                    closeStreamLocked(stream, sync = false)
                    return
                }
            }
            if (allowed < count) markTruncated(stream)
        }
    }

    fun closeStream(stream: ArtifactStream) {
        synchronized(lock) {
            closeStreamLocked(stream, sync = true)
        }
    }

    fun closeAll() {
        synchronized(lock) {
            closeStreamLocked(ArtifactStream.STDOUT, sync = true)
            closeStreamLocked(ArtifactStream.STDERR, sync = true)
        }
    }

    fun abandonStream(stream: ArtifactStream) {
        synchronized(lock) {
            markTruncated(stream)
            closeStreamLocked(stream, sync = true)
        }
    }

    private fun outputFor(stream: ArtifactStream): FileOutputStream? = when (stream) {
        ArtifactStream.STDOUT -> if (stdoutOpen) stdout else null
        ArtifactStream.STDERR -> if (stderrOpen) stderr else null
    }

    private fun storedCounter(stream: ArtifactStream) = when (stream) {
        ArtifactStream.STDOUT -> stdoutBytes
        ArtifactStream.STDERR -> stderrBytes
    }

    private fun markTruncated(stream: ArtifactStream) {
        when (stream) {
            ArtifactStream.STDOUT -> stdoutTruncated.set(true)
            ArtifactStream.STDERR -> stderrTruncated.set(true)
        }
    }

    private fun closeStreamLocked(stream: ArtifactStream, sync: Boolean) {
        val output = outputFor(stream) ?: return
        try {
            output.flush()
            if (sync) output.fd.sync()
        } catch (_: Exception) {
            markTruncated(stream)
        } finally {
            try {
                output.close()
            } catch (_: Exception) {
                markTruncated(stream)
            }
            when (stream) {
                ArtifactStream.STDOUT -> stdoutOpen = false
                ArtifactStream.STDERR -> stderrOpen = false
            }
        }
    }

    fun startDrainThread(
        name: String,
        input: InputStream,
        stream: ArtifactStream,
    ): Thread = Thread({
        try {
            input.use {
                val buffer = ByteArray(16 * 1024)
                while (true) {
                    val read = it.read(buffer)
                    if (read < 0) break
                    write(stream, buffer, read)
                }
            }
        } catch (_: Exception) {
            // A close during cancellation is expected. The waiter still fences terminal state.
        } finally {
            closeStream(stream)
        }
    }, name).apply {
        isDaemon = true
        start()
    }
}
