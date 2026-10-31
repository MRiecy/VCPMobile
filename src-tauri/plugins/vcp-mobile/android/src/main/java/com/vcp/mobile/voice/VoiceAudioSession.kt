package com.vcp.mobile.voice

import android.app.Activity
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

internal data class VoiceOutputProfile(val generation: Long, val usage: Int, val legacySco: Boolean,
    val headphones: AudioDeviceInfo? = null)

/** Audio routing survives microphone mute. Only call boundaries change communication/media mode. */
internal class VoiceAudioSession(private val activity: Activity) {
    private data class Route(val generation: Long, val forCall: Boolean, val headphones: AudioDeviceInfo?, val routing: VoiceAudioRouting,
        val name: AtomicReference<String> = AtomicReference("unknown"))
    private val main = Handler(Looper.getMainLooper())
    private val route = AtomicReference<Route?>(null)
    private var physicalRoute: Route? = null
    private val inputs = LinkedHashSet<InputLease>()
    private val outputs = LinkedHashSet<OutputLease>()
    private val endedCalls = LinkedHashSet<String>()
    private var call: String? = null
    private var callAudioActivated = false
    private var generation = 0L
    private var transitioning = false
    private var suspended = false
    private var destroyed = false
    private var outputBarrier: ((() -> Unit) -> Unit) = { ready -> ready() }

    fun setOutputBarrier(barrier: (() -> Unit) -> Unit) { outputBarrier = barrier }
    fun currentCallId(): String? = call
    fun hasCall(id: String): Boolean = !destroyed && call == id

    fun beginCall(id: String) {
        check(!destroyed && !endedCalls.contains(id)) { "cancelled:voice_call" }
        check(call == null || call == id) { "voice_call_busy" }
        if (call == id) return
        call = id
        callAudioActivated = outputs.isNotEmpty()
        suspended = false
        reconcile()
        if (suspended) { call = null; throw IllegalStateException("audio_route_unavailable") }
    }

    fun endCall(id: String) {
        endedCalls.add(id)
        if (endedCalls.size > 32) endedCalls.remove(endedCalls.first())
        if (call != id) return
        call = null
        callAudioActivated = false
        reconcile()
    }

    inner class InputLease internal constructor(val callId: String, internal val changed: (String) -> Unit,
        internal val interrupted: (String) -> Unit) : AutoCloseable {
        private val closed = AtomicBoolean(false)
        fun awaitReady(stopped: () -> Boolean): VoiceOutputProfile = awaitProfile { closed.get() || stopped() }
        fun bindRecordingSession(id: Int) { route.get()?.routing?.bindRecordingSession(id) }
        override fun close() {
            if (!closed.compareAndSet(false, true)) return
            onMain { inputs.remove(this); reconcile() }
        }
    }

    inner class OutputLease internal constructor(val callId: String?, internal val interrupted: (String) -> Unit) : AutoCloseable {
        private val closed = AtomicBoolean(false)
        fun awaitReady(stopped: () -> Boolean): VoiceOutputProfile = awaitProfile { closed.get() || stopped() }
        fun isCurrent(profile: VoiceOutputProfile): Boolean = !closed.get() && route.get()?.generation == profile.generation
        fun routeChanged(profile: VoiceOutputProfile) = onMain {
            if (isCurrent(profile)) interrupt("audio_route_changed")
        }
        override fun close() {
            if (!closed.compareAndSet(false, true)) return
            onMain { outputs.remove(this); reconcile() }
        }
    }

    fun acquireInput(callId: String, changed: (String) -> Unit, interrupted: (String) -> Unit): InputLease {
        check(hasCall(callId)) { "cancelled:voice_call" }
        val lease = InputLease(callId, changed, interrupted)
        inputs.add(lease)
        callAudioActivated = true
        suspended = false
        reconcile()
        if (suspended) { inputs.remove(lease); throw IllegalStateException("audio_route_unavailable") }
        route.get()?.name?.get()?.let(changed)
        return lease
    }

    fun acquireOutput(interrupted: (String) -> Unit): OutputLease {
        check(!destroyed) { "cancelled:voice_output" }
        val lease = OutputLease(call, interrupted)
        outputs.add(lease)
        if (call != null) callAudioActivated = true
        suspended = false
        reconcile()
        if (suspended) { outputs.remove(lease); throw IllegalStateException("audio_route_unavailable") }
        return lease
    }

    private fun awaitProfile(stopped: () -> Boolean): VoiceOutputProfile {
        while (!stopped()) {
            val selected = route.get()
            if (selected == null) { Thread.sleep(10); continue }
            try { selected.routing.awaitReady { stopped() || route.get() !== selected } }
            catch (error: InterruptedException) {
                if (stopped()) throw error
                if (route.get() !== selected) continue
                throw error
            }
            if (route.get() === selected && !stopped()) {
                val communication = selected.forCall && selected.headphones == null
                return VoiceOutputProfile(selected.generation,
                    if (communication) AudioAttributes.USAGE_VOICE_COMMUNICATION else AudioAttributes.USAGE_MEDIA,
                    communication && Build.VERSION.SDK_INT < 31 && selected.name.get() == "bluetooth", selected.headphones)
            }
        }
        throw InterruptedException("cancelled:voice_audio_route")
    }

    private fun reconcile() {
        if (destroyed || suspended) return
        val needed = (call != null && callAudioActivated) || inputs.isNotEmpty() || outputs.isNotEmpty()
        val forCall = call != null || inputs.isNotEmpty()
        val existing = physicalRoute
        if (needed && !transitioning && existing?.forCall == forCall) return
        val request = ++generation
        if (!needed) {
            transitioning = false
            route.set(null)
            physicalRoute?.routing?.close(); physicalRoute = null
            return
        }
        transitioning = true
        route.set(null)
        outputBarrier {
            onMain {
                if (request != generation || destroyed || suspended) return@onMain
                physicalRoute?.routing?.close()
                physicalRoute = null
                try {
                    val headphones = if (forCall) VoiceAudioRouting.mediaHeadphones(activity) else null
                    lateinit var next: Route
                    val hardware = VoiceAudioRouting(activity, changed = { name ->
                        if (route.get() === next) {
                            next.name.set(name)
                            inputs.toList().forEach { it.changed(name) }
                        }
                    }, interrupted = ::interrupt, forCall = forCall, headphones = headphones)
                    next = Route(request, forCall, headphones, hardware)
                    physicalRoute = next
                    route.set(next)
                    hardware.open()
                    transitioning = false
                } catch (error: Throwable) {
                    interrupt("audio_route_unavailable")
                }
            }
        }
    }

    private fun interrupt(reason: String) = onMain {
        suspended = true
        transitioning = false
        generation += 1
        route.set(null)
        physicalRoute?.routing?.close(); physicalRoute = null
        inputs.toList().forEach { it.interrupted(reason) }
        outputs.toList().forEach { it.interrupted(reason) }
    }

    fun destroy() = onMain {
        if (destroyed) return@onMain
        destroyed = true
        generation += 1
        route.set(null)
        physicalRoute?.routing?.close(); physicalRoute = null
        inputs.toList().forEach { it.interrupted("destroyed") }
        outputs.toList().forEach { it.interrupted("destroyed") }
        inputs.clear(); outputs.clear(); call = null
    }

    private fun onMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) action() else main.post(action)
    }
}
