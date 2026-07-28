package com.vcp.mobile

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.ComponentCallbacks2
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.util.Log
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.nio.charset.StandardCharsets
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 持久化 Android 端无法通过 WebView console 获取的崩溃与内存压力信息。
 */
@Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
object CrashDiagnostics {
    private const val TAG = "VcpCrashDiagnostics"
    private const val MAX_TRACE_BYTES = 128 * 1024
    private const val MAX_LOG_BYTES = 4L * 1024 * 1024
    private val installed = AtomicBoolean(false)
    private val writeLock = Any()

    fun install(context: Context) {
        if (!installed.compareAndSet(false, true)) return

        val appContext = context.applicationContext
        val previousHandler = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, throwable ->
            appendReport(
                appContext,
                "android-uncaught.log",
                buildString {
                    appendLine("类型: Java/Kotlin 未捕获异常")
                    appendLine("线程: ${thread.name}")
                    appendLine("异常: ${throwable.javaClass.name}: ${throwable.message.orEmpty()}")
                    appendLine(Log.getStackTraceString(throwable).take(MAX_TRACE_BYTES))
                }
            )
            previousHandler?.uncaughtException(thread, throwable)
        }

        appContext.registerComponentCallbacks(object : ComponentCallbacks2 {
            override fun onTrimMemory(level: Int) {
                appendReport(
                    appContext,
                    "memory-pressure.log",
                    "类型: onTrimMemory\n级别: $level (${trimLevelName(level)})"
                )
            }

            override fun onLowMemory() {
                appendReport(appContext, "memory-pressure.log", "类型: onLowMemory")
            }

            override fun onConfigurationChanged(newConfig: Configuration) = Unit
        })

        appendReport(appContext, "process-lifecycle.log", "类型: process-start\nPID: ${android.os.Process.myPid()}")
    }

    fun collectHistoricalExitReasons(context: Context): JSObject {
        val response = JSObject()
        val entries = JSArray()
        response.put("entries", entries)

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            response.put("supported", false)
            return response
        }

        return try {
            val manager = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
            val reasons = manager.getHistoricalProcessExitReasons(context.packageName, 0, 8)
            for (info in reasons) {
                val item = JSObject().apply {
                    put("timestamp", info.timestamp)
                    put("processName", info.processName.orEmpty())
                    put("reason", reasonName(info.reason))
                    put("reasonCode", info.reason)
                    put("status", info.status)
                    put("importance", info.importance)
                    put("pssKb", info.pss)
                    put("rssKb", info.rss)
                    put("description", info.description?.toString())
                    put("trace", readTrace(info.traceInputStream))
                }
                entries.put(item)
            }
            response.put("supported", true)
            response
        } catch (error: Throwable) {
            Log.e(TAG, "Failed to collect historical exit reasons", error)
            response.put("supported", false)
            response.put("error", error.message ?: error.javaClass.name)
            response
        }
    }

    private fun diagnosticsDir(context: Context): File =
        File(context.dataDir, "diagnostics").apply { mkdirs() }

    private fun appendReport(context: Context, fileName: String, body: String) {
        synchronized(writeLock) {
            try {
                val target = File(diagnosticsDir(context), fileName)
                if (target.length() >= MAX_LOG_BYTES) {
                    val backup = File(target.parentFile, "$fileName.1")
                    if (backup.exists()) backup.delete()
                    target.renameTo(backup)
                }
                FileOutputStream(target, true).bufferedWriter(StandardCharsets.UTF_8).use { writer ->
                    writer.appendLine()
                    writer.appendLine("===== ${timestamp()} =====")
                    writer.appendLine(body)
                    writer.flush()
                }
            } catch (error: Throwable) {
                Log.e(TAG, "Failed to persist crash diagnostics", error)
            }
        }
    }

    private fun readTrace(stream: InputStream?): String? {
        if (stream == null) return null
        return try {
            stream.use { input ->
                val buffer = ByteArray(8 * 1024)
                val output = java.io.ByteArrayOutputStream()
                while (output.size() < MAX_TRACE_BYTES) {
                    val remaining = MAX_TRACE_BYTES - output.size()
                    val count = input.read(buffer, 0, minOf(buffer.size, remaining))
                    if (count <= 0) break
                    output.write(buffer, 0, count)
                }
                output.toString(StandardCharsets.UTF_8.name())
            }
        } catch (error: Throwable) {
            "读取系统退出 trace 失败: ${error.message}"
        }
    }

    private fun timestamp(): String =
        SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSSZ", Locale.US).format(Date())

    private fun trimLevelName(level: Int): String = when (level) {
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_MODERATE -> "RUNNING_MODERATE"
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW -> "RUNNING_LOW"
        ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL -> "RUNNING_CRITICAL"
        ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN -> "UI_HIDDEN"
        ComponentCallbacks2.TRIM_MEMORY_BACKGROUND -> "BACKGROUND"
        ComponentCallbacks2.TRIM_MEMORY_MODERATE -> "MODERATE"
        ComponentCallbacks2.TRIM_MEMORY_COMPLETE -> "COMPLETE"
        else -> "UNKNOWN"
    }

    private fun reasonName(reason: Int): String = when (reason) {
        ApplicationExitInfo.REASON_EXIT_SELF -> "EXIT_SELF"
        ApplicationExitInfo.REASON_SIGNALED -> "SIGNALED"
        ApplicationExitInfo.REASON_LOW_MEMORY -> "LOW_MEMORY"
        ApplicationExitInfo.REASON_CRASH -> "CRASH"
        ApplicationExitInfo.REASON_CRASH_NATIVE -> "CRASH_NATIVE"
        ApplicationExitInfo.REASON_ANR -> "ANR"
        ApplicationExitInfo.REASON_INITIALIZATION_FAILURE -> "INITIALIZATION_FAILURE"
        ApplicationExitInfo.REASON_PERMISSION_CHANGE -> "PERMISSION_CHANGE"
        ApplicationExitInfo.REASON_EXCESSIVE_RESOURCE_USAGE -> "EXCESSIVE_RESOURCE_USAGE"
        ApplicationExitInfo.REASON_USER_REQUESTED -> "USER_REQUESTED"
        ApplicationExitInfo.REASON_USER_STOPPED -> "USER_STOPPED"
        ApplicationExitInfo.REASON_DEPENDENCY_DIED -> "DEPENDENCY_DIED"
        ApplicationExitInfo.REASON_OTHER -> "OTHER"
        ApplicationExitInfo.REASON_FREEZER -> "FREEZER"
        ApplicationExitInfo.REASON_PACKAGE_STATE_CHANGE -> "PACKAGE_STATE_CHANGE"
        ApplicationExitInfo.REASON_PACKAGE_UPDATED -> "PACKAGE_UPDATED"
        else -> "UNKNOWN"
    }
}
