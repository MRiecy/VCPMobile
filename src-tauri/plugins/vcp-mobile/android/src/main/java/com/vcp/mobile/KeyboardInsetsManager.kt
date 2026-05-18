package com.vcp.mobile

import android.app.Activity
import android.util.Log
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * 键盘 IME Insets 管理器
 *
 * 职责：
 * 1. 监听系统 WindowInsets 中 IME（键盘）区域的变化
 * 2. 将键盘高度与安全区域信息通过 evaluateJavascript 实时推送到前端
 *    （与 Plugin.trigger() 不同，evaluateJavascript 直接注入 window.CustomEvent，
 *     前端通过 window.addEventListener 即可接收，无需 Tauri 事件通道注册）
 * 3. 不再通过 setPadding 干预 WebView 布局，完全交由前端 CSS 接管
 */
class KeyboardInsetsManager(private val activity: Activity) {

    private var webViewRef: WebView? = null

    fun attach(webView: WebView) {
        webViewRef = webView
        val rootView = activity.window.decorView.rootView

        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0

            Log.d("VCPKeyboard", "Native inset event: height=$keyboardHeight, visible=$isKeyboardVisible, safeArea=${systemBars.bottom}")

            emit(
                "vcp-keyboard-inset",
                mapOf(
                    "height" to keyboardHeight,
                    "visible" to isKeyboardVisible,
                    "safeAreaBottom" to systemBars.bottom
                )
            )

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

    private fun emit(eventName: String, detail: Map<String, Any?>) {
        val json = serializeValue(detail)
        val script = "window.dispatchEvent(new CustomEvent('$eventName', { detail: $json }))"
        webViewRef?.evaluateJavascript(script, null)
    }

    private fun serializeValue(value: Any?): String {
        return when (value) {
            null -> "null"
            is String -> "\"${escapeJson(value)}\""
            is Boolean -> value.toString()
            is Number -> value.toString()
            is Map<*, *> -> {
                val entries = value.entries.joinToString(", ") { (k, v) ->
                    "\"$k\": ${serializeValue(v)}"
                }
                "{ $entries }"
            }
            else -> "\"${escapeJson(value.toString())}\""
        }
    }

    private fun escapeJson(s: String): String {
        return s
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\b", "\\b")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
            .replace("\t", "\\t")
    }

    data class KeyboardState(val height: Int, val visible: Boolean)
}
