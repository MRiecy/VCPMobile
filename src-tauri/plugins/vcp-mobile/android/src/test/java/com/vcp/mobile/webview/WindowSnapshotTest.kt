package com.vcp.mobile.webview

import android.app.Activity
import android.graphics.Bitmap
import android.view.View
import android.view.Window
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import io.mockk.every
import io.mockk.mockk
import io.mockk.mockkStatic
import io.mockk.unmockkStatic
import io.mockk.verify
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class WindowSnapshotTest {
    private fun invocation(responses: MutableList<Long>) = Invoke(
        1, "captureWindowSnapshot", 2, 3,
        { callback, _ -> responses.add(callback) }, "{}", ObjectMapper(),
    )

    @Test
    fun drawingFailureRecyclesTheAllocatedBitmapBeforeRejecting() {
        val activity = mockk<Activity>()
        val window = mockk<Window>()
        val root = mockk<View>()
        every { activity.runOnUiThread(any()) } answers { firstArg<Runnable>().run() }
        every { activity.window } returns window
        every { window.decorView } returns root
        every { root.rootView } returns root
        every { root.width } returns 320
        every { root.height } returns 640
        every { root.draw(any()) } throws IllegalStateException("view detached while drawing")
        val allocated = mutableListOf<Bitmap>()
        mockkStatic(Bitmap::class)
        try {
            every { Bitmap.createBitmap(any<Int>(), any<Int>(), any<Bitmap.Config>()) } answers {
                (callOriginal() as Bitmap).also(allocated::add)
            }
            val responses = mutableListOf<Long>()
            WindowSnapshot(activity).captureWindowSnapshot(invocation(responses))

            assertEquals(listOf(3L), responses)
            assertEquals(1, allocated.size)
            assertTrue(allocated.single().isRecycled)
        } finally {
            unmockkStatic(Bitmap::class)
        }
    }

    @Test
    fun queuedCaptureRejectsAfterDestroyWithoutTouchingTheWindow() {
        val activity = mockk<Activity>()
        val queued = mutableListOf<Runnable>()
        every { activity.runOnUiThread(any()) } answers { queued.add(firstArg()) ; Unit }
        val responses = mutableListOf<Long>()
        val owner = WindowSnapshot(activity)
        owner.captureWindowSnapshot(invocation(responses))
        owner.destroy()
        queued.single().run()

        assertEquals(listOf(3L), responses)
        verify(exactly = 0) { activity.window }
    }
}
