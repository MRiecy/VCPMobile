package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.nio.charset.StandardCharsets
import java.security.MessageDigest

internal fun cliNotificationId(target: ForegroundGuardian.CliNotificationTarget): Int {
    val digest = MessageDigest.getInstance("SHA-256").digest(
        "${target.runtimeGeneration}\u0000${target.jobId}\u0000${target.attemptId}"
            .toByteArray(StandardCharsets.UTF_8),
    )
    val suffix = ((digest[0].toInt() and 0xff) shl 16) or
        ((digest[1].toInt() and 0xff) shl 8) or
        (digest[2].toInt() and 0xff)
    return 0x43000000 or suffix
}

internal fun allocateCliNotificationId(
    target: ForegroundGuardian.CliNotificationTarget,
    occupied: Set<Int>,
): Int {
    var candidate = cliNotificationId(target)
    repeat(0x01000000) {
        if (candidate !in occupied && candidate != StreamKeepaliveService.NOTIFICATION_ID) {
            return candidate
        }
        val suffix = ((candidate and 0x00ffffff) + 1) and 0x00ffffff
        candidate = 0x43000000 or suffix
    }
    error("CLI notification id namespace is exhausted")
}

/** 前台服务常驻通知的用户可见文案。 */
internal data class NotificationCopy(val title: String, val contentText: String)

/**
 * 将 ForegroundGuardian 消费者 label 映射为通知标题与正文。
 *
 * 纯函数（无 Android 依赖），供 JVM 单测直接覆盖。规则：
 * - CLI 任务运行中时正文优先提示 CLI 状态；
 * - 已知领域标签（[数据同步] / [预渲染重建]）仅作正文分支依据，从标题中剥离；
 * - 内部标识（"distributed"、"VCP Log Linger"、任何残留的 "[...]" 域标签、空 label）
 *   一律回落为应用名 "VCP Mobile"，避免内部实现细节泄漏到通知栏；
 * - 其余 label 视为 Agent 名，直接作为标题。
 */
internal fun resolveNotificationCopy(label: String, hasCliJobs: Boolean): NotificationCopy {
    val stripped = label
        .replace("[数据同步]", "")
        .replace("[预渲染重建]", "")
        .trim()
    val isBracketTag = stripped.startsWith("[") && stripped.endsWith("]")
    val contentText = when {
        hasCliJobs -> "CLI 任务正在运行；系统仍可能中断"
        label.contains("[数据同步]") -> "正在与云端服务器进行高精度同步..."
        label.contains("[预渲染重建]") -> "正在优化与加速本地响应缓存..."
        label == "distributed" || label.contains("分布式") -> "分布式后台连接维系中..."
        // Guardian 内部 label（VCP Log Linger）与未单独列出的 "[...]" 域标签（如 [后台保活]）
        label == "VCP Log Linger" || isBracketTag -> "正在保持后台连接..."
        label.isNotEmpty() -> "思考中……"
        else -> "已连接"
    }
    val isInternalLabel = stripped.isEmpty() ||
        stripped == "distributed" ||
        stripped == "VCP Log Linger" ||
        isBracketTag
    val title = if (isInternalLabel) "VCP Mobile" else stripped
    return NotificationCopy(title, contentText)
}

/**
 * ForegroundGuardian 的唯一前台服务壳。
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

    companion object {
        const val CHANNEL_ID = "vcp_stream_keepalive"
        const val CLI_CHANNEL_ID = "vcp_cli_jobs"
        const val NOTIFICATION_ID = 0x53545201 // "STR" + 01
        const val EXTRA_GENERATION = "vcp_guardian_generation"
        const val EXTRA_CLI_KIND = "vcpCliNotificationKind"
        const val EXTRA_CLI_ACTION = "vcpCliNotificationAction"
        const val EXTRA_CLI_JOB_ID = "vcpCliJobId"
        const val EXTRA_CLI_ATTEMPT_ID = "vcpCliAttemptId"
        const val EXTRA_CLI_RUNTIME_GENERATION = "vcpCliRuntimeGeneration"
        private const val TAG = "VcpMobileService"

        @Volatile
        var isServiceRunning = false
    }

    private var latestGeneration = 0L
    private val postedCliNotifications = linkedMapOf<String, Int>()

    override fun onCreate() {
        super.onCreate()
        isServiceRunning = true
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val generation = intent?.getLongExtra(EXTRA_GENERATION, 0L) ?: 0L
        if (generation <= 0L) {
            Log.e(TAG, "Missing ForegroundGuardian generation; refusing stale start")
            stopSelfResult(startId)
            return START_NOT_STICKY
        }
        latestGeneration = generation
        val label = ForegroundGuardian.getNotificationLabel()
        val notification = buildNotification(label)

        // Android 14+ 必须声明前台服务类型，且加 try-catch 兜底，防止 ForegroundServiceStartNotAllowedException
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                val typeMask = ForegroundGuardian.foregroundServiceTypeMask()
                require(typeMask != 0) { "Foreground service has no declared active consumer type" }
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    typeMask,
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            syncCliNotifications()
            ForegroundGuardian.onServiceReady(generation)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to startForeground", e)
            ForegroundGuardian.onServiceStartFailed(applicationContext, generation, e.message ?: e.javaClass.simpleName)
            stopSelfResult(startId)
        }

        return START_NOT_STICKY
    }

    override fun onDestroy() {
        cancelCliNotifications()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        
        ForegroundGuardian.onServiceDestroyed(applicationContext, latestGeneration)

        isServiceRunning = false
        super.onDestroy()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        ForegroundGuardian.releaseNonCliConsumers(applicationContext)
        if (!ForegroundGuardian.isActive) stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "后台连接保持",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "Agent 流式响应与后台保活"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(channel)

            val cliChannel = NotificationChannel(
                CLI_CHANNEL_ID,
                "VCP CLI 任务",
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = "用户可见的本地 CLI 异步任务与精确停止入口"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(cliChannel)
        }
    }

    private fun buildNotification(label: String): Notification {
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

        val copy = resolveNotificationCopy(
            label,
            ForegroundGuardian.activeCliTargets().isNotEmpty(),
        )

        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(copy.title)
            .setContentText(copy.contentText)
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }

    private fun syncCliNotifications() {
        val manager = getSystemService(NotificationManager::class.java) ?: return
        val activeKeys = mutableSetOf<String>()
        ForegroundGuardian.activeCliTargets().forEach { (_, target) ->
            val key = "${target.runtimeGeneration}:${target.jobId}:${target.attemptId}"
            activeKeys += key
            val id = postedCliNotifications.getOrPut(key) {
                allocateCliNotificationId(target, postedCliNotifications.values.toSet())
            }
            manager.notify(id, buildCliJobNotification(target, id))
        }
        postedCliNotifications.entries.removeAll { (key, id) ->
            if (key in activeKeys) {
                false
            } else {
                manager.cancel(id)
                true
            }
        }
    }

    private fun cancelCliNotifications() {
        val manager = getSystemService(NotificationManager::class.java)
        postedCliNotifications.values.forEach { id -> manager?.cancel(id) }
        postedCliNotifications.clear()
    }

    private fun buildCliJobNotification(
        target: ForegroundGuardian.CliNotificationTarget,
        notificationId: Int,
    ): Notification {
        val open = cliActivityIntent(target, "open")
        val confirmStop = cliActivityIntent(target, "confirm_stop")
        val openPending = PendingIntent.getActivity(
            this,
            notificationId,
            open,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stopPending = PendingIntent.getActivity(
            this,
            notificationId xor 0x01000000,
            confirmStop,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CLI_CHANNEL_ID)
            .setContentTitle("VCP CLI · 任务运行中")
            .setContentText("${target.displayLabel.take(80)} · 后台增强，系统仍可能中断")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPending)
            .addAction(0, "停止…", stopPending)
            .build()
    }

    private fun cliActivityIntent(
        target: ForegroundGuardian.CliNotificationTarget,
        action: String,
    ): Intent {
        val intent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(this, mainActivityClass)
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
        return intent.apply {
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            putExtra(EXTRA_CLI_KIND, "job")
            putExtra(EXTRA_CLI_ACTION, action)
            putExtra(EXTRA_CLI_JOB_ID, target.jobId)
            putExtra(EXTRA_CLI_ATTEMPT_ID, target.attemptId)
            putExtra(EXTRA_CLI_RUNTIME_GENERATION, target.runtimeGeneration)
        }
    }
}
