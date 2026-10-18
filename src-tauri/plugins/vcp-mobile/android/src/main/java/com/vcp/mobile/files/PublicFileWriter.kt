package com.vcp.mobile.files

import android.app.Activity
import android.content.ContentValues
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import java.io.InputStream
import java.io.OutputStream

/** A fixed buffer also bounds copies from files whose size changes after inspection. */
internal fun copyExportStream(input: InputStream, output: OutputStream, maxBytes: Long): Long {
    require(maxBytes > 0) { "文件大小限制无效" }
    val buffer = ByteArray(64 * 1024)
    var total = 0L
    while (true) {
        if (Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("文件保存已停止")
        val count = input.read(buffer)
        if (Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("文件保存已停止")
        if (count == -1) break
        if (count == 0) continue
        if (count.toLong() > maxBytes - total) throw IllegalArgumentException("文件大小超出允许范围")
        output.write(buffer, 0, count)
        total += count
    }
    require(total > 0) { "写入内容为空" }
    return total
}

/** The reserved destination belongs to this write; a failure only removes that file. */
internal fun writeLegacyExportFile(
    directory: java.io.File,
    displayName: String,
    write: (OutputStream) -> Long,
): Pair<java.io.File, Long> {
    check(directory.isDirectory || directory.mkdirs()) { "无法创建导出目录" }
    val base = displayName.substringBeforeLast('.', displayName)
    val ext = displayName.substringAfterLast('.', "")
    var outputFile = java.io.File(directory, displayName)
    var index = 1
    while (!outputFile.createNewFile()) {
        outputFile = java.io.File(directory, if (ext.isBlank()) "${base}_$index" else "${base}_$index.$ext")
        index += 1
    }
    try {
        val size = outputFile.outputStream().use(write)
        if (Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("文件保存已停止")
        return outputFile to size
    } catch (error: Throwable) {
        try {
            if (!outputFile.delete() && outputFile.exists()) {
                error.addSuppressed(java.io.IOException("无法删除未完成导出: ${outputFile.name}"))
            }
        } catch (cleanupError: Throwable) {
            error.addSuppressed(cleanupError)
        }
        throw error
    }
}

internal data class SavedPublicFile(val uri: Uri, val displayName: String, val size: Long)

/** Commits one MediaStore row or one exclusively reserved legacy destination. */
internal class PublicFileWriter(private val activity: Activity) {
    private companion object { const val TAG = "VcpMobilePlugin" }

    fun writeFileToDownloads(input: InputStream, displayName: String, mimeType: String, maxBytes: Long): SavedPublicFile {
        val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) MediaStore.Downloads.EXTERNAL_CONTENT_URI else null
        return writePublicExport(displayName, mimeType, Environment.DIRECTORY_DOWNLOADS, collection) {
            copyExportStream(input, it, maxBytes)
        }
    }

    fun writeImageToGallery(input: InputStream, displayName: String, mimeType: String, maxBytes: Long): SavedPublicFile {
        val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) MediaStore.Images.Media.EXTERNAL_CONTENT_URI else null
        return writePublicExport(displayName, mimeType, Environment.DIRECTORY_PICTURES, collection) {
            copyExportStream(input, it, maxBytes)
        }
    }

    private fun writePublicExport(
        displayName: String,
        mimeType: String,
        directory: String,
        collection: Uri?,
        write: (OutputStream) -> Long,
    ): SavedPublicFile {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = activity.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
                put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
                put(MediaStore.MediaColumns.RELATIVE_PATH, "$directory/VCPMobile")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val uri = resolver.insert(checkNotNull(collection), values)
                ?: throw IllegalStateException("无法创建导出文件")
            try {
                // MediaProvider may append a MIME extension or a duplicate-name suffix.
                val savedName = resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use {
                    if (it.moveToFirst()) it.getString(0) else null
                }?.takeIf { it.isNotBlank() } ?: throw java.io.IOException("无法读取导出文件名")
                val size = resolver.openOutputStream(uri)?.use(write)
                    ?: throw IllegalStateException("无法写入导出文件")
                if (Thread.currentThread().isInterrupted) throw java.io.InterruptedIOException("文件保存已停止")
                values.clear()
                values.put(MediaStore.MediaColumns.IS_PENDING, 0)
                check(resolver.update(uri, values, null, null) == 1) { "无法完成导出文件" }
                return SavedPublicFile(uri, savedName, size)
            } catch (error: Throwable) {
                try {
                    resolver.delete(uri, null, null)
                } catch (cleanupError: Throwable) {
                    error.addSuppressed(cleanupError)
                }
                throw error
            }
        }

        val publicDirectory = Environment.getExternalStoragePublicDirectory(directory)
        val (outputFile, size) = writeLegacyExportFile(java.io.File(publicDirectory, "VCPMobile"), displayName, write)
        try {
            MediaScannerConnection.scanFile(activity, arrayOf(outputFile.absolutePath), arrayOf(mimeType), null)
        } catch (error: Exception) {
            // The file is already committed. A scanner failure must not report a failed save.
            Log.w(TAG, "Saved export but could not notify media scanner", error)
        }
        return SavedPublicFile(Uri.fromFile(outputFile), outputFile.name, size)
    }
}
