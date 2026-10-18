package com.vcp.mobile.sse

import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** Disk format and atomic publication only; the coordinator owns terminal decisions. */
internal class CompletedMailbox(private val cacheDir: File) {
    private companion object { const val TAG = "VcpSseProxy" }

    fun completedFile(requestId: String, generation: String): File =
        File(cacheDir, "sse_offline_completed/${sha256(requestId)}.$generation.completed")

    // Called with the session monitor held after it has physically terminated.
    // On failure the session remains available for an explicit resume.
    fun publish(session: StreamSession): Boolean {
        val target = completedFile(session.requestId, session.generation)
        val temporary = File(target.parentFile, "${target.nameWithoutExtension}.tmp")
        return try {
            val directory = target.parentFile ?: throw java.io.IOException("Missing mailbox directory")
            if (!directory.isDirectory && !directory.mkdirs()) {
                throw java.io.IOException("Cannot create SSE mailbox directory")
            }
            val metadata = JSONObject().apply {
                put("schemaVersion", 1)
                put("requestId", session.requestId)
                put("generation", session.generation)
                put("completedAt", System.currentTimeMillis())
                put("context", session.contextJson)
                put("finishReason", session.lastFinishReason ?: "completed")
                put("error", session.terminalError)
            }
            FileOutputStream(temporary).use { stream ->
                val writer = stream.bufferedWriter(Charsets.UTF_8)
                writer.write(metadata.toString())
                writer.write("\n")
                var offset = 0
                val length = session.fullContent.length
                while (offset < length) {
                    val chunk = session.contentChunk(offset, length)
                    writer.write(chunk)
                    offset += chunk.length
                }
                writer.flush()
                stream.fd.sync()
            }
            Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            Log.i(TAG, "Published completed SSE reply for ${session.requestId}")
            true
        } catch (error: Exception) {
            Log.e(TAG, "Failed to publish completed SSE reply; retaining session ${session.requestId}", error)
            if (temporary.exists() && !temporary.delete()) {
                Log.w(TAG, "Failed to remove incomplete SSE temporary file: $temporary")
            }
            false
        }
    }

    private fun sha256(input: String): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        val hash = digest.digest(input.toByteArray(Charsets.UTF_8))
        return hash.joinToString("") { "%02x".format(it) }
    }
}
