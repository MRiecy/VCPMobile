package com.vcp.mobile

import android.app.Activity
import android.content.Context
import android.content.IntentFilter
import android.content.res.Configuration
import android.os.Build
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.TauriPlugin
import androidx.activity.result.ActivityResult
import app.tauri.plugin.Plugin
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
import android.webkit.MimeTypeMap
import android.os.PowerManager
import android.net.Uri
import android.provider.Settings
import android.content.pm.PackageManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import app.tauri.plugin.JSArray
import app.tauri.plugin.Invoke
import com.vcp.mobile.service.StreamKeepaliveService
import android.graphics.Bitmap
import android.graphics.Canvas
import android.content.ContentValues
import android.provider.MediaStore
import android.os.Environment
import android.media.MediaScannerConnection
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLDecoder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE"], alias = "storageLegacy"),
    Permission(strings = ["android.permission.RECORD_AUDIO"], alias = "microphone"),
    Permission(strings = ["android.permission.CAMERA"], alias = "camera")
])
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private companion object {
        const val TAG = "VcpMobilePlugin"
    }

    private var webViewRef: WebView? = null
    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val lifecycleBridge = LifecycleBridge()
    private val batteryStatusManager = BatteryStatusManager(activity)
    private val fileIoExecutor = java.util.concurrent.Executors.newSingleThreadExecutor()
    private var cameraTempFile: java.io.File? = null

    // ==================================================================
    // Permissions & App Control
    // ==================================================================
    @Command
    fun checkAllPermissions(invoke: Invoke) {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
        
        val notificationGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }

        val storageGranted = if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val result = JSObject()
        result.put("notification", notificationGranted)
        result.put("storage", storageGranted)
        result.put("microphone", microphoneGranted)
        result.put("camera", cameraGranted)
        result.put("battery", batteryOptimizationIgnored)
        
        invoke.resolve(result)
    }

    @Command
    fun requestAndroidPermission(invoke: Invoke) {
        val args = invoke.parseArgs(RequestPermissionArgs::class.java)
        when (args.type) {
            "notification" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("notification", invoke, "onPermissionResult")
                } else {
                    emitPermissionsToWebView()
                    invoke.resolve()
                }
            }
            "storage" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("storage", invoke, "onPermissionResult")
                } else {
                    requestPermissionForAlias("storageLegacy", invoke, "onPermissionResult")
                }
            }
            "microphone" -> {
                requestPermissionForAlias("microphone", invoke, "onPermissionResult")
            }
            "camera" -> {
                requestPermissionForAlias("camera", invoke, "onPermissionResult")
            }
            "battery" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    }
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                } catch (e: Exception) {
                    val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                }
            }
        }
    }

    @Command
    fun moveTaskToBack(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
    }

    // ==================================================================
    // Permission Result Callbacks
    // ==================================================================
    @PermissionCallback
    fun onPermissionResult(invoke: Invoke) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    @ActivityCallback
    fun onBatteryOptimizationResult(invoke: Invoke, result: ActivityResult) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    private fun emitPermissionsToWebView() {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager

        val notificationGranted = if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }

        val storageGranted = if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val json = """{"notification":$notificationGranted,"storage":$storageGranted,"microphone":$microphoneGranted,"camera":$cameraGranted,"battery":$batteryOptimizationIgnored}"""
        val script = "window.dispatchEvent(new CustomEvent('vcp-permission-change', { detail: $json }))"
        webViewRef?.evaluateJavascript(script, null)
    }

    // ==================================================================
    // Screen
    // ==================================================================
    @Command
    fun setKeepScreenOn(invoke: Invoke) {
        activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    @Command
    fun clearKeepScreenOn(invoke: Invoke) {
        activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    @Command
    fun getBatteryStatus(invoke: Invoke) {
        try {
            val status = batteryStatusManager.getStatusJson()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "getBatteryStatus failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    // ==================================================================
    // Stream Service
    // ==================================================================
    @Command
    fun startStreamingService(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StartStreamArgs::class.java)
            val intent = StreamKeepaliveService.createIntent(activity, args.agentName)
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    activity.startForegroundService(intent)
                } else {
                    activity.startService(intent)
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "POST_NOTIFICATIONS permission denied, degrading to normal service", e)
                activity.startService(intent) // 静默降级为普通 Service，防止闪退崩溃
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startStreamingService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        try {
            val intent = StreamKeepaliveService.createIntent(activity, "")
            activity.stopService(intent)
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "stopStreamingService failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    // ==================================================================
    // Plugin Lifecycle
    // ==================================================================

    override fun load(webView: WebView) {
        super.load(webView)
        webViewRef = webView

        keyboardInsetsManager.attach(webView)
        lifecycleBridge.attach(activity, webView)
    }

    override fun onDestroy(activity: AppCompatActivity) {
        webViewRef = null
        try {
            fileIoExecutor.shutdown()
        } catch (_: Exception) {}
        super.onDestroy(activity)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        lifecycleBridge.onConfigurationChanged(newConfig)
    }

    // ==================================================================
    // Scoped Storage File Picker & Native Thumbnail Generation (Scheme B)
    // ==================================================================
    @PermissionCallback
    fun onCameraPermissionResult(invoke: Invoke) {
        if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCameraIntent(invoke)
        } else {
            Log.w(TAG, "[onCameraPermissionResult] Camera permission denied")
            invoke.reject("Camera permission denied")
        }
    }

    private fun launchCameraIntent(invoke: Invoke) {
        try {
            val uploadsDir = java.io.File(activity.cacheDir, "uploads").apply { mkdirs() }
            val tempFile = java.io.File(uploadsDir, "camera_${System.currentTimeMillis()}.jpg")
            cameraTempFile = tempFile
            
            val authority = "${activity.packageName}.fileprovider"
            val uri = try {
                FileProvider.getUriForFile(activity, authority, tempFile)
            } catch (e: Exception) {
                FileProvider.getUriForFile(activity, "${activity.packageName}.opener.fileprovider", tempFile)
            }
            
            val intent = Intent(android.provider.MediaStore.ACTION_IMAGE_CAPTURE).apply {
                putExtra(android.provider.MediaStore.EXTRA_OUTPUT, uri)
                addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            }
            startActivityForResult(invoke, intent, "onCameraResult")
        } catch (e: Throwable) {
            Log.e(TAG, "[launchCameraIntent] Failed to launch camera intent", e)
            invoke.reject("Failed to launch camera: ${e.message}")
        }
    }

    @Command
    fun pickFile(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(PickFileArgs::class.java)
            val mode = args.mode
            Log.i(TAG, "[pickFile] Invoked with mode: $mode")

            when (mode) {
                "camera" -> {
                    if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                        requestPermissionForAlias("camera", invoke, "onCameraPermissionResult")
                        return
                    }
                    launchCameraIntent(invoke)
                }
                "gallery" -> {
                    val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "image/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    startActivityForResult(invoke, intent, "onPickFileResult")
                }
                else -> {
                    val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    startActivityForResult(invoke, intent, "onPickFileResult")
                }
            }
        } catch (e: Throwable) {
            Log.e(TAG, "[pickFile] Failed to start activity for result", e)
            invoke.reject("Failed to start native file picker: ${e.message}")
        }
    }

    @ActivityCallback
    fun onCameraResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            Log.w(TAG, "[onCameraResult] Camera capture cancelled or failed")
            cameraTempFile?.delete()
            cameraTempFile = null
            invoke.reject("Cancelled")
            return
        }

        val photoFile = cameraTempFile
        if (photoFile == null || !photoFile.exists()) {
            Log.e(TAG, "[onCameraResult] Temporary photo file is null or does not exist")
            invoke.reject("Capture failed: temp file not found")
            return
        }

        cameraTempFile = null // reset

        fileIoExecutor.execute {
            try {
                val context = activity
                val originalName = "Camera_${System.currentTimeMillis()}.jpg"
                val mimeType = "image/jpeg"
                val size = photoFile.length()

                Log.i(TAG, "[onCameraResult] Processing captured photo: $originalName (size=$size)")

                // 发送预准备事件给前端，让前端立即创建进度卡片
                val startDetail = JSObject().apply {
                    put("name", originalName)
                    put("size", size)
                    put("mime", mimeType)
                }
                val safeStartDetail = escapeJsonForJsString(startDetail.toString())
                activity.runOnUiThread {
                    webViewRef?.evaluateJavascript("window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', { detail: JSON.parse(\"$safeStartDetail\") }))", null)
                }

                // 计算 SHA-256 哈希
                val digest = java.security.MessageDigest.getInstance("SHA-256")
                java.io.FileInputStream(photoFile).use { fis ->
                    val buffer = ByteArray(65536)
                    var bytesRead: Int
                    while (fis.read(buffer).also { bytesRead = it } != -1) {
                        digest.update(buffer, 0, bytesRead)
                    }
                }
                val hashBytes = digest.digest()
                val hash = hashBytes.joinToString("") { "%02x".format(it) }

                // 重命名去重
                val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
                val finalTempFile = java.io.File(uploadsDir, "$hash.jpg")
                if (finalTempFile.exists()) {
                    photoFile.delete() // 缓存去重，复用已有文件
                } else {
                    photoFile.renameTo(finalTempFile)
                }

                // 生成缩略图
                val thumbnailPath = generateNativeThumbnail(context, finalTempFile, hash)

                // 组装结果物理路径并回传给 Rust 桥接
                val resultObject = JSObject()
                resultObject.put("path", finalTempFile.absolutePath)
                resultObject.put("name", originalName)
                resultObject.put("mime", mimeType)
                resultObject.put("size", finalTempFile.length())
                resultObject.put("hash", hash)
                if (thumbnailPath != null) {
                    resultObject.put("thumbnailPath", thumbnailPath)
                }

                // 双轨通信：推送最终结果给前端，穿透 JNI 断裂层
                val pickedDetail = JSObject().apply {
                    put("path", finalTempFile.absolutePath)
                    put("name", originalName)
                    put("mime", mimeType)
                    put("size", finalTempFile.length())
                    put("hash", hash)
                    if (thumbnailPath != null) {
                        put("thumbnailPath", thumbnailPath)
                    } else {
                        put("thumbnailPath", org.json.JSONObject.NULL)
                    }
                }
                val safePickedDetail = escapeJsonForJsString(pickedDetail.toString())
                val pickedScript = "window.dispatchEvent(new CustomEvent('vcp-mobile-file-picked', { detail: JSON.parse(\"$safePickedDetail\") }))"
                activity.runOnUiThread {
                    webViewRef?.evaluateJavascript(pickedScript, null)
                }

                invoke.resolve(resultObject)
            } catch (e: Throwable) {
                Log.e(TAG, "[onCameraResult] Photo processing failed", e)
                invoke.reject("Handling captured photo failed: ${e.message}")
            }
        }
    }

    @ActivityCallback
    fun onPickFileResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            Log.w(TAG, "[onPickFileResult] Pick cancelled or failed")
            invoke.reject("Cancelled")
            return
        }

        val uri = result.data?.data
        if (uri == null) {
            Log.w(TAG, "[onPickFileResult] Selected URI is null")
            invoke.reject("No file selected")
            return
        }

        fileIoExecutor.execute {
            var currentTempFile: java.io.File? = null
            try {
                val context = activity
                val contentResolver = context.contentResolver

                // 1. 获取文件名和大小
                var originalName = "unknown"
                var size = 0L
                contentResolver.query(uri, null, null, null, null)?.use { cursor ->
                    val nameIndex = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
                    val sizeIndex = cursor.getColumnIndex(android.provider.OpenableColumns.SIZE)
                    if (cursor.moveToFirst()) {
                        if (nameIndex != -1) originalName = cursor.getString(nameIndex)
                        if (sizeIndex != -1) size = cursor.getLong(sizeIndex)
                    }
                }

                // 2. 获取 MIME 类型
                val mimeType = contentResolver.getType(uri) ?: "application/octet-stream"
                Log.i(TAG, "[onPickFileResult] Processing picked file: $originalName (size=$size, mime=$mimeType)")

                // 3. 发送预准备事件给前端，让前端立即创建进度卡片
                val startDetail = JSObject().apply {
                    put("name", originalName)
                    put("size", size)
                    put("mime", mimeType)
                }
                val safeStartDetail = escapeJsonForJsString(startDetail.toString())
                activity.runOnUiThread {
                    webViewRef?.evaluateJavascript("window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', { detail: JSON.parse(\"$safeStartDetail\") }))", null)
                }

                // 4. 流式安全拷贝至 cacheDir 并同步计算 SHA-256 (64KB buffer)
                val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
                val tempFile = java.io.File(uploadsDir, "pick_${System.currentTimeMillis()}_temp")
                currentTempFile = tempFile
                val digest = java.security.MessageDigest.getInstance("SHA-256")
                
                contentResolver.openInputStream(uri).use { inputStream ->
                    if (inputStream == null) {
                        Log.e(TAG, "[onPickFileResult] openInputStream returned null")
                        invoke.reject("Could not open input stream")
                        return@execute
                    }
                    java.io.FileOutputStream(tempFile).use { outputStream ->
                        val buffer = ByteArray(65536)
                        var bytesRead: Int
                        var totalRead = 0L
                        var lastReportTime = System.currentTimeMillis()
                        
                        while (inputStream.read(buffer).also { bytesRead = it } != -1) {
                            outputStream.write(buffer, 0, bytesRead)
                            digest.update(buffer, 0, bytesRead)
                            totalRead += bytesRead
                            
                            val now = System.currentTimeMillis()
                            if (now - lastReportTime > 200) {
                                lastReportTime = now
                                val progress = if (size > 0) ((totalRead.toDouble() / size) * 100).toInt() else 0
                                val progressDetail = JSObject().apply {
                                    put("loaded", totalRead)
                                    put("total", size)
                                    put("progress", progress)
                                    put("name", originalName)
                                    put("mime", mimeType)
                                }
                                val safeProgressDetail = escapeJsonForJsString(progressDetail.toString())
                                val progressScript = "window.dispatchEvent(new CustomEvent('vcp-mobile-file-progress', { detail: JSON.parse(\"$safeProgressDetail\") }))"
                                activity.runOnUiThread {
                                    webViewRef?.evaluateJavascript(progressScript, null)
                                }
                            }
                        }
                    }
                }

                val hashBytes = digest.digest()
                val hash = hashBytes.joinToString("") { "%02x".format(it) }

                // 内容寻址哈希命名重命名去重
                val fileExtension = java.io.File(originalName).extension.let { 
                    if (it.isEmpty()) "" else ".$it" 
                }
                val finalTempFile = java.io.File(uploadsDir, "$hash$fileExtension")
                
                if (finalTempFile.exists()) {
                    tempFile.delete() // 缓存去重，复用已有文件
                } else {
                    tempFile.renameTo(finalTempFile)
                }

                val finalSize = if (size > 0) size else finalTempFile.length()

                // 4. 图片资源触发 Native 硬件加速缩略图硬解
                var thumbnailPath: String? = null
                if (mimeType.startsWith("image/")) {
                    thumbnailPath = generateNativeThumbnail(context, finalTempFile, hash)
                }

                // 5. 组装结果物理路径并回传给 Rust 桥接
                val resultObject = JSObject()
                resultObject.put("path", finalTempFile.absolutePath)
                resultObject.put("name", originalName)
                resultObject.put("mime", mimeType)
                resultObject.put("size", finalSize)
                resultObject.put("hash", hash)
                if (thumbnailPath != null) {
                    resultObject.put("thumbnailPath", thumbnailPath)
                }

                Log.i(TAG, "[onPickFileResult] File copy & process complete: path=${finalTempFile.absolutePath}, hash=$hash")
                
                // 双轨通信：主动推送最终结果给前端，穿透 JNI 断裂层
                val pickedDetail = JSObject().apply {
                    put("path", finalTempFile.absolutePath)
                    put("name", originalName)
                    put("mime", mimeType)
                    put("size", finalSize)
                    put("hash", hash)
                    if (thumbnailPath != null) {
                        put("thumbnailPath", thumbnailPath)
                    } else {
                        put("thumbnailPath", org.json.JSONObject.NULL)
                    }
                }
                val safePickedDetail = escapeJsonForJsString(pickedDetail.toString())
                val pickedScript = "window.dispatchEvent(new CustomEvent('vcp-mobile-file-picked', { detail: JSON.parse(\"$safePickedDetail\") }))"
                activity.runOnUiThread {
                    webViewRef?.evaluateJavascript(pickedScript, null)
                }
                
                invoke.resolve(resultObject)
            } catch (e: Throwable) {
                Log.e(TAG, "[onPickFileResult] File pick handling failed", e)
                try {
                    currentTempFile?.delete()
                } catch (_: Exception) {}
                invoke.reject("Handling picked file failed: ${e.message}")
            }
        }
    }

    private fun generateNativeThumbnail(context: Context, originalFile: java.io.File, hash: String): String? {
        val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
        val thumbDir = java.io.File(uploadsDir, "thumbnails").apply { mkdirs() }
        val thumbFile = java.io.File(thumbDir, "${hash}_thumb.webp")
        if (thumbFile.exists()) return thumbFile.absolutePath

        try {
            val bitmap = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                // Q以上享用系统硬件级图片自适应缩放加速
                android.media.ThumbnailUtils.createImageThumbnail(originalFile, android.util.Size(200, 200), null)
            } else {
                // 兼容低版本并防止大图软解 OOM 的智能预采样
                val options = android.graphics.BitmapFactory.Options().apply {
                    inJustDecodeBounds = true
                }
                android.graphics.BitmapFactory.decodeFile(originalFile.absolutePath, options)
                val width = options.outWidth
                val height = options.outHeight
                
                var inSampleSize = 1
                if (width > 200 || height > 200) {
                    val halfHeight = height / 2
                    val halfWidth = width / 2
                    while (halfHeight / inSampleSize >= 200 && halfWidth / inSampleSize >= 200) {
                        inSampleSize *= 2
                    }
                }
                
                options.inJustDecodeBounds = false
                options.inSampleSize = inSampleSize
                val rawBitmap = android.graphics.BitmapFactory.decodeFile(originalFile.absolutePath, options) ?: return null
                
                val w = rawBitmap.width
                val h = rawBitmap.height
                val (newW, newH) = if (w >= h) {
                    val ratio = w.toFloat() / h.toFloat()
                    ((200f * ratio).toInt() to 200)
                } else {
                    val ratio = h.toFloat() / w.toFloat()
                    (200 to (200f * ratio).toInt())
                }
                val scaled = android.graphics.Bitmap.createScaledBitmap(rawBitmap, newW, newH, true)
                if (scaled != rawBitmap) {
                    rawBitmap.recycle()
                }
                scaled
            }

            java.io.FileOutputStream(thumbFile).use { out ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.WEBP_LOSSY, 80, out)
                } else {
                    @Suppress("DEPRECATION")
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.WEBP, 80, out)
                }
            }
            bitmap.recycle() // 显式释放 Native 物理内存，防范溢出
            return thumbFile.absolutePath
        } catch (e: Exception) {
            Log.e(TAG, "Native thumbnail generation failed", e)
            return null
        }
    }

    private fun escapeJsonForJsString(json: String): String {
        return json
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\'", "\\'")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
    }

    @Command
    fun openFile(invoke: Invoke) {
        val args = invoke.parseArgs(OpenFileArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.reject("Path is empty")
            return
        }
        
        fileIoExecutor.execute {
            try {
                val context = activity

                // 💥 安全边界拦截：禁止通过 openFile 访问沙箱外部物理文件
                if (!isSafeLocalPath(context, path)) {
                    invoke.reject("安全拒绝：禁止打开沙箱外部的敏感文件")
                    return@execute
                }

                val file = java.io.File(path)
                if (!file.exists()) {
                    invoke.reject("文件不存在: $path")
                    return@execute
                }

                // 1. 自动提取并修正 MIME 类型
                val ext = file.extension.lowercase()
                val mimeType = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "*/*"
                Log.i(TAG, "[openFile] Opening file: ${file.absolutePath} (ext=$ext, mime=$mimeType)")

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

                // 3. 构建并分发默认的系统 ACTION_VIEW 意图
                val intent = Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(uri, mimeType)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }

                context.startActivity(intent)
                invoke.resolve()
            } catch (e: android.content.ActivityNotFoundException) {
                val ext = java.io.File(path).extension.lowercase()
                Log.e(TAG, "[openFile] No activity found to handle file type: .$ext", e)
                invoke.reject("您的手机上未安装能打开此类文件 (.$ext) 的应用，请先安装相关阅读器 (如 WPS Office)。")
            } catch (e: Throwable) {
                Log.e(TAG, "[openFile] Native file viewing failed", e)
                invoke.reject("打开文件失败: ${e.message}")
            }
        }
    }

    // ==================================================================
    // Security Sandbox Boundary & Verification
    // ==================================================================
    private fun isSafeLocalPath(context: Context, path: String): Boolean {
        return try {
            val file = java.io.File(path).canonicalFile
            val cacheDir = context.cacheDir.canonicalFile
            val filesDir = context.filesDir.canonicalFile
            val externalFilesDir = context.getExternalFilesDir(null)?.canonicalFile
            val externalCacheDir = context.externalCacheDir?.canonicalFile

            file.path.startsWith(cacheDir.path) ||
            file.path.startsWith(filesDir.path) ||
            (externalFilesDir != null && file.path.startsWith(externalFilesDir.path)) ||
            (externalCacheDir != null && file.path.startsWith(externalCacheDir.path))
        } catch (e: Exception) {
            false
        }
    }

    // ==================================================================
    // Universal Media Exporter & Gallery Writer
    // ==================================================================
    @Command
    fun saveImageToGallery(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageArgs::class.java)
        if (args.sourceUrl.isBlank()) {
            invoke.reject("图片地址为空")
            return
        }

        fileIoExecutor.execute {
            try {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到相册需要储存空间权限")
                        return@execute
                    }
                }

                val loaded = loadImageBytes(args.sourceUrl)
                if (!loaded.mimeType.startsWith("image/")) {
                    invoke.reject("当前资源不是图片: ${loaded.mimeType}")
                    return@execute
                }

                val displayName = buildGalleryFileName(args.fileName, args.sourceUrl, loaded.mimeType)
                val savedUri = writeImageToGallery(loaded.bytes, displayName, loaded.mimeType)
                val result = JSObject().apply {
                    put("uri", savedUri.toString())
                    put("displayName", displayName)
                    put("mimeType", loaded.mimeType)
                    put("size", loaded.bytes.size)
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageToGallery failed", e)
                invoke.reject("保存图片失败: ${e.message}")
            }
        }
    }

    @Command
    fun saveImageFromPath(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageFromPathArgs::class.java)
        if (args.imagePath.isBlank()) {
            invoke.reject("物理文件路径为空")
            return
        }

        // 1. 安全边界检查：强制限定临时文件必须处于沙箱缓存目录内，严防路径遍历与本地漏洞越界
        if (!isSafeLocalPath(activity, args.imagePath)) {
            invoke.reject("非法的本地文件读取边界，已被安全沙箱拒绝")
            return
        }

        fileIoExecutor.execute {
            val file = java.io.File(args.imagePath)
            try {
                if (!file.exists()) {
                    invoke.reject("本地临时文件不存在")
                    return@execute
                }

                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到相册需要储存空间权限")
                        return@execute
                    }
                }

                // 2. 读取图片二进制流
                val bytes = file.readBytes()
                
                // 3. 安全魔数嗅探：强制检测图片格式，坚决拒收假冒图片绕过的攻击
                val mimeType = sniffImageMime(bytes, file.name, true)
                if (!mimeType.startsWith("image/")) {
                    invoke.reject("当前资源不是图片: $mimeType")
                    return@execute
                }

                val displayName = buildGalleryFileName(args.fileName, file.name, mimeType)
                val savedUri = writeImageToGallery(bytes, displayName, mimeType)
                val result = JSObject().apply {
                    put("uri", savedUri.toString())
                    put("displayName", displayName)
                    put("mimeType", mimeType)
                    put("size", bytes.size)
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "saveImageFromPath failed", e)
                invoke.reject("保存图片失败: ${e.message}")
            } finally {
                // 4. 秒结物理清理：无论写入成功与否，立即擦除临时物理文件，防范残留泄漏
                try {
                    if (file.exists()) {
                        file.delete()
                    }
                } catch (ex: Exception) {
                    Log.e(TAG, "Failed to clean up temporary save image file", ex)
                }
            }
        }
    }

    private data class LoadedImage(val bytes: ByteArray, val mimeType: String)

    private fun loadImageBytes(sourceUrl: String): LoadedImage {
        if (sourceUrl.startsWith("data:", ignoreCase = true)) {
            return loadDataUrlImage(sourceUrl)
        }

        if (sourceUrl.startsWith("content:", ignoreCase = true)) {
            val uri = Uri.parse(sourceUrl)
            val mime = activity.contentResolver.getType(uri) ?: mimeFromSource(sourceUrl)
            val bytes = activity.contentResolver.openInputStream(uri).use { input ->
                readBytesLimited(input ?: throw IllegalStateException("无法读取 content 图片"))
            }
            return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = true))
        }

        if (sourceUrl.startsWith("file:", ignoreCase = true) || sourceUrl.startsWith("/")) {
            val path = if (sourceUrl.startsWith("file:", ignoreCase = true)) {
                Uri.parse(sourceUrl).path ?: sourceUrl.removePrefix("file://")
            } else {
                sourceUrl
            }
            
            // 💥 安全防线：本地路径强制进行沙箱越权校验
            if (!isSafeLocalPath(activity, path)) {
                throw SecurityException("越权拒绝：禁止读取沙箱外部资源")
            }

            val file = java.io.File(path)
            val bytes = file.inputStream().use { readBytesLimited(it) }
            return LoadedImage(bytes, sniffImageMime(bytes, mimeFromSource(file.name), isLocal = true))
        }

        return loadNetworkImage(sourceUrl)
    }

    private fun loadNetworkImage(sourceUrl: String): LoadedImage {
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
            val bytes = connection.inputStream.use { readBytesLimited(it) }
            return LoadedImage(bytes, sniffImageMime(bytes, contentType ?: mimeFromSource(sourceUrl), isLocal = false))
        } finally {
            connection.disconnect()
        }
    }

    private fun loadDataUrlImage(dataUrl: String): LoadedImage {
        val commaIndex = dataUrl.indexOf(',')
        if (commaIndex <= 0) throw IllegalArgumentException("无效的 data URL")

        val header = dataUrl.substring(5, commaIndex)
        val mime = header.substringBefore(";").ifBlank { "application/octet-stream" }.lowercase(Locale.US)
        val payload = dataUrl.substring(commaIndex + 1)
        val bytes = if (header.contains(";base64", ignoreCase = true)) {
            Base64.decode(payload, Base64.DEFAULT)
        } else {
            URLDecoder.decode(payload, "UTF-8").toByteArray(Charsets.UTF_8)
        }
        return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = false))
    }

    private fun readBytesLimited(input: InputStream, maxBytes: Int = 50 * 1024 * 1024): ByteArray {
        val output = ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        var total = 0
        while (true) {
            val read = input.read(buffer)
            if (read == -1) break
            total += read
            if (total > maxBytes) {
                throw IllegalArgumentException("图片过大，超过 50MB")
            }
            output.write(buffer, 0, read)
        }
        return output.toByteArray()
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
            "heic", "heif" -> "image/heic"
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
            "image/heic" -> "heic"
            "image/heif" -> "heif"
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
        val ext = sanitized.substringAfterLast('.', "").lowercase(Locale.US).takeIf { it.isNotBlank() } ?: extensionForMime(mimeType)
        return "$base.$ext"
    }

    private fun writeImageToGallery(bytes: ByteArray, displayName: String, mimeType: String): Uri {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = activity.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, displayName)
                put(MediaStore.Images.Media.MIME_TYPE, mimeType)
                put(MediaStore.Images.Media.RELATIVE_PATH, "${Environment.DIRECTORY_PICTURES}/VCPMobile")
                put(MediaStore.Images.Media.IS_PENDING, 1)
            }
            val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
                ?: throw IllegalStateException("无法创建相册图片")
            try {
                resolver.openOutputStream(uri)?.use { it.write(bytes) }
                    ?: throw IllegalStateException("无法写入相册图片")
                values.clear()
                values.put(MediaStore.Images.Media.IS_PENDING, 0)
                resolver.update(uri, values, null, null)
                return uri
            } catch (e: Throwable) {
                resolver.delete(uri, null, null)
                throw e
            }
        }

        val picturesDir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_PICTURES)
        val appDir = java.io.File(picturesDir, "VCPMobile").apply { mkdirs() }
        var outputFile = java.io.File(appDir, displayName)
        if (outputFile.exists()) {
            val base = displayName.substringBeforeLast('.', displayName)
            val ext = displayName.substringAfterLast('.', "")
            var index = 1
            do {
                outputFile = java.io.File(appDir, if (ext.isBlank()) "${base}_$index" else "${base}_$index.$ext")
                index += 1
            } while (outputFile.exists())
        }

        java.io.FileOutputStream(outputFile).use { it.write(bytes) }
        MediaScannerConnection.scanFile(activity, arrayOf(outputFile.absolutePath), arrayOf(mimeType), null)
        return Uri.fromFile(outputFile)
    }

    // ==================================================================
    // Webview High Performance Capture
    // ==================================================================
    @Command
    fun captureWindowSnapshot(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(CaptureWindowSnapshotArgs::class.java)
        } catch (_: Throwable) {
            CaptureWindowSnapshotArgs()
        }

        val maxWidth = args.maxWidth.coerceIn(160, 420)
        val quality = args.quality.coerceIn(45, 85)

        // 💥 去掉锁机制，采用完全异步的 resolve/reject 调用模式，避免 Tokio 核心线程被 latch.await 挂起
        activity.runOnUiThread {
            try {
                val rootView = activity.window.decorView.rootView
                val sourceWidth = rootView.width
                val sourceHeight = rootView.height
                if (sourceWidth <= 0 || sourceHeight <= 0) {
                    invoke.reject("View has invalid size: ${sourceWidth}x${sourceHeight}")
                    return@runOnUiThread
                }

                val scale = min(1f, maxWidth.toFloat() / sourceWidth.toFloat())
                val outputWidth = max(1, (sourceWidth * scale).roundToInt())
                val outputHeight = max(1, (sourceHeight * scale).roundToInt())
                val snapshot = Bitmap.createBitmap(outputWidth, outputHeight, Bitmap.Config.RGB_565)
                val canvas = Canvas(snapshot)
                canvas.scale(scale, scale)
                rootView.draw(canvas)

                val encoded = ByteArrayOutputStream()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    snapshot.compress(Bitmap.CompressFormat.WEBP_LOSSY, quality, encoded)
                } else {
                    @Suppress("DEPRECATION")
                    snapshot.compress(Bitmap.CompressFormat.WEBP, quality, encoded)
                }
                snapshot.recycle() // 及时物理释放内存，防御 WebView 渲染高频截图导致 OOM

                val base64 = Base64.encodeToString(encoded.toByteArray(), Base64.NO_WRAP)
                val resultObject = JSObject().apply {
                    put("dataUrl", "data:image/webp;base64,$base64")
                    put("width", outputWidth)
                    put("height", outputHeight)
                }
                invoke.resolve(resultObject)
            } catch (e: Throwable) {
                Log.e(TAG, "captureWindowSnapshot failed", e)
                invoke.reject(e.message ?: "captureWindowSnapshot failed")
            }
        }
    }

    @Command
    fun processImage(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessImageArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processImageAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process image")
            }
        }
    }

    @Command
    fun processVideo(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessVideoArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processVideoAsync(args.path, activity) { result ->
            result.onSuccess { framePaths ->
                val arr = JSArray()
                for (p in framePaths) {
                    arr.put(p)
                }
                val resObj = JSObject().apply {
                    put("paths", arr)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process video")
            }
        }
    }

    @Command
    fun processAudio(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessAudioArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("Invalid arguments: ${e.message}")
            return
        }

        MediaBridge.processAudioAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "Failed to process audio")
            }
        }
    }

    private var downloadNotificationBuilder: androidx.core.app.NotificationCompat.Builder? = null
    private val DOWNLOAD_NOTIF_ID = 0x53545209
    private val DOWNLOAD_CHANNEL_ID = "apk_download"

    private fun createDownloadNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val name = "应用更新下载"
            val descriptionText = "显示 APK 安装包的下载进度"
            val importance = android.app.NotificationManager.IMPORTANCE_LOW
            val channel = android.app.NotificationChannel(DOWNLOAD_CHANNEL_ID, name, importance).apply {
                description = descriptionText
            }
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.createNotificationChannel(channel)
        }
    }

    @Command
    fun startDownloadNotification(invoke: Invoke) {
        try {
            createDownloadNotificationChannel()
            val builder = androidx.core.app.NotificationCompat.Builder(activity, DOWNLOAD_CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setContentTitle("正在下载 VCP Mobile 更新...")
                .setContentText("已下载 0%")
                .setOngoing(true)
                .setProgress(100, 0, false)
                .setOnlyAlertOnce(true)

            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            downloadNotificationBuilder = builder
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun updateDownloadNotification(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(UpdateDownloadNotifArgs::class.java)
            val progress = args.progress
            val text = args.text ?: "正在下载..."
            
            val builder = downloadNotificationBuilder
            if (builder != null) {
                builder.setProgress(100, progress, false)
                    .setContentText(text)
                val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
                notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "updateDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }

    @Command
    fun cancelDownloadNotification(invoke: Invoke) {
        try {
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.cancel(DOWNLOAD_NOTIF_ID)
            downloadNotificationBuilder = null
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "cancelDownloadNotification failed", e)
            invoke.reject(e.message ?: "Unknown error")
        }
    }
}

@InvokeArg
class StartStreamArgs {
    lateinit var agentName: String
}

@InvokeArg
class RequestPermissionArgs {
    lateinit var type: String
}

@InvokeArg
class OpenFileArgs {
    lateinit var path: String
}

@InvokeArg
class PickFileArgs {
    var mode: String = "file"
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
class CaptureWindowSnapshotArgs {
    var maxWidth: Int = 200 // 与 Rust 侧默认参数对齐
    var quality: Int = 64  // 与 Rust 侧默认参数对齐
}

@InvokeArg
class ProcessImageArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessVideoArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessAudioArgs {
    lateinit var path: String
}

@InvokeArg
class UpdateDownloadNotifArgs {
    var progress: Int = 0
    var text: String? = null
}

