package com.vcp.mobile.sse

import android.content.Context
import android.os.SystemClock
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * The sole session/admission owner. The lifecycle monitor never acquires a
 * session monitor; session operations may acquire the lifecycle monitor for
 * locks and idle shutdown. Consumer replacement and terminal handoff stay under
 * the same session monitor, including atomic mailbox publication.
 */
internal class SseSessionCoordinator(
    private val context: Context,
    private val stopService: () -> Unit,
    private val eventSourceFactory: EventSource.Factory? = null,
    private val clientDispatcher: CoroutineDispatcher = Dispatchers.IO,
) {
    private companion object {
        const val TAG = "VcpSseProxy"
        // Body size follows the existing generation/8-receiver constraint.
        // Completed bodies are never truncated or evicted by a memory quota.
        const val MAX_ACTIVE_SESSIONS = 8
        const val TERMINAL_ACK_TIMEOUT_MS = 5_000L
        const val IDLE_SERVICE_GRACE_MS = 30 * 1000L
    }

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .connectTimeout(15, TimeUnit.SECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }

    private val activeSessions = ConcurrentHashMap<String, StreamSession>()
    // Accepted sockets exist before a command can identify its session. They
    // remain service-owned through control ACK delivery and coroutine completion.
    private val clientSockets = ConcurrentHashMap.newKeySet<Socket>()
    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())
    @Volatile private var idleStopJob: Job? = null
    private var serverSocket: ServerSocket? = null
    private val mailbox = CompletedMailbox(context.cacheDir)
    private val notifications = SseNotifications(context)
    private val powerLocks = SsePowerLocks(context.applicationContext)

    @Volatile var isRunning = false
        private set

    val isIdle: Boolean get() = activeSessions.isEmpty()

    fun findSession(requestId: String): StreamSession? = activeSessions[requestId]

    /** Admission opens before foreground startup; listening starts only after it succeeds. */
    @Synchronized
    fun activate() {
        isRunning = true
    }

    fun onStart() {
        scheduleIdleStopIfNeeded()
    }

    fun destroy() {
        Log.i(TAG, "onDestroy: shutting down TCP server and all sessions.")
        val (listener, clients) = synchronized(this) {
            isRunning = false
            cancelIdleStop()
            unpublishPort()
            val listener = serverSocket.also { serverSocket = null }
            val clients = clientSockets.toList()
            clientSockets.clear()
            listener to clients
        }
        try { listener?.close() } catch (ignored: Exception) {}
        // Socket.close interrupts a pending command read. Coroutine cancellation
        // alone cannot interrupt blocking java.net I/O, or run an unstarted body.
        clients.forEach { socket -> try { socket.close() } catch (ignored: Exception) {} }

        val sessions = activeSessions.values.toList()
        for (session in sessions) {
            val detached = synchronized(session) {
                session.handoffJob?.cancel()
                session.handoffJob = null
                if (!session.isCompleted) activeSessions.remove(session.requestId, session)
                session.requestBody?.close()
                Pair(session.takeEventSource(), session.takeConnection())
            }
            detached.first?.cancel()
            detached.second?.close()
            cleanupSessionIfCompletedAndDisconnected(session)
        }
        activeSessions.clear()

        synchronized(this) { powerLocks.release() }
        serviceScope.cancel()
    }

    /**
     * 在 127.0.0.1 启动 TCP 监听，并将端口写入 sse_helper.port
     */
    fun startListening() {
        serviceScope.launch(Dispatchers.IO) {
            try {
                RequestBodyFile.cleanPreviousProcess(context.cacheDir)
                val server = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
                val port = server.localPort
                val published = synchronized(this@SseSessionCoordinator) {
                    if (!isRunning) false else {
                        serverSocket = server
                        publishPort(port)
                        true
                    }
                }
                if (!published) {
                    server.close()
                    return@launch
                }
                Log.i(TAG, "TCP Server listening on 127.0.0.1:$port")

                while (!server.isClosed) {
                    val socket = server.accept()
                    handleClientSocket(socket)
                }
            } catch (e: Exception) {
                if (isRunning) {
                    Log.e(TAG, "TCP Server failed", e)
                    stopIfIdle()
                }
            }
        }
    }

    // Called under the lifecycle monitor, also used by listener shutdown.
    private fun publishPort(port: Int) {
        val target = File(context.cacheDir, "sse_helper.port")
        val temporary = File(context.cacheDir, "sse_helper.port.tmp")
        try {
            temporary.writeText(port.toString())
            Files.move(temporary.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } finally {
            temporary.delete()
        }
    }

    private fun unpublishPort() {
        val port = serverSocket?.localPort ?: return
        try {
            val target = File(context.cacheDir, "sse_helper.port")
            if (target.exists() && target.readText().trim() == port.toString()) target.delete()
        } catch (error: Exception) {
            Log.w(TAG, "Failed to remove helper discovery file", error)
        }
    }

    /**
     * 处理客户端 Socket 接入与 JSON 行命令解析
     */
    private fun handleClientSocket(socket: Socket) {
        val admitted = synchronized(this) {
            if (!isRunning) false else {
                cancelIdleStop()
                clientSockets.add(socket)
            }
        }
        if (!admitted) {
            try { socket.close() } catch (ignored: Exception) {}
            return
        }
        val clientJob = serviceScope.launch(clientDispatcher, start = CoroutineStart.LAZY) {
            var boundRequestId: String? = null
            var boundSession: StreamSession? = null
            var boundConnection: ClientConnection? = null
            try {
                socket.soTimeout = 5_000
                val inputStream = socket.getInputStream()
                val outputStream = socket.getOutputStream()
                val connection = ClientConnection(socket, outputStream)
                boundConnection = connection

                val commandJson = readLengthPrefixed(inputStream) ?: return@launch
                val request = JSONObject(commandJson)
                val action = request.getString("action")
                val requestId = request.getString("requestId")
                boundRequestId = requestId

                Log.i(TAG, "TCP Command received: action=$action, requestId=$requestId")

                when (action) {
                    "start" -> {
                        val url = request.getString("url")
                        val headersJson = request.optString("headers", "{}")
                        val bodyToken = request.getString("bodyToken")
                        val bodyLength = request.getLong("bodyLength")
                        val contextJson = request.optJSONObject("context")
                        boundSession = handleStartStream(requestId, url, headersJson, bodyToken, bodyLength, contextJson, connection)
                        if (boundSession != null) {
                            socket.soTimeout = 0
                            readSocketUntilClose(inputStream)
                        }
                    }
                    "resume" -> {
                        val expectedGeneration = request.optString("generation").takeIf { it.isNotEmpty() }
                        boundSession = handleResumeStream(requestId, expectedGeneration, connection)
                        if (boundSession != null) {
                            socket.soTimeout = 0
                            readSocketUntilClose(inputStream)
                        }
                    }
                    "query" -> {
                        handleQueryStream(requestId, connection)
                    }
                    "observe_activity" -> {
                        val session = findSession(requestId) ?: return@launch
                        val generation = request.optString("generation").takeIf { it.isNotEmpty() }
                        synchronized(session) {
                            if (!isRunning || !isCurrentSession(session) || session.isCompleted ||
                                (generation != null && generation != session.generation)) return@launch
                            if (!connection.enqueue(sessionEvent(session, "observing").toString())) return@launch
                            session.closeActivityObserver()
                            session.activityObserver = connection
                        }
                        try {
                            socket.soTimeout = 0
                            readSocketUntilClose(inputStream)
                        } finally {
                            synchronized(session) {
                                if (session.activityObserver === connection) session.activityObserver = null
                            }
                        }
                    }
                    "cancel", "ack_committed", "discard" -> {
                        handleControl(action, requestId, request.optString("generation"), connection)
                    }
                    else -> throw IllegalArgumentException("Unknown helper action: $action")
                }
            } catch (e: Exception) {
                Log.e(TAG, "Error handling client socket for $boundRequestId", e)
            } finally {
                val connection = boundConnection
                val session = boundSession
                val detachedCurrent = if (connection != null && session != null) {
                    session.detachConnection(connection)
                } else {
                    false
                }
                connection?.close() ?: try { socket.close() } catch (ignored: Exception) {}
                if (detachedCurrent && session != null) {
                    Log.i(TAG, "Client socket disconnected for requestId=${session.requestId}")
                    cleanupSessionIfCompletedAndDisconnected(session)
                }
            }
        }
        // Install cleanup before dispatch so cancellation before the body starts
        // cannot strand an accepted socket. Remove only this socket's registration.
        clientJob.invokeOnCompletion {
            clientSockets.remove(socket)
            try { socket.close() } catch (ignored: Exception) {}
            updateLocks()
        }
        clientJob.start()
    }

    private fun readSocketUntilClose(inputStream: java.io.InputStream) {
        try {
            val buf = ByteArray(1024)
            while (inputStream.read(buf) != -1) {
                // 仅维持连接读取，阻塞直到客户端断开连接
            }
        } catch (ignored: Exception) {
        }
    }

    fun handleStartStream(
        requestId: String,
        url: String,
        headersJson: String,
        bodyToken: String,
        bodyLength: Long,
        contextJson: JSONObject?,
        connection: ClientConnection
    ): StreamSession? {
        val builder = try {
            val builder = Request.Builder().url(url)
            val headers = JSONObject(headersJson)
            val keys = headers.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                builder.header(key, headers.getString(key))
            }
            builder
        } catch (error: Exception) {
            sendProtocolError(connection, requestId, "Invalid helper request: ${error.message}", "invalid_request")
            return null
        }
        val session = StreamSession(
            requestId,
            activeConnection = connection,
            contextJson = contextJson,
        )
        val rejection = synchronized(this) {
            when {
                !isRunning -> "service_stopping" to "Helper service is shutting down"
                activeSessions.containsKey(requestId) -> "session_already_exists" to "Session already exists"
                // 限制正在接收的流；完成态由 ACK/离线交接回收，落盘失败按交接生命周期处理。
                activeSessions.values.count { !it.isCompleted } >= MAX_ACTIVE_SESSIONS ->
                    "capacity_exhausted" to "Helper session limit reached"
                else -> {
                    activeSessions[requestId] = session
                    isTaskRemoved = false
                    cancelIdleStop()
                    null
                }
            }
        }
        if (rejection != null) {
            sendProtocolError(connection, requestId, rejection.second, rejection.first)
            return null
        }
        val request = try {
            synchronized(session) {
                check(isRunning && isCurrentSession(session) && !session.isCompleted) { "Helper service is shutting down" }
                val body = RequestBodyFile.claim(context.cacheDir, bodyToken, bodyLength, session.generation)
                session.requestBody = body
                builder.post(body).build()
            }
        } catch (error: Exception) {
            synchronized(session) {
                session.requestBody?.close()
                removeSession(session)
            }
            sendProtocolError(connection, requestId, "Invalid request body: ${error.message}", "invalid_request")
            return null
        }
        synchronized(session) {
            if (!connection.enqueue(sessionEvent(session, "started").toString())) {
                session.detachConnection(connection)
            }
        }

        try {
            val listener = object : EventSourceListener() {
                override fun onOpen(eventSource: EventSource, response: Response) {
                    // Final response headers mean redirects/authentication have
                    // settled and OkHttp will no longer upload this body.
                    session.requestBody?.close()
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Connected: id=$requestId")
                }

                override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                    if (!isCurrentSession(session)) return
                    acceptUpstreamEvent(session, data)
                }

                override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                    session.requestBody?.close()
                    if (!isCurrentSession(session)) return
                    val detail = t?.message?.takeIf { it.isNotBlank() }
                        ?: response?.message?.takeIf { it.isNotBlank() }
                    // HTTP/2 has no reason phrase; its status remains useful even
                    // when OkHttp reports neither an exception nor a message.
                    val errorMsg = when {
                        response != null && detail != null -> "HTTP ${response.code}: $detail"
                        response != null -> "HTTP ${response.code}"
                        else -> detail ?: "Unknown network error"
                    }
                    Log.w(TAG, "SSE Failed: id=$requestId, error=$errorMsg")
                    completeSession(session, "error", errorMsg)
                }

                override fun onClosed(eventSource: EventSource) {
                    session.requestBody?.close()
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Closed: id=$requestId")
                    completeSession(session, session.lastFinishReason ?: "completed")
                }
            }

            val source = (eventSourceFactory ?: EventSources.createFactory(httpClient)).newEventSource(request, listener)
            val installed = synchronized(session) {
                if (isRunning && isCurrentSession(session) && !session.isCompleted) {
                    session.eventSource = source
                    true
                } else {
                    false
                }
            }
            if (!installed) {
                // stop/onDestroy may win after putIfAbsent but before OkHttp returns.
                // Never leave an EventSource owned by a session that is no longer mapped.
                source.cancel()
                updateLocks()
                return session.takeIf { isCurrentSession(it) }
            }
            updateLocks()
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start stream source for $requestId", e)
            // Once started was accepted the protocol must end with this
            // generation's terminal result, including local startup failures.
            completeSession(session, "error", "Failed to start stream source: ${e.message}")
        }
        return session
    }

    fun handleResumeStream(requestId: String, expectedGeneration: String?, connection: ClientConnection): StreamSession? {
        val session = findSession(requestId)
        if (session == null) {
            sendProtocolError(connection, requestId, "Session not found", "session_not_found")
            return null
        }
        if (expectedGeneration != null && expectedGeneration != session.generation) {
            sendProtocolError(connection, requestId, "Helper generation changed", "generation_mismatch")
            return null
        }
        var previousConnection: ClientConnection? = null
        val accepted = synchronized(session) {
            if (!isRunning || !isCurrentSession(session) ||
                (expectedGeneration != null && expectedGeneration != session.generation)) {
                false
            } else {
                synchronized(this) {
                    isTaskRemoved = false
                    cancelIdleStop()
                }
                previousConnection = session.replaceConnection(connection)
                val length = session.fullContent.length
                val completed = session.isCompleted
                val finishReason = session.lastFinishReason
                val error = session.terminalError
                // Enqueue the prefix and replace the consumer under the same
                // lock used by live appends. Later data always follows it.
                val queued = connection.enqueueFrames { emit ->
                    emit(sessionEvent(session, "snapshot_start").toString())
                    var offset = 0
                    while (offset < length) {
                        val chunk = session.contentChunk(offset, length)
                        emit(sessionEvent(session, "snapshot_chunk").put("content", chunk).toString())
                        offset += chunk.length
                    }
                    emit(sessionEvent(session, "snapshot_end").apply {
                        put("completed", completed)
                        put("finishReason", finishReason)
                        put("error", error)
                    }.toString())
                }
                if (queued && completed) scheduleCompletedHandoff(session)
                queued
            }
        }
        previousConnection?.close()
        if (!accepted) {
            if (session.detachConnection(connection)) {
                cleanupSessionIfCompletedAndDisconnected(session)
            }
            sendProtocolError(connection, requestId, "Session no longer available", "session_not_found")
            return null
        }
        updateLocks()
        return session
    }

    private fun isCurrentSession(session: StreamSession): Boolean =
        findSession(session.requestId) === session

    fun handleQueryStream(requestId: String, connection: ClientConnection) {
        val session = findSession(requestId)
        val resp = JSONObject()
        resp.put("requestId", requestId)

        if (session == null) {
            resp.put("status", "not_found")
        } else {
            synchronized(session) {
                resp.put("generation", session.generation)
                resp.put("status", if (session.isCompleted) "completed" else "streaming")
                resp.put("finishReason", session.lastFinishReason)
            }
        }

        if (!connection.enqueueAndAwait(resp.toString())) {
            Log.e(TAG, "Failed to write query response")
        }
    }

    fun handleControl(
        action: String,
        requestId: String,
        expectedGeneration: String,
        controlConnection: ClientConnection,
    ) {
        val session = findSession(requestId)
        var accepted = false
        if (isGenerationId(expectedGeneration) && session != null) {
            val detached = synchronized(session) {
                if (session.generation != expectedGeneration || !isCurrentSession(session)) {
                    return@synchronized null
                }
                if (action == "cancel") {
                    accepted = true
                    return@synchronized null
                }
                if (action == "ack_committed" && !session.isCompleted) return@synchronized null
                if (!removeSession(session)) return@synchronized null
                accepted = true
                session.requestBody?.close()
                Pair(session.takeEventSource(), session.takeConnection())
            }
            if (action == "cancel" && accepted) {
                completeSession(session, "cancelled_by_user")
            }
            detached?.first?.cancel()
            detached?.second?.close()
        }
        if (isGenerationId(expectedGeneration) && action != "cancel" &&
            (accepted || findSession(requestId) == null)) {
            val file = mailbox.completedFile(requestId, expectedGeneration)
            accepted = !file.exists() || file.delete()
            if (!accepted) Log.e(TAG, "Failed to delete completed SSE file: $file")
        }
        updateLocks()
        val ack = JSONObject().apply {
            put("action", "${action}_ack")
            put("requestId", requestId)
            put("generation", expectedGeneration)
            put("accepted", accepted)
        }
        if (!controlConnection.enqueueAndAwait(ack.toString())) {
            Log.w(TAG, "Failed to deliver $action ACK for requestId=$requestId")
        }
    }

    fun acceptUpstreamEvent(session: StreamSession, data: String) {
        if (data.isBlank()) return
        if (data.trim() == "[DONE]") {
            completeSession(session, session.lastFinishReason ?: "completed")
            return
        }
        val payload = try {
            JSONObject(data)
        } catch (error: Exception) {
            completeSession(session, "error", "Invalid upstream SSE JSON: ${error.message}")
            return
        }
        if (payload.has("error") && !payload.isNull("error")) {
            val error = payload.get("error")
            val message = (error as? JSONObject)?.opt("message") as? String ?: error.toString()
            completeSession(session, "error", message)
            return
        }
        val choice = payload.optJSONArray("choices")?.optJSONObject(0) ?: return
        val delta = choice.optJSONObject("delta")
        val reason = (choice.opt("finish_reason") as? String)?.takeIf { it.isNotEmpty() }
        if (reason == null && (delta == null || delta.length() == 0)) return
        val content = delta?.opt("content") as? String ?: ""
        var failedConnection: ClientConnection? = null
        synchronized(session) {
            if (!isCurrentSession(session) || session.isCompleted) return
            // A nonempty delta without a reason starts or continues an unfinished round.
            session.lastFinishReason = reason
            if (content.isEmpty() && reason == null) return
            session.fullContent.append(content)
            session.activeConnection?.let { connection ->
                if (!connection.enqueueFrames { emit ->
                    var offset = 0
                    do {
                        val end = contentChunkEnd(content, offset, content.length)
                        emit(sessionEvent(session, "data").apply {
                            put("content", content.substring(offset, end))
                            if (end == content.length) put("finishReason", reason)
                        }.toString())
                        offset = end
                    } while (offset < content.length)
                }) {
                    Log.w(TAG, "Client writer queue overflow. id=${session.requestId}")
                    if (session.detachConnection(connection)) {
                        failedConnection = connection
                    }
                }
            }
        }
        failedConnection?.close()
    }

    fun completeSession(session: StreamSession, reason: String, error: String? = null) {
        var failedConnection: ClientConnection? = null
        var finishedSource: EventSource? = null
        val completed = synchronized(session) {
            if (!isCurrentSession(session) || !session.complete(reason, error)) {
                false
            } else {
                // Completion and installation use the same monitor. If the
                // factory returns later, its completed check cancels that source
                // instead of restoring a network owner to this terminal session.
                finishedSource = session.takeEventSource()
                session.requestBody?.close()
                session.handoffWakeDeadlineMs = SystemClock.elapsedRealtime() + TERMINAL_ACK_TIMEOUT_MS
                session.activeConnection?.let { connection ->
                    val event = sessionEvent(session, "terminal").apply {
                        put("finishReason", session.lastFinishReason)
                        put("error", session.terminalError)
                    }
                    if (!connection.enqueue(event.toString()) && session.detachConnection(connection)) {
                        failedConnection = connection
                    }
                }
                scheduleCompletedHandoff(session)
                true
            }
        }
        if (!completed) return
        updateLocks()
        finishedSource?.cancel()
        failedConnection?.close()
        if (reason != "cancelled_by_user") {
            notifications.showStreamNotification(session, isSuccess = error == null, errorMsg = error) {
                isCurrentSession(session)
            }
        }
        cleanupSessionIfCompletedAndDisconnected(session)
        updateLocks()
    }

    private fun scheduleCompletedHandoff(session: StreamSession) {
        synchronized(session) {
            if (!isCurrentSession(session) || !session.isCompleted || session.activeConnection == null) return
            session.handoffWakeDeadlineMs = SystemClock.elapsedRealtime() + TERMINAL_ACK_TIMEOUT_MS
            scheduleHandoff(session, TERMINAL_ACK_TIMEOUT_MS, automaticRetry = false)
        }
    }

    // The session monitor owns ACK waiting, publication and retry waiting. A
    // resumed consumer replaces the same job; there is never a second retry loop.
    private fun scheduleHandoff(session: StreamSession, delayMs: Long, automaticRetry: Boolean) {
        session.handoffJob?.cancel()
        val job = serviceScope.launch(Dispatchers.IO, start = CoroutineStart.LAZY) {
            delay(delayMs)
            publishCompletedSession(session, requireDisconnected = false,
                expectedJob = coroutineContext[Job], automaticRetry = automaticRetry)
        }
        session.handoffJob = job
        job.start()
    }

    fun cleanupSessionIfCompletedAndDisconnected(session: StreamSession) {
        publishCompletedSession(session, requireDisconnected = true)
    }

    private fun publishCompletedSession(
        session: StreamSession,
        requireDisconnected: Boolean,
        expectedJob: Job? = null,
        automaticRetry: Boolean = false,
    ) {
        var connection: ClientConnection? = null
        synchronized(session) {
            if (!isCurrentSession(session) || !session.isCompleted ||
                (requireDisconnected && session.activeConnection != null) ||
                (expectedJob != null && (!isRunning || session.handoffJob !== expectedJob))) return
            // A real disconnect/destruction keeps its immediate best-effort
            // write. It supersedes pending work but never resets the retry budget.
            if (expectedJob == null) session.handoffJob?.cancel()
            if (automaticRetry) session.publicationRetriesAttempted++
            // Disk I/O gets its own bounded CPU handoff window. A failed or
            // stalled fsync must never keep a completed reply's wake lock forever.
            session.handoffWakeDeadlineMs = SystemClock.elapsedRealtime() + TERMINAL_ACK_TIMEOUT_MS
            updateLocks()
            // ACK, discard and resume all take this monitor. Publication precedes
            // removal, so not_found always permits Rust to recheck the mailbox.
            val published = mailbox.publish(session)
            if (published) {
                removeSession(session)
                connection = session.takeConnection()
            }
            session.handoffWakeDeadlineMs = 0
            if (!published) {
                session.handoffJob?.cancel()
                session.handoffJob = null
                if (isRunning && session.publicationRetriesAttempted < 2) {
                    // Delay starts after the previous write returned failure:
                    // 2 seconds before retry 1, then 4 seconds before retry 2.
                    // Backoff holds no handoff WakeLock and emits no notification.
                    val delayMs = 2_000L * (session.publicationRetriesAttempted + 1)
                    scheduleHandoff(session, delayMs, automaticRetry = true)
                }
            }
        }
        connection?.close()
        updateLocks()
    }

    private fun removeSession(session: StreamSession): Boolean {
        val removed = activeSessions.remove(session.requestId, session)
        if (removed) {
            session.closeActivityObserver()
            session.handoffJob?.cancel()
            session.handoffJob = null
            session.handoffWakeDeadlineMs = 0
            scheduleIdleStopIfNeeded()
        }
        return removed
    }

    @Synchronized
    private fun cancelIdleStop() {
        idleStopJob?.cancel()
        idleStopJob = null
    }

    @Synchronized
    private fun scheduleIdleStopIfNeeded() {
        if (!isRunning || !isIdle || clientSockets.isNotEmpty() || idleStopJob?.isActive == true) return
        idleStopJob = serviceScope.launch {
            delay(IDLE_SERVICE_GRACE_MS)
            stopIfIdle()
        }
    }

    fun stopIfIdle(taskRemovedOnly: Boolean = false) {
        val listener = synchronized(this) {
            if (!isRunning || !isIdle || clientSockets.isNotEmpty() ||
                (taskRemovedOnly && !isTaskRemoved)) return
            // The last ACK/discard removes its session before writing its reply.
            // Let admitted commands finish; their completion rechecks idle state.
            // Same monitor as admission: no upstream request can be accepted
            // after this transition, even before Android delivers onDestroy.
            isRunning = false
            cancelIdleStop()
            unpublishPort()
            serverSocket.also { serverSocket = null }
        }
        try { listener?.close() } catch (ignored: Exception) {}
        Log.i(TAG, "All helper sessions released; stopping service.")
        stopService()
    }

    @Volatile var isTaskRemoved = false
        private set

    fun onTaskRemoved() {
        Log.i(TAG, "onTaskRemoved: Main task removed by user.")
        isTaskRemoved = true
        checkSelfTermination()
    }

    private fun checkSelfTermination() {
        if (isTaskRemoved) stopIfIdle(taskRemovedOnly = true)
    }

    private fun updateLocks() {
        synchronized(this) {
            val hasRunning = activeSessions.values.any { !it.isCompleted }
            val handoffDeadline = activeSessions.values.maxOfOrNull { it.handoffWakeDeadlineMs } ?: 0
            if (hasRunning) cancelIdleStop()
            powerLocks.update(hasRunning, handoffDeadline)
            if (!hasRunning && isIdle) scheduleIdleStopIfNeeded()
        }
        checkSelfTermination()
    }
}
