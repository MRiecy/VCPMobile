package com.vcp.avatar

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge

/**
 * VCP Mobile Android 主 Activity
 *
 * 所有自定义原生能力已迁移到 tauri-plugin-vcp-mobile 插件。
 * MainActivity 仅保留骨架：enableEdgeToEdge + 流式广播接收器兼容注册。
 * 返回键拦截由插件 VcpMobilePlugin 内部处理。
 */
class MainActivity : TauriActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
    }

    override fun onWebViewCreate(webView: WebView) {
        // TauriActivity 强制 handleBackNavigation = false，Wry 不注册任何返回键回调。
        // 在此处（WebView 已初始化后）注册 OnBackPressedDispatcher，
        // 有历史时 goBack() 触发前端 popstate 拦截链，无历史时 finish Activity。
        onBackPressedDispatcher.addCallback(
            this,
            object : androidx.activity.OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    webView.evaluateJavascript("window.dispatchEvent(new CustomEvent('vcp-hardware-back'))", null)
                }
            }
        )
    }
}
