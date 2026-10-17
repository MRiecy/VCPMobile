package com.vcp.mobile

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.Executors
import kotlin.math.max

object MediaBridge {
    private const val TAG = "VCPMobile_MediaBridge"
    private val fileIoExecutor = Executors.newFixedThreadPool(4)

    /**
     * 异步图片缩放与 WebP 压缩
     * 长边等比例缩放到 1120 包络框内，小图不放大。80% 质量。
     */
    fun processImageAsync(
        inputPath: String,
        context: Context,
        callback: (Result<String>) -> Unit
    ) {
        fileIoExecutor.execute {
            var rawBitmap: Bitmap? = null
            var scaledBitmap: Bitmap? = null
            var orientedBitmap: Bitmap? = null
            var outputFile: File? = null
            val outcome = try {
                val file = File(inputPath)
                if (!file.exists()) {
                    throw FileNotFoundException("Input file not found: $inputPath")
                }

                // 1. 获取图片原始宽高而不加载入内存，防止 OOM
                val options = BitmapFactory.Options().apply {
                    inJustDecodeBounds = true
                }
                BitmapFactory.decodeFile(inputPath, options)
                val origW = options.outWidth
                val origH = options.outHeight

                if (origW <= 0 || origH <= 0) {
                    throw Exception("Invalid image dimensions")
                }

                // 2. 计算缩放包络
                val maxDim = max(origW, origH)
                val scale = if (maxDim > 1120) {
                    1120f / maxDim
                } else {
                    1.0f
                }

                val targetW = (origW * scale).toInt().coerceAtLeast(1)
                val targetH = (origH * scale).toInt().coerceAtLeast(1)
                val orientation = readExifOrientation(inputPath)

                // 3. 计算合理 sampling size 减少内存开销
                val decodeOptions = BitmapFactory.Options().apply {
                    inSampleSize = calculateInSampleSize(origW, origH, targetW, targetH)
                }
                rawBitmap = BitmapFactory.decodeFile(inputPath, decodeOptions)
                    ?: throw Exception("Failed to decode image bitmap")

                // 4. 先缩至目标包络，再做 EXIF 方向归一化，避免同时持有两张采样大图。
                val scaled = if (rawBitmap.width != targetW || rawBitmap.height != targetH) {
                    Bitmap.createScaledBitmap(rawBitmap, targetW, targetH, true)
                } else {
                    rawBitmap
                }
                scaledBitmap = scaled
                val oriented = applyExifOrientation(scaled, orientation)
                orientedBitmap = oriented
                val swapsDimensions = orientation == android.media.ExifInterface.ORIENTATION_TRANSPOSE ||
                    orientation == android.media.ExifInterface.ORIENTATION_ROTATE_90 ||
                    orientation == android.media.ExifInterface.ORIENTATION_TRANSVERSE ||
                    orientation == android.media.ExifInterface.ORIENTATION_ROTATE_270
                val visualTargetW = if (swapsDimensions) targetH else targetW
                val visualTargetH = if (swapsDimensions) targetW else targetH

                // 5. 写入现有受控 bridge 临时目录，前端用 delete_temp_file 释放。
                val bridgeTempDir = File(context.cacheDir, "vcp_bridge_temp").apply { mkdirs() }
                val outFile = File(bridgeTempDir, "avatar_work_" + UUID.randomUUID().toString() + ".webp")
                outputFile = outFile
                FileOutputStream(outFile).use { out ->
                    val compressed = if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                        oriented.compress(Bitmap.CompressFormat.WEBP_LOSSY, 80, out)
                    } else {
                        @Suppress("DEPRECATION")
                        oriented.compress(Bitmap.CompressFormat.WEBP, 80, out)
                    }
                    if (!compressed) throw Exception("Failed to encode avatar working image")
                }

                Log.d(TAG, "Image scale success: ${outFile.absolutePath} (${visualTargetW}x${visualTargetH})")
                Result.success(outFile.absolutePath)
            } catch (e: Exception) {
                Log.e(TAG, "Image scale error", e)
                outputFile?.delete()
                Result.failure(e)
            } finally {
                if (orientedBitmap !== rawBitmap) orientedBitmap?.recycle()
                if (scaledBitmap !== rawBitmap && scaledBitmap !== orientedBitmap) {
                    scaledBitmap?.recycle()
                }
                rawBitmap?.recycle()
            }

            // 确保 Native 位图已释放后再把路径交给 WebView，避免两侧解码峰值重叠。
            callback(outcome)
        }
    }

    // =============================================================================
    // 辅助函数
    // =============================================================================

    private fun calculateInSampleSize(origW: Int, origH: Int, reqW: Int, reqH: Int): Int {
        var inSampleSize = 1
        if (origH > reqH || origW > reqW) {
            val halfHeight = origH / 2
            val halfWidth = origW / 2
            while ((halfHeight / inSampleSize) >= reqH && (halfWidth / inSampleSize) >= reqW) {
                inSampleSize *= 2
            }
        }
        return inSampleSize
    }

    @Suppress("DEPRECATION")
    private fun readExifOrientation(path: String): Int {
        return try {
            android.media.ExifInterface(path).getAttributeInt(
                android.media.ExifInterface.TAG_ORIENTATION,
                android.media.ExifInterface.ORIENTATION_NORMAL
            )
        } catch (error: Exception) {
            Log.w(TAG, "Unable to read EXIF orientation for $path", error)
            android.media.ExifInterface.ORIENTATION_NORMAL
        }
    }

    @Suppress("DEPRECATION")
    private fun applyExifOrientation(source: Bitmap, orientation: Int): Bitmap {
        if (orientation == android.media.ExifInterface.ORIENTATION_NORMAL ||
            orientation == android.media.ExifInterface.ORIENTATION_UNDEFINED
        ) {
            return source
        }

        val matrix = Matrix()
        when (orientation) {
            android.media.ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.setScale(-1f, 1f)
            android.media.ExifInterface.ORIENTATION_ROTATE_180 -> matrix.setRotate(180f)
            android.media.ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.setScale(1f, -1f)
            android.media.ExifInterface.ORIENTATION_TRANSPOSE -> {
                matrix.setRotate(90f)
                matrix.postScale(-1f, 1f)
            }
            android.media.ExifInterface.ORIENTATION_ROTATE_90 -> matrix.setRotate(90f)
            android.media.ExifInterface.ORIENTATION_TRANSVERSE -> {
                matrix.setRotate(-90f)
                matrix.postScale(-1f, 1f)
            }
            android.media.ExifInterface.ORIENTATION_ROTATE_270 -> matrix.setRotate(-90f)
            else -> return source
        }
        return Bitmap.createBitmap(source, 0, 0, source.width, source.height, matrix, true)
    }

    // 快捷自定义异常
    class FileNotFoundException(message: String) : Exception(message)
}
