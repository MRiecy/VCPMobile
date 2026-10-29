package com.vcp.mobile.voice

import android.os.Handler
import android.os.Looper
import android.util.Base64
import android.util.Log
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import com.vcp.mobile.runtime.deliverError
import com.vcp.mobile.runtime.deliverResult
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

@InvokeArg
class VoicePlaybackArgs { var playbackId: String = "" }
@InvokeArg
class StartVoicePlaybackArgs { var playbackId: String = ""; var startupFrames: Int = 0 }
@InvokeArg
class AppendVoicePlaybackArgs { var playbackId: String = ""; var pcmBase64: String = "" }

/** Plugin-scoped output; it never requests microphone permission or reads capture state. */
internal class NativeVoiceOutput(
    private val audioSession: VoiceAudioSession,
    private val playbackStarted: (String) -> Unit,
) {
    data class Snapshot(val playbackId: String? = null, val playing: Boolean = false)
    private class Reply(private val invoke: Invoke) {
        private val settled = AtomicBoolean(false)
        fun resolve() { if (settled.compareAndSet(false, true)) invoke.deliverResult() }
        fun reject(reason: String) { if (settled.compareAndSet(false, true)) invoke.deliverError(reason) }
    }
    private class Output(val id: String, val lease: VoiceAudioSession.OutputLease) {
        lateinit var player: VoiceStreamPlayback
    }
    private val main = Handler(Looper.getMainLooper())
    private val current = AtomicReference<Output?>(null)
    private val state = AtomicReference(Snapshot())
    private val terminalReasons = LinkedHashMap<String, String>()
    private val worker = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(1),
        { task -> Thread(task, "vcp-voice-playback").apply { isDaemon = true } }, ThreadPoolExecutor.AbortPolicy())
    private var destroyed = false

    init {
        audioSession.setOutputBarrier { ready ->
            val output = current.get()
            if (output == null) ready() else output.player.quiesceForRoute(ready)
        }
    }

    fun snapshot(): Snapshot = state.get()

    fun startVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(StartVoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        if (!validId(args.playbackId)) { invoke.deliverError("invalid_voice_playback_id"); return@onMain }
        if (args.startupFrames !in 1..5_760) { invoke.deliverError("invalid_voice_playback_buffer"); return@onMain }
        if (destroyed) { invoke.deliverError("cancelled:voice_playback"); return@onMain }
        terminalReasons[args.playbackId]?.let { invoke.deliverError(it); return@onMain }
        if (current.get()?.id == args.playbackId) { invoke.deliverError("voice_playback_already_started"); return@onMain }
        if (current.get() != null) { invoke.deliverError("voice_playback_busy"); return@onMain }
        val reply = Reply(invoke)
        val lease = try { audioSession.acquireOutput { reason -> onMain { current.get()?.takeIf { it.id == args.playbackId }?.let { stop(it, reason, immediate = true) } } } }
        catch (_: Throwable) { reply.reject("voice_playback_route_unavailable"); return@onMain }
        val output = Output(args.playbackId, lease)
        output.player = VoiceStreamPlayback(args.playbackId, args.startupFrames, lease, worker, started = { onMain {
            if (current.get() === output && !destroyed) playbackStarted(output.id)
        } }, playingChanged = { playing -> onMain {
            if (current.get() === output) state.set(Snapshot(output.id.takeIf { playing }, playing))
        } }, ended = { error -> onMain {
            if (current.compareAndSet(output, null)) {
                state.set(Snapshot())
                remember(output.id, error ?: "cancelled:voice_playback")
                if (error != null) Log.w("VcpVoiceOutput", error)
            }
        } })
        current.set(output)
        try { output.player.start { error -> complete(output, reply, error) } }
        catch (_: Throwable) { stop(output); reply.reject("voice_playback_start_failed") }
    }

    fun endVoicePlaybackSegment(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        val output = current.get()?.takeIf { it.id == args.playbackId }
        if (output == null) { invoke.deliverError(terminalReason(args.playbackId)); return@onMain }
        val reply = Reply(invoke)
        output.player.endSegment { error -> complete(output, reply, error) }
    }

    fun appendVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(AppendVoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        val output = current.get()?.takeIf { it.id == args.playbackId }
        if (output == null) { invoke.deliverError(terminalReason(args.playbackId)); return@onMain }
        val samples = try {
            require(args.pcmBase64.isNotEmpty() && args.pcmBase64.length <= 43_692)
            val bytes = Base64.decode(args.pcmBase64, Base64.DEFAULT)
            require(bytes.isNotEmpty() && bytes.size <= 32_768 && bytes.size % 2 == 0)
            ShortArray(bytes.size / 2).also { ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().get(it) }
        } catch (_: Exception) { invoke.deliverError("invalid_voice_playback_chunk"); return@onMain }
        val reply = Reply(invoke)
        output.player.append(samples) { error -> complete(output, reply, error) }
    }

    fun finishVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        val output = current.get()?.takeIf { it.id == args.playbackId }
        if (output == null) { invoke.deliverError(terminalReason(args.playbackId)); return@onMain }
        val reply = Reply(invoke)
        output.player.finish { error -> complete(output, reply, error, finished = true) }
    }

    fun stopVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        if (!validId(args.playbackId)) { invoke.deliverError("invalid_voice_playback_id"); return@onMain }
        remember(args.playbackId)
        val output = current.get()?.takeIf { it.id == args.playbackId }
        if (output == null) invoke.deliverResult()
        else stop(output) { invoke.deliverResult() }
    }

    fun endCall(callId: String, released: () -> Unit) = onMain {
        val output = current.get()?.takeIf { it.lease.callId == callId }
        if (output == null) released() else stop(output, released = released)
    }

    fun destroy() = onMain {
        destroyed = true
        current.get()?.let { stop(it, immediate = true) }
        // A queued start must still run its terminal cleanup and settle its stop waiters.
        worker.shutdown()
    }

    private fun complete(output: Output, reply: Reply, error: String?, finished: Boolean = false) = onMain {
        if (error != null) reply.reject(error)
        else if (current.get() !== output || destroyed) reply.reject("cancelled:voice_playback")
        else {
            if (finished) { state.set(Snapshot()); current.compareAndSet(output, null); remember(output.id) }
            reply.resolve()
        }
    }

    private fun stop(output: Output, reason: String = "cancelled:voice_playback", immediate: Boolean = false,
        released: (() -> Unit)? = null) {
        if (current.get() !== output) { released?.invoke(); return }
        remember(output.id, reason)
        if (released == null) output.player.stop(reason, immediate)
        else output.player.stop(reason, immediate) { onMain {
            if (current.compareAndSet(output, null)) state.set(Snapshot())
            released()
        } }
    }

    private fun terminalReason(id: String) = terminalReasons[id] ?: "cancelled:voice_playback"
    private fun remember(id: String, reason: String = "cancelled:voice_playback") {
        terminalReasons.putIfAbsent(id, reason)
        if (terminalReasons.size > 64) terminalReasons.remove(terminalReasons.keys.first())
    }
    private fun validId(id: String) = id.matches(Regex("[A-Za-z0-9_-]{1,128}"))
    private fun onMain(action: () -> Unit) { if (Looper.myLooper() == Looper.getMainLooper()) action() else main.post(action) }
}
