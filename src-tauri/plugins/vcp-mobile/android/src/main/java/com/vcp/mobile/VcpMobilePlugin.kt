package com.vcp.mobile

import android.app.Activity
import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Environment
import android.util.Log
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
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
import com.vcp.mobile.clipboard.NativeClipboard
import com.vcp.mobile.device.BatteryStatusManager
import com.vcp.mobile.device.CpuStatusManager
import com.vcp.mobile.device.GpuStatusManager
import com.vcp.mobile.device.NetworkStatusManager
import com.vcp.mobile.device.SampleSensorDataArgs
import com.vcp.mobile.device.SensorStatusManager
import com.vcp.mobile.files.NativeFileExports
import com.vcp.mobile.files.NativeFilePicker
import com.vcp.mobile.files.NativeOutgoingFiles
import com.vcp.mobile.files.isSafeLocalPath
import com.vcp.mobile.haptics.NativeHaptics
import com.vcp.mobile.lifecycle.LifecycleBridge
import com.vcp.mobile.lifecycle.NativeEvents
import com.vcp.mobile.media.DeriveImageArgs
import com.vcp.mobile.media.MediaOperationArgs
import com.vcp.mobile.media.NativeFileImages
import com.vcp.mobile.media.NativeMediaProcessor
import com.vcp.mobile.media.ProcessAudioArgs
import com.vcp.mobile.media.ProcessVideoArgs
import com.vcp.mobile.notifications.NativeNotifications
import com.vcp.mobile.permissions.AndroidPermissions
import com.vcp.mobile.root.NativeRootAccess
import com.vcp.mobile.runtime.PluginTaskRuntime
import com.vcp.mobile.service.AcquireForegroundArgs
import com.vcp.mobile.service.ReleaseForegroundArgs
import com.vcp.mobile.sharing.ShareIntentHandler
import com.vcp.mobile.sharing.AcknowledgeSharePayloadArgs
import com.vcp.mobile.sharing.SharedFileIngestor
import com.vcp.mobile.updater.NativeUpdater
import com.vcp.mobile.voice.NativeVoiceAudio
import com.vcp.mobile.voice.NativeVoiceOutput
import com.vcp.mobile.voice.VoiceAudioSession
import com.vcp.mobile.webview.CompleteWebViewResumeArgs
import com.vcp.mobile.webview.ConnectWebViewResumeArgs
import com.vcp.mobile.webview.KeyboardInsetsManager
import com.vcp.mobile.webview.NativeWebViewResume
import com.vcp.mobile.webview.WindowSnapshot

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE"], alias = "storageLegacy"),
    Permission(strings = ["android.permission.RECORD_AUDIO"], alias = "microphone"),
    Permission(strings = ["android.permission.CAMERA"], alias = "camera"),
    Permission(strings = ["android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION"], alias = "location")
])
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private val activityLifecycleCallbacks = object : android.app.Application.ActivityLifecycleCallbacks {
        override fun onActivityPrePaused(a: Activity) {
            if (a === activity) webViewResume.onPausing()
        }
        override fun onActivityResumed(a: Activity) {
            if (a === activity) {
                ScreenKeepOnArbiter.setAppInForeground(activity, true)
                webViewResume.onResumed()
                voiceAudio.onForeground()
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
            if (a === activity) {
                voiceAudio.onBackground()
                webViewResume.onStopped()
            }
        }
        override fun onActivitySaveInstanceState(a: Activity, outState: android.os.Bundle) {}
        override fun onActivityDestroyed(a: Activity) {
            // A replacement plugin may already occupy Tauri's registry. The
            // original owner still receives its Application callback.
            if (a === activity && a is AppCompatActivity) onDestroy(a)
        }
    }

    companion object {
        const val TAG = "VcpMobilePlugin"
    }

    private var webViewRef: WebView? = null

    private val runtime = PluginTaskRuntime()
    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val webViewResume = NativeWebViewResume(activity, keyboardInsetsManager::connectDocument)
    private val mediaProcessor = NativeMediaProcessor(activity)
    private val lifecycleBridge = LifecycleBridge()
    private val batteryStatusManager = BatteryStatusManager(activity)
    private val networkStatusManager = NetworkStatusManager(activity)
    private val cpuStatusManager = CpuStatusManager(activity)
    private val gpuStatusManager = GpuStatusManager(activity)
    private val sensorStatusManager = SensorStatusManager(activity)
    private val nativeEvents = NativeEvents(activity, runtime, networkStatusManager, lifecycleBridge::snapshot)
    private val voiceAudioSession = VoiceAudioSession(activity)
    private val voiceOutput = NativeVoiceOutput(voiceAudioSession) { id ->
        nativeEvents.tryEmitVoicePlayback(JSObject().apply { put("playbackId", id); put("kind", "started") })
    }
    private val voiceAudio: NativeVoiceAudio = NativeVoiceAudio(activity, ::requestPermissionForAlias, nativeEvents::isReady, nativeEvents::tryEmitVoiceAudio, voiceAudioSession, voiceOutput)
    private val permissions = AndroidPermissions(activity, ::requestPermissionForAlias, ::startActivityForResult, ::emitWebViewEvent)
    private val filePicker = NativeFilePicker(activity, runtime, ::startActivityForResult, ::requestPermissionForAlias, ::emitWebViewEvent)
    private val fileExports = NativeFileExports(activity, runtime, ::requestPermissionForAlias)
    private val outgoingFiles = NativeOutgoingFiles(activity, runtime)
    private val rootAccess = NativeRootAccess(activity, runtime)
    private val notifications = NativeNotifications(activity, ::emitWebViewEvent)
    private val clipboard = NativeClipboard(activity)
    private val haptics = NativeHaptics(activity, runtime)
    private val windowSnapshot = WindowSnapshot(activity)
    private val updater = NativeUpdater(activity, runtime)
    private val cliProcessHost = CliProcessHostOwner.get(activity.applicationContext)
    private val cliPtyHost = CliPtyHostOwner.get(cliProcessHost)
    private val shareIntentHandler = ShareIntentHandler(activity, runtime.domains.fileIoExecutor) { webViewRef }
    private val sharedFiles = SharedFileIngestor(activity, shareIntentHandler, runtime)
    private val guardianScreenListener: (Boolean) -> Unit = ::applyGuardianScreenState

    private fun emitWebViewEvent(event: String, detail: JSObject) {
        val json = escapeJsonForJsString(detail.toString())
        activity.runOnUiThread {
            webViewRef?.evaluateJavascript("window.dispatchEvent(new CustomEvent('$event', { detail: JSON.parse(\"$json\") }))", null)
        }
    }

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
        activity.application.registerActivityLifecycleCallbacks(activityLifecycleCallbacks)
        ScreenKeepOnArbiter.setAppInForeground(activity, true)
        com.vcp.mobile.service.ForegroundGuardian.setScreenStateListener(guardianScreenListener)
        rootAccess.startOomScoreGuard()
    }

    // ==================================================================
    // Permissions & App Control
    // ==================================================================
    @Command
    fun checkAllPermissions(invoke: Invoke) = permissions.checkAllPermissions(invoke)

    @Command
    fun requestAndroidPermission(invoke: Invoke) = permissions.requestAndroidPermission(invoke)

    @Command
    fun moveTaskToBack(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
    }

    @Command
    fun check_notification_listener_permission(invoke: Invoke) = permissions.check_notification_listener_permission(invoke)

    @Command
    fun request_notification_listener_permission(invoke: Invoke) = permissions.request_notification_listener_permission(invoke)

    @Command
    fun checkAutoStartPermission(invoke: Invoke) = permissions.checkAutoStartPermission(invoke)

    @Command
    fun requestAutoStartPermission(invoke: Invoke) = permissions.requestAutoStartPermission(invoke)

    @Command
    fun requestPowerManagementPermission(invoke: Invoke) = permissions.requestPowerManagementPermission(invoke)

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
    fun onPermissionResult(invoke: Invoke) = permissions.onPermissionResult(invoke)

    @PermissionCallback
    fun onVoiceAudioPermissionResult(invoke: Invoke) = voiceAudio.onPermissionResult(invoke)

    @Command
    fun beginVoiceCall(invoke: Invoke) = voiceAudio.beginVoiceCall(invoke)

    @Command
    fun endVoiceCall(invoke: Invoke) = voiceAudio.endVoiceCall(invoke)

    @Command
    fun startVoiceAudio(invoke: Invoke) = voiceAudio.startVoiceAudio(invoke)

    @Command
    fun stopVoiceAudio(invoke: Invoke) = voiceAudio.stopVoiceAudio(invoke)

    @Command
    fun exportVoiceSegment(invoke: Invoke) = voiceAudio.exportVoiceSegment(invoke)

    @Command
    fun releaseVoiceSegments(invoke: Invoke) = voiceAudio.releaseVoiceSegments(invoke)

    @Command
    fun startVoicePlayback(invoke: Invoke) = voiceOutput.startVoicePlayback(invoke)

    @Command
    fun appendVoicePlayback(invoke: Invoke) = voiceOutput.appendVoicePlayback(invoke)

    @Command
    fun endVoicePlaybackSegment(invoke: Invoke) = voiceOutput.endVoicePlaybackSegment(invoke)

    @Command
    fun finishVoicePlayback(invoke: Invoke) = voiceOutput.finishVoicePlayback(invoke)

    @Command
    fun stopVoicePlayback(invoke: Invoke) = voiceOutput.stopVoicePlayback(invoke)

    @ActivityCallback
    fun onBatteryOptimizationResult(invoke: Invoke, result: ActivityResult) = permissions.onBatteryOptimizationResult(invoke, result)

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
    fun checkRootAccess(invoke: Invoke) = rootAccess.checkRootAccess(invoke)

    @Command
    fun writeClipboard(invoke: Invoke) = clipboard.writeClipboard(invoke)

    @Command
    fun readClipboard(invoke: Invoke) = clipboard.readClipboard(invoke)

    @Command
    fun sendLocalNotification(invoke: Invoke) = notifications.sendLocalNotification(invoke)

    @Command
    fun runRootCommand(invoke: Invoke) = rootAccess.runRootCommand(invoke)

    @Command
    fun launchRootManager(invoke: Invoke) = rootAccess.launchRootManager(invoke)

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

    @Command
    fun registerNativeEvents(invoke: Invoke) = nativeEvents.registerNativeEvents(invoke)

    @Command
    fun startNetworkMonitoring(invoke: Invoke) = nativeEvents.startNetworkMonitoring(invoke)

    override fun load(webView: WebView) {
        if (runtime.isDestroying) return
        super.load(webView)
        webViewRef = webView
        webViewResume.attach(webView)

        keyboardInsetsManager.attach(webView)
        // The process observer must not retain this Activity through the event sink.
        val nativeEventsRef = java.lang.ref.WeakReference(nativeEvents)
        lifecycleBridge.attach(activity) { event, payload ->
            nativeEventsRef.get()?.emitNativeEvent(event, payload)
        }

        // 冷启动：处理传递给 Activity 的初始 intent
        shareIntentHandler.handleInitialIntent(activity.intent)
        shareIntentHandler.injectShareData(webView)
        notifications.handleNotificationIntent(activity.intent)
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
        // Tauri forwards every Activity destruction to every registered plugin.
        if (activity !== this.activity || runtime.isDestroying) return
        runtime.beginDestroy()
        // A departed receiver or detached View must not prevent other owners
        // from releasing their callbacks, queued work and native resources.
        fun release(name: String, action: () -> Unit) {
            try { action() }
            catch (error: Throwable) { Log.e(TAG, "Failed to release $name", error) }
        }
        release("WebView resume", webViewResume::destroy)
        release("window snapshot", windowSnapshot::destroy)
        release("media", mediaProcessor::destroy)
        release("voice audio", voiceAudio::destroy)
        release("voice output", voiceOutput::destroy)
        release("voice routing", voiceAudioSession::destroy)
        release("file picker", filePicker::destroy)
        release("share inbox", shareIntentHandler::destroy)
        release("file exports", fileExports::destroy)
        release("native events", nativeEvents::destroy)
        release("root work", rootAccess::destroy)
        release("sensors", sensorStatusManager::shutdown)
        release("plugin tasks", runtime::shutdown)
        release("Activity callbacks") { activity.application.unregisterActivityLifecycleCallbacks(activityLifecycleCallbacks) }
        release("Guardian listener") { com.vcp.mobile.service.ForegroundGuardian.clearScreenStateListener(guardianScreenListener) }
        release("screen flags") { ScreenKeepOnArbiter.detach(activity) }
        release("Insets", keyboardInsetsManager::detach)
        webViewRef = null
        release("lifecycle observer", lifecycleBridge::detach)
        super.onDestroy(activity)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        if (runtime.isDestroying) return
        super.onConfigurationChanged(newConfig)
        lifecycleBridge.onConfigurationChanged(newConfig)
    }

    override fun onNewIntent(intent: Intent) {
        if (runtime.isDestroying) return
        super.onNewIntent(intent)
        shareIntentHandler.handleShareIntent(intent)
        notifications.handleNotificationIntent(intent)
    }

    // ==================================================================
    // File acquisition and image derivatives
    // ==================================================================

    @Command
    fun cancelFileOperation(invoke: Invoke) = filePicker.cancelFileOperation(invoke)

    @PermissionCallback
    fun onCameraPermissionResult(invoke: Invoke) = filePicker.onCameraPermissionResult(invoke)

    @Command
    fun pickFile(invoke: Invoke) = filePicker.pickFile(invoke)

    @ActivityCallback
    fun onCameraResult(invoke: Invoke, result: ActivityResult) = filePicker.onCameraResult(invoke, result)

    @ActivityCallback
    fun onPickFileResult(invoke: Invoke, result: ActivityResult) = filePicker.onPickFileResult(invoke, result)

    @Command
    fun deriveImage(invoke: Invoke) {
        runtime.execute(runtime.domains.fileIoExecutor, invoke, "deriveImage") {
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

    @Command
    fun readShareInboxPayload(invoke: Invoke) {
        invoke.resolve(JSObject().apply { put("payload", shareIntentHandler.readPendingPayload()) })
    }

    @Command
    fun acknowledgeShareInboxPayload(invoke: Invoke) {
        val args = invoke.parseArgs(AcknowledgeSharePayloadArgs::class.java)
        runtime.execute(runtime.domains.fileIoExecutor, invoke, "acknowledgeShareInboxPayload") {
            shareIntentHandler.acknowledgePayload(args)
            invoke.resolve()
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
    @Command
    fun processSharedFile(invoke: Invoke) = sharedFiles.processSharedFile(invoke)

    @Command
    fun openFile(invoke: Invoke) = outgoingFiles.openFile(invoke)

    // ==================================================================
    // OTA Update Support (签名连续性 / 安装权限 / 下载保活)
    // ==================================================================

    @Command
    fun verifyApkSignature(invoke: Invoke) = updater.verifyApkSignature(invoke)

    @Command
    fun canInstallPackages(invoke: Invoke) = updater.canInstallPackages(invoke)

    @Command
    fun openUnknownSourcesSettings(invoke: Invoke) = updater.openUnknownSourcesSettings(invoke)

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
    // Universal Media Exporter & Gallery Writer
    // ==================================================================
    @Command
    fun saveImageToGallery(invoke: Invoke) = fileExports.saveImageToGallery(invoke)

    @Command
    fun saveImageFromPath(invoke: Invoke) = fileExports.saveImageFromPath(invoke)

    // ==================================================================
    // Downloads Exporter (设置备份等任意小文件写入公共下载目录)
    // ==================================================================
    @Command
    fun saveToDownloads(invoke: Invoke) = fileExports.saveToDownloads(invoke)

    @Command
    fun saveFileToDownloads(invoke: Invoke) = fileExports.saveFileToDownloads(invoke)

    @PermissionCallback
    fun onLegacyWritePermissionResult(invoke: Invoke) = fileExports.onLegacyWritePermissionResult(invoke)

    // ==================================================================
    // Webview High Performance Capture
    // ==================================================================
    @Command
    fun captureWindowSnapshot(invoke: Invoke) = windowSnapshot.captureWindowSnapshot(invoke)

    @Command
    fun prepareMediaOperation(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(MediaOperationArgs::class.java)
            mediaProcessor.prepare(args.operationId)
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Failed to prepare media operation")
        }
    }

    @Command
    fun cancelMediaOperation(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(MediaOperationArgs::class.java)
            mediaProcessor.cancel(args.operationId)
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject(error.message ?: "Failed to cancel media operation")
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

        mediaProcessor.processVideo(args.operationId, args.path, args.outputDir, args.maxBytes) { result ->
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

        mediaProcessor.processAudio(args.operationId, args.path, args.outputDir, args.maxBytes) { result ->
            result.onSuccess { audio ->
                val resObj = JSObject().apply {
                    put("path", audio.path)
                    put("mime", audio.mime)
                    put("codec", audio.codec)
                    put("container", audio.container)
                    put("sampleRate", audio.sampleRate)
                    put("channels", audio.channels)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process audio")
            }
        }
    }

    @Command
    fun startDownloadNotification(invoke: Invoke) = notifications.startDownloadNotification(invoke)

    @Command
    fun updateDownloadNotification(invoke: Invoke) = notifications.updateDownloadNotification(invoke)

    @Command
    fun cancelDownloadNotification(invoke: Invoke) = notifications.cancelDownloadNotification(invoke)

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
    fun getPendingNotification(invoke: Invoke) = notifications.getPendingNotification(invoke)

    @Command
    fun triggerHapticFeedback(invoke: Invoke) = haptics.triggerHapticFeedback(invoke)
}
