package com.vcp.mobile.sse

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat

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

internal class SseNotifications(private val context: Context) {
    companion object {
        private const val TAG = "VcpSseProxy"
        const val CHANNEL_ID = "vcp_sse_proxy_helper"
        const val NOTIFICATION_ID_SERVICE = 0x53545202
    }

    fun createChannels() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val notificationManager = context.getSystemService(NotificationManager::class.java) ?: return

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

    fun serviceNotification(): Notification {
        // 点击通知：打开应用（通过反射获取主 Activity，避免跨包编译依赖）
        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(context, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(context.packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
        val openPendingIntent = PendingIntent.getActivity(
            context, 0, openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val builder = NotificationCompat.Builder(context, CHANNEL_ID)
            .setContentTitle("VCP 连接助手")
            .setContentText("正在后台托管 AI 对话流式连接...")
            .setSmallIcon(context.applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(
                context.applicationInfo.icon,
                "打开",
                openPendingIntent
            )

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }

    private fun isAppInForeground(): Boolean {
        val activityManager = context.getSystemService(Context.ACTIVITY_SERVICE) as? android.app.ActivityManager ?: return false
        val appProcesses = activityManager.runningAppProcesses ?: return false
        val packageName = context.packageName
        for (appProcess in appProcesses) {
            if (appProcess.importance == android.app.ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
                && appProcess.processName == packageName) {
                return true
            }
        }
        return false
    }

    fun showStreamNotification(session: StreamSession, isSuccess: Boolean, errorMsg: String?, isCurrent: () -> Boolean) {
        // 只有当主应用在后台时，才进行通知栏提醒
        if (isAppInForeground()) {
            Log.d(TAG, "App is in foreground, skipping notification.")
            return
        }

        // 检查 session 是否已被主动取消或从 activeSessions 移除
        if (!isCurrent()) {
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

        val notificationManager = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        val channelId = "vcp_agent_alerts"

        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(context, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
                putExtra("requestId", session.requestId)
                if (topicId != null) putExtra("topicId", topicId)
                if (ownerId != null) putExtra("ownerId", ownerId)
                if (ownerType != null) putExtra("ownerType", ownerType)
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(context.packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }

        val pendingIntentFlags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0

        val openPendingIntent = PendingIntent.getActivity(
            context, session.requestId.hashCode(), openIntent, pendingIntentFlags
        )

        val notification = NotificationCompat.Builder(context, channelId)
            .setContentTitle(title)
            .setContentText(contentText)
            .setSmallIcon(context.applicationInfo.icon)
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
