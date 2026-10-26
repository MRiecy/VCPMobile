package com.vcp.mobile.files

import android.app.Activity
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.util.Base64
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.core.content.ContextCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.PluginTaskRuntime
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLDecoder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

private const val GALLERY_HEADER_BYTES = 256

/** Only inspect complete boxes within the bounded header read by the gallery exporter. */
internal fun sniffBmffImageMime(bytes: ByteArray): String? {
    val limit = minOf(bytes.size, GALLERY_HEADER_BYTES)
    fun uint32(offset: Int): Long {
        var value = 0L
        for (index in offset until offset + 4) value = (value shl 8) or (bytes[index].toLong() and 0xff)
        return value
    }
    fun type(offset: Int) = String(bytes, offset, 4, Charsets.US_ASCII)

    var offset = 0
    while (limit - offset >= 8) {
        var boxSize = uint32(offset)
        var headerSize = 8
        if (boxSize == 1L) {
            if (limit - offset < 16 || uint32(offset + 8) != 0L) return null
            boxSize = uint32(offset + 12)
            headerSize = 16
        }
        // A zero-size box extends to EOF, which a header sniff cannot establish.
        if (boxSize < headerSize || boxSize > limit - offset) return null
        val end = offset + boxSize.toInt()
        if (type(offset + 4) == "ftyp") {
            if (boxSize < headerSize + 8 || (boxSize - headerSize - 8) % 4 != 0L) return null
            val brands = mutableSetOf(type(offset + headerSize))
            for (position in offset + headerSize + 8 until end step 4) brands.add(type(position))
            return when {
                brands.any { it == "avif" || it == "avis" } -> "image/avif"
                brands.any { it in setOf("heic", "heix", "heim", "heis") } -> "image/heic"
                brands.any { it in setOf("hevc", "hevx", "hevm", "hevs") } -> "image/heic-sequence"
                brands.any { it == "mif1" || it == "mif2" } -> "image/heif"
                "msf1" in brands -> "image/heif-sequence"
                else -> null
            }
        }
        offset = end
    }
    return null
}

/** Main-thread owner for the one file export waiting on legacy storage permission. */
internal class LegacyWritePermissionWait {
    private data class Pending(val invoke: Invoke, val save: () -> Unit)
    private var pending: Pending? = null

    fun request(invoke: Invoke, save: () -> Unit, launch: () -> Unit) {
        if (pending != null) {
            invoke.deliverError("已有文件保存正在等待储存空间权限")
            return
        }
        pending = Pending(invoke, save)
        try {
            launch()
        } catch (error: Exception) {
            if (take(invoke) != null) invoke.deliverError("无法申请储存空间权限: ${error.message}")
        }
    }

    fun complete(invoke: Invoke, granted: Boolean) {
        val request = take(invoke) ?: return
        if (granted) {
            // Ownership transfers to the file executor; destroy must not release its source early.
            try {
                request.save()
            } catch (error: Exception) {
                invoke.deliverError("保存文件失败: ${error.message}")
            }
        } else {
            invoke.deliverError("保存文件需要储存空间权限")
        }
    }

    fun destroy() {
        val request = pending ?: return
        pending = null
        request.invoke.deliverError("file export unavailable: plugin was destroyed")
    }

    private fun take(invoke: Invoke): Pending? {
        val request = pending ?: return null
        if (request.invoke !== invoke) return null
        pending = null
        return request
    }
}

/** Gallery/download source handling; the permission wait transfers ownership to the file worker. */
internal class NativeFileExports(
    private val activity: Activity,
    private val runtime: PluginTaskRuntime,
    private val requestPermission: (String, Invoke, String) -> Unit,
) {
    private companion object {
        const val TAG = "VcpMobilePlugin"
        const val MAX_DOWNLOAD_EXPORT_BYTES = 8 * 1024 * 1024
        const val MAX_GALLERY_EXPORT_BYTES = 50 * 1024 * 1024
    }
    private val writer = PublicFileWriter(activity)
    private val legacyWritePermissionWait = LegacyWritePermissionWait()
    private data class LoadedImage(val bytes: ByteArray, val mimeType: String)

    fun saveImageToGallery(invoke: Invoke) {
        if (waitForLegacyWritePermission(invoke) { saveImageToGallery(invoke) }) return
        val args = invoke.parseArgs(SaveImageArgs::class.java)
        if (args.sourceUrl.isBlank()) {
            invoke.deliverError("图片地址为空")
            return
        }

        runtime.execute(runtime.domains.fileIoExecutor, invoke, "saveImageToGallery", {
            try {
                invoke.deliverResult(saveImageSource(args.sourceUrl, args.fileName))
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageToGallery failed", e)
                invoke.deliverError("保存图片失败: ${e.message}")
            }
        })
    }

    fun saveImageFromPath(invoke: Invoke) {
        if (waitForLegacyWritePermission(invoke) { saveImageFromPath(invoke) }) return
        val args = invoke.parseArgs(SaveImageFromPathArgs::class.java)
        if (args.imagePath.isBlank()) {
            invoke.deliverError("物理文件路径为空")
            return
        }

        runtime.execute(runtime.domains.fileIoExecutor, invoke, "saveImageFromPath", {
            try {
                val file = checkedExportFile(args.imagePath, Int.MAX_VALUE.toLong())
                file.inputStream().use { input ->
                    invoke.deliverResult(saveImageStream(input, args.fileName, file.name, file.name, true, Int.MAX_VALUE.toLong()))
                }
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageFromPath failed", e)
                invoke.deliverError("保存图片失败: ${e.message}")
            }
        })
    }

    fun saveToDownloads(invoke: Invoke) {
        if (waitForLegacyWritePermission(invoke) { saveToDownloads(invoke) }) return
        val args = invoke.parseArgs(SaveToDownloadsArgs::class.java)
        if (args.fileName.isBlank()) {
            invoke.deliverError("文件名为空")
            return
        }
        if (args.contentBase64.isBlank()) {
            invoke.deliverError("写入内容为空")
            return
        }
        if (args.contentBase64.length.toLong() > ((MAX_DOWNLOAD_EXPORT_BYTES.toLong() + 2) / 3) * 4) {
            invoke.deliverError("文件大小超出允许范围 (最大 ${MAX_DOWNLOAD_EXPORT_BYTES / 1024 / 1024}MB)")
            return
        }

        runtime.execute(runtime.domains.fileIoExecutor, invoke, "saveToDownloads", fileTask@{
            try {
                val bytes = try {
                    android.util.Base64.decode(args.contentBase64, android.util.Base64.DEFAULT)
                } catch (e: IllegalArgumentException) {
                    invoke.deliverError("内容不是合法的 Base64 数据")
                    return@fileTask
                }
                if (bytes.isEmpty() || bytes.size > MAX_DOWNLOAD_EXPORT_BYTES) {
                    invoke.deliverError("文件大小超出允许范围 (最大 ${MAX_DOWNLOAD_EXPORT_BYTES / 1024 / 1024}MB)")
                    return@fileTask
                }

                val displayName = args.fileName
                    .replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_")
                    .trim()
                if (displayName.isBlank()) {
                    invoke.deliverError("文件名非法")
                    return@fileTask
                }
                val mimeType = args.mimeType?.takeIf { it.isNotBlank() } ?: "application/octet-stream"

                val saved = bytes.inputStream().use {
                    writer.writeFileToDownloads(it, displayName, mimeType, MAX_DOWNLOAD_EXPORT_BYTES.toLong())
                }
                val result = JSObject().apply {
                    put("uri", saved.uri.toString())
                    put("displayName", saved.displayName)
                    put("mimeType", mimeType)
                    put("size", saved.size)
                }
                invoke.deliverResult(result)
            } catch (e: Throwable) {
                Log.e(TAG, "saveToDownloads failed", e)
                invoke.deliverError("保存到下载目录失败: ${e.message}")
            }
        })
    }

    fun saveFileToDownloads(invoke: Invoke) {
        if (waitForLegacyWritePermission(invoke) { saveFileToDownloads(invoke) }) return
        val args = try {
            invoke.parseArgs(SaveFileToDownloadsArgs::class.java)
        } catch (error: Exception) {
            invoke.deliverError("下载导出参数无效: ${error.message}")
            return
        }
        runtime.execute(runtime.domains.fileIoExecutor, invoke, "saveFileToDownloads", {
            try {
                require(args.maxBytes in 1..Int.MAX_VALUE.toLong()) { "文件大小限制无效" }
                val file = checkedExportFile(args.filePath, args.maxBytes)
                val displayName = args.fileName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_").trim()
                require(displayName.isNotBlank() && displayName != "." && displayName != "..") { "文件名非法" }
                val mimeType = args.mimeType?.takeIf { it.isNotBlank() } ?: "application/octet-stream"
                val saved = file.inputStream().use { writer.writeFileToDownloads(it, displayName, mimeType, args.maxBytes) }
                invoke.deliverResult(JSObject().apply {
                    put("uri", saved.uri.toString())
                    put("displayName", saved.displayName)
                    put("mimeType", mimeType)
                    put("size", saved.size)
                })
            } catch (error: Throwable) {
                Log.e(TAG, "saveFileToDownloads failed", error)
                invoke.deliverError("保存到下载目录失败: ${error.message}")
            }
        })
    }

    private fun waitForLegacyWritePermission(invoke: Invoke, save: () -> Unit): Boolean {
        if (runtime.isDestroying) {
            invoke.deliverError("plugin is shutting down")
            return true
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q &&
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED
        ) {
            activity.runOnUiThread {
                if (runtime.isDestroying) {
                    invoke.deliverError("plugin is shutting down")
                } else {
                    legacyWritePermissionWait.request(invoke, save) {
                        requestPermission("storageLegacy", invoke, "onLegacyWritePermissionResult")
                    }
                }
            }
            return true
        }
        return false
    }

    fun onLegacyWritePermissionResult(invoke: Invoke) {
        activity.runOnUiThread {
            val granted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
            legacyWritePermissionWait.complete(invoke, granted)
        }
    }

    private fun checkedExportFile(path: String, maxBytes: Long): java.io.File {
        require(path.isNotBlank() && isSafeLocalPath(activity, path)) { "禁止读取应用目录外的文件" }
        val file = java.io.File(path).canonicalFile
        require(file.isFile) { "本地文件不存在" }
        require(file.length() in 1..maxBytes) { "文件大小超出允许范围" }
        return file
    }

    private fun saveImageSource(sourceUrl: String, fileName: String?): JSObject {
        if (sourceUrl.startsWith("data:", ignoreCase = true)) {
            val loaded = loadDataUrlImage(sourceUrl)
            return loaded.bytes.inputStream().use {
                saveImageStream(it, fileName, sourceUrl, loaded.mimeType, false)
            }
        }

        if (sourceUrl.startsWith("content:", ignoreCase = true)) {
            val uri = Uri.parse(sourceUrl)
            val mime = activity.contentResolver.getType(uri) ?: mimeFromSource(sourceUrl)
            return activity.contentResolver.openInputStream(uri).use { input ->
                saveImageStream(input ?: throw IllegalStateException("无法读取 content 图片"), fileName, sourceUrl, mime, true)
            }
        }

        if (sourceUrl.startsWith("file:", ignoreCase = true) || sourceUrl.startsWith("/")) {
            val path = if (sourceUrl.startsWith("file:", ignoreCase = true)) {
                Uri.parse(sourceUrl).path ?: sourceUrl.removePrefix("file://")
            } else {
                sourceUrl
            }

            val file = checkedExportFile(path, MAX_GALLERY_EXPORT_BYTES.toLong())
            return file.inputStream().use {
                saveImageStream(it, fileName, file.name, mimeFromSource(file.name), true)
            }
        }

        return saveNetworkImage(sourceUrl, fileName)
    }

    private fun saveNetworkImage(sourceUrl: String, fileName: String?): JSObject {
        val connection = (URL(sourceUrl).openConnection() as HttpURLConnection).apply {
            connectTimeout = 5000  // 💥 优化：降低至5秒
            readTimeout = 10000    // 💥 优化：降低至10秒
            instanceFollowRedirects = true
            setRequestProperty("User-Agent", "VCPMobile/1.0")
        }

        try {
            val status = connection.responseCode
            if (status !in 200..299) {
                throw IllegalStateException("HTTP $status")
            }
            val contentType = connection.contentType?.substringBefore(";")?.lowercase(Locale.US)
            require(connection.contentLengthLong <= MAX_GALLERY_EXPORT_BYTES) { "图片过大，超过 50MB" }
            return connection.inputStream.use {
                saveImageStream(it, fileName, sourceUrl, contentType ?: mimeFromSource(sourceUrl), false)
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun loadDataUrlImage(dataUrl: String): LoadedImage {
        val commaIndex = dataUrl.indexOf(',')
        if (commaIndex <= 0 || commaIndex > 256) throw IllegalArgumentException("无效的 data URL")

        val header = dataUrl.substring(5, commaIndex)
        val mime = header.substringBefore(";").ifBlank { "application/octet-stream" }.lowercase(Locale.US)
        val isBase64 = header.contains(";base64", ignoreCase = true)
        val maxEncodedLength = if (isBase64) ((MAX_GALLERY_EXPORT_BYTES.toLong() + 2) / 3) * 4 else MAX_GALLERY_EXPORT_BYTES.toLong()
        require(dataUrl.length.toLong() - commaIndex - 1 <= maxEncodedLength) { "图片过大，超过 50MB" }
        val payload = dataUrl.substring(commaIndex + 1)
        val bytes = if (isBase64) {
            Base64.decode(payload, Base64.DEFAULT)
        } else {
            URLDecoder.decode(payload, "UTF-8").toByteArray(Charsets.UTF_8)
        }
        require(bytes.size in 1..MAX_GALLERY_EXPORT_BYTES) { "图片为空或超过 50MB" }
        return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = false))
    }

    private fun saveImageStream(
        input: InputStream,
        fileName: String?,
        source: String,
        fallbackMime: String,
        isLocal: Boolean,
        maxBytes: Long = MAX_GALLERY_EXPORT_BYTES.toLong(),
    ): JSObject {
        val buffered = input.buffered(64 * 1024)
        buffered.mark(GALLERY_HEADER_BYTES)
        val header = ByteArray(GALLERY_HEADER_BYTES)
        var length = 0
        while (length < header.size) {
            val count = buffered.read(header, length, header.size - length)
            if (count == -1) break
            length += count
        }
        buffered.reset()
        val mimeType = sniffImageMime(header.copyOf(length), fallbackMime, isLocal)
        require(mimeType.startsWith("image/")) { "当前资源不是图片: $mimeType" }
        val displayName = buildGalleryFileName(fileName, source, mimeType)
        val saved = writer.writeImageToGallery(buffered, displayName, mimeType, maxBytes)
        return JSObject().apply {
            put("uri", saved.uri.toString())
            put("displayName", saved.displayName)
            put("mimeType", mimeType)
            put("size", saved.size)
        }
    }

    private fun sniffImageMime(bytes: ByteArray, fallback: String, isLocal: Boolean): String {
        val normalized = fallback.substringBefore(";").lowercase(Locale.US)

        // 💥 安全校验：若是网络资源可信任 content-type，若是本地绝对物理路径，必须强行进行 Magic bytes 头二进制分析，防止伪造扩展名泄漏明文
        if (!isLocal && normalized.startsWith("image/")) {
            return normalized
        }

        if (bytes.size >= 8 && bytes[0] == 0x89.toByte() && bytes[1] == 0x50.toByte() && bytes[2] == 0x4E.toByte() && bytes[3] == 0x47.toByte()) return "image/png"
        if (bytes.size >= 3 && bytes[0] == 0xFF.toByte() && bytes[1] == 0xD8.toByte() && bytes[2] == 0xFF.toByte()) return "image/jpeg"
        if (bytes.size >= 6 && String(bytes, 0, 6, Charsets.US_ASCII).startsWith("GIF")) return "image/gif"
        if (bytes.size >= 12 && String(bytes, 0, 4, Charsets.US_ASCII) == "RIFF" && String(bytes, 8, 4, Charsets.US_ASCII) == "WEBP") return "image/webp"
        if (bytes.size >= 2 && bytes[0] == 0x42.toByte() && bytes[1] == 0x4D.toByte()) return "image/bmp"
        sniffBmffImageMime(bytes)?.let { return it }

        val sample = bytes.take(256).toByteArray().toString(Charsets.UTF_8).trimStart()
        if (sample.startsWith("<svg", ignoreCase = true) || sample.startsWith("<?xml", ignoreCase = true)) return "image/svg+xml"

        // 本地读取兜底降级：非图片格式的敏感文件一律设为 application/octet-stream，从而在 saveImageToGallery 判定 mime.startsWith("image/") 时被拦截
        if (isLocal) {
            return "application/octet-stream"
        }
        return normalized
    }

    private fun mimeFromSource(source: String): String {
        val clean = source.substringBefore("?").substringBefore("#")
        val ext = clean.substringAfterLast('.', "").lowercase(Locale.US)
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: when (ext) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "svg" -> "image/svg+xml"
            "bmp" -> "image/bmp"
            "avif" -> "image/avif"
            "heic" -> "image/heic"
            "heif" -> "image/heif"
            else -> "application/octet-stream"
        }
    }

    private fun extensionForMime(mimeType: String): String {
        return when (mimeType.lowercase(Locale.US)) {
            "image/jpeg" -> "jpg"
            "image/png" -> "png"
            "image/gif" -> "gif"
            "image/webp" -> "webp"
            "image/svg+xml" -> "svg"
            "image/bmp" -> "bmp"
            "image/avif" -> "avif"
            "image/heic", "image/heic-sequence" -> "heic"
            "image/heif", "image/heif-sequence" -> "heif"
            else -> "png"
        }
    }

    private fun buildGalleryFileName(providedName: String?, sourceUrl: String, mimeType: String): String {
        val fromUrl = if (!sourceUrl.startsWith("data:", ignoreCase = true) && !sourceUrl.startsWith("blob:", ignoreCase = true)) {
            try {
                Uri.parse(sourceUrl).lastPathSegment?.let { URLDecoder.decode(it, "UTF-8") }
            } catch (_: Exception) {
                null
            }
        } else {
            null
        }

        val timestamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
        val rawName = providedName?.takeIf { it.isNotBlank() } ?: fromUrl ?: "vcp_image_$timestamp"
        val sanitized = rawName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_").trim().ifBlank { "vcp_image_$timestamp" }
        val base = sanitized.substringBeforeLast('.', sanitized).take(96).ifBlank { "vcp_image_$timestamp" }
        val ext = extensionForMime(mimeType)
        return "$base.$ext"
    }

    fun destroy() {
        activity.runOnUiThread { legacyWritePermissionWait.destroy() }
    }
}

@InvokeArg
class SaveImageArgs {
    lateinit var sourceUrl: String
    var fileName: String? = null
}

@InvokeArg
class SaveImageFromPathArgs {
    lateinit var imagePath: String
    var fileName: String? = null
}

@InvokeArg
class SaveToDownloadsArgs {
    lateinit var fileName: String
    lateinit var contentBase64: String
    var mimeType: String? = null
}

@InvokeArg
class SaveFileToDownloadsArgs {
    lateinit var filePath: String
    lateinit var fileName: String
    var mimeType: String? = null
    var maxBytes: Long = 0
}
