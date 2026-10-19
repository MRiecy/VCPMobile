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

@InvokeArg
class StartVoiceAudioArgs { var sessionId: String = "" }
@InvokeArg
class StopVoiceAudioArgs { var sessionId: String = "" }
@InvokeArg
class SetVoiceProbeArgs { var sessionId: String = ""; var enabled: Boolean = false; var probeRevision: Long = 0 }

/** Owns exactly one microphone. JavaScript visibility and permission dialogs never own audio. */
internal class NativeVoiceAudio(
    private val activity: Activity,
    private val requestPermission: (String, Invoke, String) -> Unit,
    private val eventsReady: () -> Boolean,
    private val emitVoiceEvent: (JSObject) -> Boolean,
) {
    private class Reply(private val invoke: Invoke) {
        private val settled = AtomicBoolean(false)
        fun resolve() { if (settled.compareAndSet(false, true)) invoke.deliverResult() }
        fun reject(reason: String) { if (settled.compareAndSet(false, true)) invoke.deliverError(reason) }
    }
    private data class ProbeState(val revision: Long = 0, val playing: Boolean = false)
    private class Session(val id: String, val invoke: Invoke) {
        val reply = Reply(invoke)
        val stopped = AtomicBoolean(false)
        val started = AtomicBoolean(false)
        val eventBackpressure = AtomicBoolean(false)
        val epoch = AtomicLong(1)
        val route = AtomicReference("unknown")
        val probeState = AtomicReference(ProbeState())
        val probe = AtomicReference<VoiceProbePlayback?>(null)
        val probeReply = AtomicReference<Reply?>(null)
        @Volatile var worker: Thread? = null
        var routing: VoiceAudioRouting? = null // Only constructed/released on main.
    }
    private data class Frame(val session: Session, val epoch: Long, val payload: JSObject)

    private val main = Handler(Looper.getMainLooper())
    private val current = AtomicReference<Session?>(null)
    private val pendingFrames = ArrayBlockingQueue<Frame>(8)
    private val framePosted = AtomicBoolean(false)
    private val capture = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(1),
        { runnable -> Thread(runnable, "vcp-voice-capture").apply { isDaemon = true } }, ThreadPoolExecutor.AbortPolicy())
    private var destroyed = false
    private var foreground = true
    private var permissionWaiter: Session? = null
    private val cancelledBeforeStart = LinkedHashSet<String>()

    private fun onMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) action() else main.post(action)
    }
    private fun isCurrent(session: Session) = current.get() === session && !session.stopped.get() && !session.eventBackpressure.get()
    private fun validId(id: String) = id.matches(Regex("[A-Za-z0-9_-]{1,128}"))

    fun startVoiceAudio(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(StartVoiceAudioArgs::class.java) }
        catch (error: Exception) { invoke.deliverError("invalid_voice_request:${error.message}"); return@onMain }
        if (!validId(args.sessionId)) { invoke.deliverError("invalid_voice_session"); return@onMain }
        if (destroyed || !foreground || cancelledBeforeStart.contains(args.sessionId)) {
            invoke.deliverError("cancelled:voice_session_inactive"); return@onMain
        }
        if (!eventsReady()) { invoke.deliverError("voice_events_unavailable"); return@onMain }
        if (current.get() != null || permissionWaiter != null) { invoke.deliverError("voice_session_busy"); return@onMain }
        val session = Session(args.sessionId, invoke)
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
        current.get()?.takeIf { it.id == args.sessionId }?.let { finish(it, "stopped", "cancelled") }
        invoke.deliverResult()
    }

    fun setVoiceProbe(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(SetVoiceProbeArgs::class.java) }
        catch (error: Exception) { invoke.deliverError("invalid_voice_request:${error.message}"); return@onMain }
        val session = current.get()?.takeIf { it.id == args.sessionId && !it.stopped.get() }
        if (session == null) { invoke.deliverError("voice_session_inactive"); return@onMain }
        if (args.probeRevision <= 0) { invoke.deliverError("invalid_probe_revision"); return@onMain }
        val previousRevision = session.probeState.get().revision
        if (args.probeRevision < previousRevision) { invoke.deliverError("cancelled:stale_probe_revision"); return@onMain }
        if (args.probeRevision == previousRevision) { invoke.deliverResult(); return@onMain }
        stopProbe(session)
        session.probeState.set(ProbeState(args.probeRevision))
        if (!args.enabled) {
            invoke.deliverResult()
            return@onMain
        }
        if (!session.started.get()) { invoke.deliverError("microphone_not_ready"); return@onMain }
        val reply = Reply(invoke)
        session.probeReply.set(reply)
        lateinit var player: VoiceProbePlayback
        player = VoiceProbePlayback(activity.assets, began = {
            onMain {
                if (isCurrent(session) && session.probe.get() === player && session.probeState.get().revision == args.probeRevision) {
                    session.probeState.set(ProbeState(args.probeRevision, true))
                    reply.resolve()
                } else reply.reject("cancelled:voice_probe")
            }
        }, ended = { error ->
            onMain {
                if (session.probe.compareAndSet(player, null)) {
                    session.probeState.updateAndGet { it.copy(playing = false) }
                    session.probeReply.compareAndSet(reply, null)
                    if (error != null) {
                        reply.reject("voice_probe_failed:${error.message}")
                        if (isCurrent(session)) finish(session, "error", "voice_probe_failed:${error.message}")
                    } else reply.reject("cancelled:voice_probe")
                }
            }
        })
        session.probe.set(player)
        player.start()
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
        permissionWaiter?.reply?.reject("cancelled:plugin_destroyed")
        permissionWaiter = null
        capture.shutdownNow()
    }

    private fun beginCapture(session: Session) {
        if (!isCurrent(session)) return
        try {
            capture.execute { runCapture(session) }
        } catch (error: Throwable) {
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
        try {
            if (!isCurrent(session)) return
            // Only this serialized worker can ask for a route lease. Its prior
            // session has already released the recorder and posted route.close.
            val openRoute = FutureTask {
                check(isCurrent(session) && foreground && !destroyed) { "voice_session_inactive" }
                check(eventsReady()) { "voice_events_unavailable" }
                VoiceAudioRouting(activity, changed = { route ->
                    if (isCurrent(session)) { session.route.set(route); session.epoch.incrementAndGet() }
                }, interrupted = { reason -> onMain { if (isCurrent(session)) finish(session, "stopped", reason) } })
                    .also { routing -> session.routing = routing; routing.open() }
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
                    if (lastDevice.getAndSet(device?.id) != device?.id) session.epoch.incrementAndGet()
                }
            }
            input.addOnRoutingChangedListener(routeListener, main)
            Process.setThreadPriority(Process.THREAD_PRIORITY_AUDIO)
            input.startRecording()
            check(input.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "microphone_start_failed" }
            lastDevice.set(input.routedDevice?.id)
            var epoch = session.epoch.get()
            var samplesSeen = 0L
            var resampler = VoiceFrameResampler(input.sampleRate)
            var frameProbe = session.probeState.get()
            val meter = VoiceAudioMeter()
            val samples = ShortArray(maxOf(160, input.sampleRate / 100))
            var lastData = android.os.SystemClock.elapsedRealtime()
            while (isCurrent(session) && !Thread.currentThread().isInterrupted) {
                if (epoch != session.epoch.get()) {
                    epoch = session.epoch.get()
                    samplesSeen = 0
                    vad.reset()
                    resampler = VoiceFrameResampler(input.sampleRate)
                    // Drop PCM buffered under the old route before the new epoch begins.
                    input.stop()
                    input.startRecording()
                    lastData = android.os.SystemClock.elapsedRealtime()
                }
                val probeAtRead = session.probeState.get()
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
                if (!resampler.hasPartialFrame) frameProbe = probeAtRead
                else if (frameProbe.revision != probeAtRead.revision || !probeAtRead.playing) {
                    frameProbe = frameProbe.copy(playing = false)
                }
                resampler.push(samples, count) { frame ->
                    if (isCurrent(session) && session.epoch.get() == epoch) {
                        val probability = vad.process(frame)
                        val reading = meter.measure(frame)
                        samplesSeen += 512
                        publishFrame(session, epoch, samplesSeen, probability, reading, aecState, frameProbe.revision, frameProbe.playing)
                    }
                    frameProbe = probeAtRead
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
            effects.forEach { runCatching { it.release() } }
            recorder?.let { runCatching { it.release() } }
            detector?.let { runCatching { it.close() } }
            onMain {
                runCatching { session.routing?.close() }
                session.routing = null
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

    private fun publishFrame(session: Session, epoch: Long, sampleIndex: Long, probability: Double, reading: VoiceAudioMeter.Reading, aecState: String, probeRevision: Long, playbackActive: Boolean) {
        if (!isCurrent(session) || session.epoch.get() != epoch) return
        val payload = JSObject().apply {
            put("kind", "frame"); put("sessionId", session.id); put("audioEpoch", epoch)
            put("sampleIndex", sampleIndex); put("probability", probability); put("level", reading.level)
            put("bands", JSArray().apply { reading.bands.forEach { put(it) } })
            put("aecState", aecState); put("route", session.route.get()); put("playbackActive", playbackActive)
            put("probeRevision", probeRevision)
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

    private fun stopProbe(session: Session) {
        session.probeState.updateAndGet { it.copy(playing = false) }
        session.probe.getAndSet(null)?.stop()
        session.probeReply.getAndSet(null)?.reject("cancelled:voice_probe")
    }

    private fun finish(session: Session, kind: String, reason: String) {
        if (!session.stopped.compareAndSet(false, true)) return
        current.compareAndSet(session, null)
        rememberCancelled(session.id)
        pendingFrames.removeAll { it.session === session }
        stopProbe(session)
        session.worker?.interrupt()
        // Capture finally releases its recorder before posting route cleanup.
        session.reply.reject(if (kind == "stopped") "cancelled:$reason" else reason)
        emitSafely(JSObject().apply {
            put("kind", kind); put("sessionId", session.id); put("audioEpoch", session.epoch.get()); put("reason", reason)
        })
    }

    private fun rememberCancelled(id: String) {
        cancelledBeforeStart.add(id)
        if (cancelledBeforeStart.size > 32) cancelledBeforeStart.remove(cancelledBeforeStart.first())
    }

    private fun emitSafely(payload: JSObject): Boolean = runCatching { emitVoiceEvent(payload) }
        .onFailure { Log.w("VcpVoiceAudio", "Voice event delivery failed", it) }.getOrDefault(false)
}
