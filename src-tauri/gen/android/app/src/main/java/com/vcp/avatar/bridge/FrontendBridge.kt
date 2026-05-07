package com.vcp.avatar.bridge

import android.webkit.WebView

/**
 * 前端事件桥接器
 *
 * 统一封装 WebView.evaluateJavascript，将 Android 原生事件以 CustomEvent 形式
 * 注入到前端 window 对象。所有需要向前端推送数据的模块都应通过此类。
 *
 * 使用示例：
 *   frontendBridge.emit("vcp-keyboard-inset", mapOf("height" to 320, "visible" to true))
 */
class FrontendBridge {
    private var webView: WebView? = null

    fun attachWebView(webView: WebView) {
        this.webView = webView
    }

    fun detachWebView() {
        this.webView = null
    }

    /**
     * 向前端发射自定义事件。
     *
     * @param eventName 事件名称，前端通过 window.addEventListener(eventName, ...) 监听
     * @param detail    事件载荷，支持 String、Number、Boolean 及嵌套 Map/List
     */
    fun emit(eventName: String, detail: Map<String, Any?>) {
        val json = serializeValue(detail)
        val script = "window.dispatchEvent(new CustomEvent('$eventName', { detail: $json }))"
        webView?.evaluateJavascript(script, null)
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
            is List<*> -> {
                val items = value.joinToString(", ") { serializeValue(it) }
                "[ $items ]"
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
