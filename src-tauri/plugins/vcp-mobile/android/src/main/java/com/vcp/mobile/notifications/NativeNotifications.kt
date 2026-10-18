package com.vcp.mobile.notifications

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject

/** Notification clicks retain one pending snapshot; download builders belong to this plugin instance. */
internal class NativeNotifications(
    private val activity: Activity,
    private val emitEvent: (String, JSObject) -> Unit,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    private var pendingNotificationData: JSObject? = null
    private var downloadNotificationBuilder: androidx.core.app.NotificationCompat.Builder? = null
    private val DOWNLOAD_NOTIF_ID = 0x53545209
    private val DOWNLOAD_CHANNEL_ID = "apk_download"

    fun handleNotificationIntent(intent: Intent) {
        val cliKind = intent.getStringExtra(
            com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_KIND,
        )
        val cliAction = intent.getStringExtra(
            com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_ACTION,
        )
        val cliJobId = intent.getStringExtra(
            com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_JOB_ID,
        )
        val cliAttemptId = intent.getStringExtra(
            com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_ATTEMPT_ID,
        )
        val cliGeneration = intent.getLongExtra(
            com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_RUNTIME_GENERATION,
            0L,
        )
        if (cliKind == "job" &&
            cliAction in setOf("open", "confirm_stop") &&
            !cliJobId.isNullOrBlank() && cliJobId.length <= 256 &&
            !cliAttemptId.isNullOrBlank() && cliAttemptId.length <= 256 &&
            cliGeneration > 0
        ) {
            val data = JSObject().apply {
                put("kind", "cli_job")
                put("action", cliAction)
                put("jobId", cliJobId)
                put("attemptId", cliAttemptId)
                put("runtimeGeneration", cliGeneration)
            }
            pendingNotificationData = data
            dispatchNotificationData(data)
            intent.removeExtra(com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_KIND)
            intent.removeExtra(com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_ACTION)
            intent.removeExtra(com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_JOB_ID)
            intent.removeExtra(com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_ATTEMPT_ID)
            intent.removeExtra(com.vcp.mobile.service.StreamKeepaliveService.EXTRA_CLI_RUNTIME_GENERATION)
            return
        }

        val topicId = intent.getStringExtra("topicId")
        val ownerId = intent.getStringExtra("ownerId")
        val ownerType = intent.getStringExtra("ownerType")
        val requestId = intent.getStringExtra("requestId")
        if (topicId != null && ownerId != null) {
            if (ownerType == "agent" || ownerType == "group") {
                Log.i(TAG, "[handleNotificationIntent] Found notification click: topicId=$topicId, ownerType=$ownerType, ownerId=$ownerId, requestId=$requestId")
                val data = JSObject().apply {
                    put("topicId", topicId)
                    put("ownerId", ownerId)
                    put("ownerType", ownerType)
                    put("requestId", requestId ?: "")
                }
                pendingNotificationData = data
                dispatchNotificationData(data)
            } else {
                Log.w(TAG, "[handleNotificationIntent] Dropping chat notification without ownerType")
            }

            // Consume the intent extras so they don't fire again
            intent.removeExtra("topicId")
            intent.removeExtra("ownerId")
            intent.removeExtra("ownerType")
            intent.removeExtra("requestId")
        }
    }

    private fun dispatchNotificationData(data: JSObject) {
        emitEvent("vcp-notification-click", data)
    }

    fun sendLocalNotification(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SendLocalNotificationArgs::class.java)
            val context = activity.applicationContext
            val notificationManager = context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager

            val channelId = "vcp_distributed_alert"
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                val channel = android.app.NotificationChannel(
                    channelId,
                    "VCP 分布式节点提醒",
                    android.app.NotificationManager.IMPORTANCE_HIGH
                )
                notificationManager.createNotificationChannel(channel)
            }

            val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
                .setContentTitle(args.title)
                .setContentText(args.body)
                .setSmallIcon(context.applicationInfo.icon)
                .setPriority(androidx.core.app.NotificationCompat.PRIORITY_HIGH)
                .setAutoCancel(true)
                .build()

            notificationManager.notify((System.currentTimeMillis() % 100000).toInt(), notification)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Failed to send notification")
        }
    }

    private fun createDownloadNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val name = "应用更新下载"
            val descriptionText = "显示 APK 安装包的下载进度"
            val importance = android.app.NotificationManager.IMPORTANCE_LOW
            val channel = android.app.NotificationChannel(DOWNLOAD_CHANNEL_ID, name, importance).apply {
                description = descriptionText
            }
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.createNotificationChannel(channel)
        }
    }

    fun startDownloadNotification(invoke: Invoke) {
        try {
            createDownloadNotificationChannel()
            val builder = androidx.core.app.NotificationCompat.Builder(activity, DOWNLOAD_CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setContentTitle("正在下载 VCP Mobile 更新...")
                .setContentText("已下载 0%")
                .setOngoing(true)
                .setProgress(100, 0, false)
                .setOnlyAlertOnce(true)

            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            downloadNotificationBuilder = builder
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    fun updateDownloadNotification(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(UpdateDownloadNotifArgs::class.java)
            val progress = args.progress
            val text = args.text ?: "正在下载..."

            val builder = downloadNotificationBuilder
            if (builder != null) {
                builder.setProgress(100, progress, false)
                    .setContentText(text)
                val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
                notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "updateDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    fun cancelDownloadNotification(invoke: Invoke) {
        try {
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.cancel(DOWNLOAD_NOTIF_ID)
            downloadNotificationBuilder = null
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "cancelDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    fun getPendingNotification(invoke: Invoke) {
        val data = pendingNotificationData
        if (data != null) {
            pendingNotificationData = null
            invoke.resolve(data)
        } else {
            invoke.resolve(JSObject())
        }
    }
}

@InvokeArg
class SendLocalNotificationArgs {
    lateinit var title: String
    lateinit var body: String
}

@InvokeArg
class UpdateDownloadNotifArgs {
    var progress: Int = 0
    var text: String? = null
}
