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
import androidx.core.app.NotificationCompat

/**
 * 流式响应前台保活服务
 *
 * 当 Agent 正在流式生成回复时启动，通过持续通知向系统声明"用户感知的重要任务"，
 * 显著降低进程被 OEM 杀后台的概率。
 *
 * 设计原则：极简、低打扰、零持久状态
 * - 通知仅显示 Agent 名称 + "思考中……"
 * - 无声音、无振动、不弹出 Heads-up
 * - 流结束立即自毁，绝不常驻
 */
class StreamKeepaliveService : Service() {

    companion object {
        const val CHANNEL_ID = "vcp_stream_keepalive"
        const val NOTIFICATION_ID = 0x53545201 // "STR" + 01
        const val EXTRA_AGENT_NAME = "agent_name"
        const val ACTION_STOP_STREAMING = "com.vcp.avatar.action.STOP_STREAMING"

        /**
         * 构造启动该服务的 Intent
         */
        @JvmStatic
        fun createIntent(context: Context, agentName: String): Intent {
            return Intent(context, StreamKeepaliveService::class.java).apply {
                putExtra(EXTRA_AGENT_NAME, agentName)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val agentName = intent?.getStringExtra(EXTRA_AGENT_NAME) ?: "Agent"
        val notification = buildNotification(agentName)

        // Android 14+ 必须声明前台服务类型
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "神经同步通道",
                NotificationManager.IMPORTANCE_DEFAULT
            ).apply {
                description = "Agent 流式响应保活"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(agentName: String): Notification {
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

        // 停止生成按钮：发送广播
        val stopIntent = Intent(this, StreamingActionReceiver::class.java).apply {
            action = ACTION_STOP_STREAMING
        }
        val stopPendingIntent = PendingIntent.getBroadcast(
            this, 0, stopIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(agentName)
            .setContentText("思考中……")
            .setSmallIcon(android.R.drawable.ic_menu_info_details)
            .setOngoing(true)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .setContentIntent(openPendingIntent)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "停止生成", stopPendingIntent)
            .build()
    }
}
