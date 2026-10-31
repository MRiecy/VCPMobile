package com.vcp.mobile.voice

import android.media.AudioAttributes
import android.media.AudioTrack
import android.media.VolumeShaper
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

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
    private var capacityFrames = 96
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
        every { player.bufferCapacityInFrames } answers { capacityFrames }
        every { player.bufferSizeInFrames } answers { effectiveFrames }
        every { player.setBufferSizeInFrames(any()) } answers {
            effectiveFrames = firstArg<Int>().coerceIn(minimumFrames, capacityFrames)
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
        current?.stop(immediate = true)
        executor.shutdownNow()
        executor.awaitTermination(2, TimeUnit.SECONDS)
        unmockkAll()
    }

    private fun start(startupFrames: Int = 5_760, onMessage: (String?) -> Unit = {}, onPlaying: (Boolean) -> Unit = {}): VoiceStreamPlayback {
        val ready = CountDownLatch(1)
        val output = VoiceStreamPlayback("test", startupFrames, lease, executor,
            started = { progressEvents.incrementAndGet() }, messageChanged = onMessage,
            playingChanged = onPlaying, ended = {})
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

    @Test fun switchesMessageOnlyWhenThePlaybackHeadPassesItsFirstFrame() {
        val head = AtomicInteger(0)
        val messages = LinkedBlockingQueue<String>()
        every { player.playbackHeadPosition } answers { synchronized(pcm) { minOf(head.get(), pcm.size) } }
        val output = start(startupFrames = 1, onMessage = { messages.add(it ?: "idle") })
        output.append(ShortArray(40), "first") { assertNull(it) }
        output.endSegment { assertNull(it) }
        output.append(ShortArray(40), "second") { assertNull(it) }
        output.endSegment { assertNull(it) }
        verify(timeout = 2_000) { player.play() }
        assertTrue(messages.isEmpty())
        head.set(40)
        assertEquals("first", messages.poll(2, TimeUnit.SECONDS))
        assertFalse(messages.contains("second"))
        head.set(41)
        assertEquals("second", messages.poll(2, TimeUnit.SECONDS))
        head.set(80)
        finish(output)
        assertEquals("idle", messages.poll(2, TimeUnit.SECONDS))
    }

    @Test fun largeDriverCapacityDoesNotDelayChatStartupAndRestoresAfterHeadProgress() {
        capacityFrames = 9_600
        effectiveFrames = capacityFrames
        val output = start(startupFrames = 2_880)
        val source = ShortArray(12_000) { (it + 1).toShort() }
        output.append(source) { assertNull(it) }
        finish(output)
        assertEquals(source.toList(), pcm)
        assertEquals(listOf(2_880), starts)
        assertEquals(capacityFrames, effectiveFrames)
        verify { anyConstructed<AudioTrack.Builder>().setBufferSizeInBytes(19_200) }
        verify { player.setBufferSizeInFrames(2_880) }
        verify { player.setBufferSizeInFrames(9_600) }
    }

    @Test
    @Config(sdk = [31])
    fun modernMediaStartupUsesItsWatermarkAndWritesRangesFromTheSameBuffer() {
        capacityFrames = 9_600
        effectiveFrames = capacityFrames
        var threshold = capacityFrames
        every { player invoke "setStartThresholdInFrames" withArguments listOf(any<Int>()) } answers {
            threshold = firstArg<Int>()
            threshold
        }
        every { player invoke "getStartThresholdInFrames" withArguments emptyList() } answers { threshold }
        var firstBuffer: ShortArray? = null
        var copiedSlices = false
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            val source = firstArg<ShortArray>()
            val offset = secondArg<Int>()
            val count = minOf(thirdArg<Int>(), 73)
            if (firstBuffer == null) firstBuffer = source
            else if (firstBuffer !== source) copiedSlices = true
            synchronized(pcm) { pcm.addAll(source.slice(offset until offset + count)) }
            count
        }
        val output = start()
        val source = ShortArray(12_000) { (it + 1).toShort() }
        output.append(source) { assertNull(it) }
        finish(output)
        assertEquals(source.toList(), pcm)
        assertEquals(listOf(5_760), starts)
        assertEquals(5_760, threshold)
        assertFalse("20 ms writes reuse the dequeued PCM instead of allocating slices", copiedSlices)
        verify(exactly = 0) { player.setBufferSizeInFrames(any()) }
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
        verify(exactly = 0) { player.createVolumeShaper(any()) }
        verify(exactly = 1) { player.release() }
    }

    @Test fun gracefulStopCancelsInputImmediatelyAndJoinsOneFadeBeforeReleasingTheRoute() {
        val shaper = mockk<VolumeShaper>(relaxed = true)
        val fadeEntered = CountDownLatch(1)
        val allowFade = CountDownLatch(1)
        val playingObserved = CountDownLatch(1)
        val stoppedPlaying = AtomicInteger()
        val released = CountDownLatch(2)
        val barrier = CountDownLatch(1)
        every { player.createVolumeShaper(any()) } returns shaper
        every { shaper.apply(VolumeShaper.Operation.PLAY) } answers {
            fadeEntered.countDown()
            check(allowFade.await(2, TimeUnit.SECONDS))
        }
        every { player.playbackHeadPosition } answers { if (playing) 1 else 0 }
        val output = start(startupFrames = 3) { value ->
            if (value) playingObserved.countDown() else stoppedPlaying.incrementAndGet()
        }
        try {
            output.append(ShortArray(960) { 100 }) { assertNull(it) }
            assertTrue(playingObserved.await(2, TimeUnit.SECONDS))
            output.stop { released.countDown() }
            assertTrue(fadeEntered.await(2, TimeUnit.SECONDS))
            output.stop { released.countDown() }
            output.quiesceForRoute { barrier.countDown() }
            var rejected: String? = null
            output.append(shortArrayOf(1)) { rejected = it }
            assertEquals("cancelled:voice_playback", rejected)
            assertEquals(2L, released.count)
            assertEquals(1L, barrier.count)
            assertEquals(0, stoppedPlaying.get())
            verify(exactly = 0) { player.release() }
            verify(exactly = 0) { lease.close() }
            allowFade.countDown()
            assertTrue(released.await(2, TimeUnit.SECONDS))
            assertTrue(barrier.await(2, TimeUnit.SECONDS))
            verify(exactly = 1) { player.createVolumeShaper(any()) }
            verify(exactly = 1) { shaper.apply(VolumeShaper.Operation.PLAY) }
            verifyOrder { player.release(); shaper.close(); lease.close() }
            assertEquals(1, stoppedPlaying.get())
        } finally { allowFade.countDown() }
    }

    @Test fun hardStopCanInterruptAnExistingFadeAndReleaseItsWaiters() {
        val shaper = mockk<VolumeShaper>(relaxed = true)
        val fadeEntered = CountDownLatch(1)
        val playingObserved = CountDownLatch(1)
        val released = CountDownLatch(2)
        every { player.createVolumeShaper(any()) } returns shaper
        every { shaper.apply(VolumeShaper.Operation.PLAY) } answers {
            fadeEntered.countDown()
            CountDownLatch(1).await()
        }
        every { player.playbackHeadPosition } answers { if (playing) 1 else 0 }
        val output = start(startupFrames = 3) { if (it) playingObserved.countDown() }
        output.append(ShortArray(960) { 100 }) { assertNull(it) }
        assertTrue(playingObserved.await(2, TimeUnit.SECONDS))
        output.stop { released.countDown() }
        assertTrue(fadeEntered.await(2, TimeUnit.SECONDS))
        output.stop(immediate = true) { released.countDown() }
        assertTrue(released.await(2, TimeUnit.SECONDS))
        verify(exactly = 1) { player.release() }
        verifyOrder { player.release(); shaper.close(); lease.close() }
    }

    @Test fun stopDuringRouteDrainKeepsTheOldTrackAliveUntilItsFadeCompletes() {
        val shaper = mockk<VolumeShaper>(relaxed = true)
        val playingObserved = CountDownLatch(1)
        val drainObserved = CountDownLatch(1)
        val routePaused = AtomicReference<AtomicBoolean>()
        val fadeEntered = CountDownLatch(1)
        val allowFade = CountDownLatch(1)
        val barrier = CountDownLatch(1)
        val released = CountDownLatch(1)
        every { player.createVolumeShaper(any()) } returns shaper
        every { shaper.apply(VolumeShaper.Operation.PLAY) } answers {
            fadeEntered.countDown()
            check(allowFade.await(2, TimeUnit.SECONDS))
        }
        every { player.playbackHeadPosition } answers {
            if (routePaused.get()?.get() == true) drainObserved.countDown()
            if (playing) 1 else 0
        }
        val output = start(startupFrames = 3) { if (it) playingObserved.countDown() }
        routePaused.set(VoiceStreamPlayback::class.java.getDeclaredField("routePause")
            .apply { isAccessible = true }.get(output) as AtomicBoolean)
        try {
            output.append(ShortArray(960) { 100 }) { assertNull(it) }
            assertTrue(playingObserved.await(2, TimeUnit.SECONDS))
            output.quiesceForRoute { barrier.countDown() }
            assertTrue("the route switch reaches the old track's drain", drainObserved.await(2, TimeUnit.SECONDS))
            output.stop { released.countDown() }
            assertTrue(fadeEntered.await(2, TimeUnit.SECONDS))
            assertEquals(1L, barrier.count)
            verify(exactly = 0) { player.release() }
            allowFade.countDown()
            assertTrue(released.await(2, TimeUnit.SECONDS))
            assertTrue(barrier.await(2, TimeUnit.SECONDS))
            verify(exactly = 1) { player.createVolumeShaper(any()) }
            verify(exactly = 1) { anyConstructed<AudioTrack.Builder>().build() }
        } finally { allowFade.countDown() }
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

    private fun exerciseMidPacketRouteReplacement(sourceFrames: Int, expectedStartupFrames: Int) {
        capacityFrames = 9_600
        effectiveFrames = capacityFrames
        val replacement = mockk<AudioTrack>(relaxed = true)
        val replacementPcm = mutableListOf<Short>()
        val replacementStarted = CountDownLatch(1)
        val barrierReached = AtomicInteger()
        var replacementPlaying = false
        var replacementBuffer = capacityFrames
        var framesAtStart = 0
        every { anyConstructed<AudioTrack.Builder>().build() } returnsMany listOf(player, replacement)
        every { replacement.state } returns AudioTrack.STATE_INITIALIZED
        every { replacement.bufferCapacityInFrames } returns capacityFrames
        every { replacement.bufferSizeInFrames } answers { replacementBuffer }
        every { replacement.setBufferSizeInFrames(any()) } answers {
            replacementBuffer = firstArg<Int>().coerceIn(1, capacityFrames)
            replacementBuffer
        }
        every { replacement.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            val source = firstArg<ShortArray>()
            val offset = secondArg<Int>()
            val count = minOf(thirdArg<Int>(), 73)
            val accepted = source.slice(offset until offset + count)
            replacementPcm.addAll(accepted)
            synchronized(pcm) { pcm.addAll(accepted) }
            count
        }
        every { replacement.play() } answers {
            framesAtStart = replacementPcm.size
            replacementPlaying = true
            replacementStarted.countDown()
        }
        every { replacement.playbackHeadPosition } answers { if (replacementPlaying) replacementPcm.size else 0 }
        every { lease.awaitReady(any()) } returnsMany listOf(
            VoiceOutputProfile(1, AudioAttributes.USAGE_MEDIA, false),
            VoiceOutputProfile(2, AudioAttributes.USAGE_MEDIA, false),
        )
        val output = start(startupFrames = 2_880)
        every { player.write(any<ShortArray>(), any(), any(), AudioTrack.WRITE_NON_BLOCKING) } answers {
            val source = firstArg<ShortArray>()
            val offset = secondArg<Int>()
            val count = thirdArg<Int>()
            synchronized(pcm) {
                pcm.addAll(source.slice(offset until offset + count))
                if (pcm.size == 4_800) output.quiesceForRoute { barrierReached.incrementAndGet() }
            }
            count
        }
        val source = ShortArray(sourceFrames) { (it + 1).toShort() }
        output.append(source) { assertNull(it) }
        assertTrue("replacement starts from its remaining PCM without waiting for EOS", replacementStarted.await(2, TimeUnit.SECONDS))
        finish(output)
        assertEquals(1, barrierReached.get())
        assertEquals(expectedStartupFrames, framesAtStart)
        assertEquals(source.toList(), pcm)
        assertEquals(1, progressEvents.get())
        verify { replacement.setBufferSizeInFrames(expectedStartupFrames) }
        verify(exactly = 1) { player.release() }
    }

    @Test fun midPacketRouteReplacementConfiguresItsOwnStartupWatermark() {
        exerciseMidPacketRouteReplacement(sourceFrames = 12_000, expectedStartupFrames = 2_880)
    }

    @Test fun shortRemainderAfterRouteReplacementStartsBeforeEos() {
        exerciseMidPacketRouteReplacement(sourceFrames = 6_240, expectedStartupFrames = 1_440)
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
