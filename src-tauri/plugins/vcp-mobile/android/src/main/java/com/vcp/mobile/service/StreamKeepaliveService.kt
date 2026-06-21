package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import android.os.PowerManager

/**
 * 流式响应前台保活服务
 *
 * 当 Agent 正在流式生成回复时启动，通过持续通知向系统声明"用户感知的重要任务"，
 * 显著降低进程被 OEM 杀后台的概率。
 *
 * 设计原则：高可见性常驻保活
 * - 通知使用 IMPORTANCE_HIGH 确保在所有 OEM（ColorOS/EMUI/HarmonyOS/MIUI）上显式显示
 * - 服务运行期间通知常驻通知栏，不可滑动关闭
 * - 流结束立即自毁，绝不空占
 */
class StreamKeepaliveService : Service() {

    private var isKeepaliveModeActive = false
    private var currentStreamName = ""
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: android.net.wifi.WifiManager.WifiLock? = null

    companion object {
        const val CHANNEL_ID = "vcp_stream_keepalive"
        const val NOTIFICATION_ID = 0x53545201 // "STR" + 01
        const val EXTRA_AGENT_NAME = "agent_name"
        const val EXTRA_IS_KEEPALIVE_MODE = "is_keepalive_mode"
        private const val TAG = "VcpMobileService"

        @Volatile
        var isServiceRunning = false

        @Volatile
        var isKeepaliveModeRequested = false

        @Volatile
        var isDistributedKeepaliveRequested = false

        @Volatile
        var isTemporaryWakeLockServiceActive = false

        /**
         * 构造启动该服务的 Intent
         */
        @JvmStatic
        fun createIntent(context: Context, agentName: String, isKeepaliveMode: Boolean? = null): Intent {
            return Intent(context, StreamKeepaliveService::class.java).apply {
                putExtra(EXTRA_AGENT_NAME, agentName)
                if (isKeepaliveMode != null) {
                    putExtra(EXTRA_IS_KEEPALIVE_MODE, isKeepaliveMode)
                }
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        isServiceRunning = true
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent != null) {
            if (intent.hasExtra(EXTRA_IS_KEEPALIVE_MODE)) {
                isKeepaliveModeActive = intent.getBooleanExtra(EXTRA_IS_KEEPALIVE_MODE, false)
                isKeepaliveModeRequested = isKeepaliveModeActive
            }
            if (intent.hasExtra(EXTRA_AGENT_NAME)) {
                currentStreamName = intent.getStringExtra(EXTRA_AGENT_NAME) ?: ""
            }
        }

        val shouldStop = !isKeepaliveModeActive && currentStreamName.isEmpty()
        val notification = buildNotification(currentStreamName, isKeepaliveModeActive)

        if (!promoteToForeground(notification)) {
            Log.e(TAG, "Foreground promotion failed. Stopping service to satisfy Android foreground-service contract.")
            stopSelf(startId)
            return START_NOT_STICKY
        }

        if (shouldStop) {
            Log.i(TAG, "No active streams and keepalive mode is inactive. Stopping service safely.")
            stopSelf(startId)
            return START_NOT_STICKY
        }

        Log.i(TAG, "Foreground service active: stream='$currentStreamName', keepalive=$isKeepaliveModeActive")

        if (isKeepaliveModeActive || currentStreamName.isNotEmpty()) {
            val powerManager = getSystemService(Context.POWER_SERVICE) as PowerManager
            if (wakeLock == null) {
                wakeLock = powerManager.newWakeLock(
                    PowerManager.PARTIAL_WAKE_LOCK,
                    "VcpMobile::StreamWakeLock"
                ).apply {
                    acquire() // 移除超时限制以实现真正的持续持有
                }
                Log.i(TAG, "WakeLock acquired for stream: $currentStreamName")
            }

            try {
                val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager
                if (wifiLock == null) {
                    @Suppress("DEPRECATION")
                    wifiLock = wifiManager.createWifiLock(
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q)
                            android.net.wifi.WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                        else
                            android.net.wifi.WifiManager.WIFI_MODE_FULL_HIGH_PERF,
                        "VcpMobile::StreamWifiLock"
                    )
                }
                if (wifiLock?.isHeld == false) {
                    wifiLock?.acquire()
                    Log.i(TAG, "WifiLock acquired for stream: $currentStreamName")
                }
            } catch (wifiEx: Exception) {
                Log.w(TAG, "Failed to acquire WifiLock inside service: ${wifiEx.message}")
            }
        }

        return START_STICKY
    }

    private fun promoteToForeground(notification: Notification): Boolean {
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            true
        } catch (e: Exception) {
            Log.e(TAG, "startForeground failed", e)
            false
        }
    }

    override fun onDestroy() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
            }
        }
        wakeLock = null

        wifiLock?.let {
            if (it.isHeld) {
                it.release()
            }
        }
        wifiLock = null

        isServiceRunning = false
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "后台服务增强通道",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "Agent 流式响应与后台保活"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(agentName: String, isKeepalive: Boolean): Notification {
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

        val contentText = when {
            agentName.contains("[数据同步]") -> "正在与云端服务器进行高精度同步..."
            agentName.contains("[预渲染重建]") -> "正在优化与加速本地响应缓存..."
            agentName.isNotEmpty() -> "思考中……"
            isKeepalive -> "分布式后台连接维系中..."
            else -> "已连接"
        }
        val cleanTitle = agentName.replace("[数据同步]", "").replace("[预渲染重建]", "").trim()

        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(if (cleanTitle.isEmpty()) "VCP Mobile" else cleanTitle)
            .setContentText(contentText)
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }
}
