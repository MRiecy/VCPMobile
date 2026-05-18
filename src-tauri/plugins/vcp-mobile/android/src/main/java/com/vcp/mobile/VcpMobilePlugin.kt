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
import app.tauri.plugin.Invoke
import com.vcp.mobile.service.StreamKeepaliveService
import com.vcp.mobile.service.StreamingActionReceiver

@TauriPlugin
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

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
        val args = invoke.parseArgs(StartStreamArgs::class.java)
        val intent = StreamKeepaliveService.createIntent(activity, args.agentName)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            activity.startForegroundService(intent)
        } else {
            activity.startService(intent)
        }
        invoke.resolve()
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        val intent = StreamKeepaliveService.createIntent(activity, "")
        activity.stopService(intent)
        invoke.resolve()
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
