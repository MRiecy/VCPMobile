package com.vcp.mobile.voice

import app.tauri.plugin.Invoke
import io.mockk.every
import io.mockk.mockk
import io.mockk.verify
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class NativeVoiceOutputTest {
    private fun request(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(VoicePlaybackArgs::class.java) } returns VoicePlaybackArgs().apply { playbackId = id }
    }

    @Test fun standaloneOutputHasNoCapturePrerequisiteAndStaleControlsCannotEvictItsOwner() {
        val session = mockk<VoiceAudioSession>(relaxed = true)
        val lease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
        every { session.acquireOutput(any()) } returns lease
        val output = NativeVoiceOutput(session)
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
            output.stopVoicePlayback(request("current"))
            verify(exactly = 1) { current.reject("cancelled:voice_playback") }
            verify(exactly = 1) { lease.close() }
        } finally { output.destroy(); release.countDown(); executor.awaitTermination(2, TimeUnit.SECONDS) }
    }
}
