package com.vcp.mobile

import android.app.Activity
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
import android.util.Log
import android.os.PowerManager
import android.net.Uri
import android.provider.Settings
import android.content.pm.PackageManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import app.tauri.plugin.Invoke
import com.vcp.mobile.service.StreamKeepaliveService

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE"], alias = "storageLegacy")
])
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private companion object {
        const val TAG = "VcpMobilePlugin"
    }

    private var webViewRef: WebView? = null
    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val lifecycleBridge = LifecycleBridge()

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

        val storageGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val result = JSObject()
        result.put("notification", notificationGranted)
        result.put("storage", storageGranted)
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

    // ==================================================================
    // Permission Result Callbacks
    // ==================================================================
    @PermissionCallback
    fun onPermissionResult(invoke: Invoke) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    @ActivityCallback
    fun onBatteryOptimizationResult(invoke: Invoke, result: ActivityResult) {
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

        val storageGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val json = """{"notification":$notificationGranted,"storage":$storageGranted,"battery":$batteryOptimizationIgnored}"""
        val script = "window.dispatchEvent(new CustomEvent('vcp-permission-change', { detail: $json }))"
        webViewRef?.evaluateJavascript(script, null)
    }

    // ==================================================================
    // Screen
    // ==================================================================
    @Command
    fun setKeepScreenOn(invoke: Invoke) {
        activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    @Command
    fun clearKeepScreenOn(invoke: Invoke) {
        activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    // ==================================================================
    // Stream Service
    // ==================================================================
    @Command
    fun startStreamingService(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StartStreamArgs::class.java)
            val intent = StreamKeepaliveService.createIntent(activity, args.agentName)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                activity.startForegroundService(intent)
            } else {
                activity.startService(intent)
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startStreamingService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        try {
            val intent = StreamKeepaliveService.createIntent(activity, "")
            activity.stopService(intent)
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "stopStreamingService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    // ==================================================================
    // Plugin Lifecycle
    // ==================================================================
    override fun load(webView: WebView) {
        super.load(webView)
        webViewRef = webView

        keyboardInsetsManager.attach(webView)
        lifecycleBridge.attach(activity, webView)
    }

    override fun onDestroy(activity: AppCompatActivity) {
        super.onDestroy(activity)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        lifecycleBridge.onConfigurationChanged(newConfig)
    }
}

@InvokeArg
class StartStreamArgs {
    lateinit var agentName: String
}

@InvokeArg
class RequestPermissionArgs {
    lateinit var type: String
}
