package com.vcp.mobile.lifecycle

import android.app.Activity
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.PluginTaskRuntime
import io.mockk.every
import io.mockk.mockk
import io.mockk.slot
import io.mockk.verify
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [31])
class NativeEventsNetworkTest {
    private val activity = mockk<Activity>()
    private val manager = mockk<ConnectivityManager>(relaxed = true)
    private val callback = slot<ConnectivityManager.NetworkCallback>()
    private val first = mockk<Network>()
    private val second = mockk<Network>()
    private lateinit var events: NativeEvents

    @Before fun setUp() {
        every { activity.getSystemService(Context.CONNECTIVITY_SERVICE) } returns manager
        every { activity.runOnUiThread(any()) } answers { firstArg<Runnable>().run() }
        every { manager.registerDefaultNetworkCallback(capture(callback), any<Handler>()) } answers { }
        every { manager.activeNetwork } returns first
        val runtime = mockk<PluginTaskRuntime>(relaxed = true)
        events = NativeEvents(activity, runtime) { JSObject() }
    }

    private fun subscribe(): List<Boolean> {
        val statuses = mutableListOf<Boolean>()
        val subscriber = mockk<Channel>()
        every { subscriber.send(any<JSObject>()) } answers {
            val event = firstArg<JSObject>()
            if (event.getString("event") == "network") {
                statuses.add(event.getJSONObject("payload").getBoolean("connected"))
            }
        }
        val invoke = mockk<Invoke>(relaxed = true)
        every { invoke.parseArgs(RegisterNativeEventsArgs::class.java) } returns RegisterNativeEventsArgs().apply { channel = subscriber }
        events.registerNativeEvents(invoke)
        verify { invoke.resolve() }
        return statuses
    }

    // Unit tests compile against the project's Android 9 Robolectric dependency.
    // Exercise the real NetworkCallback entry on the API 31 runtime.
    private fun blocked(network: Network, value: Boolean) {
        ConnectivityManager.NetworkCallback::class.java
            .getMethod("onBlockedStatusChanged", Network::class.java, Boolean::class.javaPrimitiveType)
            .invoke(callback.captured, network, value)
    }

    @Test fun unblockingTheSameNetworkWakesTheReplacementSubscriberOnce() {
        val old = subscribe()
        callback.captured.onAvailable(first)
        blocked(first, false)
        blocked(first, true)
        blocked(first, true)
        val current = subscribe()
        blocked(first, false)
        blocked(first, false)
        assertEquals(listOf(true, false), old)
        assertEquals(listOf(false, true), current)
        verify(exactly = 1) { manager.registerDefaultNetworkCallback(any(), any<Handler>()) }
        verify(exactly = 1) { manager.activeNetwork }
        verify(exactly = 0) { manager.getNetworkCapabilities(any()) }
    }

    @Test fun aDefaultHandoverIgnoresLateBlockAndLossForThePreviousNetwork() {
        val statuses = subscribe()
        callback.captured.onAvailable(second)
        blocked(first, true)
        callback.captured.onLost(first)
        blocked(second, false)
        callback.captured.onLost(second)
        blocked(second, false)
        assertEquals(listOf(true, true, false), statuses)
    }

    @Test fun aNewDefaultAlreadyBlockedForTheUidDoesNotWakeRecovery() {
        every { manager.activeNetwork } returns null
        val statuses = subscribe()
        callback.captured.onAvailable(second)
        blocked(second, true)
        assertEquals(listOf(false, false), statuses)
        blocked(second, false)
        assertEquals(listOf(false, false, true), statuses)
    }

    @Test @Config(sdk = [28])
    fun beforeAndroidQAvailabilityAlonePublishesChangesFromAnOfflineSnapshot() {
        every { manager.activeNetwork } returns null
        val statuses = subscribe()
        callback.captured.onAvailable(first)
        callback.captured.onAvailable(first)
        callback.captured.onAvailable(second)
        callback.captured.onLost(first)
        callback.captured.onLost(second)
        assertEquals(listOf(false, true, true, false), statuses)
    }
}
