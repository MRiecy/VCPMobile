package com.vcp.mobile.voice

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRouting
import android.media.AudioTrack
import android.media.VolumeShaper
import android.os.Build
import android.os.SystemClock
import android.util.Log
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/** A bounded PCM sink. Every request remains cancellable until its own acknowledgement. */
internal class VoiceStreamPlayback(
    val id: String,
    private val startupFrames: Int,
    private val lease: VoiceAudioSession.OutputLease,
    private val executor: Executor,
    private val started: () -> Unit,
    private val messageChanged: (String?) -> Unit,
    private val playingChanged: (Boolean) -> Unit,
    private val ended: (String?) -> Unit,
) {
    private class Completion(private val callback: (String?) -> Unit, private val remove: (Completion) -> Unit) {
        private val settled = AtomicBoolean(false)
        fun complete(error: String? = null) {
            if (settled.compareAndSet(false, true)) { remove(this); callback(error) }
        }
    }
    private val requests = ConcurrentHashMap.newKeySet<Completion>()
    private val queue = VoicePlaybackQueue(startupFrames = startupFrames)
    private val stopped = AtomicBoolean(false)
    private val completed = AtomicBoolean(false)
    private val immediateStop = AtomicBoolean(false)
    private val terminalLock = Any()
    private val stopWaiters = mutableListOf<() -> Unit>()
    private val failure = AtomicReference<String?>(null)
    private val track = AtomicReference<AudioTrack?>(null)
    private val finishRequest = AtomicReference<Completion?>(null)
    private val routePause = AtomicBoolean(false)
    private val quiesce = AtomicReference<(() -> Unit)?>(null)
    @Volatile private var worker: Thread? = null
    private var reportedStart = false
    private var renderedMessage: String? = null
    private var reportedMessage: String? = null
    private val createdAt = SystemClock.elapsedRealtime()
    private val firstPcmAt = AtomicLong(-1)

    private fun request(callback: (String?) -> Unit): Completion {
        val completion = Completion(callback) { requests.remove(it) }
        requests.add(completion)
        if (stopped.get() || completed.get()) completion.complete(failure.get() ?: "cancelled:voice_playback")
        return completion
    }

    fun start(callback: (String?) -> Unit) {
        val ready = request(callback)
        try {
            executor.execute { worker = Thread.currentThread(); runPlayback(ready) }
        } catch (_: RejectedExecutionException) {
            failure.set("voice_playback_busy")
            ready.complete("voice_playback_busy")
            terminate("voice_playback_busy", null, drained = false)
        }
    }

    fun append(samples: ShortArray, messageId: String? = null, callback: (String?) -> Unit) {
        val completion = request(callback)
        if (stopped.get() || completed.get()) return
        if (samples.isNotEmpty() && firstPcmAt.compareAndSet(-1, SystemClock.elapsedRealtime())) {
            Log.d("VcpVoiceOutput", "first_pcm id=$id elapsed_ms=${firstPcmAt.get() - createdAt} frames=${samples.size}")
        }
        queue.append(samples, messageId, completion::complete)
    }

    fun finish(callback: (String?) -> Unit) {
        val completion = request(callback)
        if (stopped.get() || completed.get()) return
        if (!finishRequest.compareAndSet(null, completion)) {
            completion.complete("voice_playback_already_finishing")
            return
        }
        try { queue.seal() }
        catch (error: Exception) { completion.complete(error.message ?: "voice_playback_failed") }
    }

    fun endSegment(callback: (String?) -> Unit) {
        val completion = request(callback)
        if (stopped.get() || completed.get()) return
        try { queue.endSegment(); completion.complete() }
        catch (error: Exception) { completion.complete(error.message ?: "voice_playback_failed") }
    }

    fun stop(reason: String = "cancelled:voice_playback", immediate: Boolean = false, released: (() -> Unit)? = null) {
        var firstStop = false
        val alreadyReleased = synchronized(terminalLock) {
            if (completed.get()) true else {
                released?.let { stopWaiters.add(it) }
                failure.compareAndSet(null, reason)
                if (immediate) immediateStop.set(true)
                firstStop = stopped.compareAndSet(false, true)
                // Serialize this wakeup with fade entry so it cannot interrupt the new fade.
                if (firstStop || immediate) worker?.interrupt()
                false
            }
        }
        if (alreadyReleased) { released?.invoke(); return }
        if (firstStop) {
            queue.cancel(reason)
            requests.toList().forEach { it.complete(reason) }
        }
    }

    fun quiesceForRoute(ready: () -> Unit) {
        val alreadyQuiet = synchronized(terminalLock) {
            if (completed.get() || (!stopped.get() && (routePause.get() || track.get() == null))) true
            else { quiesce.set(ready); false }
        }
        if (alreadyQuiet) ready()
    }

    private fun runPlayback(ready: Completion) {
        var terminalError: String? = null
        var sink: Sink? = null
        var converter: VoiceLegacyScoResampler? = null
        var gateReleased = false
        var drained = false
        try {
            if (stopped.get()) return
            fun ensureOutput() {
                if (sink != null && quiesce.get() == null && lease.isCurrent(sink!!.profile)) return
                routePause.set(true)
                sink?.let { old ->
                    converter?.let { old.write(it.finish()) }
                    old.finishPriming()
                    old.drain()
                    if (stopped.get()) throw InterruptedException("cancelled:voice_playback")
                    old.close()
                    sink = null
                }
                converter = null
                quiesce.getAndSet(null)?.invoke()
                while (!stopped.get()) {
                    val profile = lease.awaitReady { stopped.get() }
                    val candidate = Sink(profile)
                    if (!lease.isCurrent(profile)) { candidate.close(); continue }
                    sink = candidate
                    converter = if (profile.legacySco) VoiceLegacyScoResampler() else null
                    routePause.set(false)
                    ready.complete()
                    return
                }
                throw InterruptedException("cancelled:voice_playback")
            }
            ensureOutput()
            while (!stopped.get()) {
                ensureOutput()
                val next = queue.take(20)
                if (next.samples != null) {
                    if (!gateReleased) {
                        gateReleased = true
                        Log.d("VcpVoiceOutput", "gate id=$id elapsed_ms=${SystemClock.elapsedRealtime() - createdAt} frames=${next.samples.size}")
                    }
                    var position = 0
                    while (!stopped.get() && position < next.samples.size) {
                        ensureOutput()
                        if (position == 0 && next.messageId != null) {
                            sink!!.markMessage(next.messageId, converter?.pendingOutputFrames ?: 0L)
                        }
                        sink!!.preparePriming(next.samples.size - position)
                        val count = minOf(480, next.samples.size - position)
                        val converted = converter?.process(next.samples, position, count)
                        if (converted == null) sink!!.write(next.samples, position, count)
                        else sink!!.write(converted)
                        position += count
                    }
                } else {
                    sink?.idle()
                }
                if (next.segmentEnd && sink?.hasStarted == false && !queue.hasBufferedAudio()) {
                    converter?.let { sink?.write(it.finish()); converter = VoiceLegacyScoResampler() }
                    sink?.finishPriming()
                }
                if (next.finished) break
            }
            if (!stopped.get()) {
                converter?.let { sink?.write(it.finish()) }
                sink?.finishPriming()
                sink?.drain()
                drained = !stopped.get()
            }
        } catch (error: Throwable) {
            if (!stopped.get()) terminalError = "voice_playback_failed:${error.message ?: error.javaClass.simpleName}"
        } finally {
            terminate(terminalError, sink, drained)
        }
    }

    private fun terminate(terminalError: String?, sink: Sink?, drained: Boolean) {
        // The interrupt only wakes normal writes/route waits. The worker owns the remaining fade.
        synchronized(terminalLock) { Thread.interrupted() }
        if (stopped.get() && !immediateStop.get() && terminalError == null) {
            runCatching { sink?.fadeOut() }
                .onFailure { Log.w("VcpVoiceOutput", "fade_failed id=$id", it) }
        }
        sink?.close()
        // Closing a shaper removes its gain; never do that while its track can still sound.
        sink?.closeFade()
        routePause.set(true)
        lease.close()
        playingChanged(false)
        reportMessage(null)
        val reason = terminalError ?: failure.get() ?: "cancelled:voice_playback"
        failure.compareAndSet(null, reason)
        if (drained && !stopped.get()) finishRequest.get()?.complete()
        queue.cancel(reason)
        requests.toList().forEach { it.complete(reason) }
        val (barrier, waiters) = synchronized(terminalLock) {
            worker = null
            completed.set(true)
            quiesce.getAndSet(null) to stopWaiters.toList().also { stopWaiters.clear() }
        }
        barrier?.invoke()
        ended(terminalError)
        waiters.forEach { it() }
        Thread.interrupted()
    }

    private fun reportMessage(message: String?) {
        if (reportedMessage != message) { reportedMessage = message; messageChanged(message) }
    }

    private inner class Sink(val profile: VoiceOutputProfile) : AutoCloseable {
        private val rate = if (profile.legacySco) 16_000 else 24_000
        private val player: AudioTrack
        private val routeListener = AudioRouting.OnRoutingChangedListener { source ->
            if (track.get() === source && (source as AudioTrack).playState == AudioTrack.PLAYSTATE_PLAYING) {
                val actual = source.routedDevice
                if (actual != null && !VoiceAudioRouting.sameMediaRoute(profile.headphones, actual)) lease.routeChanged(profile)
            }
        }
        private var written = 0L
        private val messageStarts = ArrayDeque<Pair<Long, String>>()
        fun markMessage(id: String, pendingFrames: Long) { messageStarts.addLast((written + pendingFrames) to id) }
        private var lastHead = 0L
        private var wraps = 0L
        private var progress = SystemClock.elapsedRealtime()
        private var started = false
        private var verifyHeadphones = profile.headphones != null
        private var reportedPlaying = false
        private var primingConfigured = false
        private var startThreshold = 0
        private var normalBufferFrames = 0
        private var restoreBuffer = false
        private var realWritten = 0L
        private var padded = 0L
        private var stopShaper: VolumeShaper? = null
        val hasStarted: Boolean get() = started

        init {
            val minimum = AudioTrack.getMinBufferSize(rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
            check(minimum > 0 && minimum <= 256 * 1024) { "voice_playback_format_unavailable" }
            player = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(profile.usage).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(rate).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                // Capacity protects against worker scheduling stalls; it is not the startup watermark.
                .setBufferSizeInBytes(maxOf(minimum, rate * 400 / 1_000 * 2)).setTransferMode(AudioTrack.MODE_STREAM).build()
            var published = false
            try {
                check(!stopped.get() && player.state == AudioTrack.STATE_INITIALIZED) { "voice_playback_initialization_failed" }
                normalBufferFrames = player.bufferSizeInFrames
                check(normalBufferFrames > 0) { "voice_playback_buffer_unavailable" }
                track.set(player)
                published = true
                if (profile.headphones != null) player.addOnRoutingChangedListener(routeListener, null)
                if (stopped.get()) throw InterruptedException("cancelled:voice_playback")
            } catch (error: Throwable) {
                if (published) close() else runCatching { player.release() }
                throw error
            }
        }

        fun preparePriming(inputFrames: Int) {
            if (started || primingConfigured || inputFrames <= 0) return
            primingConfigured = true
            configureThreshold(maxOf(1, minOf(inputFrames, startupFrames) * rate / 24_000))
        }

        private fun configureThreshold(available: Int) {
            val requested = minOf(available, normalBufferFrames)
            startThreshold = if (Build.VERSION.SDK_INT >= 31) {
                player.setStartThresholdInFrames(requested).also { check(it > 0) { "voice_playback_threshold_failed:$it" } }
                player.startThresholdInFrames
            } else {
                player.setBufferSizeInFrames(requested).also {
                    check(it > 0) { "voice_playback_buffer_failed:$it" }
                    restoreBuffer = it != normalBufferFrames
                }
            }
            check(startThreshold in 1..player.bufferCapacityInFrames) { "voice_playback_threshold_invalid" }
        }

        private fun play() {
            if (started || stopped.get() || written == 0L) return
            player.play()
            started = true
            progress = SystemClock.elapsedRealtime()
            Log.d("VcpVoiceOutput", "play id=$id elapsed_ms=${SystemClock.elapsedRealtime() - createdAt} rate=$rate capacity=${player.bufferCapacityInFrames} buffer=${player.bufferSizeInFrames} threshold=$startThreshold real_frames=$realWritten padding_frames=$padded")
        }

        fun write(samples: ShortArray, offset: Int = 0, count: Int = samples.size - offset) {
            writeSamples(samples, offset, count, padding = false)
        }

        private fun writeSamples(samples: ShortArray, offset: Int, length: Int, padding: Boolean) {
            var position = offset
            val end = offset + length
            var lastWrite = SystemClock.elapsedRealtime()
            while (position < end && !stopped.get()) {
                val count = player.write(samples, position, minOf(rate / 50, end - position), AudioTrack.WRITE_NON_BLOCKING)
                check(count >= 0) { "voice_playback_write_failed:$count" }
                if (count == 0) {
                    // The prefill is bounded by the actual driver capacity, not this packet's size.
                    if (!started && written > 0) play()
                    played()
                    check(SystemClock.elapsedRealtime() - lastWrite < 5_000) { "voice_playback_stalled" }
                    Thread.sleep(5)
                } else {
                    if (written == 0L) Log.d("VcpVoiceOutput", "first_write id=$id elapsed_ms=${SystemClock.elapsedRealtime() - createdAt} frames=$count")
                    position += count; written += count
                    if (padding) padded += count else realWritten += count
                    lastWrite = SystemClock.elapsedRealtime()
                    if (!started && primingConfigured && written >= startThreshold) play()
                    if (started) played()
                }
            }
        }

        /** Only a completed segment/EOS can lower the threshold or require a finite silent tail. */
        fun finishPriming() {
            if (started || stopped.get() || written == 0L) return
            primingConfigured = true
            configureThreshold(written.coerceAtMost(Int.MAX_VALUE.toLong()).toInt())
            val missing = (startThreshold - written).coerceAtLeast(0).toInt()
            if (missing > 0) writeSamples(ShortArray(missing), 0, missing, padding = true)
            play()
        }

        private fun played(): Long {
            val head = player.playbackHeadPosition.toLong() and 0xffffffffL
            if (head < lastHead) wraps += 1L shl 32
            if (head != lastHead) {
                lastHead = head; progress = SystemClock.elapsedRealtime()
                if (started && !stopped.get()) {
                    if (verifyHeadphones) {
                        if (!VoiceAudioRouting.sameMediaRoute(profile.headphones, player.routedDevice)) {
                            lease.routeChanged(profile)
                            throw IllegalStateException("audio_route_changed")
                        }
                        verifyHeadphones = false
                    }
                    if (restoreBuffer) {
                        check(player.setBufferSizeInFrames(normalBufferFrames) > 0) { "voice_playback_buffer_restore_failed" }
                        restoreBuffer = false
                    }
                    if (!reportedStart) {
                        reportedStart = true
                        Log.d("VcpVoiceOutput", "first_head id=$id elapsed_ms=${SystemClock.elapsedRealtime() - createdAt} frame=$head")
                        this@VoiceStreamPlayback.started()
                    }
                    while (messageStarts.firstOrNull()?.let { it.first < wraps + head } == true) {
                        renderedMessage = messageStarts.removeFirst().second
                    }
                    report(true)
                }
            }
            return wraps + head
        }

        fun idle() { if (played() >= written) report(false) }
        fun fadeOut() {
            if (!started || immediateStop.get() || played() >= written) return
            stopShaper = player.createVolumeShaper(VolumeShaper.Configuration.Builder()
                .setInterpolatorType(VolumeShaper.Configuration.INTERPOLATOR_TYPE_LINEAR)
                .setCurve(floatArrayOf(0f, 1f), floatArrayOf(1f, 0f))
                .setDuration(60L).build())
            stopShaper!!.apply(VolumeShaper.Operation.PLAY)
            val deadline = System.nanoTime() + 60_000_000L
            while (!immediateStop.get()) {
                val remaining = deadline - System.nanoTime()
                if (remaining <= 0) break
                try { Thread.sleep(minOf(remaining / 1_000_000L + 1, 5L)) }
                catch (_: InterruptedException) { /* A hard stop is checked before waiting again. */ }
            }
        }
        fun closeFade() { stopShaper?.let { runCatching { it.close() } }; stopShaper = null }
        fun drain() {
            progress = SystemClock.elapsedRealtime()
            while (!stopped.get() && played() < written) {
                check(SystemClock.elapsedRealtime() - progress < 5_000) { "voice_playback_drain_stalled" }
                Thread.sleep(5)
            }
            if (stopped.get()) return
            report(false)
            Log.d("VcpVoiceOutput", "drained id=$id real_frames=$realWritten padding_frames=$padded underruns=${player.underrunCount}")
        }
        private fun report(value: Boolean) {
            if (reportedPlaying != value) { reportedPlaying = value; playingChanged(value) }
            reportMessage(renderedMessage.takeIf { value })
        }
        override fun close() {
            if (!track.compareAndSet(player, null)) return
            if (profile.headphones != null) player.removeOnRoutingChangedListener(routeListener)
            release(player)
        }
    }

    private fun release(player: AudioTrack) {
        runCatching { player.pause() }
        runCatching { player.flush() }
        runCatching { player.stop() }
        runCatching { player.release() }
    }
}
