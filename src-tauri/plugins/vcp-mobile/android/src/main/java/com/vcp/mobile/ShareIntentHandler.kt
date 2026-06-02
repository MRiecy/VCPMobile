package com.vcp.mobile

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import android.webkit.WebView
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import java.io.File

class ShareIntentHandler(private val plugin: VcpMobilePlugin) {

    companion object {
        private const val TAG = "ShareIntentHandler"
    }

    // WebView 未就绪时缓存待注入数据
    private var pendingShareData: JSObject? = null

    /**
     * 入口：由 VcpMobilePlugin.onNewIntent 调用
     */
    fun handleShareIntent(intent: Intent) {
        val action = intent.action
        if (action != Intent.ACTION_SEND &&
            action != Intent.ACTION_SEND_MULTIPLE &&
            action != Intent.ACTION_PROCESS_TEXT) {
            Log.d(TAG, "[handleShareIntent] Ignoring non-share intent: $action")
            return
        }

        Log.i(TAG, "[handleShareIntent] Processing share intent: type=${intent.type}, action=$action")

        val context = plugin.pluginActivity
        val shareData = extractSharedContent(intent, context)

        // 尝试立即注入 WebView
        val webView = plugin.webViewRef
        pendingShareData = shareData
        if (webView != null) {
            injectShareData(webView)
        } else {
            Log.w(TAG, "[handleShareIntent] WebView not ready, caching share data")
        }
    }

    /**
     * 内部：提取文本和文件 URI
     */
    private fun extractSharedContent(intent: Intent, context: Context): JSObject {
        val root = JSObject()

        // ACTION_PROCESS_TEXT: 浏览器/阅读器选中文字菜单
        val processText = if (intent.action == Intent.ACTION_PROCESS_TEXT) {
            intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
        } else null

        val text = intent.getStringExtra(Intent.EXTRA_TEXT)
        val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)

        // 合并来源文本：PROCESS_TEXT > EXTRA_SUBJECT + EXTRA_TEXT
        val combinedText = buildString {
            if (!processText.isNullOrBlank()) {
                append(processText)
            } else {
                if (!subject.isNullOrBlank()) {
                    append(subject)
                }
                if (!text.isNullOrBlank()) {
                    if (isNotEmpty() && !text.startsWith(subject ?: "")) {
                        append("\n")
                    }
                    append(text)
                }
            }
        }

        root.put("text", combinedText.ifBlank { "" })

        // 提取文件 URIs
        val files = JSArray()

        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            @Suppress("DEPRECATION")
            val uris = intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
            if (uris != null) {
                for (uri in uris) {
                    val fileInfo = copyStreamToCache(uri, context)
                    if (fileInfo != null) {
                        files.put(fileInfo)
                    }
                }
            }
        } else {
            val uri = intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            if (uri != null) {
                val fileInfo = copyStreamToCache(uri, context)
                if (fileInfo != null) {
                    files.put(fileInfo)
                }
            }
        }

        root.put("files", files)
        Log.i(TAG, "[extractSharedContent] text=${combinedText.take(120)}, fileCount=${files.length()}")
        return root
    }

    /**
     * 内部：将 content:// URI 复制到 app cache 目录
     */
    private fun copyStreamToCache(uri: Uri, context: Context): JSObject? {
        try {
            val contentResolver = context.contentResolver

            // 获取文件名和 MIME
            var fileName = "shared_file"
            var mimeType = contentResolver.getType(uri) ?: "application/octet-stream"

            contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                val nameIndex = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
                if (nameIndex != -1 && cursor.moveToFirst()) {
                    val name = cursor.getString(nameIndex)
                    if (name != null) fileName = name
                }
            }

            // 写入 cacheDir/shared_{timestamp}_{filename}
            val sharedDir = File(context.cacheDir, "shared").apply { mkdirs() }
            val timestamp = System.currentTimeMillis()
            val targetFile = File(sharedDir, "shared_${timestamp}_$fileName")

            contentResolver.openInputStream(uri)?.use { input ->
                targetFile.outputStream().use { output ->
                    input.copyTo(output, bufferSize = 65536)
                }
            } ?: run {
                Log.w(TAG, "[copyStreamToCache] Failed to open input stream for: $uri")
                return null
            }

            val fileInfo = JSObject()
            fileInfo.put("cachePath", targetFile.absolutePath)
            fileInfo.put("mimeType", mimeType)
            fileInfo.put("fileName", fileName)
            fileInfo.put("size", targetFile.length())

            Log.i(TAG, "[copyStreamToCache] Copied: $fileName -> ${targetFile.absolutePath} (size=${targetFile.length()})")
            return fileInfo
        } catch (e: Exception) {
            Log.e(TAG, "[copyStreamToCache] Failed to copy stream", e)
            return null
        }
    }

    /**
     * 通过 evaluateJavascript 注入 WebView
     */
    fun injectShareData(webView: WebView?) {
        if (webView == null) return

        val data = pendingShareData
        if (data == null) {
            Log.d(TAG, "[injectShareData] No pending share data")
            return
        }

        try {
            @Suppress("DEPRECATION")
            val dataJson = data.toString()
            val safeJson = escapeJsonForJsString(dataJson)
            val script = "window.dispatchEvent(new CustomEvent('vcp-share-intent', { detail: JSON.parse(\"$safeJson\") }))"
            webView.evaluateJavascript(script, null)

            Log.i(TAG, "[injectShareData] Share data injected into WebView successfully")
            pendingShareData = null
        } catch (e: Exception) {
            Log.e(TAG, "[injectShareData] Failed to inject share data", e)
        }
    }

    /**
     * JSON 字符串转义，安全嵌入 JavaScript 字符串
     */
    private fun escapeJsonForJsString(json: String): String {
        return json
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("'", "\\'")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
    }
}
