package com.vcp.mobile.sharing

import android.app.Activity
import android.content.Intent
import android.os.Looper
import android.webkit.WebView
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.LooperMode
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.CountDownLatch

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
@LooperMode(LooperMode.Mode.PAUSED)
class ShareIntentLifecycleTest {
    class ShareActivity : Activity() {
        lateinit var shareCache: File
        override fun getCacheDir(): File = if (::shareCache.isInitialized) shareCache else super.getCacheDir()
    }

    @get:Rule val temporary = TemporaryFolder(File("build").apply { mkdirs() })
    private lateinit var executor: ExecutorService
    private lateinit var activity: ShareActivity
    private lateinit var handler: ShareIntentHandler
    private val scripts = mutableListOf<String>()

    @Before fun setUp() {
        activity = Robolectric.buildActivity(ShareActivity::class.java).create().get().apply {
            shareCache = temporary.newFolder("cache")
        }
        val webView = mockk<WebView>(relaxed = true)
        every { webView.evaluateJavascript(capture(scripts), any()) } just Runs
        executor = Executors.newSingleThreadExecutor()
        handler = ShareIntentHandler(activity, executor) { webView }
        drain()
    }

    @After fun tearDown() {
        handler.destroy()
        executor.shutdownNow()
        shadowOf(Looper.getMainLooper()).idle()
        unmockkAll()
    }

    private fun textIntent(text: String) = Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_TEXT, text)
    }

    private fun drain() { executor.submit { }.get(5, TimeUnit.SECONDS) }

    @Test fun webViewReattachmentDoesNotCreateANewOwnerForTheInitialIntent() {
        val intent = textIntent("same initial contents")
        handler.handleInitialIntent(intent)
        val owner = handler.currentOwnerId()
        drain()
        shadowOf(Looper.getMainLooper()).idle()
        val delivered = scripts.size

        handler.handleInitialIntent(intent)
        drain()
        shadowOf(Looper.getMainLooper()).idle()
        assertEquals(owner, handler.currentOwnerId())
        assertEquals(delivered, scripts.size)

        // A real second share of the same content remains a distinct user action.
        handler.handleShareIntent(textIntent("same initial contents"))
        assertNotEquals(owner, handler.currentOwnerId())
    }

    @Test fun newIntentBeforeFirstLoadPreventsReplayOfTheOlderActivityIntent() {
        handler.handleShareIntent(textIntent("new contents"))
        val owner = handler.currentOwnerId()
        handler.handleInitialIntent(textIntent("old contents"))
        drain()
        shadowOf(Looper.getMainLooper()).idle()
        assertEquals(owner, handler.currentOwnerId())
        assertEquals("new contents", handler.readPendingPayload()?.getString("text"))
    }

    @Test fun destroyInvalidatesAnExtractionAlreadyQueuedForUiPublication() {
        handler.handleShareIntent(textIntent("late contents"))
        val owner = requireNotNull(handler.currentOwnerId())
        drain()
        val delivered = scripts.size
        handler.destroy()
        shadowOf(Looper.getMainLooper()).idle()
        assertFalse(handler.isCurrentOwner(owner))
        assertNull(handler.currentOwnerId())
        assertEquals(delivered, scripts.size)
        handler.handleShareIntent(textIntent("after destruction"))
        assertNull(handler.currentOwnerId())
    }

    @Test fun rejectedExtractionPublishesFailureInsteadOfLeavingReceivingForever() {
        executor.shutdownNow()
        handler.handleShareIntent(textIntent("cannot queue"))
        assertEquals(2, scripts.size)
        assertEquals("failed", handler.readPendingPayload()?.getString("state"))
    }

    @Test fun ignoredColdStartNotificationsDoNotConsumeTheReadyPayload() {
        handler.handleShareIntent(textIntent("cold start"))
        val receiving = requireNotNull(handler.readPendingPayload())
        drain()
        shadowOf(Looper.getMainLooper()).idle()
        val ready = requireNotNull(handler.readPendingPayload())
        assertEquals("ready", ready.getString("state"))
        assertEquals("cold start", ready.getString("text"))
        handler.acknowledgePayload(AcknowledgeSharePayloadArgs().apply {
            ownerId = receiving.getString("intentId")
            payloadRevision = receiving.getLong("payloadRevision")
        })
        assertEquals(ready.toString(), handler.readPendingPayload()?.toString())
        handler.acknowledgePayload(AcknowledgeSharePayloadArgs().apply {
            ownerId = ready.getString("intentId")
            payloadRevision = ready.getLong("payloadRevision")
            settled = true
        })
        assertNull(handler.readPendingPayload())
        assertTrue(handler.isCurrentOwner(ready.getString("intentId")))
    }

    @Test fun replacementHandlerDoesNotDeleteAFileAlreadyClaimedByAnEarlierWorker() {
        val shared = File(activity.cacheDir, "shared").apply { mkdirs() }
        val claimedSource = File(shared, "claimed-source").apply { writeText("still being ingested") }
        val replacementExecutor = Executors.newSingleThreadExecutor()
        val replacement = ShareIntentHandler(activity, replacementExecutor) { null }
        try {
            replacementExecutor.submit { }.get(5, TimeUnit.SECONDS)
            assertEquals("still being ingested", claimedSource.readText())
        } finally {
            replacement.destroy()
            replacementExecutor.shutdownNow()
        }
    }

    @Test fun delayedOlderInitializationCannotEraseTheReplacementHandlersStaging() {
        val otherActivity = Robolectric.buildActivity(ShareActivity::class.java).create().get().apply {
            shareCache = temporary.newFolder("interleaved-cache")
        }
        val olderExecutor = Executors.newSingleThreadExecutor()
        val newerExecutor = Executors.newSingleThreadExecutor()
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        olderExecutor.execute {
            entered.countDown()
            release.await()
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        val older = ShareIntentHandler(otherActivity, olderExecutor) { null }
        val newer = ShareIntentHandler(otherActivity, newerExecutor) { null }
        try {
            newerExecutor.submit { }.get(5, TimeUnit.SECONDS)
            val shared = File(otherActivity.cacheDir, "shared").apply { mkdirs() }
            val current = File(shared, "new-owner-staging").apply { writeText("current contents") }
            release.countDown()
            olderExecutor.submit { }.get(5, TimeUnit.SECONDS)
            assertEquals("current contents", current.readText())
        } finally {
            release.countDown()
            older.destroy()
            newer.destroy()
            olderExecutor.shutdownNow()
            newerExecutor.shutdownNow()
        }
    }
}
