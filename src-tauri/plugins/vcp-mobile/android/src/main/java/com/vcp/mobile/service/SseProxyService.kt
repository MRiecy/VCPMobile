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
import android.media.AudioAttributes
import android.media.MediaPlayer
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.BufferedWriter
import java.io.File
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

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

        @Volatile
        var isServiceRunning = false
    }

    private var mediaPlayer: MediaPlayer? = null

    class StreamSession(
        val requestId: String,
        @Volatile var eventSource: EventSource? = null,
        val eventBuffer: MutableList<JSONObject> = mutableListOf(),
        @Volatile var isCompleted: Boolean = false,
        @Volatile var lastFinishReason: String? = null,
        var activeSocketOutputStream: java.io.OutputStream? = null,
        val contextJson: JSONObject? = null
    )

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .connectTimeout(15, TimeUnit.SECONDS)
            .build()
    }

    private val activeSessions = ConcurrentHashMap<String, StreamSession>()
    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())
    
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
        return START_STICKY
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
        
        stopSilentPlayback()
        
        for (session in activeSessions.values) {
            session.eventSource?.cancel()
            try { session.activeSocketOutputStream?.close() } catch (ignored: Exception) {}
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
            try {
                val inputStream = socket.getInputStream()
                val outputStream = socket.getOutputStream()
                
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
                        handleStartStream(requestId, url, headersJson, body, contextJson, outputStream)
                        readSocketUntilClose(socket, inputStream, requestId)
                    }
                    "resume" -> {
                        val startIndex = request.optInt("startIndex", 0)
                        handleResumeStream(requestId, startIndex, outputStream)
                        readSocketUntilClose(socket, inputStream, requestId)
                    }
                    "query" -> {
                        handleQueryStream(requestId, outputStream)
                        socket.close()
                    }
                    "stop" -> {
                        handleStopStream(requestId)
                        socket.close()
                    }
                }
            } catch (e: Exception) {
                Log.e(TAG, "Error handling client socket for $boundRequestId", e)
                try { socket.close() } catch (ignored: Exception) {}
            }
        }
    }

    private fun readSocketUntilClose(socket: Socket, inputStream: java.io.InputStream, requestId: String) {
        try {
            val buf = ByteArray(1024)
            while (inputStream.read(buf) != -1) {
                // 仅维持连接读取，阻塞直到客户端断开连接
            }
        } catch (ignored: Exception) {
        } finally {
            Log.i(TAG, "Client socket disconnected for requestId=$requestId")
            val session = activeSessions[requestId]
            if (session != null) {
                synchronized(session) {
                    if (session.activeSocketOutputStream != null) {
                        try { session.activeSocketOutputStream?.close() } catch (ignored: Exception) {}
                        session.activeSocketOutputStream = null
                    }
                }
                cleanupSessionIfCompletedAndDisconnected(session)
            }
            try { socket.close() } catch (ignored: Exception) {}
            updateLocks()
        }
    }

    private fun handleStartStream(
        requestId: String,
        url: String,
        headersJson: String,
        body: String,
        contextJson: JSONObject?,
        outputStream: java.io.OutputStream
    ) {
        val session = StreamSession(requestId, activeSocketOutputStream = outputStream, contextJson = contextJson)
        activeSessions[requestId] = session
        
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
                    Log.i(TAG, "SSE Connected: id=$requestId")
                    sendEventToSession(session, "open", "")
                }
                
                override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                    sendEventToSession(session, "message", data)
                }
                
                override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
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
                    Log.i(TAG, "SSE Closed: id=$requestId")
                    session.isCompleted = true
                    session.lastFinishReason = "completed"
                    sendEventToSession(session, "closed", "")
                    showStreamNotification(session, isSuccess = true, errorMsg = null)
                    cleanupSessionIfCompletedAndDisconnected(session)
                }
            }
            
            val source = EventSources.createFactory(httpClient).newEventSource(requestBuilder.build(), listener)
            session.eventSource = source
            updateLocks()
        } catch (e: Exception) {
            activeSessions.remove(requestId)
            Log.e(TAG, "Failed to start stream source for $requestId", e)
            updateLocks()
            throw e
        }
    }

    private fun handleResumeStream(requestId: String, startIndex: Int, outputStream: java.io.OutputStream) {
        val session = activeSessions[requestId]
        if (session == null) {
            Log.w(TAG, "resume: Session not found for id=$requestId")
            val errEvent = JSONObject().apply {
                put("requestId", requestId)
                put("eventType", "error")
                put("eventData", JSONObject().apply { put("error", "Session not found") }.toString())
            }
            try {
                writeLengthPrefixed(outputStream, errEvent.toString())
            } catch (ignored: Exception) {}
            return
        }
        
        Log.i(TAG, "Resuming session id=$requestId, playing back events from $startIndex.")
        
        synchronized(session) {
            session.activeSocketOutputStream = outputStream
            val bufferSize = session.eventBuffer.size
            for (i in startIndex until bufferSize) {
                try {
                    val event = session.eventBuffer[i]
                    writeLengthPrefixed(outputStream, event.toString())
                } catch (e: Exception) {
                    Log.e(TAG, "Failed playing back events to socket", e)
                    session.activeSocketOutputStream = null
                    return
                }
            }
        }
        updateLocks()
    }

    private fun handleQueryStream(requestId: String, outputStream: java.io.OutputStream) {
        val session = activeSessions[requestId]
        val resp = JSONObject()
        resp.put("requestId", requestId)
        
        if (session == null) {
            resp.put("status", "not_found")
        } else {
            synchronized(session) {
                resp.put("status", if (session.isCompleted) "completed" else "streaming")
                resp.put("lastFinishReason", session.lastFinishReason ?: "")
                resp.put("lastEventIndex", session.eventBuffer.size - 1)
                
                // 从内存中拼接出完整的文本，供冷启动快速落盘
                val fullText = StringBuilder()
                for (event in session.eventBuffer) {
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
                resp.put("content", fullText.toString())
            }
        }
        
        try {
            writeLengthPrefixed(outputStream, resp.toString())
        } catch (e: Exception) {
            Log.e(TAG, "Failed to write query response", e)
        }
    }

    private fun handleStopStream(requestId: String) {
        Log.i(TAG, "Stopping session: id=$requestId")
        val session = activeSessions.remove(requestId)
        if (session != null) {
            synchronized(session) {
                session.eventSource?.cancel()
                if (session.activeSocketOutputStream != null) {
                    try { session.activeSocketOutputStream?.close() } catch (ignored: Exception) {}
                    session.activeSocketOutputStream = null
                }
            }
        }
        updateLocks()
    }

    private fun sendEventToSession(session: StreamSession, eventType: String, data: String) {
        synchronized(session) {
            val eventObj = JSONObject().apply {
                put("requestId", session.requestId)
                put("eventType", eventType)
                put("eventData", data)
                put("index", session.eventBuffer.size)
            }
            session.eventBuffer.add(eventObj)
            
            session.activeSocketOutputStream?.let { out ->
                try {
                    writeLengthPrefixed(out, eventObj.toString())
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to write event to socket, client might be suspended. id=${session.requestId}")
                    session.activeSocketOutputStream = null
                }
            }
        }
    }

    private fun cleanupSessionIfCompletedAndDisconnected(session: StreamSession) {
        synchronized(session) {
            if (session.isCompleted) {
                if (session.activeSocketOutputStream == null) {
                    // 主进程已断开：立即将数据转储到磁盘缓存，确保在释放 WakeLock / 进程休眠前数据已落盘！
                    Log.i(TAG, "Session id=${session.requestId} completed while disconnected. Dumping to disk immediately.")
                    dumpSessionToFile(session)

                    // 延迟 5 分钟清理内存缓存，给冷启动恢复留出时间
                    Log.i(TAG, "Scheduling memory cleanup for session id=${session.requestId} in 5 minutes.")
                    serviceScope.launch(Dispatchers.IO) {
                        kotlinx.coroutines.delay(5 * 60 * 1000L)
                        var shouldUpdate = false
                        synchronized(session) {
                            if (activeSessions[session.requestId] === session && session.activeSocketOutputStream == null) {
                                Log.i(TAG, "Session id=${session.requestId} 5-min timeout. Removing from memory.")
                                activeSessions.remove(session.requestId)
                                shouldUpdate = true
                            }
                        }
                        if (shouldUpdate) {
                            updateLocks()
                        }
                    }
                } else {
                    // 主进程在线：等待主进程发送 stop 指令，不需要自动清理
                    Log.i(TAG, "Session id=${session.requestId} completed while connected. Waiting for client stop command.")
                }
            }
        }
        updateLocks()
    }

    private fun dumpSessionToFile(session: StreamSession) {
        try {
            val cacheDir = File(applicationContext.cacheDir, "sse_cache")
            if (!cacheDir.exists()) {
                cacheDir.mkdirs()
            }
            val safeId = sha256(session.requestId)
            val file = File(cacheDir, "sse_recovered_$safeId.json")
            
            val fullText = StringBuilder()
            synchronized(session) {
                for (event in session.eventBuffer) {
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
            acquireLocks()
            startSilentPlayback()
        } else {
            releaseLocks()
            stopSilentPlayback()
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
                "Open",
                openPendingIntent
            )

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }

    @Synchronized
    private fun ensureSilentAudioFile(): File {
        val file = File(cacheDir, "silent.wav")
        if (file.exists() && file.length() > 0) {
            return file
        }
        try {
            file.outputStream().use { out ->
                // RIFF Header
                out.write(byteArrayOf(0x52, 0x49, 0x46, 0x46)) // "RIFF"
                out.write(byteArrayOf(0x64, 0x06, 0x00, 0x00)) // Size: 1636
                out.write(byteArrayOf(0x57, 0x41, 0x56, 0x45)) // "WAVE"
                
                // fmt Chunk
                out.write(byteArrayOf(0x66, 0x6d, 0x74, 0x20)) // "fmt "
                out.write(byteArrayOf(0x10, 0x00, 0x00, 0x00)) // Chunk size: 16
                out.write(byteArrayOf(0x01, 0x00))             // Format: 1 (PCM)
                out.write(byteArrayOf(0x01, 0x00))             // Channels: 1 (Mono)
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00)) // Sample rate: 8000
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00)) // Byte rate: 8000
                out.write(byteArrayOf(0x01, 0x00))             // Block align: 1
                out.write(byteArrayOf(0x08, 0x00))             // Bits per sample: 8
                
                // data Chunk
                out.write(byteArrayOf(0x64, 0x61, 0x74, 0x61)) // "data"
                out.write(byteArrayOf(0x40, 0x06, 0x00, 0x00)) // Data size: 1600
                
                // 1600 bytes of silence (0x80 for 8-bit PCM)
                val silence = ByteArray(1600) { 0x80.toByte() }
                out.write(silence)
            }
            Log.i(TAG, "Created silent.wav in cache directory.")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create silent.wav", e)
        }
        return file
    }

    private fun startSilentPlayback() {
        if (mediaPlayer != null) return
        try {
            val silentFile = ensureSilentAudioFile()
            mediaPlayer = MediaPlayer().apply {
                setDataSource(silentFile.absolutePath)
                isLooping = true
                
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                    setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build()
                    )
                }
                
                prepare()
                start()
            }
            Log.i(TAG, "Silent playback started.")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start silent playback", e)
        }
    }

    private fun stopSilentPlayback() {
        mediaPlayer?.let {
            try {
                if (it.isPlaying) {
                    it.stop()
                }
                it.release()
            } catch (ignored: Exception) {}
        }
        mediaPlayer = null
        Log.i(TAG, "Silent playback stopped.")
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

    private fun writeLengthPrefixed(out: java.io.OutputStream, json: String) {
        val bytes = json.toByteArray(Charsets.UTF_8)
        val buffer = java.nio.ByteBuffer.allocate(4 + bytes.size)
        buffer.putInt(bytes.size)
        buffer.put(bytes)
        out.write(buffer.array())
        out.flush()
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
        
        val title: String
        val contentText: String
        
        if (isSuccess) {
            title = "✨ $agentName 已回复"
            
            val fullText = StringBuilder()
            synchronized(session) {
                for (event in session.eventBuffer) {
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
