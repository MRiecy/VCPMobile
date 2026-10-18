package com.vcp.mobile.webview

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Build
import android.util.Base64
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

internal class WindowSnapshot(private val activity: Activity) {
    private companion object { const val TAG = "VcpMobilePlugin" }
    private val destroyed = AtomicBoolean(false)

    fun destroy() {
        destroyed.set(true)
    }

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
            if (destroyed.get()) {
                invoke.reject("plugin was destroyed")
                return@runOnUiThread
            }
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
                val encoded = ByteArrayOutputStream()
                val snapshot = Bitmap.createBitmap(outputWidth, outputHeight, Bitmap.Config.RGB_565)
                try {
                    val canvas = Canvas(snapshot)
                    canvas.scale(scale, scale)
                    rootView.draw(canvas)

                    val compressed = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        snapshot.compress(Bitmap.CompressFormat.WEBP_LOSSY, quality, encoded)
                    } else {
                        @Suppress("DEPRECATION")
                        snapshot.compress(Bitmap.CompressFormat.WEBP, quality, encoded)
                    }
                    check(compressed) { "Unable to encode window snapshot" }
                } finally {
                    snapshot.recycle()
                }

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
}

@InvokeArg
class CaptureWindowSnapshotArgs {
    var maxWidth: Int = 200 // 与 Rust 侧默认参数对齐
    var quality: Int = 64  // 与 Rust 侧默认参数对齐
}
