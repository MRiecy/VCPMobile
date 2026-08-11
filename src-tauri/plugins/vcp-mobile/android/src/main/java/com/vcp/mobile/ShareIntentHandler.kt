package com.vcp.mobile

import android.content.Context
import android.content.Intent
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.net.Uri
import android.util.Log
import android.webkit.WebView
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Future
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicReference
import androidx.core.content.IntentCompat

internal fun sanitizeSharedFileName(rawName: String): String {
    val leaf = rawName.replace('\\', '/').substringAfterLast('/').trim()
    val sanitized = leaf
        .filterNot { it.code < 0x20 || it.code == 0x7f }
        .take(160)
    return if (sanitized.isBlank() || sanitized == "." || sanitized == "..") {
        "shared_file"
    } else {
        sanitized
    }
}

internal fun shareUploadStagingStem(ownerId: String, stagingTicket: String, hash: String): String =
    "shared_${ownerId}_${stagingTicket}_$hash"

internal fun shareUploadStagingName(
    ownerId: String,
    stagingTicket: String,
    hash: String,
    fileExtension: String,
): String = "${shareUploadStagingStem(ownerId, stagingTicket, hash)}$fileExtension"

internal class ShareIntentOwner {
    private val current = AtomicReference<String?>(null)

    fun begin(): String = UUID.randomUUID().toString().also(current::set)
    fun isCurrent(ownerId: String): Boolean = current.get() == ownerId
}

internal data class PendingSharePayload<T>(val ownerId: String, val value: T)

internal class LatestSharePayload<T> {
    private val owner = ShareIntentOwner()
    private var pending: PendingSharePayload<T>? = null

    @Synchronized
    fun begin(): String {
        val ownerId = owner.begin()
        pending = null
        return ownerId
    }

    fun isCurrent(ownerId: String): Boolean = owner.isCurrent(ownerId)

    @Synchronized
    fun publish(ownerId: String, value: T): Boolean {
        if (!owner.isCurrent(ownerId)) return false
        pending = PendingSharePayload(ownerId, value)
        return true
    }

    @Synchronized
    fun consumeCurrent(consumer: (T) -> Unit): Boolean {
        val snapshot = pending ?: return false
        if (!owner.isCurrent(snapshot.ownerId)) {
            pending = null
            return false
        }
        consumer(snapshot.value)
        if (pending === snapshot) pending = null
        return true
    }
}

internal class ShareCopyBudget(
    private val maxFileBytes: Long = 100L * 1024 * 1024,
    private val maxTotalBytes: Long = 200L * 1024 * 1024,
    private val globalDeadlineNanos: Long = System.nanoTime() + 60_000_000_000L,
    private val perFileNanos: Long = 30_000_000_000L,
) {
    private var fileBytes = 0L
    private var totalBytes = 0L
    private var fileDeadlineNanos = globalDeadlineNanos

    fun startFile(knownSize: Long, nowNanos: Long = System.nanoTime()) {
        if (knownSize > maxFileBytes || knownSize > maxTotalBytes - totalBytes) {
            throw IllegalArgumentException("分享文件超过允许的大小预算")
        }
        fileBytes = 0L
        fileDeadlineNanos = minOf(globalDeadlineNanos, nowNanos + perFileNanos)
    }

    fun consume(bytes: Int, nowNanos: Long = System.nanoTime()) {
        if (Thread.currentThread().isInterrupted) {
            throw InterruptedException("分享读取已取消")
        }
        if (nowNanos > fileDeadlineNanos || nowNanos > globalDeadlineNanos) {
            throw IllegalStateException("分享读取超时")
        }
        fileBytes += bytes
        totalBytes += bytes
        if (fileBytes > maxFileBytes || totalBytes > maxTotalBytes) {
            throw IllegalArgumentException("分享内容超过允许的大小预算")
        }
    }
}

private data class ShareExtraction(val data: JSObject, val stagedFiles: List<File>)

class ShareIntentHandler(
    private val plugin: VcpMobilePlugin,
    private val fileExecutor: ExecutorService,
) {

    companion object {
        private const val TAG = "ShareIntentHandler"
        private const val MAX_SHARE_FILES = 16
        private const val MAX_TEXT_CHARS = 1_000_000
        private const val MIN_FREE_BYTES = 64L * 1024 * 1024
    }

    // owner 与 WebView 未就绪时的 payload 由同一个原子槽管理，新 intent 会同步清空旧 payload。
    private val sharePayload = LatestSharePayload<JSObject>()
    private val stagedFilesByOwner = ConcurrentHashMap<String, MutableSet<File>>()
    private val activeCancellation = AtomicReference<CancellationSignal?>(null)
    @Volatile
    private var activeTask: Future<*>? = null

    init {
        try {
            fileExecutor.execute {
                File(plugin.pluginActivity.cacheDir, "shared")
                    .listFiles()
                    ?.filter { it.isFile }
                    ?.forEach { it.delete() }
            }
        } catch (error: RejectedExecutionException) {
            Log.w(TAG, "[init] Unable to queue legacy share staging cleanup", error)
        }
    }

    fun isCurrentOwner(ownerId: String): Boolean = sharePayload.isCurrent(ownerId)

    fun claimStagedFile(ownerId: String, file: File): Boolean {
        if (!sharePayload.isCurrent(ownerId)) return false
        val files = stagedFilesByOwner[ownerId] ?: return false
        val claimed = synchronized(files) { files.remove(file) }
        if (files.isEmpty()) stagedFilesByOwner.remove(ownerId, files)
        return claimed
    }

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

        activeTask?.cancel(true)
        activeCancellation.getAndSet(null)?.cancel()
        stagedFilesByOwner.entries.forEach { (_, files) -> cleanupStagedFiles(files.toList()) }
        stagedFilesByOwner.clear()
        val ownerId = sharePayload.begin()
        Log.i(TAG, "[handleShareIntent] Queuing share intent owner=$ownerId, type=${intent.type}, action=$action")

        try {
            activeTask = fileExecutor.submit {
                var extraction: ShareExtraction? = null
                try {
                    extraction = extractSharedContent(intent, plugin.pluginActivity, ownerId)
                    if (!sharePayload.isCurrent(ownerId)) {
                        cleanupStagedFiles(extraction.stagedFiles)
                        return@submit
                    }
                    stagedFilesByOwner[ownerId] = extraction.stagedFiles.toMutableSet()
                    if (!sharePayload.isCurrent(ownerId)) {
                        cleanupOwnerStaging(ownerId)
                        return@submit
                    }
                    val completed = extraction
                    plugin.pluginActivity.runOnUiThread {
                        if (!sharePayload.isCurrent(ownerId)) {
                            cleanupOwnerStaging(ownerId)
                            return@runOnUiThread
                        }
                        if (!sharePayload.publish(ownerId, completed.data)) {
                            cleanupOwnerStaging(ownerId)
                            return@runOnUiThread
                        }
                        val webView = plugin.webViewRef
                        if (webView != null) {
                            injectShareData(webView)
                        } else {
                            Log.w(TAG, "[handleShareIntent] WebView not ready, caching owner=$ownerId")
                        }
                    }
                } catch (error: Throwable) {
                    extraction?.let { cleanupStagedFiles(it.stagedFiles) }
                    if (sharePayload.isCurrent(ownerId)) {
                        Log.e(TAG, "[handleShareIntent] Share extraction failed for owner=$ownerId", error)
                    }
                }
            }
        } catch (error: RejectedExecutionException) {
            Log.e(TAG, "[handleShareIntent] File executor rejected owner=$ownerId", error)
        }
    }

    /**
     * 内部：提取文本和文件 URI
     */
    private fun extractSharedContent(intent: Intent, context: Context, ownerId: String): ShareExtraction {
        val root = JSObject()
        root.put("intentId", ownerId)

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

        root.put("text", combinedText.take(MAX_TEXT_CHARS).ifBlank { "" })

        // 提取文件 URIs
        val files = JSArray()
        val uris: List<Uri> = if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            IntentCompat.getParcelableArrayListExtra(
                intent,
                Intent.EXTRA_STREAM,
                Uri::class.java,
            ) ?: emptyList()
        } else {
            val uri = IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
            if (uri == null) emptyList() else listOf(uri)
        }
        if (uris.size > MAX_SHARE_FILES) {
            throw IllegalArgumentException("单次最多分享 $MAX_SHARE_FILES 个文件")
        }

        val stagedFiles = mutableListOf<File>()
        val budget = ShareCopyBudget()
        try {
            for (uri in uris) {
                if (!sharePayload.isCurrent(ownerId)) throw InterruptedException("分享 intent 已被更新")
                val (fileInfo, stagedFile) = copyStreamToCache(uri, context, ownerId, budget)
                files.put(fileInfo)
                stagedFiles += stagedFile
            }
        } catch (error: Throwable) {
            cleanupStagedFiles(stagedFiles)
            throw error
        }

        root.put("files", files)
        val isDebug = (context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0
        if (isDebug) {
            Log.i(TAG, "[extractSharedContent] text=${combinedText.take(120)}, fileCount=${files.length()}")
        } else {
            Log.i(TAG, "[extractSharedContent] textLength=${combinedText.length}, fileCount=${files.length()}")
        }
        return ShareExtraction(root, stagedFiles)
    }

    /**
     * 内部：将 content:// URI 复制到 app cache 目录
     */
    private fun copyStreamToCache(
        uri: Uri,
        context: Context,
        ownerId: String,
        budget: ShareCopyBudget,
    ): Pair<JSObject, File> {
        val contentResolver = context.contentResolver
        val cancellation = CancellationSignal()
        activeCancellation.set(cancellation)
        val timeoutHandler = Handler(Looper.getMainLooper())
        val cancelOnDeadline = Runnable { cancellation.cancel() }
        timeoutHandler.postDelayed(cancelOnDeadline, 30_000L)

        try {
            // 获取文件名和 MIME
            var fileName = "shared_file"
            val mimeType = contentResolver.getType(uri) ?: "application/octet-stream"
            var knownSize = -1L

            contentResolver.query(uri, null, null, null, null, cancellation)?.use { cursor ->
                val nameIndex = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
                val sizeIndex = cursor.getColumnIndex(android.provider.OpenableColumns.SIZE)
                if (cursor.moveToFirst()) {
                    if (nameIndex != -1) {
                        val name = cursor.getString(nameIndex)
                        if (name != null) fileName = name
                    }
                    if (sizeIndex != -1 && !cursor.isNull(sizeIndex)) {
                        knownSize = cursor.getLong(sizeIndex)
                    }
                }
            }
            fileName = sanitizeSharedFileName(fileName)
            budget.startFile(knownSize)

            val sharedDir = File(context.cacheDir, "shared").apply { mkdirs() }
            if (sharedDir.usableSpace < MIN_FREE_BYTES) {
                throw IllegalStateException("分享 staging 可用空间不足")
            }
            val ticket = UUID.randomUUID().toString()
            val outputFile = File(sharedDir, "shared_${ownerId}_${ticket}_$fileName")

            try {
                val descriptor = contentResolver.openAssetFileDescriptor(uri, "r", cancellation)
                    ?: throw IllegalStateException("无法打开分享内容")
                descriptor.use {
                    it.createInputStream().use { input ->
                        outputFile.outputStream().use { output ->
                            val buffer = ByteArray(65536)
                            while (true) {
                                val bytesRead = input.read(buffer)
                                if (bytesRead == -1) break
                                budget.consume(bytesRead)
                                if (!sharePayload.isCurrent(ownerId)) {
                                    cancellation.cancel()
                                    throw InterruptedException("分享 intent 已被更新")
                                }
                                if (outputFile.parentFile?.usableSpace ?: 0L < MIN_FREE_BYTES) {
                                    cancellation.cancel()
                                    throw IllegalStateException("分享 staging 可用空间不足")
                                }
                                output.write(buffer, 0, bytesRead)
                            }
                        }
                    }
                }
                val canonicalOutput = outputFile.canonicalFile
                val fileInfo = JSObject()
                fileInfo.put("cachePath", canonicalOutput.absolutePath)
                fileInfo.put("mimeType", mimeType)
                fileInfo.put("fileName", fileName)
                fileInfo.put("size", canonicalOutput.length())
                fileInfo.put("stagingTicket", ticket)

                val isDebug = (context.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0
                if (isDebug) {
                    Log.i(TAG, "[copyStreamToCache] Copied shared file: name=${fileName.take(80)}, size=${canonicalOutput.length()}, mime=$mimeType")
                } else {
                    Log.i(TAG, "[copyStreamToCache] Copied shared file: size=${canonicalOutput.length()}, mime=$mimeType")
                }
                return fileInfo to canonicalOutput
            } catch (error: Throwable) {
                outputFile.delete()
                throw error
            }
        } finally {
            activeCancellation.compareAndSet(cancellation, null)
            timeoutHandler.removeCallbacks(cancelOnDeadline)
        }
    }

    private fun cleanupStagedFiles(files: List<File>) {
        files.forEach { file ->
            try {
                file.delete()
            } catch (_: Exception) {
            }
        }
    }

    private fun cleanupOwnerStaging(ownerId: String) {
        val files = stagedFilesByOwner.remove(ownerId) ?: return
        cleanupStagedFiles(files.toList())
    }

    /**
     * 通过 evaluateJavascript 注入 WebView
     */
    fun injectShareData(webView: WebView?) {
        if (webView == null) return

        try {
            val delivered = sharePayload.consumeCurrent { data ->
                @Suppress("DEPRECATION")
                val dataJson = data.toString()
                val safeJson = escapeJsonForJsString(dataJson)
                val script = "window.dispatchEvent(new CustomEvent('vcp-share-intent', { detail: JSON.parse(\"$safeJson\") }))"
                webView.evaluateJavascript(script, null)
            }
            if (delivered) {
                Log.i(TAG, "[injectShareData] Share data injected into WebView successfully")
            } else {
                Log.d(TAG, "[injectShareData] No pending share data")
            }
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
