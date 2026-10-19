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
class AppendVoicePlaybackArgs { var playbackId: String = ""; var pcmBase64: String = "" }

/** Plugin-scoped output; it never requests microphone permission or reads capture state. */
internal class NativeVoiceOutput(
    private val audioSession: VoiceAudioSession,
    private val probeBusy: () -> Boolean,
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
    private val cancelled = LinkedHashSet<String>()
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
    fun isBusy(): Boolean = current.get() != null

    fun startVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        if (!validId(args.playbackId)) { invoke.deliverError("invalid_voice_playback_id"); return@onMain }
        if (destroyed || cancelled.contains(args.playbackId)) { invoke.deliverError("cancelled:voice_playback"); return@onMain }
        if (probeBusy()) { invoke.deliverError("voice_probe_busy"); return@onMain }
        if (current.get()?.id == args.playbackId) { invoke.deliverError("voice_playback_already_started"); return@onMain }
        if (current.get() != null) { invoke.deliverError("voice_playback_busy"); return@onMain }
        val reply = Reply(invoke)
        val lease = try { audioSession.acquireOutput { reason -> onMain { current.get()?.takeIf { it.id == args.playbackId }?.let { stop(it, reason) } } } }
        catch (_: Throwable) { reply.reject("voice_playback_route_unavailable"); return@onMain }
        val output = Output(args.playbackId, lease)
        output.player = VoiceStreamPlayback(args.playbackId, lease, worker, playingChanged = { playing -> onMain {
            if (current.get() === output) state.set(Snapshot(output.id.takeIf { playing }, playing))
        } }, ended = { error -> onMain {
            if (current.get() === output) {
                state.set(Snapshot())
                if (error == null) { current.compareAndSet(output, null); remember(output.id) }
                else Log.w("VcpVoiceOutput", error)
            }
        } })
        current.set(output)
        try { output.player.start { error -> complete(output, reply, error) } }
        catch (_: Throwable) { stop(output); reply.reject("voice_playback_start_failed") }
    }

    fun appendVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(AppendVoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        val output = current.get()?.takeIf { it.id == args.playbackId }
        if (output == null) { invoke.deliverError("cancelled:voice_playback"); return@onMain }
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
        if (output == null) { invoke.deliverError("cancelled:voice_playback"); return@onMain }
        val reply = Reply(invoke)
        output.player.finish { error -> complete(output, reply, error, finished = true) }
    }

    fun stopVoicePlayback(invoke: Invoke) = onMain {
        val args = try { invoke.parseArgs(VoicePlaybackArgs::class.java) }
        catch (_: Exception) { invoke.deliverError("invalid_voice_playback_request"); return@onMain }
        if (!validId(args.playbackId)) { invoke.deliverError("invalid_voice_playback_id"); return@onMain }
        remember(args.playbackId)
        current.get()?.takeIf { it.id == args.playbackId }?.let { stop(it) }
        invoke.deliverResult()
    }

    fun endCall(callId: String) = onMain { current.get()?.takeIf { it.lease.callId == callId }?.let { stop(it) } }

    fun destroy() = onMain {
        destroyed = true
        current.get()?.let { stop(it) }
        worker.shutdownNow()
    }

    private fun complete(output: Output, reply: Reply, error: String?, finished: Boolean = false) = onMain {
        if (error != null) reply.reject(error)
        else if (current.get() !== output || destroyed) reply.reject("cancelled:voice_playback")
        else {
            if (finished) { state.set(Snapshot()); current.compareAndSet(output, null); remember(output.id) }
            reply.resolve()
        }
    }

    private fun stop(output: Output, reason: String = "cancelled:voice_playback") {
        if (!current.compareAndSet(output, null)) return
        state.set(Snapshot())
        remember(output.id)
        output.player.stop(reason)
    }

    private fun remember(id: String) { cancelled.add(id); if (cancelled.size > 64) cancelled.remove(cancelled.first()) }
    private fun validId(id: String) = id.matches(Regex("[A-Za-z0-9_-]{1,128}"))
    private fun onMain(action: () -> Unit) { if (Looper.myLooper() == Looper.getMainLooper()) action() else main.post(action) }
}
