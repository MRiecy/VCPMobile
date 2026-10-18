package com.vcp.mobile.files

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Environment
import android.os.CancellationSignal
import android.provider.DocumentsContract
import android.provider.MediaStore
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import app.tauri.plugin.JSObject
import com.vcp.mobile.media.DeriveImageArgs
import com.vcp.mobile.media.NativeFileImages
import com.vcp.mobile.runtime.PendingInvokeTask
import com.vcp.mobile.runtime.PluginTaskRuntime
import java.io.InputStream
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean

/** One chooser/camera operation owns its slot and all files until native acquisition settles. */
internal class NativeFilePicker(
    private val activity: Activity,
    private val runtime: PluginTaskRuntime,
    private val launchActivity: (Invoke, Intent, String) -> Unit,
    private val requestPermission: (String, Invoke, String) -> Unit,
    private val emitEvent: (String, JSObject) -> Unit,
    private val openSource: (Uri, CancellationSignal) -> InputStream? = { uri, cancellation ->
        val descriptor = activity.contentResolver.openAssetFileDescriptor(uri, "r", cancellation)
        if (descriptor == null) null else try {
            descriptor.createInputStream()
        } catch (error: Throwable) {
            descriptor.close()
            throw error
        }
    },
) {
    private class FileOperation(val args: PickFileArgs, val invoke: Invoke) {
        val cancelled = AtomicBoolean(false)
        val settled = AtomicBoolean(false)
        val cancellation = CancellationSignal()
        val input = java.util.concurrent.atomic.AtomicReference<InputStream?>(null)
        val cameraFile = java.util.concurrent.atomic.AtomicReference<java.io.File?>(null)
        fun check() { if (cancelled.get() || Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("cancelled: file operation") }
        fun reject(message: String) { if (settled.compareAndSet(false, true)) invoke.deliverError(message) }
        fun resolve(value: JSObject) { if (settled.compareAndSet(false, true)) invoke.deliverResult(value) }
        fun cancel() {
            if (!cancelled.compareAndSet(false, true)) return
            // A provider cancellation/close can itself block. Never do it on the UI thread.
            input.getAndSet(null)?.let { activeInput ->
                Thread({ runCatching { activeInput.close() } }, "vcp-picker-close")
                    .apply { isDaemon = true; start() }
            }
            Thread({
                runCatching { cancellation.cancel() }
            }, "vcp-picker-cancel").apply { isDaemon = true; start() }
        }
    }
    private val fileOperation = java.util.concurrent.atomic.AtomicReference<FileOperation?>(null)

    fun cancelFileOperation(invoke: Invoke) {
        val args = invoke.parseArgs(CancelFileOperationArgs::class.java)
        fileOperation.get()?.takeIf { it.args.operationId == args.operationId }?.let {
            it.cancel()
            // Android cannot close another application's chooser. Retain this
            // slot until its callback, but settle the user wait exactly once.
            it.reject("cancelled: file operation")
        }
        invoke.deliverResult()
    }

    fun onCameraPermissionResult(invoke: Invoke) {
        val operation = fileOperation.get()?.takeIf { it.invoke === invoke } ?: return
        if (operation.cancelled.get()) { finishFileOperation(operation); return }
        if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCameraIntent(invoke)
        } else {
            operation.reject("camera_permission: Camera permission denied")
            finishFileOperation(operation)
        }
    }

    private fun finishFileOperation(operation: FileOperation) {
        fileOperation.compareAndSet(operation, null)
    }

    private fun launchCameraIntent(invoke: Invoke) {
        val operation = fileOperation.get()?.takeIf { it.invoke === invoke } ?: return
        try {
            operation.check()
            val uploads = java.io.File(operation.args.stagingDir).apply { mkdirs() }
            val file = java.io.File(uploads, "${operation.args.operationId}-${java.util.UUID.randomUUID()}-camera.jpg")
            operation.cameraFile.set(file)
            val uri = try {
                FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
            } catch (_: Exception) {
                FileProvider.getUriForFile(activity, "${activity.packageName}.opener.fileprovider", file)
            }
            val intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE).apply {
                putExtra(MediaStore.EXTRA_OUTPUT, uri)
                addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            }
            launchActivity(invoke, intent, "onCameraResult")
        } catch (error: Throwable) {
            operation.cameraFile.getAndSet(null)?.delete()
            operation.reject("camera_capture: ${error.message}")
            finishFileOperation(operation)
        }
    }

    fun pickFile(invoke: Invoke) {
        if (runtime.isDestroying) { invoke.deliverError("cancelled: plugin destroyed"); return }
        val args = invoke.parseArgs(PickFileArgs::class.java)
        val operation = FileOperation(args, invoke)
        if (!fileOperation.compareAndSet(null, operation)) { invoke.deliverError("busy: native picker is still active"); return }
        try {
            require(args.operationId.matches(Regex("[A-Za-z0-9_-]{1,100}"))) { "invalid_operation_id" }
            require(args.maxBytes in 1..(512L * 1024 * 1024)) { "invalid_file_budget" }
            val expected = java.io.File(activity.cacheDir, "uploads").canonicalFile
            require(java.io.File(args.stagingDir).canonicalFile == expected) { "invalid_staging_directory" }
            when (args.mode) {
                "camera" -> {
                    if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                        requestPermission("camera", invoke, "onCameraPermissionResult")
                    } else { launchCameraIntent(invoke) }
                }
                else -> {
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                        type = if (args.mode == "gallery" || args.mode == "avatar") "image/*" else "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                        putExtra(Intent.EXTRA_ALLOW_MULTIPLE, args.selectionLimit > 1)
                        if (args.mode == "config") {
                            // Providers may classify the legacy .vcpcfg extension as binary.
                            // Both backup generations use this picker and the same content validator.
                            putExtra(Intent.EXTRA_MIME_TYPES, arrayOf(
                                "application/json", "application/vnd.vcp.config+json", "application/octet-stream"
                            ))
                            putExtra(DocumentsContract.EXTRA_INITIAL_URI, DocumentsContract.buildDocumentUri(
                                "com.android.externalstorage.documents", "primary:${Environment.DIRECTORY_DOWNLOADS}/VCPMobile"
                            ))
                        }
                    }
                    launchActivity(invoke, intent, "onPickFileResult")
                }
            }
        } catch (error: Throwable) {
            operation.reject("native_picker: ${error.message}")
            finishFileOperation(operation)
        }
    }

    fun onCameraResult(invoke: Invoke, result: ActivityResult) {
        val operation = fileOperation.get()?.takeIf { it.invoke === invoke } ?: return
        val file = operation.cameraFile.getAndSet(null)
        if (result.resultCode != Activity.RESULT_OK || operation.cancelled.get()) {
            file?.delete(); operation.reject("cancelled: camera"); finishFileOperation(operation); return
        }
        if (file == null || !file.isFile) { operation.reject("camera_capture: no output file"); finishFileOperation(operation); return }
        acquirePickedFiles(operation, listOf(Uri.fromFile(file)), "Camera.jpg", "image/jpeg", file)
    }

    fun onPickFileResult(invoke: Invoke, result: ActivityResult) {
        val operation = fileOperation.get()?.takeIf { it.invoke === invoke } ?: return
        if (result.resultCode != Activity.RESULT_OK || operation.cancelled.get()) {
            operation.reject("cancelled: file picker"); finishFileOperation(operation); return
        }
        val data = result.data
        val uris = linkedSetOf<Uri>()
        val clip = data?.clipData
        if (clip != null) {
            for (index in 0 until clip.itemCount) clip.getItemAt(index).uri?.let { uris.add(it) }
        }
        if (uris.isEmpty()) data?.data?.let { uris.add(it) }
        if (uris.isEmpty()) { operation.reject("cancelled: no file selected"); finishFileOperation(operation); return }
        acquirePickedFiles(operation, uris.take(operation.args.selectionLimit), null, null, null,
            (uris.size - operation.args.selectionLimit).coerceAtLeast(0))
    }

    private fun acquirePickedFiles(operation: FileOperation, uris: List<Uri>, suppliedName: String?, suppliedMime: String?, ownedSource: java.io.File?, overflow: Int = 0) {
        val task = PendingInvokeTask(operation.invoke, "acquirePickedFiles", task = {
            val files = mutableListOf<JSObject>()
            val errors = org.json.JSONArray()
            val owned = mutableListOf<java.io.File>()
            var delivered = false
            var remainingBytes = operation.args.maxBytes
            try {
                if (overflow > 0) errors.put(JSObject().apply {
                    put("name", "其余 $overflow 项"); put("message", "每次最多选择 ${operation.args.selectionLimit} 个附件")
                })
                for ((index, uri) in uris.withIndex()) {
                    var staging: java.io.File? = null
                    var derivative: java.io.File? = null
                    var name = suppliedName ?: "文件 ${index + 1}"
                    try {
                        operation.check()
                        val resolver = activity.contentResolver
                        if (suppliedName == null) resolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null, operation.cancellation)?.use {
                            if (it.moveToFirst()) name = it.getString(0) ?: "file"
                        }
                        name = java.io.File(name).name.take(150).ifBlank { "file" }
                        val mime = suppliedMime ?: resolver.getType(uri) ?: "application/octet-stream"
                        val declaredSize = if (ownedSource != null) ownedSource.length() else {
                            var declared = 0L
                            resolver.query(uri, arrayOf(android.provider.OpenableColumns.SIZE), null, null, null, operation.cancellation)?.use {
                                if (it.moveToFirst() && !it.isNull(0)) declared = it.getLong(0).coerceAtLeast(0L)
                            }
                            declared
                        }
                        emitFileProgress(operation, "vcp-mobile-file-start", JSObject().apply {
                            put("name", name); put("mime", mime); put("size", declaredSize)
                            put("selectionIndex", index); put("selectionCount", uris.size)
                        })
                        val uploads = java.io.File(operation.args.stagingDir).apply { mkdirs() }
                        val extension = java.io.File(name).extension.filter { it in 'A'..'Z' || it in 'a'..'z' || it in '0'..'9' }.take(12)
                        val target = java.io.File(uploads, "${operation.args.operationId}-${java.util.UUID.randomUUID()}" + if (extension.isEmpty()) "" else ".$extension")
                        staging = target
                        val digest = java.security.MessageDigest.getInstance("SHA-256")
                        var size = 0L
                        var lastProgress = 0L
                        openSource(uri, operation.cancellation).use { input ->
                            requireNotNull(input) { "file_provider: could not open input" }
                            operation.input.set(input)
                            try {
                                operation.check()
                                target.outputStream().use { output ->
                                    val buffer = ByteArray(64 * 1024)
                                    while (true) {
                                        operation.check()
                                        val count = input.read(buffer)
                                        operation.check()
                                        if (count < 0) break
                                        require(count.toLong() <= remainingBytes - size) { "file_too_large: selection exceeds byte budget" }
                                        output.write(buffer, 0, count); digest.update(buffer, 0, count); size += count
                                        val now = android.os.SystemClock.elapsedRealtime()
                                        if (now - lastProgress >= 200) {
                                            lastProgress = now
                                            emitFileProgress(operation, "vcp-mobile-file-progress", JSObject().apply {
                                                put("name", name); put("mime", mime); put("loaded", size); put("total", declaredSize)
                                                put("selectionIndex", index); put("selectionCount", uris.size)
                                                put("progress", if (declaredSize > 0) (size.toDouble() / declaredSize * 100).coerceAtMost(100.0) else 0)
                                            })
                                        }
                                    }
                                    output.fd.sync()
                                }
                            } finally {
                                operation.input.compareAndSet(input, null)
                            }
                        }
                        operation.check()
                        val hash = digest.digest().joinToString("") { "%02x".format(it) }
                        val value = if (operation.args.mode == "avatar") {
                            val image = NativeFileImages.derive(DeriveImageArgs().apply {
                                sourcePath = target.absolutePath; outputPath = target.absolutePath + "-avatar"
                                maxDimension = 2048; maxPixels = 4_194_304; maxBytes = 20 * 1024 * 1024; preserveAlpha = true
                            })
                            val output = java.io.File(image.getString("path") ?: error("image output path missing"))
                            derivative = output
                            JSObject().apply {
                                put("path", output.absolutePath); put("name", "avatar_work." + output.extension)
                                put("mime", image.getString("mime")); put("size", output.length()); put("hash", "")
                                put("thumbnailPath", org.json.JSONObject.NULL)
                            }
                        } else JSObject().apply {
                            put("path", target.absolutePath); put("name", name); put("mime", mime)
                            put("size", size); put("hash", hash); put("thumbnailPath", org.json.JSONObject.NULL)
                        }
                        operation.check()
                        files.add(value)
                        remainingBytes -= size
                        if (derivative != null) {
                            owned.add(derivative)
                            derivative = null
                        } else {
                            owned.add(target)
                            staging = null
                        }
                    } catch (error: Throwable) {
                        operation.check()
                        if (operation.args.selectionLimit == 1) throw error
                        errors.put(JSObject().apply { put("name", name); put("message", "file_acquisition: ${error.message}") })
                    } finally {
                        staging?.delete(); derivative?.delete()
                    }
                }
                operation.check()
                val value = if (operation.args.selectionLimit > 1) JSObject().apply {
                    put("files", org.json.JSONArray(files)); put("errors", errors)
                } else files.single()
                // Cancellation wins ownership until the single terminal response.
                if (operation.settled.compareAndSet(false, true)) {
                    delivered = operation.invoke.deliverResult(value)
                }
            } catch (error: Throwable) {
                operation.reject(if (operation.cancelled.get()) "cancelled: file acquisition" else "file_acquisition: ${error.message}")
            } finally {
                if (!delivered) owned.forEach { it.delete() }
                ownedSource?.delete()
                finishFileOperation(operation)
            }
        }, reject = {
            ownedSource?.delete()
            operation.reject(it)
            finishFileOperation(operation)
        })
        if (runtime.isDestroying) { task.rejectBeforeStart("plugin is shutting down"); return }
        try { runtime.domains.fileIoExecutor.execute(task) }
        catch (_: RejectedExecutionException) { task.rejectBeforeStart("executor queue is full or shutting down") }
    }

    private fun emitFileProgress(operation: FileOperation, event: String, detail: JSObject) {
        detail.put("operationId", operation.args.operationId)
        activity.runOnUiThread {
            if (fileOperation.get() === operation && !operation.cancelled.get() && !operation.settled.get()) {
                emitEvent(event, detail)
            }
        }
    }

    fun destroy() {
        fileOperation.get()?.let {
            it.cancel()
            // Keep the cancelled callback owner/path: a camera may finish opening
            // its granted URI after teardown, so its late result must clean again.
            it.cameraFile.get()?.delete()
            it.reject("cancelled: plugin destroyed")
        }
    }
}

@InvokeArg
class PickFileArgs {
    var mode: String = "file"
    var operationId: String = ""
    var stagingDir: String = ""
    var maxBytes: Long = 512L * 1024 * 1024
    var maxFiles: Int = 1
    val selectionLimit: Int get() = if (mode == "file" || mode == "gallery") maxFiles.coerceIn(1, 16) else 1
}

@InvokeArg
class CancelFileOperationArgs { lateinit var operationId: String }
