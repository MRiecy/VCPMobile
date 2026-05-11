package com.vcp.avatar

import android.content.res.Configuration
import android.os.Bundle
import android.util.Log
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import com.vcp.avatar.bridge.FrontendBridge
import com.vcp.avatar.insets.KeyboardInsetsManager
import com.vcp.avatar.lifecycle.AppLifecycleBridge

/**
 * VCP Mobile Android 主 Activity
 *
 * 采用模块化编排：所有具体逻辑下沉到独立 Manager，MainActivity 仅负责
 * 生命周期调度与模块组装，便于后续频繁扩展底层能力。
 */
class MainActivity : TauriActivity() {

    // --- 共享基础设施 ---
    private val frontendBridge = FrontendBridge()

    // --- 领域模块 ---
    private val keyboardInsetsManager = KeyboardInsetsManager(frontendBridge)
    private val appLifecycleBridge = AppLifecycleBridge(frontendBridge)

    // ======================================================================
    // WebView 回调（WryActivity 提供）
    // ======================================================================

    override fun onWebViewCreate(webView: WebView) {
        frontendBridge.attachWebView(webView)
    }

    // ======================================================================
    // Activity 生命周期
    // ======================================================================

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)

        Log.d("VCPKeyboard", "MainActivity.onCreate: NEW APK RUNNING, setPadding should be REMOVED")
        // 键盘 Insets 手动管理（Android 15+ Edge-to-Edge 必需）
        keyboardInsetsManager.attach(window.decorView.rootView)
    }

    override fun onResume() {
        super.onResume()
        appLifecycleBridge.notifyResume()
    }

    override fun onPause() {
        super.onPause()
        appLifecycleBridge.notifyPause()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        appLifecycleBridge.notifyConfigurationChanged(newConfig)
    }

    override fun onLowMemory() {
        super.onLowMemory()
        appLifecycleBridge.notifyLowMemory()
    }

    override fun onDestroy() {
        unregisterReceiver(streamingActionReceiver)
        frontendBridge.detachWebView()
        super.onDestroy()
    }
}
