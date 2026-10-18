package com.vcp.mobile.webview

import android.app.Activity
import android.os.Looper
import android.webkit.ValueCallback
import android.webkit.WebView
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
@LooperMode(LooperMode.Mode.PAUSED)
class KeyboardInsetsManagerTest {
    private class DocumentWebView(activity: Activity) : WebView(activity) {
        val deliveredScripts = mutableListOf<String>()

        override fun evaluateJavascript(script: String, resultCallback: ValueCallback<String>?) {
            deliveredScripts.add(script)
        }
    }

    @Test
    fun unchangedInsetsReachEachReplacementAndReloadedDocument() {
        val controller = Robolectric.buildActivity(Activity::class.java).setup().visible()
        val activity = controller.get()
        val first = DocumentWebView(activity)
        val replacement = DocumentWebView(activity)
        val manager = KeyboardInsetsManager(activity)
        fun drainFrames() {
            shadowOf(Looper.getMainLooper()).idleFor(32, TimeUnit.MILLISECONDS)
        }
        try {
            manager.attach(first)
            drainFrames()
            // attach requests the real Window's insets on the next traversal.
            // A manually dispatched Rect does not change that Window snapshot.
            val originalSnapshot = first.deliveredScripts.last()
            assertTrue(originalSnapshot.contains("window.__VCP_NATIVE_INSETS__"))
            assertTrue(originalSnapshot.contains("vcp-keyboard-inset"))
            val originalDeliveries = first.deliveredScripts.size

            manager.attach(replacement)
            drainFrames()
            assertEquals(originalDeliveries, first.deliveredScripts.size)
            assertEquals(originalSnapshot, replacement.deliveredScripts.single())

            // A reload keeps its Java WebView but creates a new JS document.
            replacement.deliveredScripts.clear()
            manager.attach(replacement)
            drainFrames()
            assertEquals(originalSnapshot, replacement.deliveredScripts.single())
        } finally {
            manager.detach()
            first.destroy()
            replacement.destroy()
            controller.pause().stop().destroy()
        }
    }
}
