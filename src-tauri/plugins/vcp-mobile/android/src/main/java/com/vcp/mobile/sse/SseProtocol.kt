package com.vcp.mobile.sse

import org.json.JSONObject
import java.io.InputStream
import java.util.UUID

internal const val CONTENT_FRAME_CHARS = 16 * 1024

internal fun contentChunkEnd(content: CharSequence, start: Int, limit: Int): Int {
    var end = minOf(start + CONTENT_FRAME_CHARS, limit)
    if (end < limit && end > start &&
        Character.isHighSurrogate(content[end - 1]) && Character.isLowSurrogate(content[end])) {
        end--
    }
    return end
}

internal fun sessionEvent(session: StreamSession, type: String): JSONObject = JSONObject().apply {
    put("requestId", session.requestId)
    put("generation", session.generation)
    put("eventType", type)
}

internal fun sendProtocolError(connection: ClientConnection, requestId: String, message: String, code: String = "protocol_error") {
    val errEvent = JSONObject().apply {
        put("requestId", requestId)
        put("eventType", "error")
        put("code", code)
        put("error", message)
    }
    connection.enqueueAndAwait(errEvent.toString())
}

internal fun isGenerationId(value: String): Boolean = try {
    UUID.fromString(value).toString() == value
} catch (_: IllegalArgumentException) {
    false
}

internal fun readLengthPrefixed(inputStream: InputStream): String? {
    val lengthBuffer = ByteArray(4)
    var bytesRead = 0
    while (bytesRead < 4) {
        val read = inputStream.read(lengthBuffer, bytesRead, 4 - bytesRead)
        if (read == -1) return null
        bytesRead += read
    }
    val length = java.nio.ByteBuffer.wrap(lengthBuffer).int
    if (length <= 0 || length > 10 * 1024 * 1024) { // Limit to 10MB to prevent OOM
        return null
    }
    val dataBuffer = ByteArray(length)
    var dataBytesRead = 0
    while (dataBytesRead < length) {
        val read = inputStream.read(dataBuffer, dataBytesRead, length - dataBytesRead)
        if (read == -1) return null
        dataBytesRead += read
    }
    return String(dataBuffer, Charsets.UTF_8)
}
