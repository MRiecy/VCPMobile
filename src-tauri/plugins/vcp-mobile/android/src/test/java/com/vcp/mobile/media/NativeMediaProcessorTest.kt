package com.vcp.mobile.media

import android.content.Context
import android.graphics.Bitmap
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.util.concurrent.CancellationException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class NativeMediaProcessorTest {
    @get:Rule
    val temporaryFolder = TemporaryFolder(File("build").apply { mkdirs() })

    @Test
    fun videoFingerprintTracksExactPixelsAndDimensions() {
        val a = Bitmap.createBitmap(intArrayOf(0xff010203.toInt(), 0xff040506.toInt()), 2, 1, Bitmap.Config.ARGB_8888)
        val duplicate = a.copy(Bitmap.Config.ARGB_8888, true)
        val changed = a.copy(Bitmap.Config.ARGB_8888, true).apply { setPixel(1, 0, 0xff040507.toInt()) }
        val rotated = Bitmap.createBitmap(intArrayOf(0xff010203.toInt(), 0xff040506.toInt()), 1, 2, Bitmap.Config.ARGB_8888)
        try {
            val first = videoFrameFingerprint(a) { }
            assertTrue(first.contentEquals(videoFrameFingerprint(duplicate) { }))
            assertFalse(first.contentEquals(videoFrameFingerprint(changed) { }))
            assertFalse(first.contentEquals(videoFrameFingerprint(rotated) { }))
        } finally {
            listOf(a, duplicate, changed, rotated).forEach { it.recycle() }
        }
    }

    @Test
    fun videoFingerprintObservesCancellationBetweenRows() {
        val bitmap = Bitmap.createBitmap(2, 3, Bitmap.Config.ARGB_8888)
        var checks = 0
        try {
            val error = runCatching {
                videoFrameFingerprint(bitmap) {
                    checks += 1
                    if (checks == 2) throw CancellationException("cancelled")
                }
            }.exceptionOrNull()
            assertTrue(error is CancellationException)
            assertEquals(2, checks)
        } finally {
            bitmap.recycle()
        }
    }

    @Test
    fun cancellationBeforeProcessRejectsTheLateAudioInvocation() {
        val processor = NativeMediaProcessor(ApplicationProvider.getApplicationContext<Context>())
        try {
            processor.prepare("cancel-before-process")
            processor.cancel("cancel-before-process")
            val outcomes = mutableListOf<Result<NativeMediaProcessor.AudioResult>>()
            processor.processAudio("cancel-before-process", "unused", "unused", 1024, outcomes::add)
            assertEquals(1, outcomes.size)
            assertTrue(outcomes.single().isFailure)
        } finally {
            processor.destroy()
        }
    }

    @Test
    fun queuedVideoCancellationSettlesOnlyAfterItsWorkerDrains() {
        assertQueuedVideoDrains(destroy = false)
    }

    @Test
    fun destructionDrainsQueuedVideoAndRejectsLaterAdmission() {
        assertQueuedVideoDrains(destroy = true)
    }

    private fun assertQueuedVideoDrains(destroy: Boolean) {
        val processor = NativeMediaProcessor(ApplicationProvider.getApplicationContext<Context>())
        val executorField = NativeMediaProcessor::class.java.getDeclaredField("videoExecutor")
            .apply { isAccessible = true }
        val executor = executorField.get(processor) as ExecutorService
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val completed = CountDownLatch(1)
        val completions = AtomicInteger(0)
        val failure = AtomicReference<Throwable?>()
        val input = temporaryFolder.newFile("original-video").apply { writeText("never decoded") }
        val directory = temporaryFolder.newFolder("rust-owned-output")
        val retained = File(directory, "caller-owned").apply { writeText("retain") }
        try {
            // Occupy the real executor so cancel/destroy wins before native decode.
            executor.execute { entered.countDown(); release.await() }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            processor.prepare("queued-video")
            processor.processVideo("queued-video", input.absolutePath, directory.absolutePath, 1024) {
                completions.incrementAndGet()
                failure.set(it.exceptionOrNull())
                completed.countDown()
            }
            if (destroy) processor.destroy() else processor.cancel("queued-video")
            assertFalse("The process callback must wait for worker settlement", completed.await(50, TimeUnit.MILLISECONDS))
            release.countDown()
            assertTrue(completed.await(2, TimeUnit.SECONDS))
            assertEquals(1, completions.get())
            assertTrue(failure.get() is CancellationException)
            assertEquals("never decoded", input.readText())
            assertEquals("retain", retained.readText())
            assertEquals(listOf("caller-owned"), directory.list()!!.toList())
            if (destroy) {
                assertTrue(runCatching { processor.prepare("late") }.exceptionOrNull() is IllegalStateException)
            }
        } finally {
            release.countDown()
            processor.destroy()
            executor.awaitTermination(2, TimeUnit.SECONDS)
        }
    }
}
