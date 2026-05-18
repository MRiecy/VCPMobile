package com.vcp.mobile

import android.app.Activity
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.tauri.plugin.JSObject

/**
 * 键盘 IME Insets 管理器
 *
 * 职责：
 * 1. 监听系统 WindowInsets 中 IME（键盘）区域的变化
 * 2. 将键盘高度与安全区域信息通过 Plugin.trigger() 实时推送到前端
 * 3. 不再通过 setPadding 干预 WebView 布局，完全交由前端 CSS 接管
 */
class KeyboardInsetsManager(private val activity: Activity) {

    private var webViewRef: WebView? = null
    private var emit: ((String, JSObject) -> Unit)? = null

    fun attach(webView: WebView, emitFn: (String, JSObject) -> Unit) {
        webViewRef = webView
        emit = emitFn
        val rootView = activity.window.decorView.rootView

        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0

            val payload = JSObject().apply {
                put("height", keyboardHeight)
                put("visible", isKeyboardVisible)
                put("safeAreaBottom", systemBars.bottom)
            }

            emit?.invoke("vcp-keyboard-inset", payload)

            insets
        }
    }

    fun queryCurrentState(): KeyboardState {
        val rootView = activity.window.decorView.rootView
        val insets = ViewCompat.getRootWindowInsets(rootView) ?: return KeyboardState(0, false)
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        val visible = insets.isVisible(WindowInsetsCompat.Type.ime())
        return KeyboardState(
            height = if (visible) ime.bottom else 0,
            visible = visible
        )
    }

    data class KeyboardState(val height: Int, val visible: Boolean)
}
