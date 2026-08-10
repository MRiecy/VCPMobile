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
    private var rootView: android.view.View? = null
    private var lastSent: InsetSnapshot? = null
    private var pending: InsetSnapshot? = null
    private var frameScheduled = false
    private val frameCallback = Runnable {
        frameScheduled = false
        val snapshot = pending ?: return@Runnable
        pending = null
        if (snapshot != lastSent) {
            lastSent = snapshot
            emitSnapshot(snapshot)
        }
    }

    fun attach(webView: WebView) {
        webViewRef = webView
        val rootView = activity.window.decorView.rootView
        this.rootView = rootView

        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0

            schedule(InsetSnapshot(keyboardHeight, isKeyboardVisible, systemBars.bottom))

            insets
        }
        ViewCompat.requestApplyInsets(rootView)
    }

    fun detach() {
        val view = rootView
        if (view != null) {
            view.removeCallbacks(frameCallback)
            ViewCompat.setOnApplyWindowInsetsListener(view, null)
        }
        val previous = lastSent
        if (previous?.visible == true || (previous?.height ?: 0) != 0) {
            emitSnapshot(InsetSnapshot(0, false, previous?.safeAreaBottom ?: 0))
        }
        pending = null
        frameScheduled = false
        lastSent = null
        rootView = null
        webViewRef = null
    }

    private fun schedule(snapshot: InsetSnapshot) {
        if (snapshot == lastSent && !frameScheduled) return
        pending = snapshot
        if (!frameScheduled) {
            frameScheduled = true
            rootView?.postOnAnimation(frameCallback)
        }
    }

    private fun emitSnapshot(snapshot: InsetSnapshot) {
        Log.d(
            "VCPKeyboard",
            "Native inset changed: height=${snapshot.height}, visible=${snapshot.visible}, safeArea=${snapshot.safeAreaBottom}",
        )
        emit(
            "vcp-keyboard-inset",
            mapOf(
                "height" to snapshot.height,
                "visible" to snapshot.visible,
                "safeAreaBottom" to snapshot.safeAreaBottom,
            ),
        )
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
        val targetWebView = webViewRef ?: return
        val json = serializeValue(detail)
        val script = "window.dispatchEvent(new CustomEvent('$eventName', { detail: $json }))"
        activity.runOnUiThread {
            targetWebView.evaluateJavascript(script, null)
        }
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
    private data class InsetSnapshot(
        val height: Int,
        val visible: Boolean,
        val safeAreaBottom: Int,
    )
}
