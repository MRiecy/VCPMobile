package com.vcp.mobile

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaExtractor
import android.media.MediaFormat
import android.media.MediaMetadataRetriever
import android.util.Log
import java.io.BufferedOutputStream
import java.io.File
import java.io.FileOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.UUID
import java.util.concurrent.Executors
import kotlin.math.abs
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
            try {
                val file = File(inputPath)
                if (!file.exists()) {
                    callback(Result.failure(FileNotFoundException("Input file not found: $inputPath")))
                    return@execute
                }

                // 1. 获取图片原始宽高而不加载入内存，防止 OOM
                val options = BitmapFactory.Options().apply {
                    inJustDecodeBounds = true
                }
                BitmapFactory.decodeFile(inputPath, options)
                val origW = options.outWidth
                val origH = options.outHeight

                if (origW <= 0 || origH <= 0) {
                    callback(Result.failure(Exception("Invalid image dimensions")))
                    return@execute
                }

                // 2. 计算缩放包络
                val maxDim = max(origW, origH)
                val scale = if (maxDim > 1120) {
                    1120f / maxDim
                } else {
                    1.0f
                }

                val targetW = (origW * scale).toInt()
                val targetH = (origH * scale).toInt()

                // 3. 计算合理 sampling size 减少内存开销
                val decodeOptions = BitmapFactory.Options().apply {
                    inSampleSize = calculateInSampleSize(origW, origH, targetW, targetH)
                }
                rawBitmap = BitmapFactory.decodeFile(inputPath, decodeOptions)
                    ?: throw Exception("Failed to decode image bitmap")

                // 4. 精确缩放 (filter = true 提供高保真插值)
                scaledBitmap = if (rawBitmap.width != targetW || rawBitmap.height != targetH) {
                    Bitmap.createScaledBitmap(rawBitmap, targetW, targetH, true)
                } else {
                    rawBitmap
                }

                // 5. 写入 WebP
                val uploadsDir = File(context.cacheDir, "uploads").apply { mkdirs() }
                val outFile = File(uploadsDir, "img_" + UUID.randomUUID().toString() + ".webp")
                FileOutputStream(outFile).use { out ->
                    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                        scaledBitmap.compress(Bitmap.CompressFormat.WEBP_LOSSY, 80, out)
                    } else {
                        @Suppress("DEPRECATION")
                        scaledBitmap.compress(Bitmap.CompressFormat.WEBP, 80, out)
                    }
                }

                Log.d(TAG, "Image scale success: ${outFile.absolutePath} (${targetW}x${targetH})")
                callback(Result.success(outFile.absolutePath))
            } catch (e: Exception) {
                Log.e(TAG, "Image scale error", e)
                callback(Result.failure(e))
            } finally {
                scaledBitmap?.let {
                    if (it != rawBitmap) {
                        it.recycle()
                    }
                }
                rawBitmap?.recycle()
            }
        }
    }

    /**
     * 异步视频帧提取与 JPEG 压缩
     * 时长决策 FPS：<=60s 1fps，>60s 0.5fps
     * 包络框尺寸：1280x720
     * 去重：< 1.5s
     * 最大帧数：300帧 (等距采样)
     * JPEG 质量 90%
     */
    fun processVideoAsync(
        inputPath: String,
        context: Context,
        callback: (Result<List<String>>) -> Unit
    ) {
        fileIoExecutor.execute {
            var retriever: MediaMetadataRetriever? = null
            var tempFolder: File? = null
            try {
                val file = File(inputPath)
                if (!file.exists()) {
                    callback(Result.failure(FileNotFoundException("Video file not found: $inputPath")))
                    return@execute
                }

                retriever = MediaMetadataRetriever()
                retriever.setDataSource(inputPath)

                // 1. 获取视频基本元数据
                val durationStr = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)
                    ?: throw Exception("Failed to retrieve video duration")
                val durationMs = durationStr.toLong()
                val durationSec = durationMs / 1000.0

                val origW = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toInt() ?: 1280
                val origH = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toInt() ?: 720

                // 2. 决策采样率
                val fps = if (durationSec <= 60.0) 1.0 else 0.5

                // 3. 构建均匀采样时间戳队列 (单位：秒)
                val allTimes = ArrayList<Double>()
                var t = 0.0
                while (t < durationSec) {
                    allTimes.add(t)
                    t += 1.0 / fps
                }

                // 4. 进行去重 (双指针，这里本来还有场景切换，但原方案中主要也是按间隔去重，此处对齐 >= 1.5s 的绝对间隔)
                val dedupedTimes = ArrayList<Double>()
                val dedupThreshold = 1.5
                for (time in allTimes) {
                    if (dedupedTimes.isEmpty() || abs(time - dedupedTimes.last()) >= dedupThreshold) {
                        dedupedTimes.add(time)
                    }
                }

                // 5. 限制最大帧数 (等距重采样)
                var finalTimes = dedupedTimes
                val maxFrames = 300
                if (finalTimes.size > maxFrames) {
                    val sampled = ArrayList<Double>(maxFrames)
                    val step = finalTimes.size.toDouble() / maxFrames.toDouble()
                    var idx = 0.0
                    while (idx < finalTimes.size) {
                        sampled.add(finalTimes[idx.toInt()])
                        idx += step
                    }
                    finalTimes = sampled
                }

                // 6. 计算缩放包络框尺寸 (宽限1280，高限720)
                val aspect = origW.toFloat() / origH.toFloat()
                var targetW = origW
                var targetH = origH
                if (aspect > 1.777778f) { // 宽屏
                    if (origW > 1280) {
                        targetW = 1280
                        targetH = (1280 / aspect).toInt()
                    }
                } else { // 窄屏/竖屏
                    if (origH > 720) {
                        targetH = 720
                        targetW = (720 * aspect).toInt()
                    }
                }

                // 7. 循环提帧并压缩保存
                val outputPaths = ArrayList<String>()
                val uploadsDir = File(context.cacheDir, "uploads").apply { mkdirs() }
                val localFolder = File(uploadsDir, "vid_" + UUID.randomUUID().toString())
                tempFolder = localFolder
                if (!localFolder.exists()) localFolder.mkdirs()

                // 为了速度考虑，MediaMetadataRetriever 在 API 27+ 支持指定大小获取，但兼容性较差，
                // 我们在获取后统一由 Matrix 或 Bitmap.createScaledBitmap 高保真缩小。
                for ((idx, timeSec) in finalTimes.withIndex()) {
                    val timeUs = (timeSec * 1_000_000).toLong()
                    // OPTION_CLOSEST_SYNC (更安全，防黑屏) 或 OPTION_CLOSEST
                    var frameBmp = retriever.getFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)

                    if (frameBmp == null) {
                        // 兜底尝试任何最近帧
                        frameBmp = retriever.getFrameAtTime(timeUs, MediaMetadataRetriever.OPTION_NEXT_SYNC)
                    }

                    if (frameBmp != null) {
                        // 高保真等比例缩放
                        val scaledBmp = if (frameBmp.width != targetW || frameBmp.height != targetH) {
                            Bitmap.createScaledBitmap(frameBmp, targetW, targetH, true)
                        } else {
                            frameBmp
                        }

                        val frameFile = File(tempFolder, String.format("frame_%04d.jpg", idx + 1))
                        FileOutputStream(frameFile).use { out ->
                            scaledBmp.compress(Bitmap.CompressFormat.JPEG, 90, out)
                        }
                        outputPaths.add(frameFile.absolutePath)

                        if (scaledBmp != frameBmp) {
                            scaledBmp.recycle()
                        }
                        frameBmp.recycle()
                    }
                }

                Log.d(TAG, "Video frame extraction success: ${outputPaths.size} frames extracted.")
                callback(Result.success(outputPaths))
            } catch (e: Exception) {
                Log.e(TAG, "Video processing error", e)
                // 发生异常时物理删除已创建的临时文件夹，杜绝垃圾帧泄露
                tempFolder?.let { folder ->
                    if (folder.exists()) {
                        try {
                            folder.deleteRecursively()
                        } catch (ignored: Exception) {}
                    }
                }
                callback(Result.failure(e))
            } finally {
                try {
                    retriever?.release()
                } catch (ignored: Exception) {}
            }
        }
    }

    /**
     * 异步音频转码至 AAC-LC 16kHz Mono 32kbps
     * 自动处理重采样、声道合并以及 ADTS 帧构建封装。
     */
    fun processAudioAsync(
        inputPath: String,
        context: Context,
        callback: (Result<String>) -> Unit
    ) {
        fileIoExecutor.execute {
            var extractor: MediaExtractor? = null
            var decoder: MediaCodec? = null
            var encoder: MediaCodec? = null
            val uploadsDir = File(context.cacheDir, "uploads").apply { mkdirs() }
            val outFile = File(uploadsDir, "aud_" + UUID.randomUUID().toString() + ".aac")
            var fos: FileOutputStream? = null
            var bos: BufferedOutputStream? = null

            try {
                val file = File(inputPath)
                if (!file.exists()) {
                    callback(Result.failure(FileNotFoundException("Audio file not found: $inputPath")))
                    return@execute
                }

                extractor = MediaExtractor()
                extractor.setDataSource(inputPath)

                var audioTrackIndex = -1
                var inputFormat: MediaFormat? = null
                for (i in 0 until extractor.trackCount) {
                    val format = extractor.getTrackFormat(i)
                    val mime = format.getString(MediaFormat.KEY_MIME) ?: ""
                    if (mime.startsWith("audio/")) {
                        audioTrackIndex = i
                        inputFormat = format
                        break
                    }
                }

                if (audioTrackIndex == -1 || inputFormat == null) {
                    throw Exception("No audio track found in $inputPath")
                }

                extractor.selectTrack(audioTrackIndex)

                // 1. 初始化解码器
                val mimeType = inputFormat.getString(MediaFormat.KEY_MIME)!!
                decoder = MediaCodec.createDecoderByType(mimeType)
                decoder.configure(inputFormat, null, null, 0)
                decoder.start()

                // 2. 初始化 AAC 编码器
                val outFormat = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_AAC, 16000, 1)
                outFormat.setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
                outFormat.setInteger(MediaFormat.KEY_BIT_RATE, 32000)
                outFormat.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 16384)

                encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
                encoder.configure(outFormat, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                encoder.start()

                fos = FileOutputStream(outFile)
                bos = BufferedOutputStream(fos, 32768)

                // 转码循环变量
                var isDecoderInputEOS = false
                var isDecoderOutputEOS = false
                var isEncoderOutputEOS = false

                val decoderInputBuffers = decoder.inputBuffers
                val decoderOutputBuffers = decoder.outputBuffers
                val encoderInputBuffers = encoder.inputBuffers
                val encoderOutputBuffers = encoder.outputBuffers

                val decoderBufferInfo = MediaCodec.BufferInfo()
                val encoderBufferInfo = MediaCodec.BufferInfo()

                // 获取输入的源音频参数
                val srcSampleRate = inputFormat.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                val srcChannelCount = inputFormat.getInteger(MediaFormat.KEY_CHANNEL_COUNT)

                // PCM 重采样状态状态缓存区
                var pendingPcmBytes = ByteArray(0)

                // 最大压制时长硬截断: 3500s -> 3500,000,000 Us
                val maxDurationUs = 3500L * 1_000_000L

                // 背压控制阈值：384KB (对齐线性重采样缓存)
                val BACKPRESSURE_THRESHOLD = 384 * 1024

                while (!isEncoderOutputEOS) {
                    // Feed 解码器 (当 pending 缓存低于背压阈值时才进行解码，实现生产-消费限速)
                    if (!isDecoderInputEOS && pendingPcmBytes.size < BACKPRESSURE_THRESHOLD) {
                        val inputBufIndex = decoder.dequeueInputBuffer(10000)
                        if (inputBufIndex >= 0) {
                            val dstBuffer = decoderInputBuffers[inputBufIndex]
                            dstBuffer.clear()
                            val sampleSize = extractor.readSampleData(dstBuffer, 0)
                            val presentationTimeUs = extractor.sampleTime

                            if (sampleSize < 0 || presentationTimeUs > maxDurationUs) {
                                decoder.queueInputBuffer(
                                    inputBufIndex, 0, 0, 0,
                                    MediaCodec.BUFFER_FLAG_END_OF_STREAM
                                )
                                isDecoderInputEOS = true
                            } else {
                                decoder.queueInputBuffer(
                                    inputBufIndex, 0, sampleSize, presentationTimeUs, 0
                                )
                                extractor.advance()
                            }
                        }
                    }

                    // 从解码器拿 PCM (同样受背压阀门控制)
                    if (!isDecoderOutputEOS && pendingPcmBytes.size < BACKPRESSURE_THRESHOLD) {
                        val res = decoder.dequeueOutputBuffer(decoderBufferInfo, 10000)
                        if (res >= 0) {
                            val pcmBuffer = decoderOutputBuffers[res]
                            pcmBuffer.position(decoderBufferInfo.offset)
                            pcmBuffer.limit(decoderBufferInfo.offset + decoderBufferInfo.size)

                            val chunk = ByteArray(decoderBufferInfo.size)
                            pcmBuffer.get(chunk)
                            decoder.releaseOutputBuffer(res, false)

                            // 将解码 PCM 进行重采样和降声道
                            val processedPcm = processPcmData(chunk, srcSampleRate, srcChannelCount)
                            pendingPcmBytes = concatByteArrays(pendingPcmBytes, processedPcm)

                            if ((decoderBufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                                isDecoderOutputEOS = true
                            }
                        } else if (res == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                            Log.d(TAG, "Decoder output format changed")
                        }
                    }

                    // Feed 编码器
                    var offset = 0
                    while (pendingPcmBytes.size - offset >= 4096 || (isDecoderOutputEOS && pendingPcmBytes.size > offset)) {
                        val encInputBufIndex = encoder.dequeueInputBuffer(10000)
                        if (encInputBufIndex >= 0) {
                            val sizeToFeed = Math.min(4096, pendingPcmBytes.size - offset)
                            val encBuffer = encoderInputBuffers[encInputBufIndex]
                            encBuffer.clear()
                            encBuffer.put(pendingPcmBytes, offset, sizeToFeed)
                            offset += sizeToFeed

                            val flags = if (isDecoderOutputEOS && offset >= pendingPcmBytes.size) {
                                MediaCodec.BUFFER_FLAG_END_OF_STREAM
                            } else {
                                0
                            }
                            encoder.queueInputBuffer(
                                encInputBufIndex, 0, sizeToFeed,
                                decoderBufferInfo.presentationTimeUs, flags
                            )
                        } else {
                            break
                        }
                    }

                    // 移除被编码消费的数据，并进行重整，释放闲置内存
                    if (offset > 0) {
                        pendingPcmBytes = pendingPcmBytes.copyOfRange(offset, pendingPcmBytes.size)
                    }

                    // 从编码器拿 AAC 帧，加上 ADTS 头写入文件
                    val encOutBufIndex = encoder.dequeueOutputBuffer(encoderBufferInfo, 10000)
                    if (encOutBufIndex >= 0) {
                        val encodedBuffer = encoderOutputBuffers[encOutBufIndex]
                        encodedBuffer.position(encoderBufferInfo.offset)
                        encodedBuffer.limit(encoderBufferInfo.offset + encoderBufferInfo.size)

                        if ((encoderBufferInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0) {
                            val outPacketSize = encoderBufferInfo.size + 7
                            val packet = ByteArray(outPacketSize)
                            // AAC profile LC = 2, 16kHz = index 8, channel Mono = 1
                            addADTStoPacket(packet, outPacketSize)
                            encodedBuffer.get(packet, 7, encoderBufferInfo.size)
                            bos.write(packet, 0, outPacketSize)
                        }
                        encoder.releaseOutputBuffer(encOutBufIndex, false)

                        if ((encoderBufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                            isEncoderOutputEOS = true
                        }
                    }
                }

                bos.flush()
                Log.d(TAG, "Audio transcode success: ${outFile.absolutePath}")
                callback(Result.success(outFile.absolutePath))
            } catch (e: Exception) {
                Log.e(TAG, "Audio transcode error", e)
                // 发生异常时物理删除已创建的临时转码文件，杜绝垃圾音频泄露
                try {
                    if (outFile.exists()) {
                        outFile.delete()
                    }
                } catch (ignored: Exception) {}
                callback(Result.failure(e))
            } finally {
                try { extractor?.release() } catch (ignored: Exception) {}
                try { decoder?.stop(); decoder?.release() } catch (ignored: Exception) {}
                try { encoder?.stop(); encoder?.release() } catch (ignored: Exception) {}
                try { bos?.close() } catch (ignored: Exception) {}
                try { fos?.close() } catch (ignored: Exception) {}
            }
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

    /**
     * 重采样 PCM 数据：多声道平均合并为单声道，并重采样为 16000Hz 16-bit PCM。
     */
    private fun processPcmData(srcBytes: ByteArray, srcSampleRate: Int, srcChannels: Int): ByteArray {
        if (srcBytes.isEmpty()) return srcBytes

        // 1. 将 ByteArray 解析为 ShortArray (16-bit PCM)
        val srcBuffer = ByteBuffer.wrap(srcBytes).order(ByteOrder.LITTLE_ENDIAN)
        val srcShorts = ShortArray(srcBytes.size / 2)
        srcBuffer.asShortBuffer().get(srcShorts)

        // 2. 双声道合并为单声道
        val monoShorts = if (srcChannels > 1) {
            val out = ShortArray(srcShorts.size / srcChannels)
            for (i in out.indices) {
                var sum = 0
                for (c in 0 until srcChannels) {
                    sum += srcShorts[i * srcChannels + c]
                }
                out[i] = (sum / srcChannels).toShort()
            }
            out
        } else {
            srcShorts
        }

        // 3. 重采样为 16000Hz
        if (srcSampleRate == 16000) {
            val outBuffer = ByteBuffer.allocate(monoShorts.size * 2).order(ByteOrder.LITTLE_ENDIAN)
            outBuffer.asShortBuffer().put(monoShorts)
            return outBuffer.array()
        }

        // 线性插值重采样
        val ratio = srcSampleRate.toDouble() / 16000.0
        val targetSize = (monoShorts.size / ratio).toInt()
        val destShorts = ShortArray(targetSize)

        for (i in 0 until targetSize) {
            val srcIndex = i * ratio
            val index = srcIndex.toInt()
            val fraction = srcIndex - index

            if (index >= monoShorts.size - 1) {
                destShorts[i] = monoShorts[monoShorts.size - 1]
            } else {
                val s0 = monoShorts[index].toInt()
                val s1 = monoShorts[index + 1].toInt()
                destShorts[i] = (s0 + fraction * (s1 - s0)).toInt().toShort()
            }
        }

        val outBuffer = ByteBuffer.allocate(destShorts.size * 2).order(ByteOrder.LITTLE_ENDIAN)
        outBuffer.asShortBuffer().put(destShorts)
        return outBuffer.array()
    }

    private fun concatByteArrays(a: ByteArray, b: ByteArray): ByteArray {
        val res = ByteArray(a.size + b.size)
        System.arraycopy(a, 0, res, 0, a.size)
        System.arraycopy(b, 0, res, a.size, b.size)
        return res
    }

    /**
     * 写入 ADTS 头部：AAC Profile = LC(2)，采样率 16000 (Index 8)，单声道 = 1
     */
    private fun addADTStoPacket(packet: ByteArray, packetLen: Int) {
        val profile = 2 // AAC LC
        val freqIdx = 8 // 16000Hz
        val chanCfg = 1 // Mono

        // fill in ADTS data
        packet[0] = 0xFF.toByte()
        packet[1] = 0xF9.toByte()
        packet[2] = (((profile - 1) shl 6) + (freqIdx shl 2) + (chanCfg shr 2)).toByte()
        packet[3] = (((chanCfg and 3) shl 6) + (packetLen shr 11)).toByte()
        packet[4] = ((packetLen and 0x7FF) shr 3).toByte()
        packet[5] = (((packetLen and 7) shl 5) + 0x1F).toByte()
        packet[6] = 0xFC.toByte()
    }

    // 快捷自定义异常
    class FileNotFoundException(message: String) : Exception(message)
}
