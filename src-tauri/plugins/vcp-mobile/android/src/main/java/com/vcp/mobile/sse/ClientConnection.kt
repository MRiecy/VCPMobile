package com.vcp.mobile.sse

import java.io.BufferedOutputStream
import java.net.Socket
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

internal class ClientConnection(
    val socket: Socket,
    outputStream: java.io.OutputStream,
) {
    companion object {
        private const val WRITE_TIMEOUT_SECONDS = 5L
        private const val MAX_PENDING_FRAMES = 128
        private val writeWatchdog =
            ScheduledThreadPoolExecutor(1) { runnable ->
                Thread(runnable, "vcp-sse-write-watchdog").apply { isDaemon = true }
            }.apply { removeOnCancelPolicy = true }
    }

    private val socketOutput = outputStream
    private val outputStream = BufferedOutputStream(outputStream)
    private val closed = AtomicBoolean(false)
    private val writer = ThreadPoolExecutor(
        1,
        1,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(MAX_PENDING_FRAMES),
        { runnable -> Thread(runnable, "vcp-sse-client-writer").apply { isDaemon = true } },
        ThreadPoolExecutor.AbortPolicy(),
    )

    fun enqueue(json: String): Boolean {
        if (closed.get()) return false
        return try {
            writer.execute {
                val timeout = writeWatchdog.schedule({ close() }, WRITE_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                try {
                    writeFrame(json)
                } catch (_: Exception) {
                    close()
                } finally {
                    timeout.cancel(false)
                }
            }
            true
        } catch (_: RejectedExecutionException) {
            close()
            false
        }
    }

    // A snapshot occupies one queue slot. Frames are produced lazily, so a long
    // reply never becomes a second in-memory list of serialized event objects.
    fun enqueueFrames(produce: ((String) -> Unit) -> Unit): Boolean {
        if (closed.get()) return false
        return try {
            writer.execute {
                try {
                    produce { json ->
                        if (closed.get()) throw java.io.IOException("Connection closed")
                        val timeout = writeWatchdog.schedule(
                            { close() },
                            WRITE_TIMEOUT_SECONDS,
                            TimeUnit.SECONDS,
                        )
                        try {
                            writeFrame(json)
                        } finally {
                            timeout.cancel(false)
                        }
                    }
                } catch (_: Exception) {
                    close()
                }
            }
            true
        } catch (_: RejectedExecutionException) {
            close()
            false
        }
    }

    fun enqueueAndAwait(json: String): Boolean {
        if (closed.get()) return false
        val completed = CountDownLatch(1)
        val succeeded = AtomicBoolean(false)
        return try {
            writer.execute {
                val timeout = writeWatchdog.schedule({ close() }, WRITE_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                try {
                    writeFrame(json)
                    succeeded.set(true)
                } catch (_: Exception) {
                    close()
                } finally {
                    timeout.cancel(false)
                    completed.countDown()
                }
            }
            completed.await(WRITE_TIMEOUT_SECONDS + 1, TimeUnit.SECONDS) && succeeded.get()
        } catch (_: RejectedExecutionException) {
            close()
            false
        }
    }

    private fun writeFrame(json: String) {
        val bytes = json.toByteArray(Charsets.UTF_8)
        val len = bytes.size
        outputStream.write(byteArrayOf(
            ((len ushr 24) and 0xFF).toByte(),
            ((len ushr 16) and 0xFF).toByte(),
            ((len ushr 8) and 0xFF).toByte(),
            (len and 0xFF).toByte()
        ))
        outputStream.write(bytes)
        outputStream.flush()
    }

    fun close() {
        if (!closed.compareAndSet(false, true)) return
        writer.shutdownNow()
        // Close the socket first: flushing a buffered stream can wait for a
        // blocked writer, and the watchdog must be able to interrupt that write.
        try { socket.close() } catch (ignored: Exception) {}
        try { socketOutput.close() } catch (ignored: Exception) {}
    }
}
