package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Message
import android.os.Messenger
import android.os.RemoteException
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.yield
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.json.JSONObject
import android.os.PowerManager
import android.net.wifi.WifiManager
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit

/**
 * 隔离进程通用助手 (SseProxyService)
 * 
 * 【职责】：
 * 1. 运行在独立的 ":helper" 进程中，极低内存消耗，免受主进程 LMK 强杀影响。
 * 2. 作为一个“哑网络管道”，不含任何 VCP 业务逻辑，仅执行主进程交给的 SSE 请求。
 * 3. 即使主进程（WebView/Rust）切后台被冻结或强杀，此服务依然在后台维持连接并将流式数据缓存于内存。
 * 4. 主进程重新启动（冷启动）或返回前台（温回归）后，绑定此服务即可倍速流式回放积压的 Chunks，实现无缝流恢复。
 */
class SseProxyService : Service() {

    companion object {
        private const val TAG = "SseProxyService"
        private const val CHANNEL_ID = "vcp_helper_service_channel"
        private const val NOTIFICATION_ID_SERVICE = 0x53545210

        // Messenger 消息协议 (主进程 -> 子进程)
        const val MSG_REGISTER_CLIENT = 1
        const val MSG_UNREGISTER_CLIENT = 2
        const val MSG_START_STREAM = 3
        const val MSG_STOP_STREAM = 4
        const val MSG_GET_CACHE = 5

        // Messenger 消息协议 (子进程 -> 主进程)
        const val EVENT_SSE_CHUNK = 10
        // 注：EVENT_CACHE_RESPONSE 已废弃，完全走 EVENT_SSE_CHUNK 流式倍速回放

        // Bundle 键名
        const val KEY_REQUEST_ID = "request_id"
        const val KEY_URL = "url"
        const val KEY_HEADERS = "headers"
        const val KEY_BODY = "body"
        const val KEY_EVENT_TYPE = "event_type" // "open", "message", "error", "closed"
        const val KEY_EVENT_DATA = "event_data"
    }

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS) // 维持无限长连接
            .connectTimeout(15, TimeUnit.SECONDS)
            .build()
    }

    // 追踪所有活跃的 EventSource 连接
    private val activeConnections = ConcurrentHashMap<String, EventSource>()

    // 独立于主进程的子进程物理锁，保障后台网络不中断
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    
    // 活跃的缓存文件流句柄表 (Key: requestId, Value: FileOutputStream)
    private val activeFileStreams = ConcurrentHashMap<String, FileOutputStream>()

    // 主进程的 Messenger 客户端
    private var clientMessenger: Messenger? = null

    // 自身的 Messenger，用于接收主进程的消息
    private lateinit var serviceMessenger: Messenger

    // 协程作用域，用于后台流式回放
    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())

    /**
     * 处理主进程发来的消息
     */
    private inner class IncomingHandler(looper: Looper) : Handler(looper) {
        override fun handleMessage(msg: Message) {
            when (msg.what) {
                MSG_REGISTER_CLIENT -> {
                    Log.i(TAG, "Client registered.")
                    clientMessenger = msg.replyTo
                }
                MSG_UNREGISTER_CLIENT -> {
                    Log.i(TAG, "Client unregistered.")
                    clientMessenger = null
                }
                MSG_START_STREAM -> {
                    val data = msg.data ?: return
                    val requestId = data.getString(KEY_REQUEST_ID) ?: return
                    val url = data.getString(KEY_URL) ?: return
                    val headersJson = data.getString(KEY_HEADERS) ?: "{}"
                    val body = data.getString(KEY_BODY) ?: ""
                    
                    startSseRequest(requestId, url, headersJson, body)
                }
                MSG_STOP_STREAM -> {
                    val data = msg.data ?: return
                    val requestId = data.getString(KEY_REQUEST_ID) ?: return
                    stopSseRequest(requestId)
                }
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        serviceMessenger = Messenger(IncomingHandler(Looper.getMainLooper()))
        
        // 清理上一次运行残留的缓存文件，防止磁盘堆积
        try {
            val sseCacheDir = File(applicationContext.cacheDir, "sse_cache")
            if (sseCacheDir.exists()) {
                sseCacheDir.deleteRecursively()
            }
            sseCacheDir.mkdirs()
        } catch (e: Exception) {
            Log.e(TAG, "Failed to cleanup stale cache files", e)
        }
        
        // 启动为前台服务，获得免死金牌
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
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? {
        Log.i(TAG, "onBind called.")
        return serviceMessenger.binder
    }

    override fun onDestroy() {
        Log.i(TAG, "onDestroy: cancelling all connections.")
        for (requestId in activeConnections.keys) {
            stopSseRequest(requestId)
        }
        activeConnections.clear()
        updateLocks() // 确保进程销毁时双锁被物理释放
        serviceScope.cancel() // 取消协程，释放资源
        super.onDestroy()
    }

    /**
     * 发起外部 SSE 请求
     */
    private fun startSseRequest(requestId: String, url: String, headersJson: String, postBody: String) {
        if (activeConnections.containsKey(requestId)) {
            Log.w(TAG, "Connection already exists for requestId=$requestId, skipping.")
            return
        }

        Log.i(TAG, "Starting SSE Request: id=$requestId, url=$url")
        
        // 初始化该请求的本地缓存文件流
        try {
            val sseCacheDir = File(applicationContext.cacheDir, "sse_cache")
            val cacheFile = File(sseCacheDir, "sse_cache_$requestId.txt")
            if (cacheFile.exists()) {
                cacheFile.delete()
            }
            val fos = FileOutputStream(cacheFile, true)
            activeFileStreams[requestId] = fos
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create cache file stream for id=$requestId", e)
        }

        val requestBuilder = Request.Builder().url(url)
        
        // 解析 Headers
        try {
            val headersObj = JSONObject(headersJson)
            val keys = headersObj.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                val value = headersObj.getString(key)
                requestBuilder.header(key, value)
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to parse headers JSON: ", e)
        }

        // 设定为 POST 流式请求（符合 VCP /v1/chat/completions 规范）
        if (postBody.isNotEmpty()) {
            val mediaType = "application/json; charset=utf-8".toMediaType()
            requestBuilder.post(postBody.toRequestBody(mediaType))
        }

        val request = requestBuilder.build()

        val eventSourceListener = object : EventSourceListener() {
            override fun onOpen(eventSource: EventSource, response: Response) {
                Log.i(TAG, "SSE Connected: id=$requestId")
                sendEventToClient(requestId, "open", "")
            }

            override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                // 收到流式块
                sendEventToClient(requestId, "message", data)
            }

            override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                val errorMsg = t?.message ?: response?.message ?: "Unknown network error"
                Log.w(TAG, "SSE Failed: id=$requestId, error=$errorMsg")
                
                // 将错误事件格式化为 JSON 供前端/Rust解析
                val errObj = JSONObject().apply {
                    put("error", errorMsg)
                    put("status", response?.code ?: 0)
                }
                sendEventToClient(requestId, "error", errObj.toString())
                
                activeConnections.remove(requestId)
                updateLocks()
            }

            override fun onClosed(eventSource: EventSource) {
                Log.i(TAG, "SSE Closed: id=$requestId")
                sendEventToClient(requestId, "closed", "")
                activeConnections.remove(requestId)
                updateLocks()
            }
        }

        val source = EventSources.createFactory(httpClient).newEventSource(request, eventSourceListener)
        activeConnections[requestId] = source
        updateLocks()
    }

    /**
     * 终止特定请求并清除缓存
     */
    private fun stopSseRequest(requestId: String) {
        Log.i(TAG, "Stopping SSE Request: id=$requestId")
        activeConnections.remove(requestId)?.cancel()
        
        // 关闭并移除文件流
        activeFileStreams.remove(requestId)?.let { fos ->
            try {
                fos.close()
            } catch (e: Exception) {
                Log.e(TAG, "Failed to close file stream for id=$requestId", e)
            }
        }

        // 物理删除缓存文件，防止磁盘堆积
        try {
            val sseCacheDir = File(applicationContext.cacheDir, "sse_cache")
            val cacheFile = File(sseCacheDir, "sse_cache_$requestId.txt")
            if (cacheFile.exists()) {
                cacheFile.delete()
                Log.d(TAG, "Deleted stream cache file for id=$requestId")
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to delete cache file for id=$requestId", e)
        }

        updateLocks()
    }

    /**
     * 根据当前是否有活跃连接，动态在子进程中获取/释放 WakeLock 和 WifiLock
     */
    @Synchronized
    private fun updateLocks() {
        val hasActive = activeConnections.isNotEmpty()
        if (hasActive) {
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
                    Log.i(TAG, "updateLocks: SseProxy WakeLock ACQUIRED.")
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
                    Log.i(TAG, "updateLocks: SseProxy WifiLock ACQUIRED.")
                }
            }
        } else {
            wakeLock?.let {
                if (it.isHeld) {
                    it.release()
                    Log.i(TAG, "updateLocks: SseProxy WakeLock RELEASED.")
                }
            }
            wakeLock = null

            wifiLock?.let {
                if (it.isHeld) {
                    it.release()
                    Log.i(TAG, "updateLocks: SseProxy WifiLock RELEASED.")
                }
            }
            wifiLock = null

            // 当没有活跃连接时，主动退出前台并停止服务，消除通知栏残留
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
            stopSelf()
            Log.i(TAG, "updateLocks: No active connections. SseProxyService stopping self.")
        }
    }

    /**
     * 向主进程发送事件，如果主进程挂起/死亡，则自动写入本地缓存
     */
    private fun sendEventToClient(requestId: String, eventType: String, data: String, bypassCache: Boolean = false) {
        if (!bypassCache) {
            // 构造标准的 SSE JSON 包写入缓存
            val payload = JSONObject().apply {
                put("type", eventType)
                put("data", data)
                put("timestamp", System.currentTimeMillis())
            }.toString()

            // 实时追加写入本地缓存文件（带缓冲且线程安全）
            val fos = activeFileStreams[requestId]
            if (fos != null) {
                try {
                    synchronized(fos) {
                        fos.write((payload + "\n").toByteArray(Charsets.UTF_8))
                        fos.flush() // 确保数据立即可被主进程读取
                    }
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to write chunk to cache file for id=$requestId", e)
                }
            }
        }

        // 如果客户端在线，尝试实时推过去
        clientMessenger?.let { messenger ->
            // 使用 Message.obtain() 从全局消息池中获取 Message 实例，减少 GC 压力
            val msg = Message.obtain(null, EVENT_SSE_CHUNK)
            msg.data = Bundle().apply {
                putString(KEY_REQUEST_ID, requestId)
                putString(KEY_EVENT_TYPE, eventType)
                putString(KEY_EVENT_DATA, data)
            }
            try {
                messenger.send(msg)
            } catch (e: RemoteException) {
                Log.w(TAG, "Client is unreachable, caching event. id=$requestId")
                clientMessenger = null // 客户端连接已失效
            }
        }
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
