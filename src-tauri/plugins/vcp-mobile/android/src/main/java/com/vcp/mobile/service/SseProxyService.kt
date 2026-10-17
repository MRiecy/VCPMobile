package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.BufferedOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

internal fun contentChunkEnd(content: CharSequence, start: Int, limit: Int): Int {
    var end = minOf(start + SseProxyService.CONTENT_FRAME_CHARS, limit)
    if (end < limit && end > start &&
        Character.isHighSurrogate(content[end - 1]) && Character.isLowSurrogate(content[end])) {
        end--
    }
    return end
}

// Scan the original buffer without copying a long reasoning prefix. Only the
// visible notification preview is allocated, even when a tag spans TCP chunks.
internal fun notificationReplyPreview(content: CharSequence, limit: Int = 80): String {
    val preview = StringBuilder(limit + 1)
    var offset = 0
    var pendingSpace = false
    var visibleCharacters = 0
    while (offset < content.length) {
        val endTag = when {
            content.startsWith("<think>", offset, ignoreCase = true) -> "</think>"
            content.startsWith("[--- VCP元思考链:", offset, ignoreCase = true) -> "元思考链结束 ---]"
            else -> null
        }
        if (endTag != null) {
            val end = content.indexOf(endTag, offset, ignoreCase = true)
            if (end < 0) break
            offset = end + endTag.length
            continue
        }
        val codePoint = Character.codePointAt(content, offset)
        offset += Character.charCount(codePoint)
        if (Character.isWhitespace(codePoint)) {
            pendingSpace = preview.isNotEmpty()
            continue
        }
        if (visibleCharacters == limit) return preview.append("...").toString()
        if (pendingSpace) {
            preview.append(' ')
            visibleCharacters++
            pendingSpace = false
            if (visibleCharacters == limit) return preview.append("...").toString()
        }
        preview.appendCodePoint(codePoint)
        visibleCharacters++
    }
    return preview.toString()
}

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

/**
 * 隔离进程网络助手 (SseProxyService)
 * 
 * 【职责】：
 * 1. 运行在独立的 ":helper" 进程中，通过本地 TCP 套接字与主进程通信，彻底避免 Binder 限制与高频 IPC 开销。
 * 2. 下载正文保留在内存；终态未在 5 秒内确认提交时原子发布离线结果。
 * 3. WifiLock 覆盖网络接收，WakeLock 另外覆盖有界终态交接，闲置后退出。
 */
class SseProxyService : Service() {

    companion object {
        private const val TAG = "VcpSseProxy"
        const val CHANNEL_ID = "vcp_sse_proxy_helper"
        const val NOTIFICATION_ID_SERVICE = 0x53545202

        // 2026-10-17 用户决定：正文规模沿用 LLM 单次生成的物理上限与现有 8 路接收并发约束。
        // 不另设单条正文、累计字节或总驻留 Session 配额，也不引入超限截断与容量驱逐。
        internal const val MAX_ACTIVE_SESSIONS = 8
        internal const val CONTENT_FRAME_CHARS = 16 * 1024
        internal const val TERMINAL_ACK_TIMEOUT_MS = 5_000L
        private const val IDLE_SERVICE_GRACE_MS = 30 * 1000L

        @Volatile
        var isServiceRunning = false
    }

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
    ) {
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
            return true
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

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .connectTimeout(15, TimeUnit.SECONDS)
            .retryOnConnectionFailure(false)
            .build()
    }

    private val activeSessions = ConcurrentHashMap<String, StreamSession>()
    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())
    @Volatile private var idleStopJob: Job? = null
    
    private var serverSocket: ServerSocket? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var wakeLockDeadlineMs: Long = 0
    private var wifiLock: WifiManager.WifiLock? = null

    override fun onCreate() {
        super.onCreate()
        isServiceRunning = true
        createNotificationChannel()
        
        // 启动为前台服务，获得后台守护资格
        val serviceNotification = buildServiceNotification()
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    NOTIFICATION_ID_SERVICE,
                    serviceNotification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
                )
            } else {
                startForeground(NOTIFICATION_ID_SERVICE, serviceNotification)
            }
        } catch (e: Exception) {
            Log.e(TAG, "startForeground failed: ", e)
            isServiceRunning = false
            stopSelf()
            return
        }

        // 启动本地 TCP 服务端
        startTcpServer()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Discovery is published once with the listener. Rewriting an unchanged
        // port used to expose an empty truncate/write window to control calls.
        scheduleIdleStopIfNeeded()
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        // 本地 TCP 架构下，主进程不再绑定此服务，直接通过 TCP 连接通信
        return null
    }

    override fun onDestroy() {
        Log.i(TAG, "onDestroy: shutting down TCP server and all sessions.")
        val listener = synchronized(this) {
            isServiceRunning = false
            cancelIdleStop()
            unpublishPort()
            serverSocket.also { serverSocket = null }
        }
        try { listener?.close() } catch (ignored: Exception) {}
        
        val sessions = activeSessions.values.toList()
        for (session in sessions) {
            val detached = synchronized(session) {
                session.handoffJob?.cancel()
                session.handoffJob = null
                if (!session.isCompleted) activeSessions.remove(session.requestId, session)
                Pair(session.takeEventSource(), session.takeConnection())
            }
            detached.first?.cancel()
            detached.second?.close()
            cleanupSessionIfCompletedAndDisconnected(session)
        }
        activeSessions.clear()
        
        releaseLocks()
        serviceScope.cancel()
        super.onDestroy()
    }

    /**
     * 在 127.0.0.1 启动 TCP 监听，并将端口写入 sse_helper.port
     */
    private fun startTcpServer() {
        serviceScope.launch(Dispatchers.IO) {
            try {
                val server = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
                val port = server.localPort
                val published = synchronized(this@SseProxyService) {
                    if (!isServiceRunning) false else {
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
                if (isServiceRunning) {
                    Log.e(TAG, "TCP Server failed", e)
                    stopIfIdle()
                }
            }
        }
    }

    // Called under the lifecycle monitor, also used by listener shutdown.
    private fun publishPort(port: Int) {
        val target = File(applicationContext.cacheDir, "sse_helper.port")
        val temporary = File(applicationContext.cacheDir, "sse_helper.port.tmp")
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
            val target = File(applicationContext.cacheDir, "sse_helper.port")
            if (target.exists() && target.readText().trim() == port.toString()) target.delete()
        } catch (error: Exception) {
            Log.w(TAG, "Failed to remove helper discovery file", error)
        }
    }

    /**
     * 处理客户端 Socket 接入与 JSON 行命令解析
     */
    private fun handleClientSocket(socket: Socket) {
        serviceScope.launch(Dispatchers.IO) {
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
                        val body = request.optString("body", "")
                        val contextJson = request.optJSONObject("context")
                        boundSession = handleStartStream(requestId, url, headersJson, body, contextJson, connection)
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
                updateLocks()
            }
        }
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

    private fun handleStartStream(
        requestId: String,
        url: String,
        headersJson: String,
        body: String,
        contextJson: JSONObject?,
        connection: ClientConnection
    ): StreamSession? {
        val request = try {
            val builder = Request.Builder().url(url)
            val headers = JSONObject(headersJson)
            val keys = headers.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                builder.header(key, headers.getString(key))
            }
            if (body.isNotEmpty()) {
                builder.post(body.toRequestBody("application/json; charset=utf-8".toMediaType()))
            }
            builder.build()
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
                !isServiceRunning -> "service_stopping" to "Helper service is shutting down"
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
        synchronized(session) {
            if (!connection.enqueue(sessionEvent(session, "started").toString())) {
                session.detachConnection(connection)
            }
        }
        
        try {
            val listener = object : EventSourceListener() {
                override fun onOpen(eventSource: EventSource, response: Response) {
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Connected: id=$requestId")
                }
                
                override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                    if (!isCurrentSession(session)) return
                    acceptUpstreamEvent(session, data)
                }
                
                override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                    if (!isCurrentSession(session)) return
                    val errorMsg = t?.message ?: response?.message ?: "Unknown network error"
                    Log.w(TAG, "SSE Failed: id=$requestId, error=$errorMsg")
                    completeSession(session, "error", errorMsg)
                }
                
                override fun onClosed(eventSource: EventSource) {
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Closed: id=$requestId")
                    completeSession(session, session.lastFinishReason ?: "completed")
                }
            }
            
            val source = EventSources.createFactory(httpClient).newEventSource(request, listener)
            val installed = synchronized(session) {
                if (isServiceRunning && isCurrentSession(session) && !session.isCompleted) {
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

    private fun handleResumeStream(requestId: String, expectedGeneration: String?, connection: ClientConnection): StreamSession? {
        val session = activeSessions[requestId]
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
            if (!isServiceRunning || !isCurrentSession(session) ||
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
        activeSessions[session.requestId] === session

    private fun sessionEvent(session: StreamSession, type: String): JSONObject = JSONObject().apply {
        put("requestId", session.requestId)
        put("generation", session.generation)
        put("eventType", type)
    }

    private fun sendProtocolError(connection: ClientConnection, requestId: String, message: String, code: String = "protocol_error") {
        val errEvent = JSONObject().apply {
            put("requestId", requestId)
            put("eventType", "error")
            put("code", code)
            put("error", message)
        }
        connection.enqueueAndAwait(errEvent.toString())
    }

    private fun handleQueryStream(requestId: String, connection: ClientConnection) {
        val session = activeSessions[requestId]
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

    private fun handleControl(
        action: String,
        requestId: String,
        expectedGeneration: String,
        controlConnection: ClientConnection,
    ) {
        val session = activeSessions[requestId]
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
                Pair(session.takeEventSource(), session.takeConnection())
            }
            if (action == "cancel" && accepted) {
                completeSession(session, "cancelled_by_user")
            }
            detached?.first?.cancel()
            detached?.second?.close()
        }
        if (isGenerationId(expectedGeneration) && action != "cancel" &&
            (accepted || activeSessions[requestId] == null)) {
            val file = completedFile(requestId, expectedGeneration)
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

    private fun acceptUpstreamEvent(session: StreamSession, data: String) {
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
        val content = choice.optJSONObject("delta")?.opt("content") as? String ?: ""
        val reason = (choice.opt("finish_reason") as? String)?.takeIf { it.isNotEmpty() }
        if (content.isEmpty() && reason == null) return
        var failedConnection: ClientConnection? = null
        synchronized(session) {
            if (!isCurrentSession(session) || session.isCompleted) return
            session.fullContent.append(content)
            if (reason != null) session.lastFinishReason = reason
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

    private fun completeSession(session: StreamSession, reason: String, error: String? = null) {
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
            showStreamNotification(session, isSuccess = error == null, errorMsg = error)
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

    private fun cleanupSessionIfCompletedAndDisconnected(session: StreamSession) {
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
                (expectedJob != null && (!isServiceRunning || session.handoffJob !== expectedJob))) return
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
            val published = persistOfflineCompletedSession(session)
            if (published) {
                removeSession(session)
                connection = session.takeConnection()
            }
            session.handoffWakeDeadlineMs = 0
            if (!published) {
                session.handoffJob?.cancel()
                session.handoffJob = null
                if (isServiceRunning && session.publicationRetriesAttempted < 2) {
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
        if (!isServiceRunning || activeSessions.isNotEmpty() || idleStopJob?.isActive == true) return
        idleStopJob = serviceScope.launch {
            delay(IDLE_SERVICE_GRACE_MS)
            stopIfIdle()
        }
    }

    private fun stopIfIdle(taskRemovedOnly: Boolean = false) {
        val listener = synchronized(this) {
            if (!isServiceRunning || activeSessions.isNotEmpty() || (taskRemovedOnly && !isTaskRemoved)) return
            // Same monitor as admission: no upstream request can be accepted
            // after this transition, even before Android delivers onDestroy.
            isServiceRunning = false
            cancelIdleStop()
            unpublishPort()
            serverSocket.also { serverSocket = null }
        }
        try { listener?.close() } catch (ignored: Exception) {}
        Log.i(TAG, "All helper sessions released; stopping service.")
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun isGenerationId(value: String): Boolean = try {
        UUID.fromString(value).toString() == value
    } catch (_: IllegalArgumentException) {
        false
    }

    private fun completedFile(requestId: String, generation: String): File =
        File(applicationContext.cacheDir, "sse_offline_completed/${sha256(requestId)}.$generation.completed")

    // Called with the session monitor held after it has physically terminated.
    // On failure the session remains available for an explicit resume.
    private fun persistOfflineCompletedSession(session: StreamSession): Boolean {
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

    @Volatile private var isTaskRemoved = false

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
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
            if (hasRunning) {
                cancelIdleStop()
                acquireWakeLock(0)
                acquireWifiLock()
            } else {
                releaseWifiLock()
                if (handoffDeadline > SystemClock.elapsedRealtime()) {
                    acquireWakeLock(handoffDeadline)
                } else {
                    releaseWakeLock()
                }
                if (activeSessions.isEmpty()) scheduleIdleStopIfNeeded()
            }
        }
        checkSelfTermination()
    }

    private fun acquireWakeLock(deadlineMs: Long) {
        val appContext = applicationContext
        if (wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null) {
                wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VCP:SseProxyWakeLock")
                    .apply { setReferenceCounted(false) }
            }
        }
        wakeLock?.let {
            if (!it.isHeld || wakeLockDeadlineMs != deadlineMs) {
                if (it.isHeld) it.release()
                if (deadlineMs == 0L) it.acquire()
                else it.acquire(maxOf(1, deadlineMs - SystemClock.elapsedRealtime()))
                wakeLockDeadlineMs = deadlineMs
                Log.i(TAG, "SseProxy WakeLock ACQUIRED.")
            }
        }
    }

    private fun acquireWifiLock() {
        if (wifiLock == null) {
            val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                @Suppress("DEPRECATION")
                wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "VCP:SseProxyWifiLock")
                } else {
                    wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL, "VCP:SseProxyWifiLock")
                }
            }
        }
        wifiLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.i(TAG, "SseProxy WifiLock ACQUIRED.")
            }
        }
    }

    private fun releaseLocks() {
        releaseWakeLock()
        releaseWifiLock()
    }

    private fun releaseWakeLock() {
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.i(TAG, "SseProxy WakeLock RELEASED.")
            }
        }
        wakeLock = null
        wakeLockDeadlineMs = 0
    }

    private fun releaseWifiLock() {
        wifiLock?.let {
            if (it.isHeld) {
                it.release()
                Log.i(TAG, "SseProxy WifiLock RELEASED.")
            }
        }
        wifiLock = null
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val notificationManager = getSystemService(NotificationManager::class.java) ?: return
            
            val channelService = NotificationChannel(
                CHANNEL_ID,
                "VCP 后台连接助手",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "维持后台稳定的 AI 对话流式连接"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            notificationManager.createNotificationChannel(channelService)

            val channelAlerts = NotificationChannel(
                "vcp_agent_alerts",
                "智能体消息提醒",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "接收智能体回复完成或中断的通知"
                enableLights(true)
                lightColor = android.graphics.Color.BLUE
                enableVibration(true)
            }
            notificationManager.createNotificationChannel(channelAlerts)
        }
    }

    private fun buildServiceNotification(): Notification {
        // 点击通知：打开应用（通过反射获取主 Activity，避免跨包编译依赖）
        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(this, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
        val openPendingIntent = PendingIntent.getActivity(
            this, 0, openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("VCP 连接助手")
            .setContentText("正在后台托管 AI 对话流式连接...")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(
                applicationInfo.icon,
                "打开",
                openPendingIntent
            )

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }

    private fun sha256(input: String): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        val hash = digest.digest(input.toByteArray(Charsets.UTF_8))
        return hash.joinToString("") { "%02x".format(it) }
    }

    private fun readLengthPrefixed(inputStream: java.io.InputStream): String? {
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

    private fun isAppInForeground(): Boolean {
        val activityManager = getSystemService(Context.ACTIVITY_SERVICE) as? android.app.ActivityManager ?: return false
        val appProcesses = activityManager.runningAppProcesses ?: return false
        val packageName = packageName
        for (appProcess in appProcesses) {
            if (appProcess.importance == android.app.ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND 
                && appProcess.processName == packageName) {
                return true
            }
        }
        return false
    }

    private fun showStreamNotification(session: StreamSession, isSuccess: Boolean, errorMsg: String?) {
        // 只有当主应用在后台时，才进行通知栏提醒
        if (isAppInForeground()) {
            Log.d(TAG, "App is in foreground, skipping notification.")
            return
        }

        // 检查 session 是否已被主动取消或从 activeSessions 移除
        if (activeSessions[session.requestId] !== session) {
            Log.d(TAG, "Session is no longer active in SseProxyService, skipping notification.")
            return
        }

        // 忽略主动取消相关的错误提醒
        if (!isSuccess && errorMsg != null) {
            if (errorMsg.contains("cancel", ignoreCase = true) || 
                errorMsg.contains("close", ignoreCase = true)) {
                Log.d(TAG, "Ignoring notification for manual cancellation: $errorMsg")
                return
            }
        }

        val agentName = session.contextJson?.optString("agentName") ?: "智能体"
        val topicId = session.contextJson?.optString("topicId")
        val ownerId = session.contextJson?.optString("ownerId")
        val ownerType = session.contextJson?.optString("ownerType")
            ?.takeIf { it == "agent" || it == "group" }
        
        val title: String
        val contentText: String
        
        if (isSuccess) {
            title = "✨ $agentName 已回复"
            
            val preview = synchronized(session) {
                notificationReplyPreview(session.fullContent)
            }
            
            contentText = if (preview.isNotEmpty()) {
                preview
            } else {
                "回复内容已生成，点击进入应用查看。"
            }
        } else {
            title = "⚠️ 与 $agentName 的对话中断"
            contentText = errorMsg ?: "网络连接发生异常"
        }

        val notificationManager = getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        val channelId = "vcp_agent_alerts"

        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(this, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
                putExtra("requestId", session.requestId)
                if (topicId != null) putExtra("topicId", topicId)
                if (ownerId != null) putExtra("ownerId", ownerId)
                if (ownerType != null) putExtra("ownerType", ownerType)
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
        
        val pendingIntentFlags = PendingIntent.FLAG_UPDATE_CURRENT or 
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
            
        val openPendingIntent = PendingIntent.getActivity(
            this, session.requestId.hashCode(), openIntent, pendingIntentFlags
        )

        val notification = NotificationCompat.Builder(this, channelId)
            .setContentTitle(title)
            .setContentText(contentText)
            .setSmallIcon(applicationInfo.icon)
            .setAutoCancel(true)
            .setContentIntent(openPendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setDefaults(Notification.DEFAULT_ALL)
            .build()

        val notifId = session.requestId.hashCode()
        notificationManager.notify(notifId, notification)
    }
}
