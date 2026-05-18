package com.vcp.avatar

import android.content.Context
import android.content.IntentFilter
import android.os.Build
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import com.vcp.mobile.service.StreamingActionReceiver

/**
 * VCP Mobile Android 主 Activity
 *
 * 所有自定义原生能力已迁移到 tauri-plugin-vcp-mobile 插件。
 * MainActivity 仅保留骨架：enableEdgeToEdge + 流式广播接收器兼容注册。
 * 返回键拦截由插件 VcpMobilePlugin 内部处理。
 */
class MainActivity : TauriActivity() {

    private lateinit var streamingActionReceiver: StreamingActionReceiver

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        // 注册流式中断广播接收器（插件中也会注册，双重注册无害）
        streamingActionReceiver = StreamingActionReceiver()
        val filter = IntentFilter(StreamingActionReceiver.STREAM_INTERRUPT_ACTION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            registerReceiver(streamingActionReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(streamingActionReceiver, filter)
        }
    }

    override fun onWebViewCreate(webView: WebView) {
        // TauriActivity 强制 handleBackNavigation = false，Wry 不注册任何返回键回调。
        // 在此处（WebView 已初始化后）注册 OnBackPressedDispatcher，
        // 有历史时 goBack() 触发前端 popstate 拦截链，无历史时 finish Activity。
        onBackPressedDispatcher.addCallback(
            this,
            object : androidx.activity.OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    if (webView.canGoBack()) {
                        webView.goBack()
                    } else {
                        isEnabled = false
                        onBackPressedDispatcher.onBackPressed()
                        isEnabled = true
                    }
                }
            }
        )
    }

    override fun onDestroy() {
        unregisterReceiver(streamingActionReceiver)
        super.onDestroy()
    }
}
