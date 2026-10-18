package com.vcp.mobile.media

import android.content.Context
import android.graphics.Bitmap
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import androidx.media3.common.Format
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.ChannelMixingAudioProcessor
import androidx.media3.common.audio.ChannelMixingMatrix
import androidx.media3.common.audio.SonicAudioProcessor
import androidx.media3.common.audio.ToInt16PcmAudioProcessor
import androidx.media3.common.util.UnstableApi
import androidx.media3.transformer.Codec
import androidx.media3.transformer.Composition
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.DefaultMuxer
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.Muxer
import androidx.media3.transformer.Transformer
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.OutputStream
import java.nio.ByteBuffer
import java.util.ArrayDeque
import java.util.concurrent.CancellationException
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.min

/**
 * Owns native attachment work until its process callback has settled. Rust owns the
 * unique output directory and only removes it after this owner has drained cancellation.
 * Registration before dispatch makes cancel-before-process reject the late invocation,
 * without retaining cancellation tombstones for completed requests.
 */
@androidx.annotation.OptIn(markerClass = [UnstableApi::class])
internal class NativeMediaProcessor(context: Context) {
    data class AudioResult(
        val path: String,
        val mime: String = "audio/mp4",
        val codec: String = "aac",
        val container: String = "m4a",
        val sampleRate: Int = AUDIO_SAMPLE_RATE,
        val channels: Int = 1,
    )

    private class Operation(val id: String) {
        val cancelled = AtomicBoolean(false)
        var claimed = false
        var completion: ((Result<Any>) -> Unit)? = null
        val files = ArrayList<File>()
    }

    private data class AudioWork(
        val operation: Operation,
        val input: File,
        val output: File,
        val maxBytes: Long,
    )

    private val appContext = context.applicationContext
    private val lock = Any()
    private val operations = HashMap<String, Operation>()
    private var destroyed = false
    private var audioThread: HandlerThread? = null
    private var audioHandler: Handler? = null
    private val videoExecutor = Executors.newSingleThreadExecutor()
    // Accessed only on audioHandler's looper; media codecs run on Media3's own workers.
    private val audioQueue = ArrayDeque<AudioWork>()
    private var activeAudio: AudioWork? = null
    private var transformer: Transformer? = null

    fun prepare(operationId: String) = synchronized(lock) {
        check(!destroyed) { "Media processor is destroyed" }
        require(operationId.isNotBlank()) { "Missing media operation ID" }
        check(!operations.containsKey(operationId)) { "Media operation is already registered" }
        operations[operationId] = Operation(operationId)
    }

    fun cancel(operationId: String) {
        synchronized(lock) {
            val operation = operations[operationId] ?: return
            operation.cancelled.set(true)
            if (!operation.claimed) {
                operations.remove(operationId)
                return
            }
            audioHandler?.post { cancelAudio(operation) }
            // MediaMetadataRetriever cannot cancel an in-flight native decode. The video
            // worker observes the flag before/after each decode and write, releases it,
            // then resolves processVideo. The Rust caller waits for that terminal result.
        }
    }

    fun processAudio(
        operationId: String,
        inputPath: String,
        outputDir: String,
        maxBytes: Long,
        callback: (Result<AudioResult>) -> Unit,
    ) {
        submit(operationId, callback) { operation ->
            val handler = ensureAudioHandler()
            handler.post {
                try {
                    checkCancelled(operation)
                    val (input, directory) = validateFiles(inputPath, outputDir, maxBytes)
                    val output = File(directory, "audio.m4a")
                    check(!output.exists()) { "Audio output already exists" }
                    operation.files.add(output)
                    audioQueue.add(AudioWork(operation, input, output, maxBytes))
                    startNextAudio()
                } catch (error: Exception) {
                    finish(operation, Result.failure(error))
                }
            }
        }
    }

    fun processVideo(
        operationId: String,
        inputPath: String,
        outputDir: String,
        maxBytes: Long,
        callback: (Result<List<String>>) -> Unit,
    ) {
        submit(operationId, callback) { operation ->
            videoExecutor.execute {
                val result = runCatching {
                    checkCancelled(operation)
                    val (input, directory) = validateFiles(inputPath, outputDir, maxBytes)
                    extractVideo(operation, input, directory, maxBytes)
                }
                finish(operation, result)
            }
        }
    }

    /** Called by the plugin owner, not by WebView reload or background transitions. */
    fun destroy() {
        synchronized(lock) {
            if (destroyed) return
            destroyed = true
            val iterator = operations.values.iterator()
            while (iterator.hasNext()) {
                val operation = iterator.next()
                operation.cancelled.set(true)
                if (!operation.claimed) iterator.remove()
            }
            audioHandler?.post {
                activeAudio?.let { cancelAudio(it.operation) }
                while (audioQueue.isNotEmpty()) {
                    finish(audioQueue.removeFirst().operation, Result.failure(cancelled()))
                }
                audioThread?.quitSafely()
            }
            // Already submitted videos drain cooperatively and settle their invokes.
            videoExecutor.shutdown()
        }
    }

    private fun <T> submit(
        operationId: String,
        callback: (Result<T>) -> Unit,
        dispatch: (Operation) -> Unit,
    ) {
        var failure: Exception? = null
        synchronized(lock) {
            val operation = operations[operationId]
            if (destroyed || operation == null || operation.claimed) {
                failure = IllegalStateException("Media operation is unavailable or already started")
            } else {
                operation.claimed = true
                operation.completion = { result ->
                    @Suppress("UNCHECKED_CAST")
                    callback(result as Result<T>)
                }
                try {
                    // Dispatch under the registration lock: destroy cannot shut down the
                    // worker between claiming an operation and enqueuing its completion.
                    dispatch(operation)
                } catch (error: Exception) {
                    operations.remove(operationId)
                    failure = error
                }
            }
        }
        failure?.let { callback(Result.failure(it)) }
    }

    private fun ensureAudioHandler(): Handler {
        audioHandler?.let { return it }
        val thread = HandlerThread("VCP-AttachmentAudio").apply { start() }
        return Handler(thread.looper).also {
            audioThread = thread
            audioHandler = it
        }
    }

    private fun startNextAudio() {
        if (activeAudio != null) return
        while (audioQueue.isNotEmpty()) {
            val work = audioQueue.removeFirst()
            if (work.operation.cancelled.get()) {
                finish(work.operation, Result.failure(cancelled()))
                continue
            }
            activeAudio = work
            try {
                val encoderFactory = DefaultEncoderFactory.Builder(appContext)
                    .setEnableFallback(false)
                    .build()
                val audioEncoder = object : Codec.EncoderFactory by encoderFactory {
                    override fun audioNeedsEncoding() = true

                    override fun createForAudioEncoding(format: Format): Codec =
                        encoderFactory.createForAudioEncoding(
                            format.buildUpon()
                                .setCodecs("mp4a.40.2")
                                .setAverageBitrate(AUDIO_BIT_RATE)
                                .build()
                        )
                }
                val resampler = SonicAudioProcessor().apply {
                    setOutputSampleRateHz(AUDIO_SAMPLE_RATE)
                }
                val item = EditedMediaItem.Builder(MediaItem.fromUri(Uri.fromFile(work.input)))
                    .setRemoveVideo(true)
                    .setEffects(Effects(
                        listOf(ToInt16PcmAudioProcessor(), MonoAudioProcessor(), resampler),
                        emptyList(),
                    ))
                    .build()
                val mediaTransformer = Transformer.Builder(appContext)
                    .setLooper(checkNotNull(audioHandler).looper)
                    .setAudioMimeType(MimeTypes.AUDIO_AAC)
                    .setEncoderFactory(audioEncoder)
                    .setMuxerFactory(BudgetMuxerFactory(work))
                    .addListener(object : Transformer.Listener {
                        override fun onCompleted(composition: Composition, result: ExportResult) {
                            if (activeAudio !== work) return
                            val outcome = runCatching {
                                checkCancelled(work.operation)
                                check(work.output.length() in 1..work.maxBytes) {
                                    "Processed audio exceeds the byte budget or is empty"
                                }
                                check(result.sampleRate == AUDIO_SAMPLE_RATE && result.channelCount == 1) {
                                    "Audio encoder returned an unexpected sample format"
                                }
                                AudioResult(work.output.absolutePath)
                            }
                            completeAudio(work, outcome)
                        }

                        override fun onError(
                            composition: Composition,
                            result: ExportResult,
                            error: ExportException,
                        ) {
                            if (activeAudio === work) completeAudio(work, Result.failure(error))
                        }
                    })
                    .build()
                transformer = mediaTransformer
                // Full input is processed. The old silent 3500-second clipping is gone;
                // encoded byte limits fail explicitly instead of dropping the tail.
                mediaTransformer.start(item, work.output.absolutePath)
                return
            } catch (error: Exception) {
                try { transformer?.cancel() } catch (cleanupError: Exception) {
                    Log.w(TAG, "Unable to cancel failed audio export", cleanupError)
                }
                transformer = null
                activeAudio = null
                finish(work.operation, Result.failure(error))
            }
        }
    }

    private fun completeAudio(work: AudioWork, outcome: Result<AudioResult>) {
        transformer = null // Media3 releases codecs and muxer before the terminal listener.
        activeAudio = null
        finish(work.operation, outcome)
        startNextAudio()
    }

    private fun cancelAudio(operation: Operation) {
        val work = activeAudio
        if (work?.operation === operation) {
            // cancel() synchronously drains Media3 on its application looper. It does
            // not deliver onError/onCompleted; this owner must settle processAudio.
            try { transformer?.cancel() } catch (error: Exception) {
                Log.w(TAG, "Unable to cancel audio export", error)
            } finally {
                transformer = null
                activeAudio = null
                finish(operation, Result.failure(cancelled()))
                startNextAudio()
            }
        } else {
            val iterator = audioQueue.iterator()
            while (iterator.hasNext()) {
                if (iterator.next().operation === operation) {
                    iterator.remove()
                    finish(operation, Result.failure(cancelled()))
                    break
                }
            }
        }
    }

    private class MonoAudioProcessor(
        private val mixer: ChannelMixingAudioProcessor = ChannelMixingAudioProcessor(),
    ) : AudioProcessor by mixer {
        override fun configure(input: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
            // Install a matrix for the actual decoder format, including format changes.
            // Media3 owns PCM conversion/buffers; this retains the previous equal-channel mix.
            mixer.putChannelMixingMatrix(ChannelMixingMatrix(
                input.channelCount, 1, FloatArray(input.channelCount) { 1f / input.channelCount },
            ))
            return mixer.configure(input)
        }
    }

    private class BudgetMuxerFactory(private val work: AudioWork) : Muxer.Factory {
        private val factory = DefaultMuxer.Factory()
        override fun getSupportedSampleMimeTypes(trackType: Int) =
            factory.getSupportedSampleMimeTypes(trackType)

        override fun create(path: String): Muxer {
            val delegate = factory.create(path)
            return object : Muxer by delegate {
                private var sampleBytes = 0L

                override fun addTrack(format: Format): Int {
                    val config = format.initializationData.firstOrNull()
                    val audioObjectType = config?.firstOrNull()?.toInt()?.and(0xff)?.ushr(3)
                    if (format.sampleMimeType != MimeTypes.AUDIO_AAC ||
                        format.sampleRate != AUDIO_SAMPLE_RATE || format.channelCount != 1 ||
                        audioObjectType != 2
                    ) {
                        throw Muxer.MuxerException(
                            "Expected AAC-LC, 16 kHz mono output",
                            IOException("Audio encoder returned an unexpected format"),
                        )
                    }
                    return delegate.addTrack(format)
                }

                override fun writeSampleData(
                    trackIndex: Int, data: ByteBuffer, presentationTimeUs: Long, flags: Int,
                ) {
                    if (work.operation.cancelled.get()) {
                        throw Muxer.MuxerException("Media processing cancelled", cancelled())
                    }
                    if (data.remaining().toLong() > work.maxBytes - sampleBytes ||
                        work.output.length() > work.maxBytes
                    ) {
                        throw Muxer.MuxerException(
                            "Processed audio exceeds the byte budget",
                            IOException("Audio output byte budget exceeded"),
                        )
                    }
                    sampleBytes += data.remaining()
                    delegate.writeSampleData(trackIndex, data, presentationTimeUs, flags)
                }
            }
        }
    }

    private fun extractVideo(
        operation: Operation, input: File, directory: File, maxBytes: Long,
    ): List<String> {
        val retriever = MediaMetadataRetriever()
        try {
            retriever.setDataSource(input.absolutePath)
            val durationMs = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)
                ?.toLongOrNull()?.takeIf { it > 0 }
                ?: throw IOException("Video has no usable duration")
            // The previous 1 fps (<60s) + 1.5s dedup and 0.5 fps (>60s) both
            // retained 2-second samples. Compute at most 300 indices directly.
            val candidateCount = (durationMs - 1) / 2000 + 1
            val count = min(candidateCount, 300).toInt()
            val paths = ArrayList<String>(count)
            var totalBytes = 0L
            for (index in 0 until count) {
                checkCancelled(operation)
                val candidateIndex = index.toLong() * candidateCount / count
                val timeUs = Math.multiplyExact(candidateIndex, 2_000_000L)
                var frame: Bitmap? = null
                var scaled: Bitmap? = null
                try {
                    frame = retrieveFrame(retriever, timeUs, MediaMetadataRetriever.OPTION_CLOSEST_SYNC)
                        ?: retrieveFrame(retriever, timeUs, MediaMetadataRetriever.OPTION_NEXT_SYNC)
                    checkCancelled(operation)
                    val decoded = frame ?: throw IOException("Unable to extract video frame ${index + 1}")
                    // Use actual returned dimensions, which include the platform's
                    // rotation handling. Metadata width/height can describe storage axes.
                    val scale = min(1.0, min(1280.0 / decoded.width, 720.0 / decoded.height))
                    scaled = if (scale < 1.0) Bitmap.createScaledBitmap(
                        decoded, (decoded.width * scale).toInt().coerceAtLeast(1),
                        (decoded.height * scale).toInt().coerceAtLeast(1), true,
                    ) else decoded
                    val frameFile = File(directory, "frame_%04d.jpg".format(index + 1))
                    check(!frameFile.exists()) { "Video frame output already exists" }
                    operation.files.add(frameFile)
                    val encodedFrame = checkNotNull(scaled)
                    FileOutputStream(frameFile).use { output ->
                        val limited = BudgetOutputStream(output, maxBytes - totalBytes, operation)
                        val encoded = encodedFrame.compress(Bitmap.CompressFormat.JPEG, 90, limited)
                        checkCancelled(operation)
                        if (limited.exceeded) throw IOException("Extracted video frames exceed the byte budget")
                        check(encoded) { "Unable to encode video frame" }
                        totalBytes += limited.written
                    }
                    check(frameFile.length() > 0) { "Encoded video frame is empty" }
                    paths.add(frameFile.absolutePath)
                } finally {
                    if (scaled !== frame) scaled?.recycle()
                    frame?.recycle()
                }
            }
            checkCancelled(operation)
            check(paths.isNotEmpty()) { "No video frames could be extracted" }
            return paths
        } finally {
            retriever.release()
        }
    }

    private fun retrieveFrame(retriever: MediaMetadataRetriever, timeUs: Long, option: Int): Bitmap? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            // API 27+ scales before returning pixels to the app. The system preserves
            // aspect ratio; API 26 alone requires full-size decode followed by scaling.
            retriever.getScaledFrameAtTime(timeUs, option, 1280, 720)
        } else {
            retriever.getFrameAtTime(timeUs, option)
        }

    private class BudgetOutputStream(
        private val output: OutputStream,
        private val limit: Long,
        private val operation: Operation,
    ) : OutputStream() {
        var written = 0L
            private set
        var exceeded = false
            private set

        override fun write(value: Int) {
            reserve(1)
            output.write(value)
        }

        override fun write(bytes: ByteArray, offset: Int, length: Int) {
            reserve(length)
            output.write(bytes, offset, length)
        }

        private fun reserve(length: Int) {
            checkCancelled(operation)
            if (length.toLong() > limit - written) {
                exceeded = true
                throw IOException("Extracted video frames exceed the byte budget")
            }
            written += length
        }
    }

    private fun finish(operation: Operation, outcome: Result<Any>) {
        val completion = synchronized(lock) {
            if (operations[operation.id] !== operation) return
            operations.remove(operation.id)
            operation.completion.also { operation.completion = null }
        } ?: return
        val terminal = if (operation.cancelled.get()) Result.failure(cancelled()) else outcome
        if (terminal.isFailure) {
            operation.files.forEach { file ->
                try {
                    if (file.exists() && !file.delete()) Log.w(TAG, "Unable to delete partial media output")
                } catch (error: Exception) {
                    Log.w(TAG, "Unable to delete partial media output", error)
                }
            }
        }
        try { completion(terminal) } catch (error: Exception) {
            // Invoke delivery can fail after the WebView disappeared. The Rust-owned
            // directory remains eligible for its enclosing operation's cleanup.
            Log.w(TAG, "Unable to deliver media operation result", error)
        }
    }

    private fun validateFiles(inputPath: String, outputDir: String, maxBytes: Long): Pair<File, File> {
        require(maxBytes > 0) { "Invalid media byte budget" }
        val input = File(inputPath)
        require(input.isFile) { "Media input file does not exist" }
        val directory = File(outputDir)
        require(directory.isDirectory) { "Media output directory does not exist" }
        return input to directory
    }

    private companion object {
        const val TAG = "VCPMobile_Media"
        const val AUDIO_SAMPLE_RATE = 16_000
        const val AUDIO_BIT_RATE = 32_000

        fun cancelled() = CancellationException("Media processing cancelled")
        fun checkCancelled(operation: Operation) {
            if (operation.cancelled.get()) throw cancelled()
        }
    }
}
