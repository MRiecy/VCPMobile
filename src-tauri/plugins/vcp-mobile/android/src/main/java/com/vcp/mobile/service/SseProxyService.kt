package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
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
        private const val TAG = "SseProxyService"
        private const val CHANNEL_ID = "vcp_helper_service_channel"
        private const val NOTIFICATION_ID_SERVICE = 0x53545210
    }

    class StreamSession(
        val requestId: String,
        var eventSource: EventSource? = null,
        val eventBuffer: MutableList<JSONObject> = mutableListOf(),
        var isCompleted: Boolean = false,
        var lastFinishReason: String? = null,
        var activeSocketWriter: BufferedWriter? = null
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
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        // 本地 TCP 架构下，主进程不再绑定此服务，直接通过 TCP 连接通信
        return null
    }

    override fun onDestroy() {
        Log.i(TAG, "onDestroy: shutting down TCP server and all sessions.")
        try {
            serverSocket?.close()
        } catch (ignored: Exception) {}
        
        for (session in activeSessions.values) {
            session.eventSource?.cancel()
            try { session.activeSocketWriter?.close() } catch (ignored: Exception) {}
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
            var reader: BufferedReader? = null
            var writer: BufferedWriter? = null
            var boundRequestId: String? = null
            try {
                reader = BufferedReader(InputStreamReader(socket.getInputStream(), Charsets.UTF_8))
                writer = BufferedWriter(OutputStreamWriter(socket.getOutputStream(), Charsets.UTF_8))
                
                val requestLine = reader.readLine() ?: return@launch
                val request = JSONObject(requestLine)
                val action = request.getString("action")
                val requestId = request.getString("requestId")
                boundRequestId = requestId
                
                Log.i(TAG, "TCP Command received: action=$action, requestId=$requestId")
                
                when (action) {
                    "start" -> {
                        val url = request.getString("url")
                        val headersJson = request.optString("headers", "{}")
                        val body = request.optString("body", "")
                        handleStartStream(requestId, url, headersJson, body, writer)
                        readSocketUntilClose(socket, reader, requestId)
                    }
                    "resume" -> {
                        handleResumeStream(requestId, writer)
                        readSocketUntilClose(socket, reader, requestId)
                    }
                    "query" -> {
                        handleQueryStream(requestId, writer)
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

    private fun readSocketUntilClose(socket: Socket, reader: BufferedReader, requestId: String) {
        try {
            val buf = CharArray(1024)
            while (reader.read(buf) != -1) {
                // 仅维持连接读取，阻塞直到客户端断开连接
            }
        } catch (ignored: Exception) {
        } finally {
            Log.i(TAG, "Client socket disconnected for requestId=$requestId")
            val session = activeSessions[requestId]
            if (session != null) {
                synchronized(session) {
                    if (session.activeSocketWriter != null) {
                        try { session.activeSocketWriter?.close() } catch (ignored: Exception) {}
                        session.activeSocketWriter = null
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
        writer: BufferedWriter
    ) {
        val session = StreamSession(requestId, activeSocketWriter = writer)
        activeSessions[requestId] = session
        
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
                cleanupSessionIfCompletedAndDisconnected(session)
            }
            
            override fun onClosed(eventSource: EventSource) {
                Log.i(TAG, "SSE Closed: id=$requestId")
                session.isCompleted = true
                session.lastFinishReason = "completed"
                sendEventToSession(session, "closed", "")
                cleanupSessionIfCompletedAndDisconnected(session)
            }
        }
        
        val source = EventSources.createFactory(httpClient).newEventSource(requestBuilder.build(), listener)
        session.eventSource = source
        updateLocks()
    }

    private fun handleResumeStream(requestId: String, writer: BufferedWriter) {
        val session = activeSessions[requestId]
        if (session == null) {
            Log.w(TAG, "resume: Session not found for id=$requestId")
            val errEvent = JSONObject().apply {
                put("requestId", requestId)
                put("eventType", "error")
                put("eventData", JSONObject().apply { put("error", "Session not found") }.toString())
            }
            writer.write(errEvent.toString() + "\n")
            writer.flush()
            return
        }
        
        Log.i(TAG, "Resuming session id=$requestId, playing back ${session.eventBuffer.size} events.")
        
        synchronized(session) {
            session.activeSocketWriter = writer
            for (event in session.eventBuffer) {
                try {
                    writer.write(event.toString() + "\n")
                } catch (e: Exception) {
                    Log.e(TAG, "Failed playing back events to socket", e)
                    session.activeSocketWriter = null
                    return
                }
            }
            try {
                writer.flush()
            } catch (e: Exception) {
                session.activeSocketWriter = null
            }
        }
        updateLocks()
    }

    private fun handleQueryStream(requestId: String, writer: BufferedWriter) {
        val session = activeSessions[requestId]
        val resp = JSONObject()
        resp.put("requestId", requestId)
        
        if (session == null) {
            resp.put("status", "not_found")
        } else {
            synchronized(session) {
                resp.put("status", if (session.isCompleted) "completed" else "streaming")
                resp.put("lastFinishReason", session.lastFinishReason ?: "")
                
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
                                    val content = delta?.optString("content", "") ?: ""
                                    fullText.append(content)
                                }
                            } catch (ignored: Exception) {}
                        }
                    }
                }
                resp.put("content", fullText.toString())
            }
        }
        
        try {
            writer.write(resp.toString() + "\n")
            writer.flush()
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
                if (session.activeSocketWriter != null) {
                    try { session.activeSocketWriter?.close() } catch (ignored: Exception) {}
                    session.activeSocketWriter = null
                }
            }
        }
        updateLocks()
    }

    private fun sendEventToSession(session: StreamSession, eventType: String, data: String) {
        val eventObj = JSONObject().apply {
            put("requestId", session.requestId)
            put("eventType", eventType)
            put("eventData", data)
        }
        
        synchronized(session) {
            session.eventBuffer.add(eventObj)
            
            session.activeSocketWriter?.let { writer ->
                try {
                    writer.write(eventObj.toString() + "\n")
                    writer.flush()
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to write event to socket, client might be suspended. id=${session.requestId}")
                    session.activeSocketWriter = null
                }
            }
        }
    }

    private fun cleanupSessionIfCompletedAndDisconnected(session: StreamSession) {
        synchronized(session) {
            if (session.isCompleted) {
                if (session.activeSocketWriter == null) {
                    // 主进程已断开：延迟 5 分钟清理，给冷启动恢复留出时间
                    Log.i(TAG, "Session id=${session.requestId} completed while disconnected. Scheduling cleanup in 5 minutes.")
                    serviceScope.launch(Dispatchers.IO) {
                        kotlinx.coroutines.delay(5 * 60 * 1000L)
                        synchronized(session) {
                            if (activeSessions[session.requestId] === session && session.activeSocketWriter == null) {
                                Log.i(TAG, "Session id=${session.requestId} 5-min timeout. Dumping to cache file and removing.")
                                dumpSessionToFile(session)
                                activeSessions.remove(session.requestId)
                                updateLocks()
                            }
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
            val file = File(cacheDir, "sse_recovered_${session.requestId}.json")
            
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
                                val content = delta?.optString("content", "") ?: ""
                                fullText.append(content)
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

    @Synchronized
    private fun updateLocks() {
        val hasRunning = activeSessions.values.any { !it.isCompleted }
        if (hasRunning) {
            acquireLocks()
        } else {
            releaseLocks()
        }
        
        if (activeSessions.isEmpty()) {
            Log.i(TAG, "No sessions left. Stopping foreground and service.")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
            stopSelf()
        }
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
            val channel = NotificationChannel(
                CHANNEL_ID,
                "VCP 后台连接助手",
                NotificationManager.IMPORTANCE_MIN
            ).apply {
                description = "维持后台稳定的 AI 对话流式连接"
                setShowBadge(false)
            }
            getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
        }
    }

    private fun buildServiceNotification(): Notification {
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("VCP 连接助手")
            .setContentText("后台低功耗网络托管中")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setCategory(Notification.CATEGORY_SERVICE)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }
}
