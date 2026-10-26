package com.vcp.mobile.sse

import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.asRequestBody
import okio.BufferedSink
import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

internal const val MAX_REQUEST_BODY_BYTES = 128L * 1024 * 1024

/** A helper generation owns a file, including any upload still draining on cancel. */
internal class RequestBodyFile private constructor(private val file: File) : RequestBody(), AutoCloseable {
    private val body = file.asRequestBody("application/json; charset=utf-8".toMediaType())
    private val length = file.length()
    private var writers = 0
    private var closed = false

    override fun contentType() = body.contentType()
    override fun contentLength() = length

    override fun writeTo(sink: BufferedSink) {
        synchronized(this) {
            if (closed) throw IOException("Request body has been released")
            writers++
        }
        try {
            body.writeTo(sink)
        } finally {
            synchronized(this) {
                writers--
                if (closed && writers == 0) file.delete()
            }
        }
    }

    @Synchronized
    override fun close() {
        closed = true
        // Cancel can race writeTo. Prevent later writes, but retain the file
        // until an already admitted upload has released its input stream.
        if (writers == 0) file.delete()
    }

    companion object {
        private val cleanedPreviousProcess = AtomicBoolean(false)

        fun cleanPreviousProcess(cacheDir: File) {
            // A coordinator can restart while an old cancelled upload drains.
            // Sweep only once in this helper process, before admitting requests.
            if (!cleanedPreviousProcess.compareAndSet(false, true)) return
            File(cacheDir, "request-bodies/owned").listFiles()?.forEach { file ->
                if (file.extension == "body") file.delete()
            }
        }

        fun claim(cacheDir: File, token: String, length: Long, generation: String): RequestBodyFile {
            require(UUID.fromString(token).toString() == token) { "Invalid request body token" }
            require(length in 1..MAX_REQUEST_BODY_BYTES) { "Request body exceeds 128 MiB" }
            val prepared = File(cacheDir, "request-bodies/prepared").canonicalFile
            val source = File(prepared, "$token.body").canonicalFile
            require(source.parentFile == prepared && source.isFile && source.length() == length) {
                "Prepared request body is missing or changed"
            }
            val owned = File(cacheDir, "request-bodies/owned").apply { mkdirs() }
            val target = File(owned, "$token.$generation.body")
            Files.move(source.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE)
            return RequestBodyFile(target)
        }
    }
}
