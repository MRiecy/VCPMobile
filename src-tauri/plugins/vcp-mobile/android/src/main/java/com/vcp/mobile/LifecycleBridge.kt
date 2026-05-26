package com.vcp.mobile

import android.app.Activity
import android.content.res.Configuration
import android.webkit.WebView
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import java.lang.ref.WeakReference

/**
 * 应用生命周期桥接器
 *
 * 通过 DefaultLifecycleObserver 自动监听 Activity 生命周期，
 * 使用 evaluateJavascript 直接注入 window.CustomEvent，保持与前端 window.addEventListener 兼容。
 */
class LifecycleBridge : DefaultLifecycleObserver {

    private var webViewRef: WebView? = null
    private var activityRef: WeakReference<Activity>? = null

    fun attach(activity: Activity, webView: WebView) {
        webViewRef = webView
        activityRef = WeakReference(activity)
        if (activity is LifecycleOwner) {
            activity.lifecycle.addObserver(this)
        }
    }

    override fun onDestroy(owner: LifecycleOwner) {
        webViewRef = null
        activityRef = null
        owner.lifecycle.removeObserver(this)
        super.onDestroy(owner)
    }

    override fun onResume(owner: LifecycleOwner) {
        emit("vcp-lifecycle", mapOf("state" to "resume"))
    }

    override fun onPause(owner: LifecycleOwner) {
        emit("vcp-lifecycle", mapOf("state" to "pause"))
    }

    override fun onStop(owner: LifecycleOwner) {
        emit("vcp-lifecycle", mapOf("state" to "stop"))
    }

    fun onConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        emit("vcp-lifecycle", mapOf(
            "state" to "config-changed",
            "isDarkMode" to isDark
        ))
    }

    fun onLowMemory() {
        emit("vcp-lifecycle", mapOf("state" to "low-memory"))
    }

    private fun emit(eventName: String, detail: Map<String, Any?>) {
        val json = serializeValue(detail)
        val script = "window.dispatchEvent(new CustomEvent('$eventName', { detail: $json }))"
        val activity = activityRef?.get()
        if (activity != null) {
            activity.runOnUiThread {
                webViewRef?.evaluateJavascript(script, null)
            }
        } else {
            webViewRef?.post {
                webViewRef?.evaluateJavascript(script, null)
            }
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
}
