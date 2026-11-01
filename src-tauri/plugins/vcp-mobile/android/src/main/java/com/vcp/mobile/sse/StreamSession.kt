package com.vcp.mobile.sse

import kotlinx.coroutines.Job
import okhttp3.sse.EventSource
import org.json.JSONObject
import java.util.UUID

/** One generation owns its body, upstream and current consumer under this monitor. */
internal class StreamSession(
    val requestId: String,
    val generation: String = UUID.randomUUID().toString(),
    @Volatile var eventSource: EventSource? = null,
    // 完整正文用于接管和终态交接；按上述规模约定保留，不另设长度截断或转文件阈值。
    val fullContent: StringBuilder = StringBuilder(),
    @Volatile var isCompleted: Boolean = false,
    @Volatile var lastFinishReason: String? = null,
    @Volatile var terminalError: String? = null,
    var activeConnection: ClientConnection? = null,
    val contextJson: JSONObject? = null,
    var requestBody: RequestBodyFile? = null,
) {
    // A detached Rust consumer can observe this generation's lifetime without
    // taking its response socket. Closing this socket releases that observer.
    var activityObserver: ClientConnection? = null
    @Volatile var handoffWakeDeadlineMs: Long = 0
    var handoffJob: Job? = null
    // Automatic publication retries belong to this generation. Replacing
    // its consumer never replenishes the two-attempt budget.
    var publicationRetriesAttempted: Int = 0
    @Synchronized
    internal fun complete(reason: String, error: String? = null): Boolean {
        if (isCompleted) return false
        lastFinishReason = reason
        terminalError = error
        isCompleted = true
        closeActivityObserver()
        return true
    }

    @Synchronized
    fun closeActivityObserver() {
        activityObserver?.close()
        activityObserver = null
    }

    @Synchronized
    internal fun contentChunk(start: Int, limit: Int): String {
        val end = contentChunkEnd(fullContent, start, limit)
        return fullContent.substring(start, end)
    }

    @Synchronized
    internal fun replaceConnection(connection: ClientConnection): ClientConnection? {
        val previous = activeConnection
        activeConnection = connection
        return previous
    }

    @Synchronized
    internal fun detachConnection(connection: ClientConnection): Boolean {
        if (activeConnection !== connection) return false
        activeConnection = null
        return true
    }

    @Synchronized
    internal fun takeConnection(): ClientConnection? {
        val connection = activeConnection
        activeConnection = null
        return connection
    }

    @Synchronized
    internal fun takeEventSource(): EventSource? {
        val source = eventSource
        eventSource = null
        return source
    }
}
