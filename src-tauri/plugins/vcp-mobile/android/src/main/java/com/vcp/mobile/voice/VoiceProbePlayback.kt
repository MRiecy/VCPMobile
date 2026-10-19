package com.vcp.mobile.voice

import android.content.res.AssetManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/** Only a bundled fixture is played; microphone samples can never enter this class. */
internal class VoiceProbePlayback(
    private val assets: AssetManager,
    private val began: () -> Unit,
    private val ended: (Throwable?) -> Unit,
) {
    private val stopped = AtomicBoolean(false)
    private val track = AtomicReference<AudioTrack?>(null)
    @Volatile private var worker: Thread? = null

    fun start() {
        val thread = Thread({
            var failure: Throwable? = null
            try {
                val pcm = assets.open("voice/voice-probe.wav").use(::readVoiceProbe)
                if (stopped.get()) return@Thread
                val minimum = AudioTrack.getMinBufferSize(pcm.sampleRate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
                check(minimum > 0) { "probe_format_unavailable" }
                val player = AudioTrack.Builder()
                    .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                    .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(pcm.sampleRate).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                    .setBufferSizeInBytes(maxOf(minimum, pcm.sampleRate / 25 * 2))
                    .setTransferMode(AudioTrack.MODE_STREAM).build()
                if (stopped.get()) { player.release(); return@Thread }
                track.set(player)
                if (stopped.get()) return@Thread
                check(player.state == AudioTrack.STATE_INITIALIZED) { "probe_initialization_failed" }
                player.play()
                began()
                var position = 0
                while (!stopped.get() && position < pcm.samples.size) {
                    val count = player.write(pcm.samples, position, minOf(pcm.sampleRate / 50, pcm.samples.size - position), AudioTrack.WRITE_NON_BLOCKING)
                    check(count >= 0) { "probe_write_failed:$count" }
                    position += count
                    if (count == 0) Thread.sleep(5)
                }
                val deadline = android.os.SystemClock.elapsedRealtime() + 2_000
                while (!stopped.get() && player.playbackHeadPosition.toLong() < pcm.samples.size) {
                    check(android.os.SystemClock.elapsedRealtime() < deadline) { "probe_playback_stalled" }
                    Thread.sleep(5)
                }
            } catch (error: Throwable) {
                if (!stopped.get()) failure = error
            } finally {
                releaseTrack()
                ended(failure)
            }
        }, "vcp-voice-probe").apply { isDaemon = true }
        worker = thread
        thread.start()
    }

    fun stop() {
        stopped.set(true)
        // Clear queued output immediately, independently of capture/VAD work.
        releaseTrack()
        worker?.interrupt()
    }

    private fun releaseTrack() {
        val player = track.getAndSet(null) ?: return
        runCatching { player.pause() }
        runCatching { player.flush() }
        runCatching { player.stop() }
        runCatching { player.release() }
    }
}
