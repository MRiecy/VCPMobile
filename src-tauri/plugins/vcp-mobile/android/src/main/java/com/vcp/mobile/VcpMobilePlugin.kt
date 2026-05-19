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
import app.tauri.annotation.TauriPlugin
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

@TauriPlugin
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private companion object {
        const val TAG = "VcpMobilePlugin"
    }

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
                    ActivityCompat.requestPermissions(activity, arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1001)
                }
            }
            "storage" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    ActivityCompat.requestPermissions(activity, arrayOf(android.Manifest.permission.READ_MEDIA_IMAGES), 1002)
                } else {
                    ActivityCompat.requestPermissions(
                        activity,
                        arrayOf(android.Manifest.permission.READ_EXTERNAL_STORAGE, android.Manifest.permission.WRITE_EXTERNAL_STORAGE),
                        1002
                    )
                }
            }
            "battery" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    }
                    activity.startActivity(intent)
                } catch (e: Exception) {
                    // Fallback to general battery optimization settings if specific intent fails
                    val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                    activity.startActivity(intent)
                }
            }
        }
        invoke.resolve()
    }

    @Command
    fun moveTaskToBack(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
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
