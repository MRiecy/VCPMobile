package com.vcp.mobile

import android.os.Bundle
import android.content.Intent
import android.view.WindowManager
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.cli.CliProcessHost
import com.vcp.mobile.cli.CliProcessHostOwner
import com.vcp.mobile.cli.CliPtyHostOwner
import com.vcp.mobile.haptics.TriggerHapticFeedbackArgs
import com.vcp.mobile.root.NativeRootAccess
import com.vcp.mobile.service.ForegroundGuardian
import com.vcp.mobile.webview.WindowSnapshot
import io.mockk.*
import org.junit.After
import org.junit.Before
import org.junit.Test
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.android.controller.ActivityController

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class VcpMobilePluginLifecycleTest {
    class OwnerActivity : AppCompatActivity() {
        override fun onCreate(savedInstanceState: Bundle?) {
            setTheme(androidx.appcompat.R.style.Theme_AppCompat)
            super.onCreate(savedInstanceState)
        }
    }

    private lateinit var owner: OwnerActivity
    private lateinit var ownerController: ActivityController<OwnerActivity>
    private lateinit var plugin: VcpMobilePlugin

    @Before fun setUp() {
        // Keep unrelated process-wide CLI and root execution out of this Activity test.
        mockkObject(CliProcessHostOwner, CliPtyHostOwner)
        val processHost = mockk<CliProcessHost>(relaxed = true)
        every { CliProcessHostOwner.get(any()) } returns processHost
        every { CliPtyHostOwner.get(processHost) } returns mockk(relaxed = true)
        mockkConstructor(NativeRootAccess::class)
        every { anyConstructed<NativeRootAccess>().startOomScoreGuard() } just Runs
        every { anyConstructed<NativeRootAccess>().destroy() } just Runs
        mockkConstructor(WindowSnapshot::class)
        every { anyConstructed<WindowSnapshot>().destroy() } just Runs
        ownerController = Robolectric.buildActivity(OwnerActivity::class.java).create()
        owner = ownerController.get()
        plugin = VcpMobilePlugin(owner)
    }

    @After fun tearDown() {
        if (::plugin.isInitialized) plugin.onDestroy(owner)
        ForegroundGuardian.setScreenStateListener(null)
        ForegroundGuardian.releaseAllLocks()
        unmockkAll()
    }

    private fun hapticRequest(): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(TriggerHapticFeedbackArgs::class.java) } returns
            TriggerHapticFeedbackArgs().apply { type = "view"; effect = "keyboard_tap" }
    }

    @Test fun unrelatedActivityDestructionDoesNotCloseTheOwnerPlugin() {
        val unrelated = Robolectric.buildActivity(OwnerActivity::class.java).create().get()
        plugin.onDestroy(unrelated)

        val invoke = hapticRequest()
        plugin.triggerHapticFeedback(invoke)

        verify(exactly = 1) { invoke.resolve(any<JSObject>()) }
        verify(exactly = 0) { invoke.reject(any<String>()) }
        verify(exactly = 0) { anyConstructed<NativeRootAccess>().destroy() }
    }

    @Test fun ownerDestructionRunsOnceAndDoesNotReattachALateWebView() {
        plugin.onDestroy(owner)
        plugin.onDestroy(owner)
        val lateView = mockk<WebView>(relaxed = true)
        plugin.load(lateView)
        val lateIntent = Intent().putExtra("topicId", "topic").putExtra("ownerId", "agent")
            .putExtra("ownerType", "agent")
        plugin.onNewIntent(lateIntent)

        val invoke = hapticRequest()
        plugin.triggerHapticFeedback(invoke)

        verify(exactly = 1) { anyConstructed<NativeRootAccess>().destroy() }
        verify(exactly = 1) { invoke.reject("插件已在销毁过程中") }
        verify { lateView wasNot Called }
        assertEquals("topic", lateIntent.getStringExtra("topicId"))
    }

    @Test fun originalActivityCallbackCleansOldPluginWithoutDetachingReplacementListener() {
        val replacementOwner = Robolectric.buildActivity(OwnerActivity::class.java).create().get()
        val replacement = VcpMobilePlugin(replacementOwner)
        try {
            // The registry now forwards the old Activity's destruction to the new plugin.
            replacement.onDestroy(owner)
            // Application still notifies the original plugin of its own Activity.
            ownerController.destroy()
            verify(exactly = 1) { anyConstructed<NativeRootAccess>().destroy() }

            ForegroundGuardian.acquire(
                replacementOwner, "sync:replacement", ForegroundGuardian.PRIORITY_SYNC,
                "replacement", screenKeepOn = true, timeoutMs = 0,
            )
            assertTrue(replacementOwner.window.attributes.flags and
                WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON != 0)
            val invoke = hapticRequest()
            replacement.triggerHapticFeedback(invoke)
            verify(exactly = 1) { invoke.resolve(any<JSObject>()) }
        } finally {
            replacement.onDestroy(replacementOwner)
        }
    }

    @Test fun failingOwnerCleanupDoesNotPreventLaterOwnersFromBeingReleased() {
        every { anyConstructed<WindowSnapshot>().destroy() } throws
            IllegalStateException("receiver already disappeared")

        plugin.onDestroy(owner)

        verify(exactly = 1) { anyConstructed<NativeRootAccess>().destroy() }
        val invoke = hapticRequest()
        plugin.triggerHapticFeedback(invoke)
        verify(exactly = 1) { invoke.reject("插件已在销毁过程中") }
    }
}
