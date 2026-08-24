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
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
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
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal class ClientConnection(
    val socket: Socket,
    val outputStream: java.io.OutputStream,
) {
    companion object {
        private const val WRITE_TIMEOUT_SECONDS = 5L
        private const val MAX_PENDING_FRAMES = 128
        private val writeWatchdog: ScheduledExecutorService =
            Executors.newSingleThreadScheduledExecutor { runnable ->
                Thread(runnable, "vcp-sse-write-watchdog").apply { isDaemon = true }
            }
    }

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

    fun enqueueBatch(frames: List<String>): Boolean {
        if (closed.get()) return false
        return try {
            writer.execute {
                try {
                    for (json in frames) {
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
        try { outputStream.close() } catch (ignored: Exception) {}
        try { socket.close() } catch (ignored: Exception) {}
    }
}

/**
 * 隔离进程网络助手 (SseProxyService)
 * 
 * 【职责】：
 * 1. 运行在独立的 ":helper" 进程中，通过本地 TCP 套接字与主进程通信，彻底避免 Binder 限制与高频 IPC 开销。
 * 2. 采用全内存设计，主进程死亡时后台下载流直接缓存在内存中，避免磁盘 I/O。
 * 3. 动态锁控：只在流下载时持有 WakeLock/WifiLock，下载完成后即刻释放，闲置时自动退出服务。
 */
class SseProxyService : Service() {

    companion object {
        private const val TAG = "VcpSseProxy"
        const val CHANNEL_ID = "vcp_sse_proxy_helper"
        const val NOTIFICATION_ID_SERVICE = 0x53545202

        internal const val MAX_ACTIVE_SESSIONS = 8
        internal const val MAX_SESSION_EVENTS = 20_000
        internal const val MAX_SESSION_BUFFER_BYTES = 8L * 1024L * 1024L
        internal const val MAX_GLOBAL_BUFFER_BYTES = 24L * 1024L * 1024L
        private const val COMPLETED_SESSION_GRACE_MS = 5 * 60 * 1000L
        private const val IDLE_SERVICE_GRACE_MS = 30 * 1000L

        @Volatile
        var isServiceRunning = false
    }

    internal class StreamSession(
        val requestId: String,
        val generation: Long = 0,
        @Volatile var eventSource: EventSource? = null,
        val eventBuffer: MutableList<JSONObject> = mutableListOf(),
        @Volatile var isCompleted: Boolean = false,
        @Volatile var lastFinishReason: String? = null,
        var activeConnection: ClientConnection? = null,
        val contextJson: JSONObject? = null,
        var bufferedBytes: Long = 0,
        val cleanupScheduled: AtomicBoolean = AtomicBoolean(false),
        val budgetReleased: AtomicBoolean = AtomicBoolean(false),
        val terminalOverflow: AtomicBoolean = AtomicBoolean(false),
    ) {
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
    }

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .connectTimeout(15, TimeUnit.SECONDS)
            .build()
    }

    private val activeSessions = ConcurrentHashMap<String, StreamSession>()
    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())
    private val sessionGeneration = AtomicLong(0)
    private val globalBufferedBytes = AtomicLong(0)
    @Volatile private var idleStopJob: Job? = null
    
    private var serverSocket: ServerSocket? = null
    private var wakeLock: PowerManager.WakeLock? = null
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
        serverSocket?.let { server ->
            serviceScope.launch(Dispatchers.IO) {
                try {
                    val portFile = File(applicationContext.cacheDir, "sse_helper.port")
                    portFile.writeText(server.localPort.toString())
                    Log.i(TAG, "onStartCommand: Rewrote port file with ${server.localPort}")
                } catch (e: Exception) {
                    Log.e(TAG, "onStartCommand: Failed to write port file", e)
                }
            }
        }
        scheduleIdleStopIfNeeded()
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        // 本地 TCP 架构下，主进程不再绑定此服务，直接通过 TCP 连接通信
        return null
    }

    override fun onDestroy() {
        Log.i(TAG, "onDestroy: shutting down TCP server and all sessions.")
        isServiceRunning = false
        try {
            val portFile = File(applicationContext.cacheDir, "sse_helper.port")
            if (portFile.exists()) {
                portFile.delete()
            }
        } catch (ignored: Exception) {}
        
        try {
            serverSocket?.close()
        } catch (ignored: Exception) {}
        
        val sessions = activeSessions.values.toList()
        activeSessions.clear()
        for (session in sessions) {
            if (session.isCompleted) dumpSessionToFile(session)
            val detached = synchronized(session) {
                Pair(session.eventSource, session.takeConnection())
            }
            detached.first?.cancel()
            detached.second?.close()
            releaseSessionBudget(session)
        }
        
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
                serverSocket = server
                val port = server.localPort
                Log.i(TAG, "TCP Server listening on 127.0.0.1:$port")
                
                val portFile = File(applicationContext.cacheDir, "sse_helper.port")
                portFile.writeText(port.toString())
                
                while (!server.isClosed) {
                    val socket = server.accept()
                    handleClientSocket(socket)
                }
            } catch (e: Exception) {
                Log.e(TAG, "TCP Server error or closed: ", e)
            }
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
                        val startIndex = request.optInt("startIndex", 0)
                        boundSession = handleResumeStream(requestId, startIndex, connection)
                        if (boundSession != null) {
                            socket.soTimeout = 0
                            readSocketUntilClose(inputStream)
                        }
                    }
                    "query" -> {
                        handleQueryStream(requestId, connection)
                    }
                    "stop" -> {
                        val expectedGeneration = request.optLong("generation", -1L)
                        handleStopStream(requestId, expectedGeneration, connection)
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
        if (!isServiceRunning) {
            sendProtocolError(connection, requestId, "Helper service is shutting down")
            return null
        }
        val session = StreamSession(
            requestId,
            generation = sessionGeneration.incrementAndGet(),
            activeConnection = connection,
            contextJson = contextJson,
        )
        val existing = synchronized(activeSessions) {
            if (activeSessions.size >= MAX_ACTIVE_SESSIONS) {
                session
            } else {
                activeSessions.putIfAbsent(requestId, session)
            }
        }
        if (existing != null) {
            val message = if (existing === session) "Helper session limit reached" else "Session already exists"
            Log.w(TAG, "start rejected for id=$requestId: $message")
            sendProtocolError(connection, requestId, message)
            return null
        }
        cancelIdleStop()
        if (!isServiceRunning) {
            removeSession(session)
            sendProtocolError(connection, requestId, "Helper service is shutting down")
            return null
        }
        
        try {
            val requestBuilder = Request.Builder().url(url)
            try {
                val headersObj = JSONObject(headersJson)
                val keys = headersObj.keys()
                while (keys.hasNext()) {
                    val key = keys.next()
                    requestBuilder.header(key, headersObj.getString(key))
                }
            } catch (e: Exception) {
                Log.e(TAG, "Failed to parse headers", e)
            }
            
            if (body.isNotEmpty()) {
                val mediaType = "application/json; charset=utf-8".toMediaType()
                requestBuilder.post(body.toRequestBody(mediaType))
            }
            
            val listener = object : EventSourceListener() {
                override fun onOpen(eventSource: EventSource, response: Response) {
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Connected: id=$requestId")
                    sendEventToSession(session, "open", "")
                }
                
                override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                    if (!isCurrentSession(session)) return
                    sendEventToSession(session, "message", data)
                }
                
                override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                    if (!isCurrentSession(session)) return
                    val errorMsg = t?.message ?: response?.message ?: "Unknown network error"
                    Log.w(TAG, "SSE Failed: id=$requestId, error=$errorMsg")
                    val errObj = JSONObject().apply {
                        put("error", errorMsg)
                        put("status", response?.code ?: 0)
                    }
                    session.isCompleted = true
                    session.lastFinishReason = "error"
                    sendEventToSession(session, "error", errObj.toString())
                    showStreamNotification(session, isSuccess = false, errorMsg = errorMsg)
                    cleanupSessionIfCompletedAndDisconnected(session)
                }
                
                override fun onClosed(eventSource: EventSource) {
                    if (!isCurrentSession(session)) return
                    Log.i(TAG, "SSE Closed: id=$requestId")
                    session.isCompleted = true
                    session.lastFinishReason = "completed"
                    sendEventToSession(session, "closed", "")
                    showStreamNotification(session, isSuccess = true, errorMsg = null)
                    cleanupSessionIfCompletedAndDisconnected(session)
                }
            }
            
            val source = EventSources.createFactory(httpClient).newEventSource(requestBuilder.build(), listener)
            val installed = synchronized(session) {
                if (isServiceRunning && isCurrentSession(session)) {
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
                return null
            }
            updateLocks()
        } catch (e: Exception) {
            removeSession(session)
            session.eventSource?.cancel()
            Log.e(TAG, "Failed to start stream source for $requestId", e)
            updateLocks()
            throw e
        }
        return session
    }

    private fun handleResumeStream(requestId: String, startIndex: Int, connection: ClientConnection): StreamSession? {
        val session = activeSessions[requestId]
        if (session == null) {
            Log.w(TAG, "resume: Session not found for id=$requestId")
            sendProtocolError(connection, requestId, "Session not found")
            return null
        }
        
        Log.i(TAG, "Resuming session id=$requestId, playing back events from $startIndex.")
        
        var previousConnection: ClientConnection? = null
        try {
            var unavailable = false
            synchronized(session) {
                if (!isServiceRunning || !isCurrentSession(session)) {
                    unavailable = true
                } else {
                    previousConnection = session.replaceConnection(connection)
                    val bufferSize = session.eventBuffer.size
                    val safeStartIndex = startIndex.coerceIn(0, bufferSize)
                    val replay = (safeStartIndex until bufferSize).map { session.eventBuffer[it].toString() }
                    if (!connection.enqueueBatch(replay)) {
                        throw IllegalStateException("Resume writer queue overflow")
                    }
                }
            }
            if (unavailable) {
                sendProtocolError(connection, requestId, "Session no longer available")
                return null
            }
        } catch (e: Exception) {
            if (session.detachConnection(connection)) {
                cleanupSessionIfCompletedAndDisconnected(session)
            }
            Log.e(TAG, "Failed playing back events to socket", e)
            throw e
        } finally {
            previousConnection?.close()
        }
        updateLocks()
        return session
    }

    private fun isCurrentSession(session: StreamSession): Boolean =
        activeSessions[session.requestId] === session

    private fun sendProtocolError(connection: ClientConnection, requestId: String, message: String) {
        val errEvent = JSONObject().apply {
            put("requestId", requestId)
            put("eventType", "error")
            put("eventData", JSONObject().apply { put("error", message) }.toString())
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
            val events = synchronized(session) {
                resp.put("generation", session.generation)
                resp.put("status", if (session.isCompleted) "completed" else "streaming")
                resp.put("lastFinishReason", session.lastFinishReason ?: "")
                resp.put("lastEventIndex", session.eventBuffer.size - 1)
                session.eventBuffer.toList()
            }
            // 从内存快照拼接完整文本，不在 session 锁内执行 JSON 解析。
            val fullText = StringBuilder()
            for (event in events) {
                if (event.getString("eventType") == "message") {
                    val eventData = event.getString("eventData")
                    if (eventData != "[DONE]") {
                        try {
                            val dataVal = JSONObject(eventData)
                            val choices = dataVal.optJSONArray("choices")
                            if (choices != null && choices.length() > 0) {
                                val delta = choices.getJSONObject(0).optJSONObject("delta")
                                val contentObj = delta?.opt("content")
                                if (contentObj != null && contentObj !== JSONObject.NULL) {
                                    fullText.append(contentObj.toString())
                                }
                            }
                        }
                        catch (ignored: Exception) {}
                    }
                }
            }
            resp.put("content", fullText.toString())
        }
        
        if (!connection.enqueueAndAwait(resp.toString())) {
            Log.e(TAG, "Failed to write query response")
        }
    }

    private fun handleStopStream(
        requestId: String,
        expectedGeneration: Long,
        controlConnection: ClientConnection,
    ) {
        Log.i(TAG, "Stopping session: id=$requestId, expectedGeneration=$expectedGeneration")
        val session = activeSessions[requestId]
        var stopped = false
        var generation = 0L
        if (session != null) {
            val detached = synchronized(session) {
                generation = session.generation
                if (expectedGeneration <= 0L || session.generation != expectedGeneration) {
                    return@synchronized null
                }
                if (!removeSession(session)) return@synchronized null
                stopped = true
                Pair(session.eventSource, session.takeConnection())
            }
            detached?.first?.cancel()
            detached?.second?.close()
        }
        updateLocks()
        val ack = JSONObject().apply {
            put("action", "stop_ack")
            put("requestId", requestId)
            put("generation", generation)
            put("stopped", stopped)
        }
        if (!controlConnection.enqueueAndAwait(ack.toString())) {
            Log.w(TAG, "Failed to deliver stop ACK for requestId=$requestId")
        }
    }

    private fun sendEventToSession(session: StreamSession, eventType: String, data: String) {
        var failedConnection: ClientConnection? = null
        var overflowConnection: ClientConnection? = null
        var overflowSource: EventSource? = null
        synchronized(session) {
            if (!isCurrentSession(session) || session.terminalOverflow.get()) return
            val eventObj = JSONObject().apply {
                put("requestId", session.requestId)
                put("generation", session.generation)
                put("eventType", eventType)
                put("eventData", data)
                put("index", session.eventBuffer.size)
            }
            val eventJson = eventObj.toString()
            val eventBytes = eventJson.toByteArray(Charsets.UTF_8).size.toLong()
            val wouldOverflow = session.eventBuffer.size >= MAX_SESSION_EVENTS ||
                session.bufferedBytes + eventBytes > MAX_SESSION_BUFFER_BYTES
            if (wouldOverflow || !reserveGlobalBytes(eventBytes)) {
                session.terminalOverflow.set(true)
                session.isCompleted = true
                session.lastFinishReason = "buffer_overflow"
                overflowSource = session.eventSource
                overflowConnection = session.takeConnection()
                return@synchronized
            }
            session.eventBuffer.add(eventObj)
            session.bufferedBytes += eventBytes
            
            session.activeConnection?.let { connection ->
                if (!connection.enqueue(eventJson)) {
                    Log.w(TAG, "Client writer queue overflow. id=${session.requestId}")
                    if (session.detachConnection(connection)) {
                        failedConnection = connection
                    }
                }
            }
        }
        if (overflowSource != null) {
            Log.e(TAG, "Session buffer budget exceeded. id=${session.requestId}")
            overflowSource?.cancel()
            dumpSessionToFile(session)
            overflowConnection?.enqueueAndAwait(JSONObject().apply {
                put("requestId", session.requestId)
                put("generation", session.generation)
                put("eventType", "error")
                put("eventData", JSONObject().apply { put("error", "Helper buffer limit exceeded") }.toString())
            }.toString())
            overflowConnection?.close()
            cleanupCompletedSession(session)
            return
        }
        failedConnection?.close()
        if (failedConnection != null) cleanupSessionIfCompletedAndDisconnected(session)
    }

    private fun cleanupSessionIfCompletedAndDisconnected(session: StreamSession) {
        val shouldDump = synchronized(session) {
            if (!isCurrentSession(session) || !session.isCompleted) return
            if (session.activeConnection == null) {
                true
            } else {
                Log.i(TAG, "Session id=${session.requestId} completed while connected; grace cleanup armed.")
                false
            }
        }
        if (shouldDump) {
            Log.i(TAG, "Session id=${session.requestId} completed while disconnected. Dumping to disk immediately.")
            dumpSessionToFile(session)
        }
        cleanupCompletedSession(session)
        updateLocks()
    }

    private fun cleanupCompletedSession(session: StreamSession) {
        if (!session.cleanupScheduled.compareAndSet(false, true)) return
        Log.i(TAG, "Scheduling bounded cleanup for session id=${session.requestId}.")
        serviceScope.launch(Dispatchers.IO) {
            delay(COMPLETED_SESSION_GRACE_MS)
            val detached = synchronized(session) {
                if (!session.isCompleted || !removeSession(session)) return@synchronized null
                Pair(session.eventSource, session.takeConnection())
            }
            detached?.first?.cancel()
            detached?.second?.close()
            updateLocks()
        }
    }

    private fun removeSession(session: StreamSession): Boolean {
        val removed = activeSessions.remove(session.requestId, session)
        if (removed) {
            releaseSessionBudget(session)
            scheduleIdleStopIfNeeded()
        }
        return removed
    }

    private fun releaseSessionBudget(session: StreamSession) {
        if (session.budgetReleased.compareAndSet(false, true)) {
            globalBufferedBytes.addAndGet(-session.bufferedBytes)
        }
    }

    private fun reserveGlobalBytes(bytes: Long): Boolean {
        while (true) {
            val current = globalBufferedBytes.get()
            if (current + bytes > MAX_GLOBAL_BUFFER_BYTES) return false
            if (globalBufferedBytes.compareAndSet(current, current + bytes)) return true
        }
    }

    @Synchronized
    private fun cancelIdleStop() {
        idleStopJob?.cancel()
        idleStopJob = null
    }

    @Synchronized
    private fun scheduleIdleStopIfNeeded() {
        if (activeSessions.isNotEmpty() || idleStopJob?.isActive == true) return
        idleStopJob = serviceScope.launch {
            delay(IDLE_SERVICE_GRACE_MS)
            if (activeSessions.isEmpty()) {
                Log.i(TAG, "Helper idle grace expired; stopping service.")
                stopSelf()
            }
        }
    }

    private fun dumpSessionToFile(session: StreamSession) {
        try {
            val cacheDir = File(applicationContext.cacheDir, "sse_cache")
            if (!cacheDir.exists()) {
                cacheDir.mkdirs()
            }
            val safeId = sha256(session.requestId)
            val file = File(cacheDir, "sse_recovered_$safeId.json")
            
            val events = synchronized(session) { session.eventBuffer.toList() }
            val fullText = StringBuilder()
            for (event in events) {
                if (event.getString("eventType") == "message") {
                    val eventData = event.getString("eventData")
                    if (eventData != "[DONE]") {
                        try {
                            val dataVal = JSONObject(eventData)
                            val choices = dataVal.optJSONArray("choices")
                            if (choices != null && choices.length() > 0) {
                                val delta = choices.getJSONObject(0).optJSONObject("delta")
                                val contentObj = delta?.opt("content")
                                if (contentObj != null && contentObj !== JSONObject.NULL) {
                                    fullText.append(contentObj.toString())
                                }
                            }
                        } catch (ignored: Exception) {}
                    }
                }
            }
            
            val dumpObj = JSONObject().apply {
                put("content", fullText.toString())
                put("finishReason", session.lastFinishReason ?: "completed")
                put("timestamp", System.currentTimeMillis())
            }
            
            file.writeText(dumpObj.toString())
            Log.i(TAG, "Successfully dumped session ${session.requestId} to file: ${file.absolutePath}")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to dump session to file", e)
        }
    }

    private var isTaskRemoved = false

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        Log.i(TAG, "onTaskRemoved: Main task removed by user.")
        isTaskRemoved = true
        checkSelfTermination()
    }

    @Synchronized
    private fun checkSelfTermination() {
        val hasRunning = activeSessions.values.any { !it.isCompleted }
        if (isTaskRemoved && !hasRunning) {
            Log.i(TAG, "Task removed and no running sessions. Stopping service.")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
            stopSelf()
        }
    }

    @Synchronized
    private fun updateLocks() {
        val hasRunning = activeSessions.values.any { !it.isCompleted }
        if (hasRunning) {
            cancelIdleStop()
            acquireLocks()
        } else {
            releaseLocks()
            if (activeSessions.isEmpty()) scheduleIdleStopIfNeeded()
        }
        checkSelfTermination()
    }

    private fun acquireLocks() {
        val appContext = applicationContext
        if (wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            if (powerManager != null) {
                wakeLock = powerManager.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VCP:SseProxyWakeLock")
            }
        }
        wakeLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.i(TAG, "SseProxy WakeLock ACQUIRED.")
            }
        }

        if (wifiLock == null) {
            val wifiManager = appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
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
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.i(TAG, "SseProxy WakeLock RELEASED.")
            }
        }
        wakeLock = null

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
        return try {
            val digest = java.security.MessageDigest.getInstance("SHA-256")
            val hash = digest.digest(input.toByteArray(Charsets.UTF_8))
            hash.joinToString("") { "%02x".format(it) }
        } catch (e: Exception) {
            input.hashCode().toString()
        }
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

    private fun cleanTextForNotification(text: String): String {
        var clean = text
        // 1. 去除元思考链 [--- VCP元思考链:xxx ---] ... [--- 元思考链结束 ---]
        clean = clean.replace(Regex("\\[--- VCP元思考链:[\\s\\S]*?元思考链结束 ---\\]", RegexOption.IGNORE_CASE), "")
        // 2. 去除通用 <think>...</think> 标签
        clean = clean.replace(Regex("<think>[\\s\\S]*?</think>", RegexOption.IGNORE_CASE), "")
        // 3. 去除未闭合的 <think> 和元思考链
        clean = clean.replace(Regex("<think>[\\s\\S]*", RegexOption.IGNORE_CASE), "")
        clean = clean.replace(Regex("\\[--- VCP元思考链:[\\s\\S]*", RegexOption.IGNORE_CASE), "")
        // 4. 去除多余空行
        clean = clean.replace(Regex("\\n\\s*\\n+"), "\n")
        return clean.trim()
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
            
            val events = synchronized(session) { session.eventBuffer.toList() }
            val fullText = StringBuilder()
            for (event in events) {
                    if (event.getString("eventType") == "message") {
                        val eventData = event.getString("eventData")
                        if (eventData != "[DONE]") {
                            try {
                                val dataVal = JSONObject(eventData)
                                val choices = dataVal.optJSONArray("choices")
                                if (choices != null && choices.length() > 0) {
                                    val delta = choices.getJSONObject(0).optJSONObject("delta")
                                    val contentObj = delta?.opt("content")
                                    if (contentObj != null && contentObj !== JSONObject.NULL) {
                                        fullText.append(contentObj.toString())
                                    }
                                }
                            } catch (ignored: Exception) {}
                        }
                    }
            }
            
            val replyText = fullText.toString().trim()
            val cleanReply = cleanTextForNotification(replyText)
            val singleLineReply = cleanReply.replace("\n", " ").replace("\r", " ").trim()
            
            contentText = if (singleLineReply.isNotEmpty()) {
                if (singleLineReply.length > 80) singleLineReply.take(80) + "..." else singleLineReply
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
