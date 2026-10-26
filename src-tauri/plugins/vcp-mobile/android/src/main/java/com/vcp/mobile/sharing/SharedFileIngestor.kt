package com.vcp.mobile.sharing

import android.app.Activity
import android.util.Log
import android.webkit.MimeTypeMap
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.PluginTaskRuntime
import com.vcp.mobile.files.copyExportStream

/** Claims one ticket from the existing share owner, then transfers its staged file to Rust. */
internal class SharedFileIngestor(
    private val activity: Activity,
    private val shareIntentHandler: ShareIntentHandler,
    private val runtime: PluginTaskRuntime,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }

    @Suppress("ASSIGNED_VALUE_IS_NEVER_READ")
    fun processSharedFile(invoke: Invoke) {
        val args = invoke.parseArgs(ProcessSharedFileArgs::class.java)
        val cachePath = args.cachePath
        val rawMimeType = args.mimeType
        val originalName = sanitizeSharedFileName(args.fileName)
        val ownerId = args.ownerId
        val stagingTicket = args.stagingTicket

        if (cachePath.isEmpty() || ownerId.isEmpty() || stagingTicket.isEmpty()) {
            invoke.deliverError("分享 staging 参数不完整")
            return
        }
        if (runCatching { java.util.UUID.fromString(ownerId) }.isFailure ||
            runCatching { java.util.UUID.fromString(stagingTicket) }.isFailure) {
            invoke.deliverError("分享 staging owner/ticket 格式非法")
            return
        }

        runtime.execute(runtime.domains.fileIoExecutor, invoke, "processSharedFile", fileTask@{
            var currentTempFile: java.io.File? = null
            var claimedSourceFile: java.io.File? = null
            try {
                if (runtime.isDestroying || !shareIntentHandler.isCurrentOwner(ownerId)) {
                    invoke.deliverError("分享 intent 已被更新")
                    return@fileTask
                }
                val context = activity
                val sharedRoot = java.io.File(context.cacheDir, "shared").apply { mkdirs() }.canonicalFile
                val sourceFile = java.io.File(cachePath).canonicalFile
                val expectedName = shareSourceStagingName(ownerId, stagingTicket)
                if (sourceFile.parentFile != sharedRoot || sourceFile.name != expectedName || !sourceFile.isFile) {
                    invoke.deliverError("拒绝处理不属于当前 intent ticket 的分享 staging 文件")
                    return@fileTask
                }
                if (!shareIntentHandler.claimStagedFile(ownerId, sourceFile)) {
                    invoke.deliverError("分享 staging ticket 已失效或已消费")
                    return@fileTask
                }
                claimedSourceFile = sourceFile
                if (!sourceFile.exists()) {
                    invoke.deliverError("Shared file not found at cache path: $cachePath")
                    return@fileTask
                }

                val size = sourceFile.length()
                if (size > 512L * 1024 * 1024) {
                    invoke.deliverError("分享文件超过单文件上限 (512 MiB)")
                    return@fileTask
                }
                var mimeType = rawMimeType
                if (mimeType.isNullOrBlank()) {
                    val ext = java.io.File(originalName).extension.lowercase()
                    mimeType = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
                }

                Log.i(TAG, "[processSharedFile] Processing shared file: size=$size, mime=$mimeType")

                // 直接在受控 staging 文件上流式重算 SHA-256，避免 shared → uploads 双倍占盘。
                val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
                val digest = java.security.MessageDigest.getInstance("SHA-256")

                sourceFile.inputStream().use { inputStream ->
                    val buffer = ByteArray(65536)
                    var totalBytes = 0L
                    var bytesRead = inputStream.read(buffer)
                    while (bytesRead != -1) {
                        if (runtime.isDestroying || Thread.currentThread().isInterrupted || !shareIntentHandler.isCurrentOwner(ownerId)) {
                            throw InterruptedException("分享 intent 已被更新")
                        }
                        totalBytes += bytesRead
                        require(totalBytes <= 512L * 1024 * 1024) { "分享文件超过单文件上限 (512 MiB)" }
                        digest.update(buffer, 0, bytesRead)
                        bytesRead = inputStream.read(buffer)
                    }
                }

                val hashBytes = digest.digest()
                val hash = hashBytes.joinToString("") { "%02x".format(it) }

                // uploads 只是交给 Rust ingest 的临时交接区。永久 CAS 去重由 Rust 负责；
                // owner/ticket 保证同内容的多个分享消费者不会争用同一个临时文件。
                val finalTempFile = java.io.File(
                    uploadsDir,
                    shareUploadStagingName(ownerId, stagingTicket, hash),
                )

                if (finalTempFile.exists()) {
                    // 目标异常占用时不能删除原始 share staging，也不能把未知旧内容返回给 Rust。
                    claimedSourceFile = null
                    throw IllegalStateException("分享 upload staging ticket 已存在")
                }
                currentTempFile = finalTempFile
                if (!sourceFile.renameTo(finalTempFile)) {
                    if (uploadsDir.usableSpace < size + 64L * 1024 * 1024) {
                        throw IllegalStateException("uploads staging 可用空间不足")
                    }
                    if (!finalTempFile.createNewFile()) {
                        currentTempFile = null
                        claimedSourceFile = null
                        throw IllegalStateException("分享 upload staging ticket 已存在")
                    }
                    sourceFile.inputStream().use { input ->
                        finalTempFile.outputStream().use { output -> copyExportStream(input, output, 512L * 1024 * 1024) }
                    }
                    if (!sourceFile.delete()) {
                        throw IllegalStateException("无法清理已复制的分享 source staging")
                    }
                }
                claimedSourceFile = null

                // Rust registers the original and requests the shared bounded
                // image service for derivatives; acquisition owns no thumbnail.

                if (runtime.isDestroying || Thread.currentThread().isInterrupted || !shareIntentHandler.isCurrentOwner(ownerId)) {
                    throw InterruptedException("分享 intent 已被更新")
                }

                // 组装结果
                val resultObject = JSObject()
                resultObject.put("path", finalTempFile.absolutePath)
                resultObject.put("name", originalName)
                resultObject.put("mime", mimeType)
                resultObject.put("size", finalTempFile.length())
                resultObject.put("hash", hash)

                Log.i(TAG, "[processSharedFile] Complete: size=${finalTempFile.length()}, mime=$mimeType")
                if (invoke.deliverResult(resultObject)) currentTempFile = null
            } catch (e: Throwable) {
                Log.e(TAG, "[processSharedFile] Failed", e)
                invoke.deliverError("Processing shared file failed: ${e.message}")
            } finally {
                // A claimed ticket is no longer owned by ShareIntentHandler. All early
                // returns and failed handoffs must release it here, exactly like errors.
                try {
                    currentTempFile?.delete()
                    claimedSourceFile?.delete()
                } catch (_: Exception) {}
            }
        })
    }
}

@InvokeArg
class ProcessSharedFileArgs {
    lateinit var cachePath: String
    var mimeType: String? = null
    lateinit var fileName: String
    lateinit var ownerId: String
    lateinit var stagingTicket: String
}
