package com.vcp.mobile.sharing

import android.content.Context
import android.app.Activity
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
import java.io.InputStream
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Future
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.atomic.AtomicBoolean
import androidx.core.content.IntentCompat
import app.tauri.annotation.InvokeArg

@InvokeArg
class AcknowledgeSharePayloadArgs {
    lateinit var ownerId: String
    var payloadRevision: Long = 0
    var settled: Boolean = false
}

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
): String = shareUploadStagingStem(ownerId, stagingTicket, hash)

internal fun shareSourceStagingName(ownerId: String, stagingTicket: String): String =
    "shared_${ownerId}_$stagingTicket"

internal class ShareIntentOwner {
    private val current = AtomicReference<String?>(null)

    fun begin(): String = UUID.randomUUID().toString().also(current::set)
    fun isCurrent(ownerId: String): Boolean = current.get() == ownerId
    fun currentId(): String? = current.get()
    fun invalidate() { current.set(null) }
}

internal data class PendingSharePayload<T>(val ownerId: String, val revision: Long, val value: T)

internal class LatestSharePayload<T> {
    private val owner = ShareIntentOwner()
    private var pending: PendingSharePayload<T>? = null
    private var revision = 0L

    @Synchronized
    fun begin(): String {
        val ownerId = owner.begin()
        pending = null
        revision = 0L
        return ownerId
    }

    fun isCurrent(ownerId: String): Boolean = owner.isCurrent(ownerId)
    fun currentId(): String? = owner.currentId()

    @Synchronized
    fun clear() {
        owner.invalidate()
        pending = null
    }

    @Synchronized
    fun publish(ownerId: String, value: T): Boolean {
        if (!owner.isCurrent(ownerId)) return false
        pending = PendingSharePayload(ownerId, ++revision, value)
        return true
    }

    @Synchronized
    fun peek(): PendingSharePayload<T>? = pending?.takeIf { owner.isCurrent(it.ownerId) }

    @Synchronized
    fun acknowledge(ownerId: String, revision: Long): Boolean {
        val snapshot = peek() ?: return false
        if (snapshot.ownerId != ownerId || snapshot.revision != revision) return false
        pending = null
        return true
    }
}

internal class ShareCopyBudget(
    private val maxFileBytes: Long = 512L * 1024 * 1024,
    private val maxTotalBytes: Long = 512L * 1024 * 1024,
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

/** Startup debris has no live owner only before this process first uses the directory. */
private object ShareStagingStartup {
    private val preparedDirectories = mutableSetOf<String>()

    @Synchronized
    fun prepare(directory: File) {
        val path = directory.canonicalPath
        if (path in preparedDirectories) return
        directory.listFiles()?.filter { it.isFile }?.forEach { it.delete() }
        preparedDirectories.add(path)
    }
}

private class ShareProviderRead {
    val cancellation = CancellationSignal()
    val input = AtomicReference<InputStream?>(null)
    private val cancelling = AtomicBoolean(false)

    fun cancel() {
        if (!cancelling.compareAndSet(false, true)) return
        // Provider binder cancellation and stream close must not block the UI thread.
        input.getAndSet(null)?.let { activeInput ->
            Thread({ runCatching { activeInput.close() } }, "vcp-share-close")
                .apply { isDaemon = true; start() }
        }
        Thread({
            runCatching { cancellation.cancel() }
        }, "vcp-share-cancel").apply { isDaemon = true; start() }
    }

    fun check() {
        if (cancelling.get() || Thread.currentThread().isInterrupted) {
            throw InterruptedException("分享读取已取消")
        }
    }
}

class ShareIntentHandler(
    private val activity: Activity,
    private val fileExecutor: ExecutorService,
    private val currentWebView: () -> WebView?,
) {

    companion object {
        private const val TAG = "ShareIntentHandler"
        private const val MAX_SHARE_FILES = 16
        private const val MAX_TEXT_BYTES = 1024 * 1024
        private const val MIN_FREE_BYTES = 64L * 1024 * 1024
    }

    // owner 与 WebView 未就绪时的 payload 由同一个原子槽管理，新 intent 会同步清空旧 payload。
    private val sharePayload = LatestSharePayload<JSObject>()
    private val stagedFilesByOwner = ConcurrentHashMap<String, MutableSet<File>>()
    private val activeRead = AtomicReference<ShareProviderRead?>(null)
    private val destroyed = AtomicBoolean(false)
    private val initialIntentObserved = AtomicBoolean(false)
    @Volatile
    private var activeTask: Future<*>? = null

    init {
        try {
            fileExecutor.execute {
                ShareStagingStartup.prepare(File(activity.cacheDir, "shared"))
            }
        } catch (error: RejectedExecutionException) {
            Log.w(TAG, "[init] Unable to queue legacy share staging cleanup", error)
        }
    }

    fun isCurrentOwner(ownerId: String): Boolean = sharePayload.isCurrent(ownerId)
    fun currentOwnerId(): String? = sharePayload.currentId()

    fun readPendingPayload(): JSObject? {
        val snapshot = sharePayload.peek() ?: return null
        return JSObject().apply {
            snapshot.value.keys().forEach { key -> put(key, snapshot.value.get(key)) }
            put("payloadRevision", snapshot.revision)
        }
    }

    fun acknowledgePayload(args: AcknowledgeSharePayloadArgs) {
        if (sharePayload.acknowledge(args.ownerId, args.payloadRevision) && args.settled) {
            cleanupOwnerStaging(args.ownerId)
        }
    }

    fun claimStagedFile(ownerId: String, file: File): Boolean {
        if (!sharePayload.isCurrent(ownerId)) return false
        val files = stagedFilesByOwner[ownerId] ?: return false
        return synchronized(files) {
            if (!sharePayload.isCurrent(ownerId) || stagedFilesByOwner[ownerId] !== files) return@synchronized false
            val claimed = files.remove(file)
            if (files.isEmpty()) stagedFilesByOwner.remove(ownerId, files)
            claimed
        }
    }

    /**
     * 入口：由 VcpMobilePlugin.onNewIntent 调用
     */
    fun handleInitialIntent(intent: Intent) {
        if (!initialIntentObserved.compareAndSet(false, true)) return
        handleShareIntent(intent)
    }

    fun handleShareIntent(intent: Intent) {
        if (destroyed.get()) return
        initialIntentObserved.set(true)
        val action = intent.action
        if (action != Intent.ACTION_SEND &&
            action != Intent.ACTION_SEND_MULTIPLE &&
            action != Intent.ACTION_PROCESS_TEXT) {
            Log.d(TAG, "[handleShareIntent] Ignoring non-share intent: $action")
            return
        }

        val ownerId = sharePayload.begin()
        activeTask?.cancel(true)
        activeRead.getAndSet(null)?.cancel()
        stagedFilesByOwner.keys.toList().forEach(::cleanupOwnerStaging)
        // Acceptance invalidates an older draft before provider IO starts.
        val receiving = JSObject().apply {
            put("intentId", ownerId)
            put("state", "receiving")
            put("text", "")
            put("files", JSArray())
        }
        sharePayload.publish(ownerId, receiving)
        injectShareData(currentWebView())
        Log.i(TAG, "[handleShareIntent] Queuing share intent owner=$ownerId, type=${intent.type}, action=$action")

        try {
            activeTask = fileExecutor.submit {
                var extraction: ShareExtraction? = null
                try {
                    extraction = extractSharedContent(intent, activity, ownerId)
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
                    activity.runOnUiThread {
                        if (!sharePayload.isCurrent(ownerId)) {
                            cleanupOwnerStaging(ownerId)
                            return@runOnUiThread
                        }
                        if (!sharePayload.publish(ownerId, completed.data)) {
                            cleanupOwnerStaging(ownerId)
                            return@runOnUiThread
                        }
                        val webView = currentWebView()
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
                        activity.runOnUiThread {
                            val failed = JSObject().apply {
                                put("intentId", ownerId)
                                put("state", "failed")
                                put("error", error.message ?: "无法读取分享内容")
                                put("text", "")
                                put("files", JSArray())
                            }
                            if (sharePayload.publish(ownerId, failed)) injectShareData(currentWebView())
                        }
                    }
                }
            }
        } catch (error: RejectedExecutionException) {
            Log.e(TAG, "[handleShareIntent] File executor rejected owner=$ownerId", error)
            val failed = JSObject().apply {
                put("intentId", ownerId)
                put("state", "failed")
                put("error", "分享读取队列已满或已关闭")
                put("text", "")
                put("files", JSArray())
            }
            if (sharePayload.publish(ownerId, failed)) injectShareData(currentWebView())
        }
    }

    /**
     * 内部：提取文本和文件 URI
     */
    private fun extractSharedContent(intent: Intent, context: Context, ownerId: String): ShareExtraction {
        val root = JSObject()
        root.put("intentId", ownerId)
        root.put("state", "ready")

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

        require(combinedText.toByteArray(Charsets.UTF_8).size <= MAX_TEXT_BYTES) { "分享文字超过 1 MiB" }
        root.put("text", combinedText.ifBlank { "" })

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
        val read = ShareProviderRead()
        val cancellation = read.cancellation
        activeRead.set(read)
        val timeoutHandler = Handler(Looper.getMainLooper())
        val cancelOnDeadline = Runnable { read.cancel() }
        timeoutHandler.postDelayed(cancelOnDeadline, 30_000L)

        try {
            if (!sharePayload.isCurrent(ownerId)) throw InterruptedException("分享 intent 已被更新")
            read.check()
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
            // A replacement Handler can run before an older init task. Both must
            // pass this once-per-process boundary before any owned staging is created.
            ShareStagingStartup.prepare(sharedDir)
            if (sharedDir.usableSpace < MIN_FREE_BYTES) {
                throw IllegalStateException("分享 staging 可用空间不足")
            }
            val ticket = UUID.randomUUID().toString()
            val outputFile = File(sharedDir, shareSourceStagingName(ownerId, ticket))

            try {
                val descriptor = contentResolver.openAssetFileDescriptor(uri, "r", cancellation)
                    ?: throw IllegalStateException("无法打开分享内容")
                descriptor.use {
                    it.createInputStream().use { input ->
                        read.input.set(input)
                        read.check()
                        try {
                            outputFile.outputStream().use { output ->
                                val buffer = ByteArray(65536)
                                while (true) {
                                    read.check()
                                    val bytesRead = input.read(buffer)
                                    read.check()
                                    if (bytesRead == -1) break
                                    budget.consume(bytesRead)
                                    if (!sharePayload.isCurrent(ownerId)) {
                                        read.cancel()
                                        throw InterruptedException("分享 intent 已被更新")
                                    }
                                    if (outputFile.parentFile?.usableSpace ?: 0L < MIN_FREE_BYTES) {
                                        read.cancel()
                                        throw IllegalStateException("分享 staging 可用空间不足")
                                    }
                                    output.write(buffer, 0, bytesRead)
                                }
                            }
                        } finally {
                            read.input.compareAndSet(input, null)
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
            activeRead.compareAndSet(read, null)
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
        val owned = synchronized(files) { files.toList().also { files.clear() } }
        cleanupStagedFiles(owned)
    }

    fun destroy() {
        if (!destroyed.compareAndSet(false, true)) return
        sharePayload.clear()
        activeTask?.cancel(true)
        activeTask = null
        activeRead.getAndSet(null)?.cancel()
        stagedFilesByOwner.keys.toList().forEach(::cleanupOwnerStaging)
    }

    /**
     * 通知只唤醒前端拉取；快照直到 Rust 持久化后精确确认才消费。
     */
    fun injectShareData(webView: WebView?) {
        if (webView == null || destroyed.get()) return

        try {
            if (sharePayload.peek() == null) return
            webView.evaluateJavascript("window.dispatchEvent(new Event('vcp-share-intent'))", null)
        } catch (e: Exception) {
            Log.e(TAG, "[injectShareData] Failed to inject share data", e)
        }
    }

}
