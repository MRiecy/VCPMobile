package com.vcp.mobile.voice

import android.Manifest
import android.app.Activity
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import io.mockk.every
import io.mockk.mockk
import io.mockk.verify
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class NativeVoiceAudioPermissionTest {
    private lateinit var owner: NativeVoiceAudio
    private val permissionRequests = mutableListOf<Invoke>()
    private val events = mutableListOf<JSObject>()
    private var eventsReady = true

    @Before fun setUp() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        shadowOf(activity.application).denyPermissions(Manifest.permission.RECORD_AUDIO)
        owner = NativeVoiceAudio(activity, { alias, invoke, callback ->
            assertEquals("microphone", alias)
            assertEquals("onVoiceAudioPermissionResult", callback)
            permissionRequests.add(invoke)
        }, { eventsReady }, { payload ->
            events.add(payload)
            true
        })
    }

    @After fun tearDown() { owner.destroy() }

    private fun start(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(StartVoiceAudioArgs::class.java) } returns StartVoiceAudioArgs().apply { sessionId = id }
    }
    private fun stop(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(StopVoiceAudioArgs::class.java) } returns StopVoiceAudioArgs().apply { sessionId = id }
    }
    private fun probe(id: String, revision: Long, enabled: Boolean): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(SetVoiceProbeArgs::class.java) } returns SetVoiceProbeArgs().apply {
            sessionId = id; probeRevision = revision; this.enabled = enabled
        }
    }

    @Test fun absentEventChannelDoesNotRequestPermissionOrAcknowledgeCapture() {
        eventsReady = false
        val start = start("no-event-channel")
        owner.startVoiceAudio(start)
        verify(exactly = 1) { start.reject("voice_events_unavailable") }
        verify(exactly = 0) { start.resolve() }
        assertEquals(0, permissionRequests.size)
    }

    @Test fun oldProbeIntentCannotOverrideANewerRevisionAndRepeatedRevisionIsIdempotent() {
        owner.startVoiceAudio(start("probe-order"))
        val newer = probe("probe-order", 2, false)
        owner.setVoiceProbe(newer)
        val stale = probe("probe-order", 1, true)
        owner.setVoiceProbe(stale)
        val repeated = probe("probe-order", 2, false)
        owner.setVoiceProbe(repeated)
        verify(exactly = 1) { newer.resolve() }
        verify(exactly = 1) { stale.reject("cancelled:stale_probe_revision") }
        verify(exactly = 1) { repeated.resolve() }
        assertEquals(1, permissionRequests.size)
    }

    @Test fun cancellingPermissionWaitSettlesOnceAndCannotStartFromTheLateCallback() {
        val start = start("permission-wait")
        owner.startVoiceAudio(start)
        assertEquals(listOf(start), permissionRequests)
        val stop = stop("permission-wait")
        owner.stopVoiceAudio(stop)
        owner.onPermissionResult(start)
        owner.onPermissionResult(start)
        verify(exactly = 1) { start.reject("cancelled:cancelled") }
        verify(exactly = 1) { stop.resolve() }
        verify(exactly = 0) { start.resolve() }
        assertEquals(1, events.size)
        assertEquals("stopped", events.single().getString("kind"))
        assertEquals("permission-wait", events.single().getString("sessionId"))
    }

    @Test fun stopBeforeStartCannotAcquireTheMicrophoneLater() {
        owner.stopVoiceAudio(stop("cancel-before-start"))
        val late = start("cancel-before-start")
        owner.startVoiceAudio(late)
        verify(exactly = 1) { late.reject("cancelled:voice_session_inactive") }
        assertEquals(0, permissionRequests.size)
    }

    @Test fun backgroundStopsPendingCaptureAndForegroundDoesNotReacquire() {
        val start = start("background-wait")
        owner.startVoiceAudio(start)
        owner.onBackground()
        owner.onForeground()
        owner.onPermissionResult(start)
        verify(exactly = 1) { start.reject("cancelled:background") }
        verify(exactly = 0) { start.resolve() }
        assertEquals(1, permissionRequests.size)
        assertEquals("background", events.single().getString("reason"))
    }

    @Test fun destructionRejectsNewRequestsAndIgnoresOldPermissionDelivery() {
        val start = start("destroy-wait")
        owner.startVoiceAudio(start)
        owner.destroy()
        owner.onPermissionResult(start)
        val late = start("after-destroy")
        owner.startVoiceAudio(late)
        verify(exactly = 1) { start.reject("cancelled:destroyed") }
        verify(exactly = 1) { late.reject("cancelled:voice_session_inactive") }
        assertEquals(1, events.size)
    }
}
