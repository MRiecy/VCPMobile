package com.vcp.mobile.voice

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.SystemClock
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/** A bounded PCM sink. Every request remains cancellable until its own acknowledgement. */
internal class VoiceStreamPlayback(
    val id: String,
    private val lease: VoiceAudioSession.OutputLease,
    private val executor: Executor,
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
    private val queue = VoicePlaybackQueue()
    private val stopped = AtomicBoolean(false)
    private val completed = AtomicBoolean(false)
    private val failure = AtomicReference<String?>(null)
    private val track = AtomicReference<AudioTrack?>(null)
    private val finishRequest = AtomicReference<Completion?>(null)
    private val routePause = AtomicBoolean(false)
    private val quiesce = AtomicReference<(() -> Unit)?>(null)
    @Volatile private var worker: Thread? = null

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
            completed.set(true)
            failure.set("voice_playback_busy")
            ready.complete("voice_playback_busy")
            ended("voice_playback_busy")
        }
    }

    fun append(samples: ShortArray, callback: (String?) -> Unit) {
        val completion = request(callback)
        if (stopped.get() || completed.get()) return
        queue.append(samples, completion::complete)
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

    fun stop(reason: String = "cancelled:voice_playback") {
        failure.compareAndSet(null, reason)
        stopped.set(true)
        queue.cancel(reason)
        requests.toList().forEach { it.complete(reason) }
        releaseTrack()
        lease.close()
        quiesce.getAndSet(null)?.invoke()
        playingChanged(false)
        worker?.interrupt()
    }

    fun quiesceForRoute(ready: () -> Unit) {
        if (stopped.get() || completed.get() || routePause.get() || track.get() == null) ready()
        else quiesce.set(ready)
    }

    private fun runPlayback(ready: Completion) {
        var terminalError: String? = null
        var sink: Sink? = null
        var converter: VoiceLegacyScoResampler? = null
        try {
            if (stopped.get()) return
            fun ensureOutput() {
                if (sink != null && quiesce.get() == null && lease.isCurrent(sink!!.profile)) return
                routePause.set(true)
                sink?.let { old ->
                    converter?.let { old.write(it.finish()) }
                    old.drain()
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
                    var position = 0
                    while (!stopped.get() && position < next.samples.size) {
                        ensureOutput()
                        val end = minOf(position + 480, next.samples.size)
                        val source = next.samples.copyOfRange(position, end)
                        sink!!.write(converter?.process(source) ?: source)
                        position = end
                    }
                } else {
                    sink?.idle()
                    if (next.finished) break
                }
            }
            if (!stopped.get()) converter?.let { sink?.write(it.finish()) }
            sink?.drain()
            if (!stopped.get()) finishRequest.get()?.complete()
        } catch (error: Throwable) {
            if (!stopped.get()) terminalError = "voice_playback_failed:${error.message ?: error.javaClass.simpleName}"
        } finally {
            completed.set(true)
            val reason = terminalError ?: failure.get() ?: "cancelled:voice_playback"
            failure.compareAndSet(null, reason)
            queue.cancel(reason)
            requests.toList().forEach { it.complete(reason) }
            releaseTrack()
            routePause.set(true)
            quiesce.getAndSet(null)?.invoke()
            lease.close()
            playingChanged(false)
            ended(terminalError)
            worker = null
            Thread.interrupted()
        }
    }

    private inner class Sink(val profile: VoiceOutputProfile) : AutoCloseable {
        private val rate = if (profile.legacySco) 16_000 else 24_000
        private val player: AudioTrack
        private var written = 0L
        private var lastHead = 0L
        private var wraps = 0L
        private var progress = SystemClock.elapsedRealtime()
        private var started = false
        private var reportedPlaying = false

        init {
            val minimum = AudioTrack.getMinBufferSize(rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
            check(minimum > 0 && minimum <= 256 * 1024) { "voice_playback_format_unavailable" }
            player = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(profile.usage).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(rate).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                .setBufferSizeInBytes(maxOf(minimum, rate / 25 * 2)).setTransferMode(AudioTrack.MODE_STREAM).build()
            if (stopped.get() || player.state != AudioTrack.STATE_INITIALIZED) {
                player.release()
                throw IllegalStateException("voice_playback_initialization_failed")
            }
            track.set(player)
            if (stopped.get()) { releaseTrack(); throw InterruptedException("cancelled:voice_playback") }
        }

        fun write(samples: ShortArray) {
            var position = 0
            var lastWrite = SystemClock.elapsedRealtime()
            while (position < samples.size && !stopped.get()) {
                if (!started) { player.play(); started = true }
                val count = player.write(samples, position, minOf(rate / 50, samples.size - position), AudioTrack.WRITE_NON_BLOCKING)
                check(count >= 0) { "voice_playback_write_failed:$count" }
                if (count == 0) {
                    check(SystemClock.elapsedRealtime() - lastWrite < 5_000) { "voice_playback_stalled" }
                    Thread.sleep(5)
                } else {
                    report(true); position += count; written += count; lastWrite = SystemClock.elapsedRealtime()
                }
            }
        }

        private fun played(): Long {
            val head = player.playbackHeadPosition.toLong() and 0xffffffffL
            if (head < lastHead) wraps += 1L shl 32
            if (head != lastHead) { lastHead = head; progress = SystemClock.elapsedRealtime() }
            return wraps + head
        }

        fun idle() { if (played() >= written) report(false) }
        fun drain() {
            progress = SystemClock.elapsedRealtime()
            while (!stopped.get() && played() < written) {
                check(SystemClock.elapsedRealtime() - progress < 5_000) { "voice_playback_drain_stalled" }
                Thread.sleep(5)
            }
            report(false)
        }
        private fun report(value: Boolean) { if (reportedPlaying != value) { reportedPlaying = value; playingChanged(value) } }
        override fun close() { if (track.compareAndSet(player, null)) release(player) }
    }

    private fun releaseTrack() {
        val player = track.getAndSet(null) ?: return
        release(player)
    }

    private fun release(player: AudioTrack) {
        runCatching { player.pause() }
        runCatching { player.flush() }
        runCatching { player.stop() }
        runCatching { player.release() }
    }
}
