package com.vcp.avatar

import android.app.Activity
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback

/**
 * 返回导航管理器
 *
 * 职责：接管 Android 物理返回键 / 边缘滑动返回事件，优先交给 WebView 的 history.back()
 * 处理，确保前端 popstate 拦截链（modal → drawer → pageStack → 退出提示）能够正常工作。
 *
 * 背景：TauriActivity 的 handleBackNavigation = false 导致系统直接 finish Activity，
 * 完全绕过前端。本 Manager 在 MainActivity 中注册独立的 OnBackPressedDispatcher 回调，
 * 恢复 WebView 的 goBack() 行为，且不侵入 MainActivity 的核心生命周期。
 */
class BackNavigationManager {
    private var webViewRef: WebView? = null

    fun attachWebView(webView: WebView) {
        webViewRef = webView
    }

    fun detachWebView() {
        webViewRef = null
    }

    /**
     * 将返回键拦截绑定到指定 Activity。
     *
     * @param activity 通常是 MainActivity 实例
     */
    fun attach(activity: Activity) {
        activity.onBackPressedDispatcher.addCallback(
            activity,
            object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    val webView = webViewRef
                    if (webView != null && webView.canGoBack()) {
                        // 将返回事件交给 WebView，触发前端的 popstate 拦截链
                        webView.goBack()
                    } else {
                        // WebView 无历史可回退，允许系统默认行为（finish Activity）
                        isEnabled = false
                        activity.onBackPressedDispatcher.onBackPressed()
                        isEnabled = true
                    }
                }
            }
        )
    }
}
