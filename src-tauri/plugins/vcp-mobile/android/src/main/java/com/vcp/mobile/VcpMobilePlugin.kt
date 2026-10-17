package com.vcp.mobile

import android.app.Activity
import android.content.ClipData
import android.content.Context
import android.content.IntentFilter
import android.content.res.Configuration
import android.os.Build
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.TauriPlugin
import androidx.activity.result.ActivityResult
import app.tauri.plugin.Plugin
import android.content.Intent
import android.content.ComponentName
import android.util.Log
import androidx.core.content.FileProvider
import android.webkit.MimeTypeMap
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.Message
import android.os.PowerManager
import android.net.Uri
import android.provider.Settings
import android.content.pm.PackageManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import app.tauri.plugin.JSArray
import app.tauri.plugin.Invoke
import app.tauri.plugin.Channel
import com.vcp.mobile.cli.CancelCliProcessArgs
import com.vcp.mobile.cli.CloseCliPtyArgs
import com.vcp.mobile.cli.CliPtyHostOwner
import com.vcp.mobile.cli.CliProcessHostOwner
import com.vcp.mobile.cli.InspectCliProcessArgs
import com.vcp.mobile.cli.OpenCliPtyArgs
import com.vcp.mobile.cli.PrepareCliRuntimeArgs
import com.vcp.mobile.cli.ReadCliPtyArgs
import com.vcp.mobile.cli.ResizeCliPtyArgs
import com.vcp.mobile.cli.StartCliProcessArgs
import com.vcp.mobile.cli.WriteCliPtyArgs
import com.vcp.mobile.service.StreamKeepaliveService
import android.graphics.Bitmap
import android.graphics.Canvas
import android.content.ContentValues
import android.provider.MediaStore
import android.os.Environment
import android.media.MediaScannerConnection
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLDecoder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import com.topjohnwu.superuser.Shell
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/** A fixed buffer also bounds copies from files whose size changes after inspection. */
internal fun copyExportStream(input: InputStream, output: OutputStream, maxBytes: Long): Long {
    require(maxBytes > 0) { "文件大小限制无效" }
    val buffer = ByteArray(64 * 1024)
    var total = 0L
    while (true) {
        if (Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("文件保存已停止")
        val count = input.read(buffer)
        if (count == -1) break
        if (count == 0) continue
        if (count.toLong() > maxBytes - total) throw IllegalArgumentException("文件大小超出允许范围")
        output.write(buffer, 0, count)
        total += count
    }
    require(total > 0) { "写入内容为空" }
    return total
}

/** The reserved destination belongs to this write; a failure only removes that file. */
internal fun writeLegacyExportFile(
    directory: java.io.File,
    displayName: String,
    write: (OutputStream) -> Long,
): Pair<java.io.File, Long> {
    check(directory.isDirectory || directory.mkdirs()) { "无法创建导出目录" }
    val base = displayName.substringBeforeLast('.', displayName)
    val ext = displayName.substringAfterLast('.', "")
    var outputFile = java.io.File(directory, displayName)
    var index = 1
    while (!outputFile.createNewFile()) {
        outputFile = java.io.File(directory, if (ext.isBlank()) "${base}_$index" else "${base}_$index.$ext")
        index += 1
    }
    try {
        val size = outputFile.outputStream().use(write)
        return outputFile to size
    } catch (error: Throwable) {
        try {
            if (!outputFile.delete() && outputFile.exists()) {
                error.addSuppressed(java.io.IOException("无法删除未完成导出: ${outputFile.name}"))
            }
        } catch (cleanupError: Throwable) {
            error.addSuppressed(cleanupError)
        }
        throw error
    }
}

/** Main-thread owner for the one downloads export waiting on legacy storage permission. */
internal class DownloadsWritePermissionWait {
    private var pending: Invoke? = null

    fun request(invoke: Invoke, launch: () -> Unit) {
        if (pending != null) {
            invoke.reject("已有下载导出正在等待储存空间权限")
            return
        }
        pending = invoke
        try {
            launch()
        } catch (error: Exception) {
            if (take(invoke)) invoke.reject("无法申请下载目录写入权限: ${error.message}")
        }
    }

    fun complete(invoke: Invoke, granted: Boolean, save: () -> Unit) {
        if (!take(invoke)) return
        if (granted) {
            // Ownership transfers to the file executor; destroy must not release its source early.
            save()
        } else {
            invoke.reject("保存到下载目录需要储存空间权限")
        }
    }

    fun destroy() {
        val invoke = pending ?: return
        pending = null
        invoke.reject("saveFileToDownloads unavailable: plugin was destroyed")
    }

    private fun take(invoke: Invoke): Boolean {
        if (pending !== invoke) return false
        pending = null
        return true
    }
}

internal class PluginExecutorDomains(
    fileThreadCount: Int = 1,
    rootQueueCapacity: Int = 16,
    fileQueueCapacity: Int = 64,
) {
    private val threadSequence = AtomicInteger(0)
    private fun threadFactory(prefix: String) = java.util.concurrent.ThreadFactory { runnable ->
        Thread(runnable, "$prefix-${threadSequence.incrementAndGet()}")
    }

    val oomScheduler = Executors.newSingleThreadScheduledExecutor(threadFactory("vcp-oom-guard"))
    val rootExecutor: ExecutorService = ThreadPoolExecutor(
        1,
        1,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(rootQueueCapacity),
        threadFactory("vcp-root"),
        ThreadPoolExecutor.AbortPolicy(),
    )
    val fileIoExecutor: ExecutorService = ThreadPoolExecutor(
        fileThreadCount,
        fileThreadCount,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(fileQueueCapacity),
        threadFactory("vcp-file-io"),
        ThreadPoolExecutor.AbortPolicy(),
    )

    fun shutdownNow(): List<Runnable> {
        oomScheduler.shutdownNow()
        return rootExecutor.shutdownNow() + fileIoExecutor.shutdownNow()
    }
}

object ScreenKeepOnArbiter {
    private val stateLock = Any()
    private var activityRef: java.lang.ref.WeakReference<Activity>? = null
    private var manualRequested = false
    private var guardianRequested = false
    private var appInForeground = true

    @JvmStatic
    fun setManualRequested(activity: Activity, requested: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            manualRequested = requested
        }
        apply(activity)
    }

    fun setGuardianRequested(activity: Activity, requested: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            guardianRequested = requested
        }
        apply(activity)
    }

    fun setAppInForeground(activity: Activity, foreground: Boolean) {
        synchronized(stateLock) {
            activityRef = java.lang.ref.WeakReference(activity)
            appInForeground = foreground
        }
        apply(activity)
    }

    fun detach(activity: Activity) {
        val ownsActivity = synchronized(stateLock) {
            if (activityRef?.get() !== activity) {
                false
            } else {
                activityRef?.clear()
                activityRef = null
                manualRequested = false
                appInForeground = false
                true
            }
        }
        if (ownsActivity) {
            activity.runOnUiThread {
                activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }

    private fun apply(activity: Activity) {
        activity.runOnUiThread {
            val shouldKeepScreenOn = synchronized(stateLock) {
                if (activityRef?.get() !== activity) {
                    null
                } else {
                    appInForeground && (manualRequested || guardianRequested)
                }
            } ?: return@runOnUiThread

            if (shouldKeepScreenOn) {
                activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }
}

internal fun pickerUploadStagingStem(stagingTicket: String, hash: String): String =
    "picked_${stagingTicket}_$hash"

internal fun pickerUploadStagingName(
    stagingTicket: String,
    hash: String,
    fileExtension: String,
): String = "${pickerUploadStagingStem(stagingTicket, hash)}$fileExtension"

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE"], alias = "storageLegacy"),
    Permission(strings = ["android.permission.RECORD_AUDIO"], alias = "microphone"),
    Permission(strings = ["android.permission.CAMERA"], alias = "camera"),
    Permission(strings = ["android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION"], alias = "location")
])
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private class PendingInvokeTask(
        private val invoke: Invoke,
        private val operation: String,
        private val task: () -> Unit,
        private val reject: (String) -> Unit = { invoke.reject(it) },
    ) : Runnable {
        private val claimed = AtomicBoolean(false)

        override fun run() {
            if (!claimed.compareAndSet(false, true)) return
            try {
                task()
            } catch (error: Throwable) {
                Log.e(TAG, "$operation failed", error)
                reject("$operation failed: ${error.message ?: error.javaClass.simpleName}")
            }
        }

        fun rejectBeforeStart(reason: String) {
            if (claimed.compareAndSet(false, true)) {
                reject("$operation unavailable: $reason")
            }
        }
    }

    private val activityLifecycleCallbacks = object : android.app.Application.ActivityLifecycleCallbacks {
        override fun onActivityPrePaused(a: Activity) {
            if (a === activity) webViewResume.onPausing()
        }
        override fun onActivityResumed(a: Activity) {
            if (a === activity) {
                ScreenKeepOnArbiter.setAppInForeground(activity, true)
                webViewResume.onResumed()
            }
        }
        override fun onActivityPaused(a: Activity) {
            if (a === activity) {
                // On API 26–28 this runs inside Activity.super.onPause(), before
                // the locked WryActivity calls WebView.onPause().
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) webViewResume.onPausing()
                ScreenKeepOnArbiter.setAppInForeground(activity, false)
            }
        }
        override fun onActivityCreated(a: Activity, savedInstanceState: android.os.Bundle?) {}
        override fun onActivityStarted(a: Activity) {
            if (a === activity) webViewResume.onStarted()
        }
        override fun onActivityStopped(a: Activity) {
            if (a === activity) webViewResume.onStopped()
        }
        override fun onActivitySaveInstanceState(a: Activity, outState: android.os.Bundle) {}
        override fun onActivityDestroyed(a: Activity) {
            if (a === activity) webViewResume.destroy()
        }
    }

    companion object {
        const val TAG = "VcpMobilePlugin"
        // 下载目录导出上限：设置备份（JSON + base64 头像）正常远低于此值
        private const val MAX_DOWNLOAD_EXPORT_BYTES = 8 * 1024 * 1024
        private const val MAX_GALLERY_EXPORT_BYTES = 50 * 1024 * 1024
        private var instanceRef: java.lang.ref.WeakReference<VcpMobilePlugin>? = null

        fun getInstance(): VcpMobilePlugin? {
            return instanceRef?.get()
        }
    }

    val pluginActivity: Activity get() = activity
    var webViewRef: WebView? = null
    private var pendingNotificationData: JSObject? = null

    private fun handleNotificationIntent(intent: Intent) {
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
        val webView = webViewRef
        if (webView != null) {
            val dataJson = data.toString()
            val safeJson = escapeJsonForJsString(dataJson)
            val script = "window.dispatchEvent(new CustomEvent('vcp-notification-click', { detail: JSON.parse(\"$safeJson\") }))"
            activity.runOnUiThread {
                webView.evaluateJavascript(script, null)
            }
        } else {
            Log.w(TAG, "[handleNotificationIntent] WebView not ready, caching notification data")
        }
    }
    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val webViewResume = NativeWebViewResume(activity)
    private val lifecycleBridge = LifecycleBridge()
    private val batteryStatusManager = BatteryStatusManager(activity)
    private val networkStatusManager = NetworkStatusManager(activity)
    private val cpuStatusManager = CpuStatusManager(activity)
    private val gpuStatusManager = GpuStatusManager(activity)
    private val floatingWindowManager by lazy { FloatingWindowManager(activity) }
    private val sensorStatusManager = SensorStatusManager(activity)
    private val executorDomains = PluginExecutorDomains()
    private val cliProcessHost = CliProcessHostOwner.get(activity.applicationContext)
    private val cliPtyHost = CliPtyHostOwner.get(cliProcessHost)
    private val shareIntentHandler = ShareIntentHandler(this, executorDomains.fileIoExecutor)
    private val isDestroying = AtomicBoolean(false)
    private val downloadsWritePermissionWait = DownloadsWritePermissionWait()
    @Volatile private var oomGuardFuture: ScheduledFuture<*>? = null
    private var cameraTempFile: java.io.File? = null
    private var networkCallback: android.net.ConnectivityManager.NetworkCallback? = null
    private var lastConnected: Boolean? = null
    private var lastDefaultNetwork: android.net.Network? = null
    private var isNetworkMonitoringStarted = false
    private var nativeEventChannel: Channel? = null

    // ==================================================================
    // SSE Proxy Service Binder & IPC (Messenger)
    // ==================================================================
    // ==================================================================
    // SSE Proxy Service Lifecycle
    // ==================================================================
    private fun startHelperServiceInternal() {
        val intent = Intent(activity, com.vcp.mobile.service.SseProxyService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            activity.startForegroundService(intent)
        } else {
            activity.startService(intent)
        }
        Log.i(TAG, "SseProxyService start initiated on demand.")
    }

    private fun applyGuardianScreenState(required: Boolean) {
        ScreenKeepOnArbiter.setGuardianRequested(activity, required)
    }

    private fun resolveWhenGuardianReady(invoke: Invoke, generation: Long) {
        com.vcp.mobile.service.ForegroundGuardian.awaitServiceReadiness(
            activity,
            generation,
        ) { ready, message ->
            if (ready) {
                invoke.resolve()
            } else {
                invoke.reject(message ?: "Foreground service failed to become ready")
            }
        }
    }

    init {
        instanceRef = java.lang.ref.WeakReference(this)
        activity.application.registerActivityLifecycleCallbacks(activityLifecycleCallbacks)
        ScreenKeepOnArbiter.setAppInForeground(activity, true)
        com.vcp.mobile.service.ForegroundGuardian.setScreenStateListener(::applyGuardianScreenState)
        startOomScoreGuard()
    }


    // ==================================================================
    // Permissions & App Control
    // ==================================================================
    @Command
    fun checkAllPermissions(invoke: Invoke) {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager

        val notificationGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }

        val storageGranted = if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else if (Build.VERSION.SDK_INT >= 29) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        val locationGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

        val am = activity.getSystemService(Context.ACTIVITY_SERVICE) as? android.app.ActivityManager
        val isRestricted = if (Build.VERSION.SDK_INT >= 28) {
            am?.isBackgroundRestricted ?: false
        } else {
            false
        }
        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName) && !isRestricted
        val result = JSObject()
        result.put("notification", notificationGranted)
        result.put("storage", storageGranted)
        result.put("microphone", microphoneGranted)
        result.put("camera", cameraGranted)
        result.put("location", locationGranted)
        result.put("battery", batteryOptimizationIgnored)
        
        invoke.resolve(result)
    }

    @Command
    fun requestAndroidPermission(invoke: Invoke) {
        val args = invoke.parseArgs(RequestPermissionArgs::class.java)
        when (args.type) {
            "notification" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("notification", invoke, "onPermissionResult")
                } else {
                    emitPermissionsToWebView()
                    invoke.resolve()
                }
            }
            "storage" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("storage", invoke, "onPermissionResult")
                } else {
                    requestPermissionForAlias("storageLegacy", invoke, "onPermissionResult")
                }
            }
            "microphone" -> {
                requestPermissionForAlias("microphone", invoke, "onPermissionResult")
            }
            "camera" -> {
                requestPermissionForAlias("camera", invoke, "onPermissionResult")
            }
            "location" -> {
                requestPermissionForAlias("location", invoke, "onPermissionResult")
            }
            "battery" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    }
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                } catch (e: Exception) {
                    val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                }
            }
        }
    }

    @Command
    fun moveTaskToBack(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
    }

    @Command
    fun check_notification_listener_permission(invoke: Invoke) {
        val context = activity.applicationContext
        val pkgName = context.packageName
        val flat = Settings.Secure.getString(context.contentResolver, "enabled_notification_listeners")
        var isEnabled = false
        if (!flat.isNullOrEmpty()) {
            val names = flat.split(":")
            for (name in names) {
                val cn = ComponentName.unflattenFromString(name)
                if (cn != null && cn.packageName == pkgName) {
                    isEnabled = true
                    break
                }
            }
        }
        val ret = JSObject()
        ret.put("enabled", isEnabled)
        invoke.resolve(ret)
    }

    @Command
    fun request_notification_listener_permission(invoke: Invoke) {
        try {
            val intent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            activity.startActivity(intent)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("Failed to open notification listener settings: ${e.message}")
        }
    }

    private fun startOomScoreGuard() {
        oomGuardFuture = executorDomains.oomScheduler.schedule({
            if (isDestroying.get()) return@schedule
            try {
                if (!Shell.getShell().isRoot) {
                    Log.i(TAG, "OomScoreGuard disabled: root access is unavailable.")
                    return@schedule
                }

                val pid = android.os.Process.myPid()
                Shell.cmd("echo -900 > /proc/$pid/oom_score_adj").exec()
                if (!isDestroying.get()) {
                    oomGuardFuture = executorDomains.oomScheduler.scheduleWithFixedDelay({
                        if (isDestroying.get()) return@scheduleWithFixedDelay
                        try {
                            Shell.cmd("echo -900 > /proc/$pid/oom_score_adj").exec()
                        } catch (e: Exception) {
                            Log.e(TAG, "OomScoreGuard refresh failed", e)
                        }
                    }, 20L, 20L, TimeUnit.SECONDS)
                }
            } catch (e: Exception) {
                Log.e(TAG, "OomScoreGuard initialization failed", e)
            }
        }, 0L, TimeUnit.MILLISECONDS)
    }

    private fun executePluginTask(
        executor: ExecutorService,
        invoke: Invoke,
        operation: String,
        task: () -> Unit,
    ) {
        val pendingTask = PendingInvokeTask(invoke, operation, task)
        if (isDestroying.get()) {
            pendingTask.rejectBeforeStart("plugin is shutting down")
            return
        }
        try {
            executor.execute(pendingTask)
        } catch (_: RejectedExecutionException) {
            pendingTask.rejectBeforeStart("executor queue is full or shutting down")
        }
    }

    private fun checkAutoStartStatus(): String {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi") || 
            manufacturer.contains("vivo") || manufacturer.contains("meizu")) {
            val ops = activity.getSystemService(Context.APP_OPS_SERVICE) as? android.app.AppOpsManager
            if (ops != null) {
                try {
                    val method = ops.javaClass.getMethod(
                        "checkOpNoThrow",
                        Int::class.javaPrimitiveType,
                        Int::class.javaPrimitiveType,
                        String::class.java
                    )
                    // 10008 is OP_AUTO_START in MIUI / HyperOS / Flyme / OriginOS
                    val mode = method.invoke(
                        ops,
                        10008,
                        activity.applicationInfo.uid,
                        activity.packageName
                    ) as Int
                    return if (mode == android.app.AppOpsManager.MODE_ALLOWED) "true" else "false"
                } catch (e: Exception) {
                    Log.e(TAG, "checkAutoStartStatus: reflection failed", e)
                }
            }
        }
        return "unsupported"
    }

    @Command
    fun checkAutoStartPermission(invoke: Invoke) {
        val status = checkAutoStartStatus()
        val result = JSObject()
        result.put("status", status)
        invoke.resolve(result)
    }

    @Command
    fun requestAutoStartPermission(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        var success = false
        val intents = mutableListOf<Intent>()
        
        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi")) {
            // 小米 / HyperOS
            intents.add(Intent().setComponent(ComponentName("com.miui.securitycenter", "com.miui.permcenter.autostart.AutoStartManagementActivity")))
        } else if (manufacturer.contains("huawei") || manufacturer.contains("honor")) {
            // 华为 / 荣耀
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity")))
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.bootstart.BootStartActivity")))
        } else if (manufacturer.contains("oppo") || manufacturer.contains("oneplus") || manufacturer.contains("realme")) {
            // OPPO / 一加 / 真我
            // 针对自启动跳错，我们优先拉起系统应用管理主页，或直接拉起应用详情页，保障在 OPPO/ColorOS 上的准确性
            intents.add(Intent(Settings.ACTION_MANAGE_APPLICATIONS_SETTINGS))
        } else if (manufacturer.contains("vivo")) {
            // VIVO
            intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.phoneoptimize.BgStartUpManager")))
            intents.add(Intent().setComponent(ComponentName("com.vivo.permissionmanager", "com.vivo.permissionmanager.activity.BgStartUpManagerActivity")))
            intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.MainActivity")))
        } else if (manufacturer.contains("meizu")) {
            // 魅族
            intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.permission.SmartBGActivity")))
            intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.MainActivity")))
        }

        // 尝试打开厂商特定的 Activity
        for (intent in intents) {
            try {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                success = true
                break
            } catch (e: Exception) {
                // Try next
            }
        }
        
        // 兜底退避
        if (!success) {
            try {
                val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = Uri.parse("package:${activity.packageName}")
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                success = true
            } catch (e: Exception) {
                try {
                    val intent = Intent(Settings.ACTION_SETTINGS).apply {
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    activity.startActivity(intent)
                    success = true
                } catch (e2: Exception) {}
            }
        }
        
        val result = JSObject()
        result.put("success", success)
        invoke.resolve(result)
    }

    @Command
    fun requestPowerManagementPermission(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        var success = false
        val intents = mutableListOf<Intent>()
        
        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi")) {
            // 小米省电策略
            try {
                val miuiIntent = Intent("miui.intent.action.OP_POWER_PRIORITY_SETTINGS").apply {
                    putExtra("package_name", activity.packageName)
                    putExtra("package_label", activity.applicationInfo.loadLabel(activity.packageManager).toString())
                }
                intents.add(miuiIntent)
            } catch (e: Exception) {}
            intents.add(Intent().setComponent(ComponentName("com.miui.powerkeeper", "com.miui.powerkeeper.ui.HiddenAppsConfigActivity")).apply {
                putExtra("package_name", activity.packageName)
                putExtra("package_label", activity.applicationInfo.loadLabel(activity.packageManager).toString())
            })
            intents.add(Intent().setComponent(ComponentName("com.miui.securitycenter", "com.miui.powercenter.PowerSettings")))
        } else if (manufacturer.contains("oppo") || manufacturer.contains("oneplus") || manufacturer.contains("realme")) {
            // OPPO 省电与后台完全行为
            intents.add(Intent().setComponent(ComponentName("com.coloros.oppoguardelf", "com.coloros.powermanager.fuelgaurd.PowerUsageModelActivity")))
            intents.add(Intent().setComponent(ComponentName("com.coloros.oppoguardelf", "com.coloros.powermanager.fuelgaurd.PowerSavedModeActivity")))
            try {
                intents.add(Intent(Intent.ACTION_POWER_USAGE_SUMMARY))
            } catch (e: Exception) {}
        } else if (manufacturer.contains("huawei") || manufacturer.contains("honor")) {
            // 华为
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.power.ui.PowerConsumptionActivity")))
            intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.process.ProtectActivity")))
        } else if (manufacturer.contains("vivo")) {
            // VIVO
            intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.poweroptimize.PowerOptimizeActivity")))
        }

        // 尝试打开特定的电池设置页面
        for (intent in intents) {
            try {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                success = true
                break
            } catch (e: Exception) {
                // Try next
            }
        }
        
        // 兜底退避
        if (!success) {
            try {
                val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                success = true
            } catch (e: Exception) {
                try {
                    val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    activity.startActivity(intent)
                    success = true
                } catch (e2: Exception) {}
            }
        }
        
        val result = JSObject()
        result.put("success", success)
        invoke.resolve(result)
    }

    @Command
    fun getFreeDiskSpace(invoke: Invoke) {
        try {
            val path = Environment.getDataDirectory()
            val stat = android.os.StatFs(path.path)
            val blockSize = stat.blockSizeLong
            val availableBlocks = stat.availableBlocksLong
            val totalBlocks = stat.blockCountLong
            
            val freeBytes = availableBlocks * blockSize
            val totalBytes = totalBlocks * blockSize
            
            val freeGB = freeBytes.toDouble() / (1024.0 * 1024.0 * 1024.0)
            val totalGB = totalBytes.toDouble() / (1024.0 * 1024.0 * 1024.0)
            
            val result = JSObject()
            result.put("freeBytes", freeBytes.toDouble())
            result.put("freeGb", freeGB)
            result.put("totalBytes", totalBytes.toDouble())
            result.put("totalGb", totalGB)
            invoke.resolve(result)
        } catch (e: Exception) {
            Log.e(TAG, "getFreeDiskSpace failed", e)
            invoke.reject(e.message ?: "Failed to get free disk space")
        }
    }

    // ==================================================================
    // Permission Result Callbacks
    // ==================================================================
    @PermissionCallback
    fun onPermissionResult(invoke: Invoke) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    @ActivityCallback
    fun onBatteryOptimizationResult(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    private fun emitPermissionsToWebView() {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager

        val notificationGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }

        val storageGranted = if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else if (Build.VERSION.SDK_INT >= 29) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        val locationGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

        val am = activity.getSystemService(Context.ACTIVITY_SERVICE) as? android.app.ActivityManager
        val isRestricted = if (Build.VERSION.SDK_INT >= 28) {
            am?.isBackgroundRestricted ?: false
        } else {
            false
        }
        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName) && !isRestricted
        val json = """{"notification":$notificationGranted,"storage":$storageGranted,"microphone":$microphoneGranted,"camera":$cameraGranted,"battery":$batteryOptimizationIgnored,"location":$locationGranted}"""
        val script = "window.dispatchEvent(new CustomEvent('vcp-permission-change', { detail: $json }))"
        webViewRef?.evaluateJavascript(script, null)
    }

    @Command
    fun requestOverlayPermission(invoke: Invoke) {
        floatingWindowManager.requestOverlayPermission()
        invoke.resolve()
    }

    @Command
    fun toggleFloatingBall(invoke: Invoke) {
        val args = invoke.parseArgs(ToggleFloatingBallArgs::class.java)
        val success = floatingWindowManager.toggleFloatingBall(args.show)
        val result = JSObject()
        result.put("success", success)
        invoke.resolve(result)
    }

    // ==================================================================
    // Screen
    // ==================================================================
    @Command
    fun setKeepScreenOn(invoke: Invoke) {
        ScreenKeepOnArbiter.setManualRequested(activity, true)
        invoke.resolve()
    }

    @Command
    fun clearKeepScreenOn(invoke: Invoke) {
        ScreenKeepOnArbiter.setManualRequested(activity, false)
        invoke.resolve()
    }

    @Command
    fun getBatteryStatus(invoke: Invoke) {
        try {
            val status = batteryStatusManager.getStatusJson()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "getBatteryStatus failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun getNetworkStatus(invoke: Invoke) {
        try {
            val status = networkStatusManager.getNetworkStatus()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "getNetworkStatus failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun getCpuThermalStatus(invoke: Invoke) {
        try {
            val status = cpuStatusManager.getThermalStatus()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "getCpuThermalStatus failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun getGpuStatus(invoke: Invoke) {
        try {
            val status = gpuStatusManager.getGpuStatusJson()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "getGpuStatus failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun checkRootAccess(invoke: Invoke) {
        executePluginTask(executorDomains.rootExecutor, invoke, "checkRootAccess") {
            try {
                val isRoot = Shell.getShell().isRoot
                val result = JSObject()
                result.put("isRoot", isRoot)
                invoke.resolve(result)
            } catch (e: Exception) {
                val result = JSObject()
                result.put("isRoot", false)
                invoke.resolve(result)
            }
        }
    }

    @Command
    fun writeClipboard(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(WriteClipboardArgs::class.java)
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clip = android.content.ClipData.newPlainText("VCP Distributed Copy", args.content)
                    clipboard.setPrimaryClip(clip)
                    invoke.resolve()
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Failed to write clipboard on UI thread")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Failed to parse arguments")
        }
    }

    @Command
    fun readClipboard(invoke: Invoke) {
        try {
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clipData = clipboard.primaryClip
                    val content = if (clipData != null && clipData.itemCount > 0) {
                        clipData.getItemAt(0).text?.toString() ?: ""
                    } else {
                        ""
                    }
                    val result = JSObject().apply {
                        put("content", content)
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Failed to read clipboard on UI thread")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Failed to execute readClipboard")
        }
    }

    @Command
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

    @Command
    fun runRootCommand(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(RunRootCommandArgs::class.java)
            executePluginTask(executorDomains.rootExecutor, invoke, "runRootCommand") {
                try {
                    val output = Shell.cmd(args.command).exec().out
                    val result = JSObject().apply {
                        put("success", true)
                        put("output", output.joinToString("\n"))
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    val result = JSObject().apply {
                        put("success", false)
                        put("output", e.message ?: "Unknown Shell execution error")
                    }
                    invoke.resolve(result)
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Args parsing error")
        }
    }

    @Command
    fun launchRootManager(invoke: Invoke) {
        try {
            val managers = listOf(
                "com.topjohnwu.magisk" to "Magisk",
                "me.weishu.kernelsu" to "KernelSU",
                "me.tool.apatch" to "APatch"
            )
            var launched = false
            for ((pkg, name) in managers) {
                try {
                    val intent = activity.packageManager.getLaunchIntentForPackage(pkg)
                    if (intent != null) {
                        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                        activity.startActivity(intent)
                        launched = true
                        val result = JSObject().apply {
                            put("success", true)
                            put("manager", name)
                        }
                        invoke.resolve(result)
                        break
                    }
                } catch (e: Exception) {
                    // Continue checking next package
                }
            }
            if (!launched) {
                val result = JSObject().apply {
                    put("success", false)
                    put("message", "未找到支持的 Root 管理器 (Magisk, KernelSU, APatch)。")
                }
                invoke.resolve(result)
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "启动 Root 管理器失败")
        }
    }

    // ==================================================================
    // Foreground Guardian & Stream Service
    // ==================================================================
    @Command
    fun acquireForeground(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(AcquireForegroundArgs::class.java)
            val generation = com.vcp.mobile.service.ForegroundGuardian.acquire(
                activity, args.tag, args.priority, args.label, args.screenKeepOn,
            )
            resolveWhenGuardianReady(invoke, generation)
        } catch (e: Exception) {
            Log.e(TAG, "acquireForeground failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun releaseForeground(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(ReleaseForegroundArgs::class.java)
            com.vcp.mobile.service.ForegroundGuardian.release(activity, args.tag)
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "releaseForeground failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun acquireWakeLock(invoke: Invoke) {
        try {
            val generation = com.vcp.mobile.service.ForegroundGuardian.acquire(
                activity, "manual_keepalive", 
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_DISTRIBUTED, 
                "[后台保活]"
            )
            resolveWhenGuardianReady(invoke, generation)
        } catch (e: Exception) {
            Log.e(TAG, "acquireWakeLock failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun releaseWakeLock(invoke: Invoke) {
        try {
            com.vcp.mobile.service.ForegroundGuardian.release(activity, "manual_keepalive")
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "releaseWakeLock failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun sampleSensorData(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SampleSensorDataArgs::class.java)
            activity.runOnUiThread {
                try {
                    sensorStatusManager.sample(
                        location = args.location,
                        motion = args.motion,
                        ambient = args.ambient,
                        completion = invoke::resolve,
                    )
                } catch (error: Exception) {
                    Log.e(TAG, "sampleSensorData failed on main thread", error)
                    invoke.reject(error.message ?: "Unknown error")
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "sampleSensorData failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    // ==================================================================
    // Plugin Lifecycle
    // ==================================================================

    /** 原生 Channel 直达 Rust；主线程串行订阅、快照和后续事件。 */
    internal fun emitNativeEvent(event: String, payload: JSObject) {
        activity.runOnUiThread {
            if (isDestroying.get()) return@runOnUiThread
            nativeEventChannel?.send(JSObject().apply {
                put("event", event)
                put("payload", payload)
            })
        }
    }

    @Command
    fun registerNativeEvents(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(RegisterNativeEventsArgs::class.java)
            activity.runOnUiThread {
                if (isDestroying.get()) {
                    invoke.reject("plugin was destroyed")
                    return@runOnUiThread
                }
                try {
                    // 每次注册替换唯一订阅者；快照与实时事件不经过 WebView JS。
                    nativeEventChannel = args.channel
                    emitNativeEvent("lifecycle", lifecycleBridge.snapshot())
                    startNetworkMonitoringInternal()
                    emitNetworkStatus(force = true)
                    invoke.resolve()
                } catch (error: Exception) {
                    Log.e(TAG, "Failed to register native events", error)
                    invoke.reject(error.message ?: "Failed to register native events")
                }
            }
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Invalid native event registration")
        }
    }

    private fun emitNetworkStatus(force: Boolean = false) {
        if (isDestroying.get()) return
        val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val network = cm.activeNetwork
        val status = networkStatusManager.getNetworkStatus()
        val connected = status.optBoolean("connected", false)
        if (force || connected != lastConnected || network != lastDefaultNetwork) {
            lastConnected = connected
            lastDefaultNetwork = network
            emitNativeEvent("network", status)
        }
    }

    private fun startNetworkMonitoringInternal() {
        if (isNetworkMonitoringStarted) return
        val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
        val callback = object : android.net.ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: android.net.Network) {
                emitNetworkStatus()
            }
            override fun onLost(network: android.net.Network) {
                emitNetworkStatus()
            }
            override fun onCapabilitiesChanged(network: android.net.Network, networkCapabilities: android.net.NetworkCapabilities) {
                emitNetworkStatus()
            }
        }
        cm.registerDefaultNetworkCallback(callback, Handler(Looper.getMainLooper()))
        networkCallback = callback
        isNetworkMonitoringStarted = true
        Log.i(TAG, "[Network] Native default network monitoring started.")
    }

    @Command
    fun startNetworkMonitoring(invoke: Invoke) {
        activity.runOnUiThread {
            if (isDestroying.get()) {
                invoke.reject("plugin was destroyed")
                return@runOnUiThread
            }
            try {
                startNetworkMonitoringInternal()
                invoke.resolve()
            } catch (error: Exception) {
                Log.e(TAG, "Failed to register network callback", error)
                invoke.reject(error.message ?: "Failed to register network callback")
            }
        }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        webViewRef = webView
        webViewResume.attach(webView)

        keyboardInsetsManager.attach(webView)
        lifecycleBridge.attach(activity, this)

        // 冷启动：处理传递给 Activity 的初始 intent
        shareIntentHandler.handleShareIntent(activity.intent)
        shareIntentHandler.injectShareData(webView)
        handleNotificationIntent(activity.intent)
    }

    @Command
    fun connectWebViewResume(invoke: Invoke) {
        val args = try { invoke.parseArgs(ConnectWebViewResumeArgs::class.java) }
        catch (error: Exception) { invoke.reject("Invalid resume connection: ${error.message}"); return }
        if (args.documentId.isBlank()) { invoke.reject("Resume document identity is required"); return }
        activity.runOnUiThread {
            val token = webViewResume.connect(args.documentId)
            invoke.resolve(JSObject().apply { put("token", token ?: org.json.JSONObject.NULL) })
        }
    }

    @Command
    fun completeWebViewResume(invoke: Invoke) {
        val args = try { invoke.parseArgs(CompleteWebViewResumeArgs::class.java) }
        catch (error: Exception) { invoke.reject("Invalid resume completion: ${error.message}"); return }
        if (args.documentId.isBlank() || args.token.isBlank()) {
            invoke.reject("Resume document and token are required")
            return
        }
        activity.runOnUiThread {
            webViewResume.complete(args.documentId, args.token)
            // Renderer/copy callbacks are owned by NativeWebViewResume, not this IPC.
            invoke.resolve()
        }
    }

    @Command
    fun prepareCliRuntime(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(PrepareCliRuntimeArgs::class.java)
            cliProcessHost.prepare(
                args,
                success = { result -> invoke.resolve(result) },
                failure = { reason -> invoke.reject("prepareCliRuntime failed: $reason") },
            )
        } catch (error: Exception) {
            invoke.reject("prepareCliRuntime failed: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    @Command
    fun startCliProcess(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StartCliProcessArgs::class.java)
            cliProcessHost.start(
                args,
                success = { result -> invoke.resolve(result) },
                failure = { reason -> invoke.reject("startCliProcess failed: $reason") },
            )
        } catch (error: Exception) {
            invoke.reject("startCliProcess failed: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    @Command
    fun inspectCliProcess(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(InspectCliProcessArgs::class.java)
            cliProcessHost.inspect(
                args,
                success = { result -> invoke.resolve(result) },
                failure = { reason -> invoke.reject("inspectCliProcess failed: $reason") },
            )
        } catch (error: Exception) {
            invoke.reject("inspectCliProcess failed: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    @Command
    fun cancelCliProcess(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(CancelCliProcessArgs::class.java)
            cliProcessHost.cancel(
                args,
                success = { result -> invoke.resolve(result) },
                failure = { reason -> invoke.reject("cancelCliProcess failed: $reason") },
            )
        } catch (error: Exception) {
            invoke.reject("cancelCliProcess failed: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    @Command
    fun openCliPty(invoke: Invoke) {
        val args = invoke.parseArgs(OpenCliPtyArgs::class.java)
        cliPtyHost.open(args, invoke::resolve) { invoke.reject("openCliPty failed: $it") }
    }

    @Command
    fun readCliPty(invoke: Invoke) {
        val args = invoke.parseArgs(ReadCliPtyArgs::class.java)
        cliPtyHost.read(args, invoke::resolve) { invoke.reject("readCliPty failed: $it") }
    }

    @Command
    fun writeCliPty(invoke: Invoke) {
        val args = invoke.parseArgs(WriteCliPtyArgs::class.java)
        cliPtyHost.write(args, invoke::resolve) { invoke.reject("writeCliPty failed: $it") }
    }

    @Command
    fun resizeCliPty(invoke: Invoke) {
        val args = invoke.parseArgs(ResizeCliPtyArgs::class.java)
        cliPtyHost.resize(args, invoke::resolve) { invoke.reject("resizeCliPty failed: $it") }
    }

    @Command
    fun closeCliPty(invoke: Invoke) {
        val args = invoke.parseArgs(CloseCliPtyArgs::class.java)
        cliPtyHost.close(args, invoke::resolve) { invoke.reject("closeCliPty failed: $it") }
    }

    override fun onDestroy(activity: AppCompatActivity) {
        isDestroying.set(true)
        webViewResume.destroy()
        fileOperation.get()?.let { it.cancelled.set(true); it.reject("cancelled: plugin destroyed") }
        activity.runOnUiThread { downloadsWritePermissionWait.destroy() }
        nativeEventChannel = null
        oomGuardFuture?.cancel(true)
        oomGuardFuture = null
        sensorStatusManager.shutdown()
        executorDomains.shutdownNow().forEach { pendingTask ->
            (pendingTask as? PendingInvokeTask)?.rejectBeforeStart("plugin was destroyed")
        }
        activity.application.unregisterActivityLifecycleCallbacks(activityLifecycleCallbacks)
        com.vcp.mobile.service.ForegroundGuardian.setScreenStateListener(null)
        ScreenKeepOnArbiter.detach(activity)
        keyboardInsetsManager.detach()
        webViewRef = null
        lifecycleBridge.detach()
        try {
            if (networkCallback != null) {
                val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
                cm.unregisterNetworkCallback(networkCallback!!)
                networkCallback = null
                isNetworkMonitoringStarted = false
            }
        } catch (_: Exception) {}
        try {
            // Locks are managed by StreamKeepaliveService
        } catch (_: Exception) {}
        super.onDestroy(activity)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        lifecycleBridge.onConfigurationChanged(newConfig)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        shareIntentHandler.handleShareIntent(intent)
        handleNotificationIntent(intent)
    }

    // ==================================================================
    // Scoped Storage File Picker & Native Thumbnail Generation (Scheme B)
    // ==================================================================
    private class FileOperation(val args: PickFileArgs, val invoke: Invoke) {
        val cancelled = AtomicBoolean(false)
        val settled = AtomicBoolean(false)
        fun check() { if (cancelled.get() || Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("cancelled: file operation") }
        fun reject(message: String) { if (settled.compareAndSet(false, true)) invoke.reject(message) }
        fun resolve(value: JSObject) { if (settled.compareAndSet(false, true)) invoke.resolve(value) }
    }
    private val fileOperation = java.util.concurrent.atomic.AtomicReference<FileOperation?>(null)
    private val nativeOutgoingLock = Any()

    @Command
    fun cancelFileOperation(invoke: Invoke) {
        val args = invoke.parseArgs(CancelFileOperationArgs::class.java)
        fileOperation.get()?.takeIf { it.args.operationId == args.operationId }?.let {
            it.cancelled.set(true)
            // Android cannot close another application's chooser. Retain this
            // slot until its callback, but settle the user wait exactly once.
            it.reject("cancelled: file operation")
        }
        invoke.resolve()
    }

    @PermissionCallback
    fun onCameraPermissionResult(invoke: Invoke) {
        val operation = fileOperation.get() ?: return
        if (operation.cancelled.get()) { finishFileOperation(operation); return }
        if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCameraIntent(invoke)
        } else {
            operation.reject("camera_permission: Camera permission denied")
            finishFileOperation(operation)
        }
    }

    private fun finishFileOperation(operation: FileOperation) {
        fileOperation.compareAndSet(operation, null)
    }

    private fun launchCameraIntent(invoke: Invoke) {
        val operation = fileOperation.get() ?: return
        try {
            operation.check()
            val uploads = java.io.File(operation.args.stagingDir).apply { mkdirs() }
            val file = java.io.File(uploads, "${operation.args.operationId}-${java.util.UUID.randomUUID()}-camera.jpg")
            cameraTempFile = file
            val uri = try {
                FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
            } catch (_: Exception) {
                FileProvider.getUriForFile(activity, "${activity.packageName}.opener.fileprovider", file)
            }
            val intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE).apply {
                putExtra(MediaStore.EXTRA_OUTPUT, uri)
                addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            }
            startActivityForResult(invoke, intent, "onCameraResult")
        } catch (error: Throwable) {
            cameraTempFile?.delete(); cameraTempFile = null
            operation.reject("camera_capture: ${error.message}")
            finishFileOperation(operation)
        }
    }

    @Command
    fun pickFile(invoke: Invoke) {
        val args = invoke.parseArgs(PickFileArgs::class.java)
        val operation = FileOperation(args, invoke)
        if (!fileOperation.compareAndSet(null, operation)) { invoke.reject("busy: native picker is still active"); return }
        try {
            require(args.operationId.matches(Regex("[A-Za-z0-9_-]{1,100}"))) { "invalid_operation_id" }
            require(args.maxBytes in 1..(512L * 1024 * 1024)) { "invalid_file_budget" }
            val expected = java.io.File(activity.cacheDir, "uploads").canonicalFile
            require(java.io.File(args.stagingDir).canonicalFile == expected) { "invalid_staging_directory" }
            when (args.mode) {
                "camera" -> {
                    if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                        requestPermissionForAlias("camera", invoke, "onCameraPermissionResult")
                    } else { launchCameraIntent(invoke) }
                }
                else -> {
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                        type = if (args.mode == "gallery" || args.mode == "avatar") "image/*" else "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                        putExtra(Intent.EXTRA_ALLOW_MULTIPLE, args.selectionLimit > 1)
                    }
                    startActivityForResult(invoke, intent, "onPickFileResult")
                }
            }
        } catch (error: Throwable) {
            operation.reject("native_picker: ${error.message}")
            finishFileOperation(operation)
        }
    }

    @ActivityCallback
    fun onCameraResult(invoke: Invoke, result: ActivityResult) {
        val operation = fileOperation.get() ?: return
        val file = cameraTempFile
        cameraTempFile = null
        if (result.resultCode != Activity.RESULT_OK || operation.cancelled.get()) {
            file?.delete(); operation.reject("cancelled: camera"); finishFileOperation(operation); return
        }
        if (file == null || !file.isFile) { operation.reject("camera_capture: no output file"); finishFileOperation(operation); return }
        acquirePickedFiles(operation, listOf(Uri.fromFile(file)), "Camera.jpg", "image/jpeg", file)
    }

    @ActivityCallback
    fun onPickFileResult(invoke: Invoke, result: ActivityResult) {
        val operation = fileOperation.get() ?: return
        if (result.resultCode != Activity.RESULT_OK || operation.cancelled.get()) {
            operation.reject("cancelled: file picker"); finishFileOperation(operation); return
        }
        val data = result.data
        val uris = linkedSetOf<Uri>()
        val clip = data?.clipData
        if (clip != null) {
            for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let { uris.add(it) }
        }
        if (uris.isEmpty()) data?.data?.let { uris.add(it) }
        if (uris.isEmpty()) { operation.reject("cancelled: no file selected"); finishFileOperation(operation); return }
        acquirePickedFiles(operation, uris.take(operation.args.selectionLimit), null, null, null,
            (uris.size - operation.args.selectionLimit).coerceAtLeast(0))
    }

    private fun acquirePickedFiles(operation: FileOperation, uris: List<Uri>, suppliedName: String?, suppliedMime: String?, ownedSource: java.io.File?, overflow: Int = 0) {
        val task = PendingInvokeTask(operation.invoke, "acquirePickedFiles", task = {
            val files = mutableListOf<JSObject>()
            val errors = org.json.JSONArray()
            val owned = mutableListOf<java.io.File>()
            var delivered = false
            var remainingBytes = operation.args.maxBytes
            try {
                if (overflow > 0) errors.put(JSObject().apply {
                    put("name", "其余 $overflow 项"); put("message", "每次最多选择 ${operation.args.selectionLimit} 个附件")
                })
                for ((index, uri) in uris.withIndex()) {
                    var staging: java.io.File? = null
                    var derivative: java.io.File? = null
                    var name = suppliedName ?: "文件 ${index + 1}"
                    try {
                        operation.check()
                        val resolver = activity.contentResolver
                        if (suppliedName == null) resolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                            if (it.moveToFirst()) name = it.getString(0) ?: "file"
                        }
                        name = java.io.File(name).name.take(150).ifBlank { "file" }
                        val mime = suppliedMime ?: resolver.getType(uri) ?: "application/octet-stream"
                        val declaredSize = if (ownedSource != null) ownedSource.length() else {
                            var declared = 0L
                            resolver.query(uri, arrayOf(android.provider.OpenableColumns.SIZE), null, null, null)?.use {
                                if (it.moveToFirst() && !it.isNull(0)) declared = it.getLong(0).coerceAtLeast(0L)
                            }
                            declared
                        }
                        emitFileProgress(operation, "vcp-mobile-file-start", JSObject().apply {
                            put("name", name); put("mime", mime); put("size", declaredSize)
                            put("selectionIndex", index); put("selectionCount", uris.size)
                        })
                        val uploads = java.io.File(operation.args.stagingDir).apply { mkdirs() }
                        val extension = java.io.File(name).extension.filter { it in 'A'..'Z' || it in 'a'..'z' || it in '0'..'9' }.take(12)
                        val target = java.io.File(uploads, "${operation.args.operationId}-${java.util.UUID.randomUUID()}" + if (extension.isEmpty()) "" else ".$extension")
                        staging = target
                        val digest = java.security.MessageDigest.getInstance("SHA-256")
                        var size = 0L
                        var lastProgress = 0L
                        resolver.openInputStream(uri).use { input ->
                            requireNotNull(input) { "file_provider: could not open input" }
                            target.outputStream().use { output ->
                                val buffer = ByteArray(64 * 1024)
                                while (true) {
                                    operation.check()
                                    val count = input.read(buffer)
                                    if (count < 0) break
                                    require(count.toLong() <= remainingBytes - size) { "file_too_large: selection exceeds byte budget" }
                                    output.write(buffer, 0, count); digest.update(buffer, 0, count); size += count
                                    val now = android.os.SystemClock.elapsedRealtime()
                                    if (now - lastProgress >= 200) {
                                        lastProgress = now
                                        emitFileProgress(operation, "vcp-mobile-file-progress", JSObject().apply {
                                            put("name", name); put("mime", mime); put("loaded", size); put("total", declaredSize)
                                            put("selectionIndex", index); put("selectionCount", uris.size)
                                            put("progress", if (declaredSize > 0) (size.toDouble() / declaredSize * 100).coerceAtMost(100.0) else 0)
                                        })
                                    }
                                }
                                output.fd.sync()
                            }
                        }
                        operation.check()
                        val hash = digest.digest().joinToString("") { "%02x".format(it) }
                        val value = if (operation.args.mode == "avatar") {
                            val image = NativeFileImages.derive(DeriveImageArgs().apply {
                                sourcePath = target.absolutePath; outputPath = target.absolutePath + "-avatar"
                                maxDimension = 2048; maxPixels = 4_194_304; maxBytes = 20 * 1024 * 1024; preserveAlpha = true
                            })
                            val output = java.io.File(image.getString("path") ?: error("image output path missing"))
                            derivative = output
                            JSObject().apply {
                                put("path", output.absolutePath); put("name", "avatar_work." + output.extension)
                                put("mime", image.getString("mime")); put("size", output.length()); put("hash", "")
                                put("thumbnailPath", org.json.JSONObject.NULL)
                            }
                        } else JSObject().apply {
                            put("path", target.absolutePath); put("name", name); put("mime", mime)
                            put("size", size); put("hash", hash); put("thumbnailPath", org.json.JSONObject.NULL)
                        }
                        operation.check()
                        files.add(value)
                        remainingBytes -= size
                        if (derivative != null) {
                            owned.add(derivative)
                            derivative = null
                        } else {
                            owned.add(target)
                            staging = null
                        }
                    } catch (error: Throwable) {
                        operation.check()
                        if (operation.args.selectionLimit == 1) throw error
                        errors.put(JSObject().apply { put("name", name); put("message", "file_acquisition: ${error.message}") })
                    } finally {
                        staging?.delete(); derivative?.delete()
                    }
                }
                operation.check()
                val value = if (operation.args.selectionLimit > 1) JSObject().apply {
                    put("files", org.json.JSONArray(files)); put("errors", errors)
                } else files.single()
                // Cancellation wins ownership until the single terminal response.
                if (operation.settled.compareAndSet(false, true)) {
                    operation.invoke.resolve(value)
                    delivered = true
                }
            } catch (error: Throwable) {
                operation.reject(if (operation.cancelled.get()) "cancelled: file acquisition" else "file_acquisition: ${error.message}")
            } finally {
                if (!delivered) owned.forEach { it.delete() }
                ownedSource?.delete()
                finishFileOperation(operation)
            }
        }, reject = {
            ownedSource?.delete()
            operation.reject(it)
            finishFileOperation(operation)
        })
        if (isDestroying.get()) { task.rejectBeforeStart("plugin is shutting down"); return }
        try { executorDomains.fileIoExecutor.execute(task) }
        catch (_: RejectedExecutionException) { task.rejectBeforeStart("executor queue is full or shutting down") }
    }

    @Command
    fun deriveImage(invoke: Invoke) {
        executePluginTask(executorDomains.fileIoExecutor, invoke, "deriveImage") {
            try {
                val args = invoke.parseArgs(DeriveImageArgs::class.java)
                require(isSafeLocalPath(activity, args.sourcePath) && isSafeLocalPath(activity, args.outputPath)) { "image_path: outside application storage" }
                invoke.resolve(NativeFileImages.derive(args))
            } catch (error: Throwable) { invoke.reject("image_derivative: ${error.message}") }
        }
    }

    @Command
    fun shareInboxOwner(invoke: Invoke) {
        invoke.resolve(JSObject().apply { put("deliveryId", shareIntentHandler.currentOwnerId()) })
    }

    private fun emitFileProgress(operation: FileOperation, event: String, detail: JSObject) {
        detail.put("operationId", operation.args.operationId)
        val json = escapeJsonForJsString(detail.toString())
        activity.runOnUiThread {
            if (fileOperation.get() === operation && !operation.cancelled.get() && !operation.settled.get()) {
                webViewRef?.evaluateJavascript("window.dispatchEvent(new CustomEvent('$event', {detail:JSON.parse(\"$json\")}))", null)
            }
        }
    }

    private fun escapeJsonForJsString(json: String): String {
        return json
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\'", "\\'")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
    }

    // ==================================================================
    // External Share File Processor (no chooser, processes cached file)
    // ==================================================================
    @Suppress("ASSIGNED_VALUE_IS_NEVER_READ")
    @Command
    fun processSharedFile(invoke: Invoke) {
        val args = invoke.parseArgs(ProcessSharedFileArgs::class.java)
        val cachePath = args.cachePath
        val rawMimeType = args.mimeType
        val originalName = sanitizeSharedFileName(args.fileName)
        val ownerId = args.ownerId
        val stagingTicket = args.stagingTicket

        if (cachePath.isEmpty() || ownerId.isEmpty() || stagingTicket.isEmpty()) {
            invoke.reject("分享 staging 参数不完整")
            return
        }
        if (runCatching { java.util.UUID.fromString(ownerId) }.isFailure ||
            runCatching { java.util.UUID.fromString(stagingTicket) }.isFailure) {
            invoke.reject("分享 staging owner/ticket 格式非法")
            return
        }

        executePluginTask(executorDomains.fileIoExecutor, invoke, "processSharedFile", fileTask@{
            var currentTempFile: java.io.File? = null
            var claimedSourceFile: java.io.File? = null
            try {
                if (!shareIntentHandler.isCurrentOwner(ownerId)) {
                    invoke.reject("分享 intent 已被更新")
                    return@fileTask
                }
                val context = activity
                val sharedRoot = java.io.File(context.cacheDir, "shared").apply { mkdirs() }.canonicalFile
                val sourceFile = java.io.File(cachePath).canonicalFile
                val expectedName = "shared_${ownerId}_${stagingTicket}_$originalName"
                if (sourceFile.parentFile != sharedRoot || sourceFile.name != expectedName || !sourceFile.isFile) {
                    invoke.reject("拒绝处理不属于当前 intent ticket 的分享 staging 文件")
                    return@fileTask
                }
                if (!shareIntentHandler.claimStagedFile(ownerId, sourceFile)) {
                    invoke.reject("分享 staging ticket 已失效或已消费")
                    return@fileTask
                }
                claimedSourceFile = sourceFile
                if (!sourceFile.exists()) {
                    invoke.reject("Shared file not found at cache path: $cachePath")
                    return@fileTask
                }

                val size = sourceFile.length()
                if (size > 512L * 1024 * 1024) {
                    invoke.reject("分享文件超过单文件上限 (512 MiB)")
                    return@fileTask
                }
                var mimeType = rawMimeType
                if (mimeType.isNullOrBlank()) {
                    val ext = sourceFile.extension.lowercase()
                    mimeType = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
                }

                Log.i(TAG, "[processSharedFile] Processing shared file: size=$size, mime=$mimeType")

                // 直接在受控 staging 文件上流式重算 SHA-256，避免 shared → uploads 双倍占盘。
                val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
                val digest = java.security.MessageDigest.getInstance("SHA-256")

                sourceFile.inputStream().use { inputStream ->
                    val buffer = ByteArray(65536)
                    var bytesRead = inputStream.read(buffer)
                    while (bytesRead != -1) {
                        if (!shareIntentHandler.isCurrentOwner(ownerId)) {
                            throw InterruptedException("分享 intent 已被更新")
                        }
                        digest.update(buffer, 0, bytesRead)
                        bytesRead = inputStream.read(buffer)
                    }
                }

                val hashBytes = digest.digest()
                val hash = hashBytes.joinToString("") { "%02x".format(it) }

                // uploads 只是交给 Rust ingest 的临时交接区。永久 CAS 去重由 Rust 负责；
                // owner/ticket 保证同内容的多个分享消费者不会争用同一个临时文件。
                val fileExtension = java.io.File(originalName).extension.let {
                    if (it.isEmpty()) "" else ".$it"
                }
                val finalTempFile = java.io.File(
                    uploadsDir,
                    shareUploadStagingName(ownerId, stagingTicket, hash, fileExtension),
                )

                if (finalTempFile.exists()) {
                    // 目标异常占用时不能删除原始 share staging，也不能把未知旧内容返回给 Rust。
                    claimedSourceFile = null
                    throw IllegalStateException("分享 upload staging ticket 已存在")
                }
                currentTempFile = finalTempFile
                if (!sourceFile.renameTo(finalTempFile)) {
                    if (uploadsDir.usableSpace < size + 64L * 1024 * 1024) {
                        throw IllegalStateException("uploads staging 可用空间不足")
                    }
                    sourceFile.copyTo(finalTempFile, overwrite = false)
                    if (!sourceFile.delete()) {
                        throw IllegalStateException("无法清理已复制的分享 source staging")
                    }
                }
                claimedSourceFile = null

                // Rust registers the original and requests the shared bounded
                // image service for derivatives; acquisition owns no thumbnail.

                if (!shareIntentHandler.isCurrentOwner(ownerId)) {
                    throw InterruptedException("分享 intent 已被更新")
                }

                // 组装结果
                val resultObject = JSObject()
                resultObject.put("path", finalTempFile.absolutePath)
                resultObject.put("name", originalName)
                resultObject.put("mime", mimeType)
                resultObject.put("size", finalTempFile.length())
                resultObject.put("hash", hash)

                Log.i(TAG, "[processSharedFile] Complete: size=${finalTempFile.length()}, mime=$mimeType")
                invoke.resolve(resultObject)
                currentTempFile = null
            } catch (e: Throwable) {
                Log.e(TAG, "[processSharedFile] Failed", e)
                try {
                    currentTempFile?.delete()
                    claimedSourceFile?.delete()
                } catch (_: Exception) {}
                invoke.reject("Processing shared file failed: ${e.message}")
            }
        })
    }

    @Command
    fun openFile(invoke: Invoke) {
        val args = invoke.parseArgs(OpenFileArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.reject("Path is empty")
            return
        }
        if (args.action != "view" && args.action != "share") {
            invoke.reject("Unsupported native file action")
            return
        }
        
        executePluginTask(executorDomains.fileIoExecutor, invoke, "openFile", fileTask@{
            try {
                val context = activity

                // 💥 安全边界拦截：禁止通过 openFile 访问沙箱外部物理文件
                if (!isSafeLocalPath(context, path)) {
                    invoke.reject("安全拒绝：禁止打开沙箱外部的敏感文件")
                    return@fileTask
                }

                val sourceFile = java.io.File(path)
                if (!sourceFile.exists()) {
                    invoke.reject("文件不存在: $path")
                    return@fileTask
                }

                // ACTION_VIEW/SEND has no trustworthy receiver-finished callback.
                // Give FileProvider its own copy, retaining it for a day; callers
                // may release their temporary source as soon as the chooser opens.
                val file = synchronized(nativeOutgoingLock) {
                    val exportRoot = java.io.File(context.cacheDir, "native-outgoing").apply { mkdirs() }
                    val cutoff = System.currentTimeMillis() - TimeUnit.DAYS.toMillis(1)
                    exportRoot.listFiles()?.filter { it.isDirectory && it.lastModified() < cutoff }?.forEach { it.deleteRecursively() }
                    val retainedBytes = exportRoot.walkTopDown().filter { it.isFile }.sumOf { it.length() }
                    val availableBytes = minOf(512L * 1024 * 1024, 1024L * 1024 * 1024 - retainedBytes)
                    require(availableBytes > 0 && sourceFile.length() <= availableBytes) { "分享暂存空间已满，请稍后重试" }
                    val exportDir = java.io.File(exportRoot, java.util.UUID.randomUUID().toString()).apply { mkdirs() }
                    val displayName = args.fileName?.let { java.io.File(it).name }?.takeIf { it.isNotBlank() && it != "." && it != ".." } ?: sourceFile.name
                    val file = java.io.File(exportDir, displayName)
                    try {
                        sourceFile.inputStream().use { input -> file.outputStream().use { output -> copyExportStream(input, output, availableBytes) } }
                    } catch (error: Throwable) { exportDir.deleteRecursively(); throw error }
                    file
                }

                // 1. 自动提取并修正 MIME 类型
                val ext = file.extension.lowercase()
                val mimeType = when (ext) {
                    "log", "txt" -> "text/plain"
                    else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
                }
                Log.i(TAG, "[openFile] Dispatching file action=${args.action}: ${file.absolutePath} (ext=$ext, mime=$mimeType)")

                // 2. 借助 FileProvider 生成临时读取授权的 content:// URI
                val uri = try {
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.fileprovider",
                        file
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "[openFile] Fallback to opener FileProvider authority", e)
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.opener.fileprovider",
                        file
                    )
                }

                // 3. 查看沿用 ACTION_VIEW；分享使用带临时 URI 权限的 ACTION_SEND。
                val intent = if (args.action == "share") {
                    val sendIntent = Intent(Intent.ACTION_SEND).apply {
                        type = mimeType
                        putExtra(Intent.EXTRA_STREAM, uri)
                        putExtra(Intent.EXTRA_TITLE, file.name)
                        clipData = ClipData.newRawUri(file.name, uri)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                    Intent.createChooser(sendIntent, "分享文件").apply {
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                } else {
                    Intent(Intent.ACTION_VIEW).apply {
                        setDataAndType(uri, mimeType)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                }

                context.startActivity(intent)
                invoke.resolve()
            } catch (e: android.content.ActivityNotFoundException) {
                val ext = java.io.File(path).extension.lowercase()
                Log.e(TAG, "[openFile] No activity found for action=${args.action}, type=.$ext", e)
                invoke.reject(if (args.action == "share") {
                    "您的手机上未安装可接收此文件的应用"
                } else {
                    "您的手机上未安装能打开此类文件 (.$ext) 的应用，请先安装相关阅读器 (如 WPS Office)。"
                })
            } catch (e: Throwable) {
                Log.e(TAG, "[openFile] Native file action=${args.action} failed", e)
                invoke.reject(if (args.action == "share") {
                    "分享文件失败: ${e.message}"
                } else {
                    "打开文件失败: ${e.message}"
                })
            }
        })
    }

    // ==================================================================
    // OTA Update Support (签名连续性 / 安装权限 / 下载保活)
    // ==================================================================

    @Command
    fun verifyApkSignature(invoke: Invoke) {
        val args = invoke.parseArgs(VerifyApkSignatureArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.reject("Path is empty")
            return
        }
        executePluginTask(executorDomains.fileIoExecutor, invoke, "verifyApkSignature", task@{
            try {
                if (!isSafeLocalPath(activity, path)) {
                    invoke.reject("安全拒绝：禁止校验沙箱外部的文件")
                    return@task
                }
                val file = java.io.File(path)
                if (!file.exists() || !file.isFile) {
                    invoke.reject("APK 文件不存在: $path")
                    return@task
                }
                val apkSha256 = ApkSignatureInspector.apkSignatureSha256(activity, path)
                val selfSha256 = ApkSignatureInspector.selfSignatureSha256(activity)
                val result = JSObject().apply {
                    put("apkSha256", apkSha256)
                    put("selfSha256", selfSha256)
                    put("matched", ApkSignatureInspector.digestsMatch(apkSha256, selfSha256))
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "[verifyApkSignature] failed", e)
                invoke.reject("签名校验失败: ${e.message}")
            }
        })
    }

    @Command
    fun canInstallPackages(invoke: Invoke) {
        try {
            val allowed = activity.packageManager.canRequestPackageInstalls()
            invoke.resolve(JSObject().apply { put("allowed", allowed) })
        } catch (e: Throwable) {
            Log.e(TAG, "[canInstallPackages] failed", e)
            invoke.reject("检查安装权限失败: ${e.message}")
        }
    }

    @Command
    fun openUnknownSourcesSettings(invoke: Invoke) {
        try {
            val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES).apply {
                data = Uri.parse("package:${activity.packageName}")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            activity.startActivity(intent)
            invoke.resolve()
        } catch (e: Throwable) {
            Log.e(TAG, "[openUnknownSourcesSettings] failed", e)
            invoke.reject("打开安装权限设置失败: ${e.message}")
        }
    }

    @Command
    fun acquireOtaKeepalive(invoke: Invoke) {
        try {
            val generation = com.vcp.mobile.service.ForegroundGuardian.acquire(
                activity,
                "ota",
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_OTA,
                "[正在下载更新]",
                false,
            )
            resolveWhenGuardianReady(invoke, generation)
        } catch (e: Exception) {
            Log.e(TAG, "acquireOtaKeepalive failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun releaseOtaKeepalive(invoke: Invoke) {
        try {
            com.vcp.mobile.service.ForegroundGuardian.release(activity, "ota")
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "releaseOtaKeepalive failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    // ==================================================================
    // Security Sandbox Boundary & Verification
    // ==================================================================
    private fun isSafeLocalPath(context: Context, path: String): Boolean {
        return try {
            val file = java.io.File(path).canonicalFile
            val cacheDir = context.cacheDir.canonicalFile
            val filesDir = context.filesDir.canonicalFile
            val externalFilesDir = context.getExternalFilesDir(null)?.canonicalFile
            val externalCacheDir = context.externalCacheDir?.canonicalFile

            file.path.startsWith(cacheDir.path + java.io.File.separator) ||
            file.path.startsWith(filesDir.path + java.io.File.separator) ||
            (externalFilesDir != null && file.path.startsWith(externalFilesDir.path + java.io.File.separator)) ||
            (externalCacheDir != null && file.path.startsWith(externalCacheDir.path + java.io.File.separator))
        } catch (e: Exception) {
            false
        }
    }

    // ==================================================================
    // Universal Media Exporter & Gallery Writer
    // ==================================================================
    @Command
    fun saveImageToGallery(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageArgs::class.java)
        if (args.sourceUrl.isBlank()) {
            invoke.reject("图片地址为空")
            return
        }

        executePluginTask(executorDomains.fileIoExecutor, invoke, "saveImageToGallery", fileTask@{
            try {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到相册需要储存空间权限")
                        return@fileTask
                    }
                }

                invoke.resolve(saveImageSource(args.sourceUrl, args.fileName))
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageToGallery failed", e)
                invoke.reject("保存图片失败: ${e.message}")
            }
        })
    }

    @Command
    fun saveImageFromPath(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageFromPathArgs::class.java)
        if (args.imagePath.isBlank()) {
            invoke.reject("物理文件路径为空")
            return
        }

        executePluginTask(executorDomains.fileIoExecutor, invoke, "saveImageFromPath", fileTask@{
            try {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到相册需要储存空间权限")
                        return@fileTask
                    }
                }

                val file = checkedExportFile(args.imagePath, Int.MAX_VALUE.toLong())
                file.inputStream().use { input ->
                    invoke.resolve(saveImageStream(input, args.fileName, file.name, file.name, true, Int.MAX_VALUE.toLong()))
                }
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageFromPath failed", e)
                invoke.reject("保存图片失败: ${e.message}")
            }
        })
    }

    // ==================================================================
    // Downloads Exporter (设置备份等任意小文件写入公共下载目录)
    // ==================================================================
    @Command
    fun saveToDownloads(invoke: Invoke) {
        val args = invoke.parseArgs(SaveToDownloadsArgs::class.java)
        if (args.fileName.isBlank()) {
            invoke.reject("文件名为空")
            return
        }
        if (args.contentBase64.isBlank()) {
            invoke.reject("写入内容为空")
            return
        }
        if (args.contentBase64.length.toLong() > ((MAX_DOWNLOAD_EXPORT_BYTES.toLong() + 2) / 3) * 4) {
            invoke.reject("文件大小超出允许范围 (最大 ${MAX_DOWNLOAD_EXPORT_BYTES / 1024 / 1024}MB)")
            return
        }

        executePluginTask(executorDomains.fileIoExecutor, invoke, "saveToDownloads", fileTask@{
            try {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到下载目录需要储存空间权限")
                        return@fileTask
                    }
                }

                val bytes = try {
                    android.util.Base64.decode(args.contentBase64, android.util.Base64.DEFAULT)
                } catch (e: IllegalArgumentException) {
                    invoke.reject("内容不是合法的 Base64 数据")
                    return@fileTask
                }
                if (bytes.isEmpty() || bytes.size > MAX_DOWNLOAD_EXPORT_BYTES) {
                    invoke.reject("文件大小超出允许范围 (最大 ${MAX_DOWNLOAD_EXPORT_BYTES / 1024 / 1024}MB)")
                    return@fileTask
                }

                val displayName = args.fileName
                    .replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_")
                    .trim()
                if (displayName.isBlank()) {
                    invoke.reject("文件名非法")
                    return@fileTask
                }
                val mimeType = args.mimeType?.takeIf { it.isNotBlank() } ?: "application/octet-stream"

                val saved = bytes.inputStream().use {
                    writeFileToDownloads(it, displayName, mimeType, MAX_DOWNLOAD_EXPORT_BYTES.toLong())
                }
                val result = JSObject().apply {
                    put("uri", saved.uri.toString())
                    put("displayName", saved.displayName)
                    put("mimeType", mimeType)
                    put("size", saved.size)
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "saveToDownloads failed", e)
                invoke.reject("保存到下载目录失败: ${e.message}")
            }
        })
    }

    @Command
    fun saveFileToDownloads(invoke: Invoke) {
        if (isDestroying.get()) {
            invoke.reject("plugin is shutting down")
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q &&
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            activity.runOnUiThread {
                if (isDestroying.get()) {
                    invoke.reject("plugin is shutting down")
                } else {
                    downloadsWritePermissionWait.request(invoke) {
                        requestPermissionForAlias("storageLegacy", invoke, "onDownloadsWritePermissionResult")
                    }
                }
            }
            return
        }
        val args = try {
            invoke.parseArgs(SaveFileToDownloadsArgs::class.java)
        } catch (error: Exception) {
            invoke.reject("下载导出参数无效: ${error.message}")
            return
        }
        executePluginTask(executorDomains.fileIoExecutor, invoke, "saveFileToDownloads", {
            try {
                require(args.maxBytes in 1..Int.MAX_VALUE.toLong()) { "文件大小限制无效" }
                val file = checkedExportFile(args.filePath, args.maxBytes)
                val displayName = args.fileName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_").trim()
                require(displayName.isNotBlank() && displayName != "." && displayName != "..") { "文件名非法" }
                val mimeType = args.mimeType?.takeIf { it.isNotBlank() } ?: "application/octet-stream"
                val saved = file.inputStream().use { writeFileToDownloads(it, displayName, mimeType, args.maxBytes) }
                invoke.resolve(JSObject().apply {
                    put("uri", saved.uri.toString())
                    put("displayName", saved.displayName)
                    put("mimeType", mimeType)
                    put("size", saved.size)
                })
            } catch (error: Throwable) {
                Log.e(TAG, "saveFileToDownloads failed", error)
                invoke.reject("保存到下载目录失败: ${error.message}")
            }
        })
    }

    @PermissionCallback
    fun onDownloadsWritePermissionResult(invoke: Invoke) {
        activity.runOnUiThread {
            val granted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
            downloadsWritePermissionWait.complete(invoke, granted) { saveFileToDownloads(invoke) }
        }
    }

    private fun checkedExportFile(path: String, maxBytes: Long): java.io.File {
        require(path.isNotBlank() && isSafeLocalPath(activity, path)) { "禁止读取应用目录外的文件" }
        val file = java.io.File(path).canonicalFile
        require(file.isFile) { "本地文件不存在" }
        require(file.length() in 1..maxBytes) { "文件大小超出允许范围" }
        return file
    }

    private data class SavedPublicFile(val uri: Uri, val displayName: String, val size: Long)

    private fun writeFileToDownloads(input: InputStream, displayName: String, mimeType: String, maxBytes: Long): SavedPublicFile {
        val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) MediaStore.Downloads.EXTERNAL_CONTENT_URI else null
        return writePublicExport(displayName, mimeType, Environment.DIRECTORY_DOWNLOADS, collection) {
            copyExportStream(input, it, maxBytes)
        }
    }

    private data class LoadedImage(val bytes: ByteArray, val mimeType: String)

    private fun saveImageSource(sourceUrl: String, fileName: String?): JSObject {
        if (sourceUrl.startsWith("data:", ignoreCase = true)) {
            val loaded = loadDataUrlImage(sourceUrl)
            return loaded.bytes.inputStream().use {
                saveImageStream(it, fileName, sourceUrl, loaded.mimeType, false)
            }
        }

        if (sourceUrl.startsWith("content:", ignoreCase = true)) {
            val uri = Uri.parse(sourceUrl)
            val mime = activity.contentResolver.getType(uri) ?: mimeFromSource(sourceUrl)
            return activity.contentResolver.openInputStream(uri).use { input ->
                saveImageStream(input ?: throw IllegalStateException("无法读取 content 图片"), fileName, sourceUrl, mime, true)
            }
        }

        if (sourceUrl.startsWith("file:", ignoreCase = true) || sourceUrl.startsWith("/")) {
            val path = if (sourceUrl.startsWith("file:", ignoreCase = true)) {
                Uri.parse(sourceUrl).path ?: sourceUrl.removePrefix("file://")
            } else {
                sourceUrl
            }
            
            val file = checkedExportFile(path, MAX_GALLERY_EXPORT_BYTES.toLong())
            return file.inputStream().use {
                saveImageStream(it, fileName, file.name, mimeFromSource(file.name), true)
            }
        }

        return saveNetworkImage(sourceUrl, fileName)
    }

    private fun saveNetworkImage(sourceUrl: String, fileName: String?): JSObject {
        val connection = (URL(sourceUrl).openConnection() as HttpURLConnection).apply {
            connectTimeout = 5000  // 💥 优化：降低至5秒
            readTimeout = 10000    // 💥 优化：降低至10秒
            instanceFollowRedirects = true
            setRequestProperty("User-Agent", "VCPMobile/1.0")
        }

        try {
            val status = connection.responseCode
            if (status !in 200..299) {
                throw IllegalStateException("HTTP $status")
            }
            val contentType = connection.contentType?.substringBefore(";")?.lowercase(Locale.US)
            require(connection.contentLengthLong <= MAX_GALLERY_EXPORT_BYTES) { "图片过大，超过 50MB" }
            return connection.inputStream.use {
                saveImageStream(it, fileName, sourceUrl, contentType ?: mimeFromSource(sourceUrl), false)
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun loadDataUrlImage(dataUrl: String): LoadedImage {
        val commaIndex = dataUrl.indexOf(',')
        if (commaIndex <= 0 || commaIndex > 256) throw IllegalArgumentException("无效的 data URL")

        val header = dataUrl.substring(5, commaIndex)
        val mime = header.substringBefore(";").ifBlank { "application/octet-stream" }.lowercase(Locale.US)
        val isBase64 = header.contains(";base64", ignoreCase = true)
        val maxEncodedLength = if (isBase64) ((MAX_GALLERY_EXPORT_BYTES.toLong() + 2) / 3) * 4 else MAX_GALLERY_EXPORT_BYTES.toLong()
        require(dataUrl.length.toLong() - commaIndex - 1 <= maxEncodedLength) { "图片过大，超过 50MB" }
        val payload = dataUrl.substring(commaIndex + 1)
        val bytes = if (isBase64) {
            Base64.decode(payload, Base64.DEFAULT)
        } else {
            URLDecoder.decode(payload, "UTF-8").toByteArray(Charsets.UTF_8)
        }
        require(bytes.size in 1..MAX_GALLERY_EXPORT_BYTES) { "图片为空或超过 50MB" }
        return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = false))
    }

    private fun saveImageStream(
        input: InputStream,
        fileName: String?,
        source: String,
        fallbackMime: String,
        isLocal: Boolean,
        maxBytes: Long = MAX_GALLERY_EXPORT_BYTES.toLong(),
    ): JSObject {
        val buffered = input.buffered(64 * 1024)
        buffered.mark(256)
        val header = ByteArray(256)
        var length = 0
        while (length < header.size) {
            val count = buffered.read(header, length, header.size - length)
            if (count == -1) break
            length += count
        }
        buffered.reset()
        val mimeType = sniffImageMime(header.copyOf(length), fallbackMime, isLocal)
        require(mimeType.startsWith("image/")) { "当前资源不是图片: $mimeType" }
        val displayName = buildGalleryFileName(fileName, source, mimeType)
        val saved = writeImageToGallery(buffered, displayName, mimeType, maxBytes)
        return JSObject().apply {
            put("uri", saved.uri.toString())
            put("displayName", saved.displayName)
            put("mimeType", mimeType)
            put("size", saved.size)
        }
    }

    private fun sniffImageMime(bytes: ByteArray, fallback: String, isLocal: Boolean): String {
        val normalized = fallback.substringBefore(";").lowercase(Locale.US)
        
        // 💥 安全校验：若是网络资源可信任 content-type，若是本地绝对物理路径，必须强行进行 Magic bytes 头二进制分析，防止伪造扩展名泄漏明文
        if (!isLocal && normalized.startsWith("image/")) {
            return normalized
        }
        
        if (bytes.size >= 8 && bytes[0] == 0x89.toByte() && bytes[1] == 0x50.toByte() && bytes[2] == 0x4E.toByte() && bytes[3] == 0x47.toByte()) return "image/png"
        if (bytes.size >= 3 && bytes[0] == 0xFF.toByte() && bytes[1] == 0xD8.toByte() && bytes[2] == 0xFF.toByte()) return "image/jpeg"
        if (bytes.size >= 6 && String(bytes, 0, 6, Charsets.US_ASCII).startsWith("GIF")) return "image/gif"
        if (bytes.size >= 12 && String(bytes, 0, 4, Charsets.US_ASCII) == "RIFF" && String(bytes, 8, 4, Charsets.US_ASCII) == "WEBP") return "image/webp"
        if (bytes.size >= 2 && bytes[0] == 0x42.toByte() && bytes[1] == 0x4D.toByte()) return "image/bmp"
        
        val sample = bytes.take(256).toByteArray().toString(Charsets.UTF_8).trimStart()
        if (sample.startsWith("<svg", ignoreCase = true) || sample.startsWith("<?xml", ignoreCase = true)) return "image/svg+xml"
        
        // 本地读取兜底降级：非图片格式的敏感文件一律设为 application/octet-stream，从而在 saveImageToGallery 判定 mime.startsWith("image/") 时被拦截
        if (isLocal) {
            return "application/octet-stream"
        }
        return normalized
    }

    private fun mimeFromSource(source: String): String {
        val clean = source.substringBefore("?").substringBefore("#")
        val ext = clean.substringAfterLast('.', "").lowercase(Locale.US)
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: when (ext) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "svg" -> "image/svg+xml"
            "bmp" -> "image/bmp"
            "avif" -> "image/avif"
            "heic", "heif" -> "image/heic"
            else -> "application/octet-stream"
        }
    }

    private fun extensionForMime(mimeType: String): String {
        return when (mimeType.lowercase(Locale.US)) {
            "image/jpeg" -> "jpg"
            "image/png" -> "png"
            "image/gif" -> "gif"
            "image/webp" -> "webp"
            "image/svg+xml" -> "svg"
            "image/bmp" -> "bmp"
            "image/avif" -> "avif"
            "image/heic" -> "heic"
            "image/heif" -> "heif"
            else -> "png"
        }
    }

    private fun buildGalleryFileName(providedName: String?, sourceUrl: String, mimeType: String): String {
        val fromUrl = if (!sourceUrl.startsWith("data:", ignoreCase = true) && !sourceUrl.startsWith("blob:", ignoreCase = true)) {
            try {
                Uri.parse(sourceUrl).lastPathSegment?.let { URLDecoder.decode(it, "UTF-8") }
            } catch (_: Exception) {
                null
            }
        } else {
            null
        }

        val timestamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
        val rawName = providedName?.takeIf { it.isNotBlank() } ?: fromUrl ?: "vcp_image_$timestamp"
        val sanitized = rawName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_").trim().ifBlank { "vcp_image_$timestamp" }
        val base = sanitized.substringBeforeLast('.', sanitized).take(96).ifBlank { "vcp_image_$timestamp" }
        val ext = sanitized.substringAfterLast('.', "").lowercase(Locale.US).takeIf { it.isNotBlank() } ?: extensionForMime(mimeType)
        return "$base.$ext"
    }

    private fun writeImageToGallery(input: InputStream, displayName: String, mimeType: String, maxBytes: Long): SavedPublicFile {
        val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) MediaStore.Images.Media.EXTERNAL_CONTENT_URI else null
        return writePublicExport(displayName, mimeType, Environment.DIRECTORY_PICTURES, collection) {
            copyExportStream(input, it, maxBytes)
        }
    }

    private fun writePublicExport(
        displayName: String,
        mimeType: String,
        directory: String,
        collection: Uri?,
        write: (OutputStream) -> Long,
    ): SavedPublicFile {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = activity.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
                put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
                put(MediaStore.MediaColumns.RELATIVE_PATH, "$directory/VCPMobile")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val uri = resolver.insert(checkNotNull(collection), values)
                ?: throw IllegalStateException("无法创建导出文件")
            try {
                val size = resolver.openOutputStream(uri)?.use(write)
                    ?: throw IllegalStateException("无法写入导出文件")
                values.clear()
                values.put(MediaStore.MediaColumns.IS_PENDING, 0)
                check(resolver.update(uri, values, null, null) == 1) { "无法完成导出文件" }
                return SavedPublicFile(uri, displayName, size)
            } catch (error: Throwable) {
                try {
                    resolver.delete(uri, null, null)
                } catch (cleanupError: Throwable) {
                    error.addSuppressed(cleanupError)
                }
                throw error
            }
        }

        val publicDirectory = Environment.getExternalStoragePublicDirectory(directory)
        val (outputFile, size) = writeLegacyExportFile(java.io.File(publicDirectory, "VCPMobile"), displayName, write)
        try {
            MediaScannerConnection.scanFile(activity, arrayOf(outputFile.absolutePath), arrayOf(mimeType), null)
        } catch (error: Exception) {
            // The file is already committed. A scanner failure must not report a failed save.
            Log.w(TAG, "Saved export but could not notify media scanner", error)
        }
        return SavedPublicFile(Uri.fromFile(outputFile), outputFile.name, size)
    }

    // ==================================================================
    // Webview High Performance Capture
    // ==================================================================
    @Command
    fun captureWindowSnapshot(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(CaptureWindowSnapshotArgs::class.java)
        } catch (_: Throwable) {
            CaptureWindowSnapshotArgs()
        }

        val maxWidth = args.maxWidth.coerceIn(160, 420)
        val quality = args.quality.coerceIn(45, 85)

        // 💥 去掉锁机制，采用完全异步的 resolve/reject 调用模式，避免 Tokio 核心线程被 latch.await 挂起
        activity.runOnUiThread {
            try {
                val rootView = activity.window.decorView.rootView
                val sourceWidth = rootView.width
                val sourceHeight = rootView.height
                if (sourceWidth <= 0 || sourceHeight <= 0) {
                    invoke.reject("View has invalid size: ${sourceWidth}x${sourceHeight}")
                    return@runOnUiThread
                }

                val scale = min(1f, maxWidth.toFloat() / sourceWidth.toFloat())
                val outputWidth = max(1, (sourceWidth * scale).roundToInt())
                val outputHeight = max(1, (sourceHeight * scale).roundToInt())
                val snapshot = Bitmap.createBitmap(outputWidth, outputHeight, Bitmap.Config.RGB_565)
                val canvas = Canvas(snapshot)
                canvas.scale(scale, scale)
                rootView.draw(canvas)

                val encoded = ByteArrayOutputStream()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    snapshot.compress(Bitmap.CompressFormat.WEBP_LOSSY, quality, encoded)
                } else {
                    @Suppress("DEPRECATION")
                    snapshot.compress(Bitmap.CompressFormat.WEBP, quality, encoded)
                }
                snapshot.recycle() // 及时物理释放内存，防御 WebView 渲染高频截图导致 OOM

                val base64 = Base64.encodeToString(encoded.toByteArray(), Base64.NO_WRAP)
                val resultObject = JSObject().apply {
                    put("dataUrl", "data:image/webp;base64,$base64")
                    put("width", outputWidth)
                    put("height", outputHeight)
                }
                invoke.resolve(resultObject)
            } catch (e: Throwable) {
                Log.e(TAG, "captureWindowSnapshot failed", e)
                invoke.reject(e.message ?: "captureWindowSnapshot failed")
            }
        }
    }

    @Command
    fun processImage(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessImageArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processImageAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process image")
            }
        }
    }

    @Command
    fun processVideo(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessVideoArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processVideoAsync(args.path, activity) { result ->
            result.onSuccess { framePaths ->
                val arr = JSArray()
                for (p in framePaths) {
                    arr.put(p)
                }
                val resObj = JSObject().apply {
                    put("paths", arr)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process video")
            }
        }
    }

    @Command
    fun processAudio(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessAudioArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processAudioAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process audio")
            }
        }
    }

    private var downloadNotificationBuilder: androidx.core.app.NotificationCompat.Builder? = null
    private val DOWNLOAD_NOTIF_ID = 0x53545209
    private val DOWNLOAD_CHANNEL_ID = "apk_download"

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

    @Command
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

    @Command
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

    @Command
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

    @Command
    fun startHelperService(invoke: Invoke) {
        try {
            startHelperServiceInternal()
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startHelperService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
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
class ConnectWebViewResumeArgs {
    var documentId: String = ""
}

@InvokeArg
class CompleteWebViewResumeArgs {
    var documentId: String = ""
    var token: String = ""
}

@InvokeArg
class RegisterNativeEventsArgs {
    lateinit var channel: Channel
}

@InvokeArg
class RequestPermissionArgs {
    lateinit var type: String
}

@InvokeArg
class OpenFileArgs {
    lateinit var path: String
    var action: String = "view"
    var fileName: String? = null
}

@InvokeArg
class VerifyApkSignatureArgs {
    lateinit var path: String
}

@InvokeArg
class PickFileArgs {
    var mode: String = "file"
    var operationId: String = ""
    var stagingDir: String = ""
    var maxBytes: Long = 512L * 1024 * 1024
    var maxFiles: Int = 1
    val selectionLimit: Int get() = if (mode == "file" || mode == "gallery") maxFiles.coerceIn(1, 16) else 1
}

@InvokeArg
class CancelFileOperationArgs { lateinit var operationId: String }

@InvokeArg
class SaveImageArgs {
    lateinit var sourceUrl: String
    var fileName: String? = null
}

@InvokeArg
class SaveImageFromPathArgs {
    lateinit var imagePath: String
    var fileName: String? = null
}

@InvokeArg
class SaveToDownloadsArgs {
    lateinit var fileName: String
    lateinit var contentBase64: String
    var mimeType: String? = null
}

@InvokeArg
class SaveFileToDownloadsArgs {
    lateinit var filePath: String
    lateinit var fileName: String
    var mimeType: String? = null
    var maxBytes: Long = 0
}

@InvokeArg
class CaptureWindowSnapshotArgs {
    var maxWidth: Int = 200 // 与 Rust 侧默认参数对齐
    var quality: Int = 64  // 与 Rust 侧默认参数对齐
}

@InvokeArg
class ProcessImageArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessVideoArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessAudioArgs {
    lateinit var path: String
}

@InvokeArg
class UpdateDownloadNotifArgs {
    var progress: Int = 0
    var text: String? = null
}

@InvokeArg
class ToggleFloatingBallArgs {
    var show: Boolean = false
}

@InvokeArg
class ProcessSharedFileArgs {
    lateinit var cachePath: String
    var mimeType: String? = null
    lateinit var fileName: String
    lateinit var ownerId: String
    lateinit var stagingTicket: String
}

@InvokeArg
class SampleSensorDataArgs {
    var location: Boolean = false
    var motion: Boolean = false
    var ambient: Boolean = false
}

@InvokeArg
class RunRootCommandArgs {
    lateinit var command: String
}

@InvokeArg
class AcquireForegroundArgs {
    lateinit var tag: String
    var priority: Int = 0
    lateinit var label: String
    var screenKeepOn: Boolean = false
}

@InvokeArg
class ReleaseForegroundArgs {
    lateinit var tag: String
}

@InvokeArg
class WriteClipboardArgs {
    lateinit var content: String
}

@InvokeArg
class SendLocalNotificationArgs {
    lateinit var title: String
    lateinit var body: String
}
