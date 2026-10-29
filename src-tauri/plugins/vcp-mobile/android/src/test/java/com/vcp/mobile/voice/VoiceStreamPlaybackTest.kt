package com.vcp.mobile.voice

import android.media.AudioAttributes
import android.media.AudioTrack
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** Exercises the real worker/queue against a short-writing driver, not a second playback state machine. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class VoiceStreamPlaybackTest {
    private val executor = Executors.newSingleThreadExecutor()
    private val player = mockk<AudioTrack>(relaxed = true)
    private val lease = mockk<VoiceAudioSession.OutputLease>(relaxed = true)
    private val pcm = mutableListOf<Short>()
    private val starts = mutableListOf<Int>()
    private val progressEvents = AtomicInteger()
    private var minimumFrames = 1
    private var effectiveFrames = 96
    @Volatile private var playing = false
    @Volatile private var holdHead = false
    private var current: VoiceStreamPlayback? = null

    @Before fun setup() {
        mockkStatic(AudioTrack::class)
        mockkConstructor(AudioTrack.Builder::class)
        every { AudioTrack.getMinBufferSize(any(), any(), any()) } returns 192
        every { anyConstructed<AudioTrack.Builder>().setAudioAttributes(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setAudioFormat(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setBufferSizeInBytes(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().setTransferMode(any()) } answers { self as AudioTrack.Builder }
        every { anyConstructed<AudioTrack.Builder>().build() } returns player
        every { player.state } returns AudioTrack.STATE_INITIALIZED
        every { player.bufferCapacityInFrames } returns 96
        every { player.bufferSizeInFrames } answers { effectiveFrames }
        every { player.setBufferSizeInFrames(any()) } answers {
            effectiveFrames = firstArg<Int>().coerceIn(minimumFrames, 96)
            effectiveFrames
        }
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            val source = firstArg<ShortArray>()
            val offset = secondArg<Int>()
            val count = minOf(thirdArg<Int>(), 7)
            synchronized(pcm) { pcm.addAll(source.slice(offset until offset + count)) }
            count
        }
        every { player.play() } answers {
            synchronized(pcm) { starts.add(pcm.size) }
            playing = true
        }
        every { player.playbackHeadPosition } answers {
            if (!playing || holdHead) 0 else synchronized(pcm) { pcm.size }
        }
        every { lease.awaitReady(any()) } returns VoiceOutputProfile(1, AudioAttributes.USAGE_MEDIA, false)
        every { lease.isCurrent(any()) } returns true
    }

    @After fun cleanup() {
        current?.stop()
        executor.shutdownNow()
        executor.awaitTermination(2, TimeUnit.SECONDS)
        unmockkAll()
    }

    private fun start(startupFrames: Int = 5_760): VoiceStreamPlayback {
        val ready = CountDownLatch(1)
        val output = VoiceStreamPlayback("test", startupFrames, lease, executor,
            started = { progressEvents.incrementAndGet() }, playingChanged = {}, ended = {})
        current = output
        output.start { assertNull(it); ready.countDown() }
        assertTrue("start acknowledges resources without PCM", ready.await(2, TimeUnit.SECONDS))
        return output
    }

    private fun finish(output: VoiceStreamPlayback) {
        val finished = CountDownLatch(1)
        var failure: String? = null
        output.finish { failure = it; finished.countDown() }
        assertTrue("finish settles the hardware drain", finished.await(2, TimeUnit.SECONDS))
        assertNull(failure)
    }

    @Test fun completedShortSegmentPrimesBeforePlayAndPreservesSamplesAcrossShortWrites() {
        val output = start()
        val source = ShortArray(31) { (it + 1).toShort() }
        output.append(source) { assertNull(it) }
        verify(exactly = 0) { player.play() }
        output.endSegment { assertNull(it) }
        finish(output)
        assertEquals(source.toList(), pcm)
        assertEquals(listOf(source.size), starts)
        assertEquals(1, progressEvents.get())
    }

    @Test fun clampedLegacyThresholdAddsOnlyTheNecessaryTailAndDrainsIt() {
        minimumFrames = 64
        val output = start()
        val source = shortArrayOf(111, 222, 333)
        output.append(source) { assertNull(it) }
        output.endSegment { assertNull(it) }
        finish(output)
        assertEquals(source.toList(), pcm.take(source.size))
        assertEquals(64, pcm.size)
        assertTrue(pcm.drop(source.size).all { it == 0.toShort() })
        assertEquals(listOf(64), starts)
        assertEquals(1, progressEvents.get())
    }

    @Test fun cancellationWhileBufferingNeverStartsTheTrack() {
        val output = start()
        output.append(shortArrayOf(11)) { assertNull(it) }
        output.stop()
        executor.shutdown()
        assertTrue(executor.awaitTermination(2, TimeUnit.SECONDS))
        verify(exactly = 0) { player.play() }
        assertTrue(pcm.isEmpty())
        assertEquals(0, progressEvents.get())
    }

    @Test fun startedEventWaitsForPlaybackHeadInsteadOfAppendOrPlayAcknowledgement() {
        holdHead = true
        val output = start(startupFrames = 3)
        val played = CountDownLatch(1)
        every { player.play() } answers { playing = true; played.countDown() }
        output.append(shortArrayOf(1, 2, 3)) { assertNull(it) }
        output.endSegment { assertNull(it) }
        assertTrue(played.await(2, TimeUnit.SECONDS))
        assertEquals(0, progressEvents.get())
        holdHead = false
        finish(output)
        assertEquals(1, progressEvents.get())
    }

    @Test fun aTemporarilyFullDriverStartsWithoutLosingTheUnwrittenSuffix() {
        val output = start(startupFrames = 100)
        var returnedZero = false
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            synchronized(pcm) {
                if (!returnedZero && pcm.size == 14) { returnedZero = true; 0 }
                else {
                    val source = firstArg<ShortArray>()
                    val offset = secondArg<Int>()
                    val count = minOf(thirdArg<Int>(), 7)
                    pcm.addAll(source.slice(offset until offset + count))
                    count
                }
            }
        }
        val source = ShortArray(100) { (it + 1).toShort() }
        output.append(source) { assertNull(it) }
        finish(output)
        assertTrue(returnedZero)
        assertEquals(source.toList(), pcm)
        assertEquals(listOf(14), starts)
    }

    @Test fun queuedRealAudioWinsOverPaddingAtAShortSegmentBoundary() {
        minimumFrames = 64
        val enteredWrite = CountDownLatch(1)
        val releaseWrite = CountDownLatch(1)
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            enteredWrite.countDown()
            check(releaseWrite.await(2, TimeUnit.SECONDS))
            val source = firstArg<ShortArray>()
            val offset = secondArg<Int>()
            val count = minOf(thirdArg<Int>(), 7)
            synchronized(pcm) { pcm.addAll(source.slice(offset until offset + count)) }
            count
        }
        val output = start()
        output.append(shortArrayOf(1, 2, 3)) { assertNull(it) }
        output.endSegment { assertNull(it) }
        assertTrue(enteredWrite.await(2, TimeUnit.SECONDS))
        val following = ShortArray(100) { (it + 4).toShort() }
        output.append(following) { assertNull(it) }
        releaseWrite.countDown()
        finish(output)
        assertEquals((1..103).map(Int::toShort), pcm)
    }

    @Test fun routeBarrierIsServicedWhileTheStartupGateIsClosed() {
        val output = start()
        output.append(shortArrayOf(1)) { assertNull(it) }
        val quiesced = CountDownLatch(1)
        output.quiesceForRoute { quiesced.countDown() }
        assertTrue(quiesced.await(2, TimeUnit.SECONDS))
        verify(exactly = 0) { player.play() }
    }

    @Test
    @Config(sdk = [31])
    fun thresholdReadbackControlsShortPlaybackOnModernAndroid() {
        // The test compile classpath includes android-all 28; resolve API31 methods in its runtime sandbox.
        every { player invoke "setStartThresholdInFrames" withArguments listOf(any<Int>()) } answers { firstArg<Int>() }
        every { player invoke "getStartThresholdInFrames" withArguments emptyList() } returns 64
        val output = start()
        output.append(shortArrayOf(10, 20, 30)) { assertNull(it) }
        output.endSegment { assertNull(it) }
        finish(output)
        assertEquals(listOf<Short>(10, 20, 30), pcm.take(3))
        assertEquals(64, pcm.size)
        assertTrue(pcm.drop(3).all { it == 0.toShort() })
        assertEquals(listOf(64), starts)
        verify(atLeast = 1) { player invoke "getStartThresholdInFrames" withArguments emptyList() }
        verify(exactly = 0) { player.setBufferSizeInFrames(any()) }
    }
}
