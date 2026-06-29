package com.vcp.mobile.service

import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * 隔离进程推送服务 (PushListenerService)
 * 
 * 【设计初衷】：
 * 1. 进程级隔离运行在 ":push" 中，只加载极简 OkHttp 与 JVM 基础类，内存占用仅约 10MB~15MB。
 * 2. 彻底避免由于 Tauri/WebView 主进程内存过大（150MB+）退后台时被系统 LMK (低内存杀手) 优先回收的惨剧。
 * 3. 放弃 CPU 双锁常驻，改用 AlarmManager 指数退避重连机制。在连接正常时让 CPU 睡觉，网卡通过 TCP 硬件中断（AP 唤醒）接收推送，最大化省电。
 */
class PushListenerService : Service() {

    companion object {
        private const val TAG = "VcpPushService"
        private const val CHANNEL_ID = "vcp_push_listener_channel"
        private const val NOTIFICATION_ID_SERVICE = 0x53545209 // 前台服务本身的通知 ID
        private const val NOTIFICATION_ID_MESSAGE_BASE = 0x60000000 // 消息推送通知的基准 ID
        
        // 用于退避重连的 SharedPreferences 键名
        private const val PREFS_NAME = "vcp_push_prefs"
        private const val KEY_RETRY_DELAY = "retry_delay"
        
        // 允许从外部传入的动作命令
        const val ACTION_START = "com.vcp.mobile.action.START_PUSH"
        const val ACTION_STOP = "com.vcp.mobile.action.STOP_PUSH"
    }

    private var client: OkHttpClient? = null
    private var eventSource: EventSource? = null
    private var isListenerActive = false
    
    private val prefs: SharedPreferences by lazy {
        getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val action = intent?.action
        Log.i(TAG, "onStartCommand: action=$action")

        if (action == ACTION_STOP) {
            stopPushListener()
            stopSelf()
            return START_NOT_STICKY
        }

        // 1. 作为前台服务启动，展示一个静默的通知栏图标（符合系统规范）
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

        // 2. 启动/重建长连接监听
        startPushListener()

        return START_STICKY
    }

    override fun onDestroy() {
        stopPushListener()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /**
     * 建立 SSE 链接监听
     */
    private synchronized fun startPushListener() {
        if (isListenerActive) {
            Log.d(TAG, "Push listener is already active, skipping start.")
            return
        }

        // 从 SharedPreferences 中读取凭证（由主进程写入）
        val vcpUrl = prefs.getString("vcp_url", "") ?: ""
        val vcpKey = prefs.getString("vcp_key", "") ?: ""
        val topic = prefs.getString("vcp_topic", "") ?: ""

        if (vcpUrl.isEmpty() || vcpKey.isEmpty() || topic.isEmpty()) {
            Log.w(TAG, "Missing credentials. vcpUrl=$vcpUrl, topic=$topic. Cannot start listener.")
            return
        }

        // 构建符合 ntfy/VCPPush 规范的 SSE URL
        var cleanUrl = vcpUrl.trimEnd('/')
        if (!cleanUrl.contains("/vcp-push")) {
            cleanUrl = "$cleanUrl/vcp-push"
        }
        val sseUrl = "$cleanUrl/$topic"

        Log.i(TAG, "Starting SSE listener: url=$sseUrl")

        val okHttpClient = OkHttpClient.Builder()
            .readTimeout(0, TimeUnit.MILLISECONDS) // 设为 0 维持无限长连接
            .build()
        client = okHttpClient

        val request = Request.Builder()
            .url(sseUrl)
            .header("Accept", "text/event-stream")
            // 如果采用自建模式，可选加入 Authorization
            .header("Authorization", "Bearer $vcpKey")
            .build()

        isListenerActive = true
        
        eventSource = EventSources.createFactory(okHttpClient)
            .newEventSource(request, object : EventSourceListener() {
                override fun onOpen(eventSource: EventSource, response: Response) {
                    Log.i(TAG, "SSE Connection opened successfully.")
                    // 连接成功，重置退避延迟计数
                    prefs.edit().putLong(KEY_RETRY_DELAY, 10000L).apply() // 默认 10 秒
                }

                override fun onEvent(eventSource: EventSource, id: String?, type: String?, data: String) {
                    Log.d(TAG, "SSE Received event: id=$id, type=$type, data=$data")
                    try {
                        val json = JSONObject(data)
                        handleIncomingPush(json)
                    } catch (e: Exception) {
                        Log.e(TAG, "Failed to parse push JSON: ", e)
                    }
                }

                override fun onFailure(eventSource: EventSource, t: Throwable?, response: Response?) {
                    Log.w(TAG, "SSE Connection failed: ${t?.message}. Reconnecting...")
                    isListenerActive = false
                    scheduleReconnect()
                }

                override fun onClosed(eventSource: EventSource) {
                    Log.w(TAG, "SSE Connection closed.")
                    isListenerActive = false
                }
            })
    }

    private synchronized fun stopPushListener() {
        Log.i(TAG, "Stopping SSE listener...")
        eventSource?.cancel()
        eventSource = null
        client?.dispatcher?.executorService?.shutdown()
        client = null
        isListenerActive = false
    }

    /**
     * 使用 AlarmManager 执行指数退避重连
     * 避免在断网期间死循环重连（CPU 空转耗电），定时器在时间到之前保持 CPU 深度睡眠。
     */
    private fun scheduleReconnect() {
        val currentDelay = prefs.getLong(KEY_RETRY_DELAY, 10000L)
        // 指数增加延迟，最大 5 分钟
        val nextDelay = (currentDelay * 2).coerceAtMost(300000L)
        prefs.edit().putLong(KEY_RETRY_DELAY, nextDelay).apply()

        Log.i(TAG, "Scheduling reconnect alarm in ${currentDelay / 1000} seconds.")

        val alarmManager = getSystemService(Context.ALARM_SERVICE) as? AlarmManager
        val intent = Intent(this, PushListenerService::class.java).apply {
            action = ACTION_START
        }
        val pendingIntent = PendingIntent.getService(
            this, 0, intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        alarmManager?.let {
            val triggerTime = SystemClock.elapsedRealtime() + currentDelay
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                it.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, triggerTime, pendingIntent)
            } else {
                it.set(AlarmManager.ELAPSED_REALTIME_WAKEUP, triggerTime, pendingIntent)
            }
        }
    }

    /**
     * 处理推送业务逻辑并下发通知栏
     */
    private fun handleIncomingPush(json: JSONObject) {
        val eventType = json.optString("event_type", "")
        val agentName = json.optString("agent_name", "VCP Agent")
        
        val notificationManager = getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
        if (notificationManager == null) return

        // 点击通知：启动主应用 Activity
        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(this, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
                // 传入深链参数，告诉主进程需要定位到哪
                val topicId = json.optString("topic_id", "")
                val msgId = json.optString("msg_id", "")
                putExtra("topic_id", topicId)
                putExtra("msg_id", msgId)
                action = Intent.ACTION_VIEW
                data = android.net.Uri.parse("vcp://chat?topic_id=$topicId&msg_id=$msgId")
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }

        val pendingOpen = PendingIntent.getActivity(
            this, System.currentTimeMillis().toInt(), openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val uniqueId = (System.currentTimeMillis() % 100000).toInt()
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(applicationInfo.icon)
            .setAutoCancel(true)
            .setContentIntent(pendingOpen)

        when (eventType) {
            "async_task_completed" -> {
                // 场景一：异步任务完成通知
                val taskTitle = json.optString("task_title", "异步任务")
                val summary = json.optString("summary", "任务已完成。")
                
                builder.setContentTitle("[$agentName] 任务已完成")
                    .setContentText("$taskTitle: $summary")
                    .setStyle(NotificationCompat.BigTextStyle().bigText("$taskTitle: $summary"))
                    .setPriority(NotificationCompat.PRIORITY_DEFAULT)
                
                notificationManager.notify(NOTIFICATION_ID_MESSAGE_BASE + uniqueId, builder.build())
            }
            
            "tool_approval_requested" -> {
                // 场景二：安全审计工具授权请求 (点击将拉起主应用进行处理)
                val toolName = json.optString("tool_name", "未知命令")
                val detail = json.optString("detail", "")
                val reason = json.optString("reason", "")

                builder.setContentTitle("[$agentName] 等待工具执行授权")
                    .setContentText("申请调用 $toolName 进行操作，请点击处理。")
                    .setStyle(NotificationCompat.BigTextStyle().bigText("申请调用: $toolName\n操作指令: $detail\n原因说明: $reason"))
                    .setPriority(NotificationCompat.PRIORITY_HIGH) // 强提醒
                
                notificationManager.notify(NOTIFICATION_ID_MESSAGE_BASE + uniqueId, builder.build())
            }
            
            else -> {
                // 默认普通通知
                val title = json.optString("title", "来自 $agentName 的消息")
                val message = json.optString("message", "")
                builder.setContentTitle(title)
                    .setContentText(message)
                    .setPriority(NotificationCompat.PRIORITY_DEFAULT)
                notificationManager.notify(NOTIFICATION_ID_MESSAGE_BASE + uniqueId, builder.build())
            }
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "VCP 智能体传呼通道",
                NotificationManager.IMPORTANCE_DEFAULT
            ).apply {
                description = "接收来自桌面端 VCPToolBox 的 Agent 决策与状态通知"
                setShowBadge(true)
            }
            getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
        }
    }

    private fun buildServiceNotification(): Notification {
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("VCP 传呼服务")
            .setContentText("后台低功耗监听维系中")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_MIN) // 设为极低优先级，静默显示不打扰用户
            .setCategory(Notification.CATEGORY_SERVICE)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }
}
