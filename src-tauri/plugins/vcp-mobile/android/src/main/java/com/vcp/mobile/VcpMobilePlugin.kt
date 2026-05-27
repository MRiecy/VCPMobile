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
import app.tauri.plugin.Invoke
import com.vcp.mobile.service.StreamKeepaliveService

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE"], alias = "storageLegacy"),
    Permission(strings = ["android.permission.RECORD_AUDIO"], alias = "microphone")
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

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val result = JSObject()
        result.put("notification", notificationGranted)
        result.put("storage", storageGranted)
        result.put("microphone", microphoneGranted)
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
                if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
                    ActivityCompat.requestPermissions(
                        activity,
                        arrayOf(android.Manifest.permission.RECORD_AUDIO),
                        999
                    )
                }
                invoke.resolve()
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

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)

        val json = """{"notification":$notificationGranted,"storage":$storageGranted,"microphone":$microphoneGranted,"battery":$batteryOptimizationIgnored}"""
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
    @Command
    fun pickFile(invoke: Invoke) {
        try {
            val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
                type = "*/*"
                addCategory(Intent.CATEGORY_OPENABLE)
            }
            startActivityForResult(invoke, intent, "onPickFileResult")
        } catch (e: Throwable) {
            Log.e(TAG, "[pickFile] Failed to start activity for result", e)
            invoke.reject("Failed to start native file picker: ${e.message}")
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
                val tempFile = java.io.File(context.cacheDir, "pick_${System.currentTimeMillis()}_temp")
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
                val finalTempFile = java.io.File(context.cacheDir, "$hash$fileExtension")
                
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
                invoke.reject("Handling picked file failed: ${e.message}")
            }
        }
    }

    private fun generateNativeThumbnail(context: Context, originalFile: java.io.File, hash: String): String? {
        val thumbDir = java.io.File(context.cacheDir, "thumbnails").apply { mkdirs() }
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
        val path = invoke.getString("path") ?: ""
        if (path.isEmpty()) {
            invoke.reject("Path is empty")
            return
        }
        
        fileIoExecutor.execute {
            try {
                val context = activity
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
                        "${context.packageName}.opener.fileprovider",
                        file
                    )
                } catch (e: Exception) {
                    Log.w(TAG, "[openFile] Fallback to default FileProvider authority", e)
                    FileProvider.getUriForFile(
                        context,
                        "${context.packageName}.fileprovider",
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
            } catch (e: Throwable) {
                Log.e(TAG, "[openFile] Native file viewing failed", e)
                invoke.reject("打开文件失败: ${e.message}")
            }
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
