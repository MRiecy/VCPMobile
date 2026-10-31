package com.vcp.mobile.voice

import android.media.AudioAttributes
import android.media.AudioTrack
import android.os.Looper
import app.tauri.plugin.Invoke
import io.mockk.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class NativeVoiceOutputTest {
    private fun request(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(VoicePlaybackArgs::class.java) } returns VoicePlaybackArgs().apply { playbackId = id }
        every { invoke.parseArgs(StartVoicePlaybackArgs::class.java) } returns StartVoicePlaybackArgs().apply { playbackId = id; startupFrames = 5_760 }
        every { invoke.parseArgs(AppendVoicePlaybackArgs::class.java) } returns AppendVoicePlaybackArgs().apply { playbackId = id; pcmBase64 = "AQACAA==" }
    }

    @Test fun standaloneOutputHasNoCapturePrerequisiteAndStaleControlsCannotEvictItsOwner() {
        val session = mockk<VoiceAudioSession>(relaxed = true)
        val lease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        every { session.acquireOutput(any()) } returns lease
        val output = NativeVoiceOutput(session, {}, { _, _ -> })
        val executor = NativeVoiceOutput::class.java.getDeclaredField("worker").apply { isAccessible = true }.get(output) as ThreadPoolExecutor
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            executor.execute { entered.countDown(); runCatching { release.await() } }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            output.stopVoicePlayback(request("old"))
            val current = request("current")
            output.startVoicePlayback(current)
            verify(exactly = 1) { session.acquireOutput(any()) }
            assertFalse(output.snapshot().playing)
            val competing = request("competing")
            output.startVoicePlayback(competing)
            verify(exactly = 1) { competing.reject("voice_playback_busy") }
            val stale = request("old")
            output.startVoicePlayback(stale)
            verify(exactly = 1) { stale.reject("cancelled:voice_playback") }
            output.stopVoicePlayback(request("old"))
            verify(exactly = 0) { current.reject(any<String>()) }
            verify(exactly = 0) { lease.close() }
            val stopped = request("current")
            output.stopVoicePlayback(stopped)
            verify(exactly = 1) { current.reject("cancelled:voice_playback") }
            verify(exactly = 0) { stopped.resolve() }
            verify(exactly = 0) { lease.close() }
            release.countDown()
            executor.shutdown()
            assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS))
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 1) { stopped.resolve() }
            verify(exactly = 1) { lease.close() }
        } finally { output.destroy(); release.countDown(); executor.awaitTermination(2, TimeUnit.SECONDS) }
    }

    @Test fun failedWorkerReleasesItsSlotAndLateControlsRetainTheFailureWithoutEvictingNewOutput() {
        val session = mockk<VoiceAudioSession>(relaxed = true)
        val failedLease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        val nextLease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        every { session.acquireOutput(any()) } returnsMany listOf(failedLease, nextLease)
        every { failedLease.awaitReady(any()) } throws IllegalStateException("route_failed")
        val output = NativeVoiceOutput(session, {}, { _, _ -> })
        val executor = NativeVoiceOutput::class.java.getDeclaredField("worker").apply { isAccessible = true }.get(output) as ThreadPoolExecutor
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            val failed = request("failed")
            output.startVoicePlayback(failed)
            // The worker has posted both its rejected request and its terminal owner callback.
            executor.submit(Runnable {}).get(2, TimeUnit.SECONDS)
            shadowOf(Looper.getMainLooper()).idle()
            val reason = "voice_playback_failed:route_failed"
            verify(exactly = 1) { failed.reject(reason) }
            verify(exactly = 1) { failedLease.close() }
            assertEquals(NativeVoiceOutput.Snapshot(), output.snapshot())

            executor.execute { entered.countDown(); runCatching { release.await() } }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            val next = request("next")
            output.startVoicePlayback(next)
            verify(exactly = 2) { session.acquireOutput(any()) }
            verify(exactly = 0) { next.reject(any<String>()) }

            val staleStart = request("failed")
            val staleAppend = request("failed")
            val staleEnd = request("failed")
            val staleFinish = request("failed")
            output.startVoicePlayback(staleStart)
            output.appendVoicePlayback(staleAppend)
            output.endVoicePlaybackSegment(staleEnd)
            output.finishVoicePlayback(staleFinish)
            listOf(staleStart, staleAppend, staleEnd, staleFinish).forEach { stale ->
                verify(exactly = 1) { stale.reject(reason) }
            }
            verify(exactly = 0) { nextLease.close() }
            verify(exactly = 0) { next.reject(any<String>()) }

            // Rust's cleanup stop must preserve the first failure and leave the new owner alone.
            output.stopVoicePlayback(request("failed"))
            val stoppedFinish = request("failed")
            output.finishVoicePlayback(stoppedFinish)
            verify(exactly = 1) { stoppedFinish.reject(reason) }
            verify(exactly = 0) { nextLease.close() }
            output.stopVoicePlayback(request("next"))
            verify(exactly = 1) { next.reject("cancelled:voice_playback") }
            verify(exactly = 0) { nextLease.close() }
            release.countDown()
            executor.shutdown()
            assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS))
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 1) { nextLease.close() }
        } finally { output.destroy(); release.countDown(); executor.awaitTermination(2, TimeUnit.SECONDS) }
    }

    @Test fun driverFailureAfterStartAndAppendAcknowledgementsReleasesOwnerWithoutPendingRequests() {
        val session = mockk<VoiceAudioSession>(relaxed = true)
        val failedLease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        val nextLease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        val player = mockk<AudioTrack>(relaxed = true)
        val readyPosted = CountDownLatch(1)
        val writeEntered = CountDownLatch(1)
        val failWrite = CountDownLatch(1)
        mockkStatic(AudioTrack::class)
        mockkConstructor(AudioTrack.Builder::class)
        every { AudioTrack.getMinBufferSize(any(), any(), any()) } returns 192
        every { anyConstructed<AudioTrack.Builder>().setAudioAttributes(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setAudioFormat(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setBufferSizeInBytes(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setTransferMode(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().build() } returns player
        every { player.state } returns AudioTrack.STATE_INITIALIZED
        every { player.bufferCapacityInFrames } returns 9_600
        every { player.bufferSizeInFrames } returns 9_600
        every { player.setBufferSizeInFrames(any()) } answers { firstArg() }
        every { player.playbackHeadPosition } answers { readyPosted.countDown(); 0 }
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            writeEntered.countDown()
            check(failWrite.await(2, TimeUnit.SECONDS))
            AudioTrack.ERROR_INVALID_OPERATION
        }
        every { session.acquireOutput(any()) } returnsMany listOf(failedLease, nextLease)
        every { failedLease.awaitReady(any()) } returns VoiceOutputProfile(1, AudioAttributes.USAGE_MEDIA, false)
        every { failedLease.isCurrent(any()) } returns true
        val output = NativeVoiceOutput(session, {}, { _, _ -> })
        val executor = NativeVoiceOutput::class.java.getDeclaredField("worker").apply { isAccessible = true }.get(output) as ThreadPoolExecutor
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            val started = request("failed")
            every { started.parseArgs(StartVoicePlaybackArgs::class.java) } returns StartVoicePlaybackArgs().apply { playbackId = "failed"; startupFrames = 2 }
            output.startVoicePlayback(started)
            assertTrue(readyPosted.await(2, TimeUnit.SECONDS))
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 1) { started.resolve() }

            val appended = request("failed")
            output.appendVoicePlayback(appended)
            assertTrue(writeEntered.await(2, TimeUnit.SECONDS))
            verify(exactly = 1) { appended.resolve() }
            // Both IPC requests are already successful before the driver fails asynchronously.
            failWrite.countDown()
            executor.submit(Runnable {}).get(2, TimeUnit.SECONDS)
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 0) { started.reject(any<String>()) }
            verify(exactly = 0) { appended.reject(any<String>()) }
            verify(exactly = 1) { player.release() }
            verify(exactly = 1) { failedLease.close() }
            assertEquals(NativeVoiceOutput.Snapshot(), output.snapshot())

            executor.execute { entered.countDown(); runCatching { release.await() } }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            val next = request("next")
            output.startVoicePlayback(next)
            verify(exactly = 2) { session.acquireOutput(any()) }
            verify(exactly = 0) { next.reject(any<String>()) }

            output.stopVoicePlayback(request("failed"))
            val lateFinish = request("failed")
            output.finishVoicePlayback(lateFinish)
            verify(exactly = 1) { lateFinish.reject("voice_playback_failed:voice_playback_write_failed:${AudioTrack.ERROR_INVALID_OPERATION}") }
            verify(exactly = 0) { nextLease.close() }
            output.stopVoicePlayback(request("next"))
            verify(exactly = 1) { next.reject("cancelled:voice_playback") }
            verify(exactly = 0) { nextLease.close() }
            release.countDown()
            executor.shutdown()
            assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS))
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 1) { nextLease.close() }
        } finally {
            failWrite.countDown()
            output.destroy()
            release.countDown()
            executor.awaitTermination(2, TimeUnit.SECONDS)
            unmockkAll()
        }
    }

    @Test fun destroyingAQueuedStartStillRunsCleanupAndSettlesEveryStopWaiter() {
        val session = mockk<VoiceAudioSession>(relaxed = true)
        val lease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        every { session.acquireOutput(any()) } returns lease
        val output = NativeVoiceOutput(session, {}, { _, _ -> })
        val executor = NativeVoiceOutput::class.java.getDeclaredField("worker").apply { isAccessible = true }.get(output) as ThreadPoolExecutor
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        try {
            executor.execute { entered.countDown(); release.await() }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            val started = request("queued")
            val firstStop = request("queued")
            val secondStop = request("queued")
            output.startVoicePlayback(started)
            output.stopVoicePlayback(firstStop)
            output.stopVoicePlayback(secondStop)
            output.destroy()
            verify(exactly = 0) { firstStop.resolve() }
            verify(exactly = 0) { secondStop.resolve() }
            release.countDown()
            assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS))
            shadowOf(Looper.getMainLooper()).idle()
            verify(exactly = 1) { started.reject("cancelled:voice_playback") }
            verify(exactly = 1) { firstStop.resolve() }
            verify(exactly = 1) { secondStop.resolve() }
            verify(exactly = 1) { lease.close() }
            verify(exactly = 0) { lease.awaitReady(any()) }
        } finally { release.countDown(); output.destroy(); executor.awaitTermination(2, TimeUnit.SECONDS) }
    }
}
