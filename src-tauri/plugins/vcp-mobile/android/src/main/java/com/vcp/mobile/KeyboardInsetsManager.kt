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
            schedule(snapshotFrom(insets))

            insets
        }
        ViewCompat.getRootWindowInsets(rootView)?.let { schedule(snapshotFrom(it)) }
        ViewCompat.requestApplyInsets(rootView)
    }

    fun detach() {
        val view = rootView
        if (view != null) {
            view.removeCallbacks(frameCallback)
            ViewCompat.setOnApplyWindowInsetsListener(view, null)
        }
        val previous = pending
            ?: lastSent
            ?: view?.let(ViewCompat::getRootWindowInsets)?.let(::snapshotFrom)
        val detached = previous?.withoutIme()
        if (detached != null && detached != lastSent) {
            lastSent = detached
            emitSnapshot(detached)
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
            "Native insets changed: safe=[${snapshot.safeTopPx},${snapshot.safeRightPx}," +
                "${snapshot.safeBottomPx},${snapshot.safeLeftPx}], " +
                "imeBottom=${snapshot.imeBottomPx}, imeVisible=${snapshot.imeVisible}",
        )
        val targetWebView = webViewRef ?: return
        val script = buildInsetsJavascript(snapshot)
        activity.runOnUiThread {
            targetWebView.evaluateJavascript(script, null)
        }
    }

    private fun snapshotFrom(insets: WindowInsetsCompat): InsetSnapshot {
        val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
        val displayCutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout())
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        return mergeInsetSnapshot(
            systemBars = EdgeInsetsPx(
                top = systemBars.top,
                right = systemBars.right,
                bottom = systemBars.bottom,
                left = systemBars.left,
            ),
            displayCutout = EdgeInsetsPx(
                top = displayCutout.top,
                right = displayCutout.right,
                bottom = displayCutout.bottom,
                left = displayCutout.left,
            ),
            imeBottomPx = ime.bottom,
            imeVisible = insets.isVisible(WindowInsetsCompat.Type.ime()),
        )
    }
}

internal data class EdgeInsetsPx(
    val top: Int,
    val right: Int,
    val bottom: Int,
    val left: Int,
)

internal data class InsetSnapshot(
    val safeTopPx: Int,
    val safeRightPx: Int,
    val safeBottomPx: Int,
    val safeLeftPx: Int,
    val imeBottomPx: Int,
    val imeVisible: Boolean,
) {
    fun withoutIme(): InsetSnapshot = copy(imeBottomPx = 0, imeVisible = false)
}

internal fun mergeInsetSnapshot(
    systemBars: EdgeInsetsPx,
    displayCutout: EdgeInsetsPx,
    imeBottomPx: Int,
    imeVisible: Boolean,
): InsetSnapshot {
    return InsetSnapshot(
        safeTopPx = maxOf(0, systemBars.top, displayCutout.top),
        safeRightPx = maxOf(0, systemBars.right, displayCutout.right),
        safeBottomPx = maxOf(0, systemBars.bottom, displayCutout.bottom),
        safeLeftPx = maxOf(0, systemBars.left, displayCutout.left),
        imeBottomPx = if (imeVisible) imeBottomPx.coerceAtLeast(0) else 0,
        imeVisible = imeVisible,
    )
}

internal fun netImeBottomPx(snapshot: InsetSnapshot): Int {
    if (!snapshot.imeVisible) return 0
    return (snapshot.imeBottomPx - snapshot.safeBottomPx).coerceAtLeast(0)
}

internal fun buildInsetsJavascript(snapshot: InsetSnapshot): String {
    val json = "{" +
        "\"safeTopPx\":${snapshot.safeTopPx}," +
        "\"safeRightPx\":${snapshot.safeRightPx}," +
        "\"safeBottomPx\":${snapshot.safeBottomPx}," +
        "\"safeLeftPx\":${snapshot.safeLeftPx}," +
        "\"imeBottomPx\":${snapshot.imeBottomPx}," +
        "\"imeVisible\":${snapshot.imeVisible}" +
        "}"
    return "window.__VCP_NATIVE_INSETS__ = $json; " +
        "window.dispatchEvent(new CustomEvent('vcp-keyboard-inset', " +
        "{ detail: window.__VCP_NATIVE_INSETS__ }))"
}
