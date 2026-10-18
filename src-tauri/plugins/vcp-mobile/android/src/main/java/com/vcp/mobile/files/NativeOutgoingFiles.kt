package com.vcp.mobile.files

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import com.vcp.mobile.runtime.PluginTaskRuntime
import java.util.concurrent.TimeUnit

/** External receivers own no callback; retained provider copies outlive the caller's source. */
internal class NativeOutgoingFiles(
    private val activity: Activity,
    private val runtime: PluginTaskRuntime,
) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    private val nativeOutgoingLock = Any()

    fun openFile(invoke: Invoke) {
        val args = invoke.parseArgs(OpenFileArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.deliverError("Path is empty")
            return
        }
        if (args.action != "view" && args.action != "share") {
            invoke.deliverError("Unsupported native file action")
            return
        }

        runtime.execute(runtime.domains.fileIoExecutor, invoke, "openFile", fileTask@{
            var retainedDirectory: java.io.File? = null
            var handedToUi = false
            try {
                val context = activity

                // 💥 安全边界拦截：禁止通过 openFile 访问沙箱外部物理文件
                if (!isSafeLocalPath(context, path)) {
                    invoke.deliverError("安全拒绝：禁止打开沙箱外部的敏感文件")
                    return@fileTask
                }

                val sourceFile = java.io.File(path)
                if (!sourceFile.exists()) {
                    invoke.deliverError("文件不存在: $path")
                    return@fileTask
                }

                // ACTION_VIEW/SEND has no trustworthy receiver-finished callback.
                // Give FileProvider its own copy, retaining it for a day; callers
                // may release their temporary source as soon as the chooser opens.
                val file = synchronized(nativeOutgoingLock) {
                    val exportRoot = java.io.File(context.cacheDir, "native-outgoing").apply { mkdirs() }
                    val cutoff = System.currentTimeMillis() - TimeUnit.DAYS.toMillis(1)
                    exportRoot.listFiles()?.filter { it.isDirectory && it.lastModified() < cutoff }?.forEach { it.deleteRecursively() }
                    val retainedBytes = exportRoot.walkTopDown().filter { it.isFile }.sumOf { it.length() }
                    val availableBytes = minOf(512L * 1024 * 1024, 1024L * 1024 * 1024 - retainedBytes)
                    require(availableBytes > 0 && sourceFile.length() <= availableBytes) { "分享暂存空间已满，请稍后重试" }
                    val exportDir = java.io.File(exportRoot, java.util.UUID.randomUUID().toString()).apply { mkdirs() }
                    retainedDirectory = exportDir
                    val displayName = args.fileName?.let { java.io.File(it).name }?.takeIf { it.isNotBlank() && it != "." && it != ".." } ?: sourceFile.name
                    val file = java.io.File(exportDir, displayName)
                    try {
                        sourceFile.inputStream().use { input -> file.outputStream().use { output -> copyExportStream(input, output, availableBytes) } }
                    } catch (error: Throwable) { exportDir.deleteRecursively(); throw error }
                    file
                }

                // 1. 自动提取并修正 MIME 类型
                val ext = file.extension.lowercase()
                val mimeType = when (ext) {
                    "log", "txt" -> "text/plain"
                    else -> MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
                }
                Log.i(TAG, "[openFile] Dispatching file action=${args.action}: ${file.absolutePath} (ext=$ext, mime=$mimeType)")

                // 2. 借助 FileProvider 生成临时读取授权的 content:// URI
                val uri = try {
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.fileprovider",
                        file
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "[openFile] Fallback to opener FileProvider authority", e)
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.opener.fileprovider",
                        file
                    )
                }

                // 3. 查看沿用 ACTION_VIEW；分享使用带临时 URI 权限的 ACTION_SEND。
                val intent = if (args.action == "share") {
                    val sendIntent = Intent(Intent.ACTION_SEND).apply {
                        type = mimeType
                        putExtra(Intent.EXTRA_STREAM, uri)
                        putExtra(Intent.EXTRA_TITLE, file.name)
                        clipData = ClipData.newRawUri(file.name, uri)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                    Intent.createChooser(sendIntent, "分享文件").apply {
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                } else {
                    Intent(Intent.ACTION_VIEW).apply {
                        setDataAndType(uri, mimeType)
                        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                }

                val ownedDirectory = checkNotNull(retainedDirectory)
                activity.runOnUiThread {
                    var opened = false
                    try {
                        check(!runtime.isDestroying) { "plugin is shutting down" }
                        activity.startActivity(intent)
                        // Receiver ownership starts at successful dispatch, independently
                        // of whether the old WebView can still consume the Invoke response.
                        opened = true
                        invoke.deliverResult()
                    } catch (error: Throwable) {
                        rejectOpenFailure(invoke, args, error)
                    } finally {
                        if (!opened) ownedDirectory.deleteRecursively()
                    }
                }
                handedToUi = true
            } catch (e: Throwable) {
                rejectOpenFailure(invoke, args, e)
            } finally {
                if (!handedToUi) retainedDirectory?.deleteRecursively()
            }
        })
    }

    private fun rejectOpenFailure(invoke: Invoke, args: OpenFileArgs, error: Throwable) {
        if (error is android.content.ActivityNotFoundException) {
            val ext = java.io.File(args.path).extension.lowercase()
            Log.e(TAG, "[openFile] No activity found for action=${args.action}, type=.$ext", error)
            invoke.deliverError(if (args.action == "share") {
                "您的手机上未安装可接收此文件的应用"
            } else {
                "您的手机上未安装能打开此类文件 (.$ext) 的应用，请先安装相关阅读器 (如 WPS Office)。"
            })
        } else {
            Log.e(TAG, "[openFile] Native file action=${args.action} failed", error)
            invoke.deliverError(if (args.action == "share") "分享文件失败: ${error.message}" else "打开文件失败: ${error.message}")
        }
    }
}

@InvokeArg
class OpenFileArgs {
    lateinit var path: String
    var action: String = "view"
    var fileName: String? = null
}
