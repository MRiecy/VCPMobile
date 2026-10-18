package com.vcp.avatar

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen

/**
 * VCP Mobile Android 主 Activity
 *
 * 所有自定义原生能力已迁移到 tauri-plugin-vcp-mobile 插件。
 * MainActivity 保留系统 SplashScreen、edge-to-edge 与主 WebView 的返回键桥。
 */
class MainActivity : TauriActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
    }

    override fun onWebViewCreate(webView: WebView) {
        // TauriActivity 强制 handleBackNavigation = false，Wry 不注册任何返回键回调。
        // 在此处（WebView 已初始化后）注册 OnBackPressedDispatcher，
        // 交给前端现有覆盖层/页面返回链处理。
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
