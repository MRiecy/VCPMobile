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
    private data class Evaluation(val script: String, val callback: ValueCallback<String>?) {
        fun complete(result: Boolean) { callback?.onReceiveValue(result.toString()) }
    }

    private class DocumentWebView(activity: Activity) : WebView(activity) {
        val evaluations = mutableListOf<Evaluation>()
        val insets: List<Evaluation>
            get() = evaluations.filter { it.script.contains("window.__VCP_NATIVE_INSETS__ =") }

        override fun evaluateJavascript(script: String, resultCallback: ValueCallback<String>?) {
            evaluations.add(Evaluation(script, resultCallback))
        }
    }

    private class Fixture {
        private val controller = Robolectric.buildActivity(Activity::class.java).setup().visible()
        val activity = controller.get()
        val webView = DocumentWebView(activity)
        val manager = KeyboardInsetsManager(activity)
        val resume = NativeWebViewResume(activity, manager::connectDocument)

        init {
            manager.attach(webView)
            resume.attach(webView)
        }

        fun connect(id: String): Evaluation {
            resume.connect(id)
            return webView.evaluations.last()
        }

        fun close() {
            resume.destroy()
            manager.detach()
            webView.destroy()
            controller.pause().stop().destroy()
        }
    }

    private fun drainFrames() {
        shadowOf(Looper.getMainLooper()).idleFor(32, TimeUnit.MILLISECONDS)
    }

    private fun snapshot(evaluation: Evaluation): String = evaluation.script
        .substringAfter("window.__VCP_NATIVE_INSETS__ = ").substringBefore(';')

    @Test
    fun unchangedInsetsReachAReplacementViewAfterItsDocumentConnects() {
        val fixture = Fixture()
        val replacement = DocumentWebView(fixture.activity)
        try {
            fixture.connect("first").complete(true)
            drainFrames()
            // attach requests the real Window's insets on the next traversal.
            // A manually dispatched Rect does not change that Window snapshot.
            val original = fixture.webView.insets.single()
            original.complete(true)

            fixture.manager.attach(replacement)
            fixture.resume.attach(replacement)
            fixture.resume.connect("replacement")
            replacement.evaluations.last().complete(true)
            drainFrames()
            assertEquals(1, fixture.webView.insets.size)
            assertEquals(snapshot(original), snapshot(replacement.insets.single()))
            replacement.insets.single().complete(true)
        } finally {
            fixture.close()
            replacement.destroy()
        }
    }

    @Test
    fun newDocumentHandshakeReplaysUnchangedInsetsWithoutAttachingTheViewAgain() {
        val fixture = Fixture()
        try {
            // Initial native layout can precede the first document's JS marker.
            drainFrames()
            assertTrue(fixture.webView.insets.isEmpty())
            fixture.connect("first").complete(true)
            drainFrames()
            val original = fixture.webView.insets.single()
            original.complete(true)

            // WebView.reload keeps the Java View. Only the new page's normal
            // connectWebViewResume handshake is delivered; attach is not called.
            fixture.connect("reloaded").complete(true)
            drainFrames()
            assertEquals(2, fixture.webView.insets.size)
            val replay = fixture.webView.insets.last()
            assertEquals(snapshot(original), snapshot(replay))
            assertTrue(replay.script.contains("m.documentId!==\"reloaded\""))
            assertTrue(replay.script.contains("vcp-keyboard-inset"))
            replay.complete(true)

            fixture.connect("reloaded").complete(true)
            drainFrames()
            assertEquals(2, fixture.webView.insets.size)
        } finally {
            fixture.close()
        }
    }

    @Test
    fun rejectedPageInjectionIsNotRememberedAsDelivered() {
        val fixture = Fixture()
        try {
            fixture.connect("first").complete(false)
            drainFrames()
            assertTrue(fixture.webView.insets.isEmpty())

            fixture.connect("first").complete(true)
            drainFrames()
            val abandoned = fixture.webView.insets.single()
            // Navigation can start after marker verification but before the
            // Insets script executes. Its document guard then returns false.
            abandoned.complete(false)

            fixture.connect("first").complete(true)
            drainFrames()
            assertEquals(2, fixture.webView.insets.size)
            fixture.webView.insets.last().complete(true)
        } finally {
            fixture.close()
        }
    }

    @Test
    fun lateOldValidationAndAcknowledgementCannotConsumeNewDocumentInsets() {
        val fixture = Fixture()
        try {
            fixture.connect("first").complete(true)
            drainFrames()
            val oldDelivery = fixture.webView.insets.single()
            val oldValidation = fixture.connect("first")
            fixture.connect("reloaded").complete(true)
            oldValidation.complete(true)
            drainFrames()

            assertEquals(2, fixture.webView.insets.size)
            val currentDelivery = fixture.webView.insets.last()
            assertTrue(currentDelivery.script.contains("m.documentId!==\"reloaded\""))
            currentDelivery.complete(true)
            oldDelivery.complete(true)

            fixture.connect("reloaded").complete(true)
            drainFrames()
            assertEquals(2, fixture.webView.insets.size)
        } finally {
            fixture.close()
        }
    }
}
