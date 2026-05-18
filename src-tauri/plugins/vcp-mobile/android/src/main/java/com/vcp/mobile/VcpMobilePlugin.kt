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
import android.util.Log
import app.tauri.plugin.Invoke
import com.vcp.mobile.service.StreamKeepaliveService
import com.vcp.mobile.service.StreamingActionReceiver

@TauriPlugin
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private companion object {
        const val TAG = "VcpMobilePlugin"
    }

    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val lifecycleBridge = LifecycleBridge()
    private lateinit var streamingActionReceiver: StreamingActionReceiver

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
        Log.i(TAG, "startStreamingService called")
        try {
            val args = invoke.parseArgs(StartStreamArgs::class.java)
            Log.i(TAG, "parsed args: agentName=${args.agentName}")
            val intent = StreamKeepaliveService.createIntent(activity, args.agentName)
            Log.i(TAG, "intent created: $intent")
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Log.i(TAG, "calling startForegroundService")
                activity.startForegroundService(intent)
            } else {
                Log.i(TAG, "calling startService")
                activity.startService(intent)
            }
            Log.i(TAG, "service start call completed")
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startStreamingService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        Log.i(TAG, "stopStreamingService called")
        try {
            val intent = StreamKeepaliveService.createIntent(activity, "")
            activity.stopService(intent)
            Log.i(TAG, "stopService completed")
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

        // 注册流式中断广播接收器
        streamingActionReceiver = StreamingActionReceiver()
        val filter = IntentFilter(StreamingActionReceiver.STREAM_INTERRUPT_ACTION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            activity.registerReceiver(streamingActionReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            activity.registerReceiver(streamingActionReceiver, filter)
        }
    }

    override fun onDestroy(activity: AppCompatActivity) {
        activity.unregisterReceiver(streamingActionReceiver)
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
