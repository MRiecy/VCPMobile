package com.vcp.mobile.voice

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioRouting
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.util.Log
import androidx.core.content.ContextCompat
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.FutureTask
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Semaphore
import java.io.File

@InvokeArg
class StartVoiceAudioArgs { var sessionId: String = ""; var callId: String = "" }
@InvokeArg
class VoiceCallArgs { var callId: String = "" }
@InvokeArg
class StopVoiceAudioArgs { var sessionId: String = "" }
@InvokeArg
class ExportVoiceSegmentArgs {
    var sessionId: String = ""; var audioEpoch: Long = 0; var startSample: Long = 0; var endSample: Long = 0
}
@InvokeArg
class ReleaseVoiceSegmentsArgs { var sessionId: String = "" }

/** Owns exactly one microphone. JavaScript visibility and permission dialogs never own audio. */
internal class NativeVoiceAudio(
    private val activity: Activity,
    private val requestPermission: (String, Invoke, String) -> Unit,
    private val eventsReady: () -> Boolean,
    private val emitVoiceEvent: (JSObject) -> Boolean,
    private val audioSession: VoiceAudioSession,
    private val voiceOutput: NativeVoiceOutput,
) {
    private class Reply(private val invoke: Invoke) {
        private val settled = AtomicBoolean(false)
        fun resolve() { if (settled.compareAndSet(false, true)) invoke.deliverResult() }
        fun resolve(value: JSObject): Boolean = settled.compareAndSet(false, true) && invoke.deliverResult(value)
        fun reject(reason: String) { if (settled.compareAndSet(false, true)) invoke.deliverError(reason) }
    }
    private data class SegmentPosition(val epoch: Long = 1, val sampleIndex: Long = 0)
    private class Session(val id: String, val callId: String, val invoke: Invoke) {
        val reply = Reply(invoke)
        val stopped = AtomicBoolean(false)
        val started = AtomicBoolean(false)
        val eventBackpressure = AtomicBoolean(false)
        val epoch = AtomicLong(1)
        val route = AtomicReference("unknown")
        val segments = VoiceSegmentStore(1)
        val segmentReplies = ConcurrentHashMap.newKeySet<Reply>()
        val segmentsReleased = AtomicBoolean(false)
        val position = AtomicReference(SegmentPosition())
        val captureSubmitted = AtomicBoolean(false)
        val sealed = AtomicBoolean(false)
        val stopReplies = ConcurrentHashMap.newKeySet<Reply>()
        var terminalKind = "stopped"
        var terminalReason = "cancelled"
        @Volatile var worker: Thread? = null
        var routing: VoiceAudioSession.InputLease? = null
    }
    private data class Frame(val session: Session, val epoch: Long, val payload: JSObject)

    private val main = Handler(Looper.getMainLooper())
    private val current = AtomicReference<Session?>(null)
    private val pendingFrames = ArrayBlockingQueue<Frame>(8)
    private val framePosted = AtomicBoolean(false)
    private val capture = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(1),
        { runnable -> Thread(runnable, "vcp-voice-capture").apply { isDaemon = true } }, ThreadPoolExecutor.AbortPolicy())
    private val segmentIo = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(4),
        { runnable -> Thread(runnable, "vcp-voice-export").apply { isDaemon = true } }, ThreadPoolExecutor.AbortPolicy())
    private val segmentCleanup = ConcurrentHashMap.newKeySet<VoiceSegmentStore>()
    private val segmentSlots = Semaphore(5)
    private val retainedCaptures = ConcurrentHashMap<String, Session>()
    private val releasedSegmentIds = LinkedHashSet<String>()
    private val cleanupPosted = AtomicBoolean(false)
    private var destroyed = false
    private var foreground = true
    private var permissionWaiter: Session? = null
    private val cancelledBeforeStart = LinkedHashSet<String>()

    private fun onMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) action() else main.post(action)
    }
    private fun isCurrent(session: Session) = current.get() === session && !session.stopped.get() && !session.eventBackpressure.get()
    private fun validId(id: String) = id.matches(Regex("[A-Za-z0-9_-]{1,128}"))

    fun beginVoiceCall(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoiceCallArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_call"); return@onMain }
        if (!validId(args.callId) || destroyed) { invoke.deliverError("cancelled:voice_call"); return@onMain }
        try { audioSession.beginCall(args.callId); invoke.deliverResult() }
        catch (error: Exception) { invoke.deliverError(error.message ?: "voice_call_unavailable") }
    }

    fun endVoiceCall(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoiceCallArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_call"); return@onMain }
        if (!validId(args.callId)) { invoke.deliverError("invalid_voice_call"); return@onMain }
        if (audioSession.hasCall(args.callId)) {
            current.get()?.takeIf { it.callId == args.callId }?.let { finish(it, "stopped", "call_ended") }
        }
        voiceOutput.endCall(args.callId) {
            audioSession.endCall(args.callId)
            invoke.deliverResult()
        }
    }

    fun startVoiceAudio(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(StartVoiceAudioArgs::class.java) }
        catch (error: Exception) { invoke.deliverError("invalid_voice_request:${error.message}"); return@onMain }
        if (!validId(args.sessionId)) { invoke.deliverError("invalid_voice_session"); return@onMain }
        if (destroyed || !foreground || cancelledBeforeStart.contains(args.sessionId) || releasedSegmentIds.contains(args.sessionId)) {
            invoke.deliverError("cancelled:voice_session_inactive"); return@onMain
        }
        if (!audioSession.hasCall(args.callId)) { invoke.deliverError("cancelled:voice_call"); return@onMain }
        if (!eventsReady()) { invoke.deliverError("voice_events_unavailable"); return@onMain }
        if (current.get() != null || permissionWaiter != null) { invoke.deliverError("voice_session_busy"); return@onMain }
        if (retainedCaptures.size >= 4) { invoke.deliverError("voice_segment_sessions_busy"); return@onMain }
        val session = Session(args.sessionId, args.callId, invoke)
        current.set(session)
        if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            permissionWaiter = session
            try { requestPermission("microphone", invoke, "onVoiceAudioPermissionResult") }
            catch (error: Exception) {
                permissionWaiter = null
                finish(session, "error", "microphone_permission_request_failed:${error.message}")
            }
        } else beginCapture(session)
    }

    fun onPermissionResult(invoke: Invoke) = onMain {
        val waiting = permissionWaiter?.takeIf { it.invoke === invoke } ?: return@onMain
        permissionWaiter = null
        if (!isCurrent(waiting)) return@onMain
        if (destroyed || !foreground) { finish(waiting, "stopped", "background"); return@onMain }
        if (ContextCompat.checkSelfPermission(activity, Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            finish(waiting, "error", "microphone_permission_denied")
        } else beginCapture(waiting)
    }

    fun stopVoiceAudio(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(StopVoiceAudioArgs::class.java) }
        catch (error: Exception) { invoke.deliverError("invalid_voice_request:${error.message}"); return@onMain }
        if (!validId(args.sessionId)) { invoke.deliverError("invalid_voice_session"); return@onMain }
        rememberCancelled(args.sessionId)
        val session = current.get()?.takeIf { it.id == args.sessionId } ?: retainedCaptures[args.sessionId]
        if (session == null) {
            invoke.deliverResult(JSObject().apply { put("audioEpoch", 0); put("sampleIndex", 0) })
        } else {
            val reply = Reply(invoke)
            if (session.sealed.get()) reply.resolve(segmentEnd(session))
            else { session.stopReplies.add(reply); finish(session, "stopped", "cancelled") }
        }
    }

    fun exportVoiceSegment(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(ExportVoiceSegmentArgs::class.java) }
        catch (error: Exception) { invoke.deliverError("invalid_voice_segment_request"); return@onMain }
        val session = current.get()?.takeIf { it.id == args.sessionId } ?: retainedCaptures[args.sessionId]
        if (session == null || session.segmentsReleased.get()) { invoke.deliverError("cancelled:voice_segment"); return@onMain }
        if (args.startSample < 0 || args.endSample <= args.startSample || args.endSample - args.startSample > 30L * 16_000) {
            invoke.deliverError("invalid_voice_segment_range"); return@onMain
        }
        if (!segmentSlots.tryAcquire()) { invoke.deliverError("voice_segment_export_busy"); return@onMain }
        val pcm = try { session.segments.copy(args.audioEpoch, args.startSample, args.endSample) }
        catch (error: Exception) {
            segmentSlots.release()
            invoke.deliverError(error.message ?: "invalid_voice_segment_range")
            return@onMain
        }
        val reply = Reply(invoke)
        session.segmentReplies.add(reply)
        try {
            segmentIo.execute {
                var file: File? = null
                try {
                    check(!session.segmentsReleased.get()) { "cancelled:voice_segment" }
                    val exported = session.segments.writeWav(File(activity.cacheDir, "voice-segments"), session.id, pcm)
                    file = exported
                    check(!session.segmentsReleased.get()) { "cancelled:voice_segment" }
                    val delivered = reply.resolve(JSObject().apply {
                        put("path", exported.absolutePath); put("sampleRate", 16_000); put("sampleCount", pcm.size)
                    })
                    if (!delivered) session.segments.discard(exported)
                } catch (error: Throwable) {
                    file?.let { session.segments.discard(it) }
                    val reason = error.message.orEmpty()
                    reply.reject(if (reason.startsWith("voice_segment_") || reason.startsWith("invalid_voice_segment_") || reason.startsWith("cancelled:")) reason else "voice_segment_export_failed")
                } finally {
                    session.segmentReplies.remove(reply)
                    segmentSlots.release()
                    session.segments.cleanupIfClosed()
                    drainSegmentCleanup()
                }
            }
        } catch (_: RejectedExecutionException) {
            session.segmentReplies.remove(reply)
            segmentSlots.release()
            reply.reject("voice_segment_export_busy")
        }
    }

    fun releaseVoiceSegments(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(ReleaseVoiceSegmentsArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_segment_request"); return@onMain }
        if (!validId(args.sessionId)) { invoke.deliverError("invalid_voice_session"); return@onMain }
        releasedSegmentIds.add(args.sessionId)
        if (releasedSegmentIds.size > 32) releasedSegmentIds.remove(releasedSegmentIds.first())
        val session = retainedCaptures.remove(args.sessionId) ?: current.get()?.takeIf { it.id == args.sessionId }
        session?.let(::closeSegments)
        invoke.deliverResult()
    }


    fun onForeground() = onMain { if (!destroyed) foreground = true }
    fun onBackground() = onMain {
        foreground = false
        current.get()?.let { finish(it, "stopped", "background") }
    }

    fun destroy() = onMain {
        if (destroyed) return@onMain
        destroyed = true
        foreground = false
        current.get()?.let { finish(it, "stopped", "destroyed") }
        val closing = retainedCaptures.values.toList()
        closing.forEach(::closeSegments)
        retainedCaptures.clear()
        permissionWaiter?.reply?.reject("cancelled:plugin_destroyed")
        permissionWaiter = null
        capture.shutdownNow()
        closing.filter { it.worker == null }.forEach(::sealCapture)
        segmentIo.shutdown() // Already-admitted file jobs settle and clean their files.
    }

    private fun beginCapture(session: Session) {
        if (!isCurrent(session)) return
        try {
            session.captureSubmitted.set(true)
            capture.execute { runCapture(session) }
        } catch (error: Throwable) {
            session.captureSubmitted.set(false)
            val reason = if (error is RejectedExecutionException) "voice_capture_busy" else "voice_start_failed:${error.message}"
            finish(session, "error", reason)
        }
    }

    private fun createRecorder(session: Session): AudioRecord {
        val rates = if (Build.VERSION.SDK_INT < 31 && session.route.get() == "bluetooth") intArrayOf(8_000, 16_000) else intArrayOf(48_000, 16_000, 44_100)
        for (rate in rates) {
            if (!isCurrent(session)) throw InterruptedException("cancelled")
            val minimum = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            if (minimum <= 0 || minimum > 256 * 1024) continue
            val recorder = try {
                AudioRecord.Builder().setAudioSource(MediaRecorder.AudioSource.VOICE_COMMUNICATION)
                    .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(rate).setChannelMask(AudioFormat.CHANNEL_IN_MONO).build())
                    .setBufferSizeInBytes(maxOf(minimum * 2, rate / 20 * 2)).build()
            } catch (_: IllegalArgumentException) { continue }
            if (recorder.state == AudioRecord.STATE_INITIALIZED) return recorder
            recorder.release()
        }
        throw IllegalStateException("microphone_format_unavailable")
    }

    private fun runCapture(session: Session) {
        session.worker = Thread.currentThread()
        var recorder: AudioRecord? = null
        var detector: SileroVoiceDetector? = null
        val effects = mutableListOf<AudioEffect>()
        var routeListener: AudioRouting.OnRoutingChangedListener? = null
        var pendingPcm: VoiceFrameResampler? = null
        var sampleEpoch = session.epoch.get()
        var samplesSeen = 0L
        try {
            if (!isCurrent(session)) return
            VoiceSegmentStore.prepareCache(File(activity.cacheDir, "voice-segments"))
            if (!isCurrent(session)) return
            // Only this serialized worker can ask for a route lease. Its prior
            // session has already released the recorder and posted route.close.
            val openRoute = FutureTask {
                check(isCurrent(session) && foreground && !destroyed) { "voice_session_inactive" }
                check(eventsReady()) { "voice_events_unavailable" }
                audioSession.acquireInput(session.callId, changed = { route ->
                    if (isCurrent(session)) { session.route.set(route); resetAudioEpoch(session) }
                }, interrupted = { reason -> onMain { if (isCurrent(session)) finish(session, "stopped", reason) } })
                    .also { routing -> session.routing = routing }
            }
            main.post(openRoute)
            val routing = openRoute.get()
            routing.awaitReady { !isCurrent(session) }
            val vad = try { SileroVoiceDetector(activity.assets) }
            catch (error: Throwable) { throw IllegalStateException("voice_model_unavailable", error) }
            detector = vad
            if (!isCurrent(session)) return
            val input = createRecorder(session)
            recorder = input
            routing.bindRecordingSession(input.audioSessionId)
            val aecState = attachEffects(input.audioSessionId, effects)
            val lastDevice = AtomicReference<Int?>(null)
            routeListener = AudioRouting.OnRoutingChangedListener { source ->
                if (isCurrent(session)) {
                    val device = source.routedDevice
                    if (lastDevice.getAndSet(device?.id) != device?.id) resetAudioEpoch(session)
                }
            }
            input.addOnRoutingChangedListener(routeListener, main)
            Process.setThreadPriority(Process.THREAD_PRIORITY_AUDIO)
            input.startRecording()
            check(input.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "microphone_start_failed" }
            lastDevice.set(input.routedDevice?.id)
            var epoch = session.epoch.get()
            var resampler = VoiceFrameResampler(input.sampleRate)
            pendingPcm = resampler
            sampleEpoch = epoch
            var frameOutput = voiceOutput.snapshot()
            val meter = VoiceAudioMeter()
            val samples = ShortArray(maxOf(160, input.sampleRate / 100))
            var lastData = android.os.SystemClock.elapsedRealtime()
            while (isCurrent(session) && !Thread.currentThread().isInterrupted) {
                if (epoch != session.epoch.get()) {
                    epoch = session.epoch.get()
                    samplesSeen = 0
                    vad.reset()
                    resampler = VoiceFrameResampler(input.sampleRate)
                    pendingPcm = resampler
                    sampleEpoch = epoch
                    // Drop PCM buffered under the old route before the new epoch begins.
                    input.stop()
                    input.startRecording()
                    lastData = android.os.SystemClock.elapsedRealtime()
                }
                val outputAtRead = voiceOutput.snapshot()
                val count = input.read(samples, 0, samples.size, AudioRecord.READ_NON_BLOCKING)
                check(count >= 0) { "microphone_read_failed:$count" }
                if (count == 0) {
                    check(android.os.SystemClock.elapsedRealtime() - lastData < 5_000) { "microphone_no_data" }
                    Thread.sleep(5)
                    continue
                }
                lastData = android.os.SystemClock.elapsedRealtime()
                if (session.started.compareAndSet(false, true)) onMain {
                    if (isCurrent(session)) {
                        if (eventsReady()) session.reply.resolve() else finish(session, "error", "voice_events_unavailable")
                    }
                }
                if (!resampler.hasPartialFrame) frameOutput = outputAtRead
                else if (frameOutput.playbackId != outputAtRead.playbackId || !outputAtRead.playing) {
                    frameOutput = frameOutput.copy(playing = false)
                }
                resampler.push(samples, count) { frame ->
                    if (session.epoch.get() == epoch) {
                        samplesSeen += 512
                        session.position.updateAndGet { if (it.epoch == epoch) SegmentPosition(epoch, samplesSeen) else it }
                        if (session.segments.append(epoch, samplesSeen, frame)) {
                            if (isCurrent(session)) {
                                val probability = vad.process(frame)
                                val reading = meter.measure(frame)
                                publishFrame(session, epoch, samplesSeen, probability, reading, aecState, frameOutput)
                            }
                        }
                    }
                    frameOutput = outputAtRead
                }
            }
        } catch (error: Throwable) {
            if (isCurrent(session)) onMain {
                if (isCurrent(session)) {
                    val cause = (error as? java.util.concurrent.ExecutionException)?.cause ?: error
                    Log.e("VcpVoiceAudio", "Native capture failed", cause)
                    finish(session, "error", when (cause.message) {
                        "voice_model_unavailable" -> "voice_model_unavailable"
                        "voice_events_unavailable" -> "voice_events_unavailable"
                        else -> "voice_capture_failed:${cause.message ?: cause.javaClass.simpleName}"
                    })
                }
            }
        } finally {
            recorder?.let { input ->
                routeListener?.let { listener -> runCatching { input.removeOnRoutingChangedListener(listener) } }
                runCatching { input.stop() }
            }
            if (session.epoch.get() == sampleEpoch) {
                fun retainTail(frame: FloatArray) {
                    if (frame.isEmpty()) return
                    samplesSeen += frame.size
                    session.position.updateAndGet { if (it.epoch == sampleEpoch) SegmentPosition(sampleEpoch, samplesSeen) else it }
                    session.segments.append(sampleEpoch, samplesSeen, frame)
                }
                pendingPcm?.let { converter -> retainTail(converter.finish(::retainTail)) }
            }
            effects.forEach { runCatching { it.release() } }
            recorder?.let { runCatching { it.release() } }
            detector?.let { runCatching { it.close() } }
            onMain {
                runCatching { session.routing?.close() }
                session.routing = null
                sealCapture(session)
            }
            session.worker = null
            // The executor may serve a later session after this cancellation.
            Thread.interrupted()
            runCatching { Process.setThreadPriority(Process.THREAD_PRIORITY_DEFAULT) }
        }
    }

    private fun attachEffects(audioSession: Int, effects: MutableList<AudioEffect>): String {
        val aec = try {
            if (!AcousticEchoCanceler.isAvailable()) null else AcousticEchoCanceler.create(audioSession)
        } catch (_: Exception) { return "failed" }
        val state = if (aec == null) "unavailable" else {
            effects.add(aec)
            try {
                if (!aec.enabled && aec.setEnabled(true) != AudioEffect.SUCCESS) "failed"
                else if (aec.enabled) "enabled" else "failed"
            } catch (_: Exception) { "failed" }
        }
        // A handle controls the platform effect already associated with this
        // session; there is no second software AEC/NS or automatic gain stage.
        runCatching {
            if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(audioSession)?.let { suppression ->
                effects.add(suppression)
                if (!suppression.enabled) suppression.setEnabled(true)
            }
        }
        return state
    }

    private fun publishFrame(session: Session, epoch: Long, sampleIndex: Long, probability: Double, reading: VoiceAudioMeter.Reading, aecState: String, output: NativeVoiceOutput.Snapshot) {
        if (!isCurrent(session) || session.epoch.get() != epoch) return
        val payload = JSObject().apply {
            put("kind", "frame"); put("sessionId", session.id); put("audioEpoch", epoch)
            put("sampleIndex", sampleIndex); put("probability", probability); put("level", reading.level)
            put("bands", JSArray().apply { reading.bands.forEach { put(it) } })
            put("aecState", aecState); put("route", session.route.get()); put("playbackActive", output.playing)
            put("playbackId", output.playbackId.takeIf { output.playing } ?: org.json.JSONObject.NULL)
        }
        if (!pendingFrames.offer(Frame(session, epoch, payload))) {
            if (session.eventBackpressure.compareAndSet(false, true)) onMain { finish(session, "error", "audio_event_backpressure") }
            return
        }
        scheduleFrame()
    }

    private fun scheduleFrame() {
        if (!framePosted.compareAndSet(false, true)) return
        main.post {
            repeat(8) {
                val frame = pendingFrames.poll() ?: return@repeat
                if (isCurrent(frame.session) && frame.epoch == frame.session.epoch.get() && !emitSafely(frame.payload)) {
                    finish(frame.session, "error", "voice_events_unavailable")
                }
            }
            framePosted.set(false)
            if (pendingFrames.isNotEmpty()) scheduleFrame()
        }
    }

    private fun resetAudioEpoch(session: Session) {
        val epoch = session.epoch.incrementAndGet()
        session.position.set(SegmentPosition(epoch))
        session.segments.reset(epoch)
    }

    private fun drainSegmentCleanup() {
        segmentCleanup.toList().forEach { store -> store.cleanupIfClosed(); segmentCleanup.remove(store) }
    }

    private fun closeSegments(session: Session) {
        session.segmentsReleased.set(true)
        session.segments.close()
        session.segmentReplies.toList().forEach { it.reject("cancelled:voice_segment") }
        if (!session.segments.hasFiles()) return
        segmentCleanup.add(session.segments)
        if (!cleanupPosted.compareAndSet(false, true)) return
        try {
            segmentIo.execute { cleanupPosted.set(false); drainSegmentCleanup() }
        } catch (_: RejectedExecutionException) {
            // A full queue already has file jobs; each drains closed stores in finally.
            cleanupPosted.set(false)
        }
    }

    private fun finish(session: Session, kind: String, reason: String) {
        if (!session.stopped.compareAndSet(false, true)) return
        current.compareAndSet(session, null)
        session.terminalKind = kind
        session.terminalReason = reason
        if (!session.segmentsReleased.get() && !releasedSegmentIds.contains(session.id)) retainedCaptures[session.id] = session
        rememberCancelled(session.id)
        pendingFrames.removeAll { it.session === session }
        session.worker?.interrupt()
        // Capture finally releases its recorder before posting route cleanup.
        session.reply.reject(if (kind == "stopped") "cancelled:$reason" else reason)
        if (!session.captureSubmitted.get()) sealCapture(session)
    }

    private fun segmentEnd(session: Session): JSObject = JSObject().apply {
        val position = session.position.get()
        put("audioEpoch", position.epoch); put("sampleIndex", position.sampleIndex)
    }

    private fun sealCapture(session: Session) {
        if (!session.stopped.get() || !session.sealed.compareAndSet(false, true)) return
        session.segments.seal()
        if (session.segmentsReleased.get() || releasedSegmentIds.contains(session.id) || destroyed || session.position.get().sampleIndex == 0L) {
            retainedCaptures.remove(session.id)
            closeSegments(session)
        }
        emitSafely(JSObject().apply {
            val position = session.position.get()
            put("kind", session.terminalKind); put("sessionId", session.id); put("audioEpoch", position.epoch)
            put("sampleIndex", position.sampleIndex); put("reason", session.terminalReason)
        })
        session.stopReplies.toList().forEach { it.resolve(segmentEnd(session)); session.stopReplies.remove(it) }
    }

    private fun rememberCancelled(id: String) {
        cancelledBeforeStart.add(id)
        if (cancelledBeforeStart.size > 32) cancelledBeforeStart.remove(cancelledBeforeStart.first())
    }

    private fun emitSafely(payload: JSObject): Boolean = runCatching { emitVoiceEvent(payload) }
        .onFailure { Log.w("VcpVoiceAudio", "Voice event delivery failed", it) }.getOrDefault(false)
}
