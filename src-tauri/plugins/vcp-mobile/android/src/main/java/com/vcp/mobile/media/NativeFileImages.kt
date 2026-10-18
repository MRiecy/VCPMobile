package com.vcp.mobile.media

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ColorSpace
import android.graphics.Matrix
import android.media.ExifInterface
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.JSObject
import java.io.File
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

@InvokeArg
class DeriveImageArgs {
    lateinit var sourcePath: String
    lateinit var outputPath: String
    var maxDimension: Int = 2048
    var maxPixels: Long = 4_194_304
    var maxBytes: Long = 8 * 1024 * 1024
    var quality: Double = 0.82
    var preserveAlpha: Boolean = true
}

/** Shared serial decode owner for avatar work and Rust-requested derivatives. */
internal object NativeFileImages {
    @Synchronized
    fun derive(args: DeriveImageArgs): JSObject {
        require(args.maxDimension in 1..8192 && args.maxPixels in 1..33_554_432 && args.maxBytes > 0) { "image_budget: invalid dimensions" }
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(args.sourcePath, bounds)
        require(bounds.outWidth in 1..1_000_000 && bounds.outHeight in 1..1_000_000) { "image_decode: invalid source dimensions" }
        val scale = min(1.0, min(args.maxDimension.toDouble() / max(bounds.outWidth, bounds.outHeight), sqrt(args.maxPixels.toDouble() / (bounds.outWidth.toDouble() * bounds.outHeight))))
        val width = max(1, floor(bounds.outWidth * scale).toInt())
        val height = max(1, floor(bounds.outHeight * scale).toInt())
        var sample = 1
        // Pick a sample that bounds the decoder itself; don't decode a larger
        // bitmap and hope a later resize makes its peak allocation disappear.
        while ((bounds.outWidth.toLong() + sample - 1) / sample > width || (bounds.outHeight.toLong() + sample - 1) / sample > height) sample *= 2
        val options = BitmapFactory.Options().apply {
            inSampleSize = sample
            inPreferredConfig = Bitmap.Config.ARGB_8888
            inPreferredColorSpace = ColorSpace.get(ColorSpace.Named.SRGB)
        }
        val decoded = BitmapFactory.decodeFile(args.sourcePath, options) ?: error("image_decode: bounded decode failed")
        var transformed: Bitmap? = null
        var destination: File? = null
        var completed = false
        try {
            require(decoded.width.toLong() * decoded.height <= args.maxPixels) { "image_budget: decoded image exceeds pixel budget" }
            val orientation = try { ExifInterface(args.sourcePath).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL) } catch (_: Exception) { ExifInterface.ORIENTATION_NORMAL }
            val matrix = Matrix().apply {
                when (orientation) {
                    ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> setScale(-1f, 1f)
                    ExifInterface.ORIENTATION_ROTATE_180 -> setRotate(180f)
                    ExifInterface.ORIENTATION_FLIP_VERTICAL -> setScale(1f, -1f)
                    ExifInterface.ORIENTATION_TRANSPOSE -> { setRotate(90f); postScale(-1f, 1f) }
                    ExifInterface.ORIENTATION_ROTATE_90 -> setRotate(90f)
                    ExifInterface.ORIENTATION_TRANSVERSE -> { setRotate(-90f); postScale(-1f, 1f) }
                    ExifInterface.ORIENTATION_ROTATE_270 -> setRotate(-90f)
                }
            }
            val normalized = if (matrix.isIdentity) decoded else Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, matrix, true).also { transformed = it }
            if (android.os.Build.VERSION.SDK_INT >= 34) normalized.setGainmap(null)
            val png = args.preserveAlpha && normalized.hasAlpha()
            val result = File(args.outputPath + if (png) ".png" else ".jpg")
            destination = result
            check(result.parentFile?.let { it.isDirectory || it.mkdirs() } == true) { "image_write: output directory unavailable" }
            result.outputStream().use { stream ->
                check(normalized.compress(if (png) Bitmap.CompressFormat.PNG else Bitmap.CompressFormat.JPEG, (args.quality.coerceIn(0.1, 1.0) * 100).toInt(), stream)) { "image_encode: compression failed" }
            }
            require(result.length() in 1..args.maxBytes) { "image_too_large: encoded derivative exceeds byte budget" }
            completed = true
            return JSObject().apply {
                put("path", result.absolutePath); put("mime", if (png) "image/png" else "image/jpeg")
                put("size", result.length()); put("width", normalized.width); put("height", normalized.height)
            }
        } finally {
            transformed?.takeIf { it !== decoded }?.recycle()
            decoded.recycle()
            if (!completed) destination?.delete()
        }
    }
}
