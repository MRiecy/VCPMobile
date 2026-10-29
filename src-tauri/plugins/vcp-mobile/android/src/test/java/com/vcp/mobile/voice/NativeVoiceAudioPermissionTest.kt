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
    private val audioSession = mockk<VoiceAudioSession>(relaxed = true)
    private val output = mockk<NativeVoiceOutput>(relaxed = true)

    @Before fun setUp() {
        val activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        shadowOf(activity.application).denyPermissions(Manifest.permission.RECORD_AUDIO)
        every { audioSession.hasCall("call-one") } returns true
        every { output.snapshot() } returns NativeVoiceOutput.Snapshot()
        owner = NativeVoiceAudio(activity, { alias, invoke, callback ->
            assertEquals("microphone", alias)
            assertEquals("onVoiceAudioPermissionResult", callback)
            permissionRequests.add(invoke)
        }, { eventsReady }, { payload ->
            events.add(payload)
            true
        }, audioSession, output)
        owner.connectVoiceDocument(document("1"))
    }

    @After fun tearDown() { owner.destroy() }

    private fun start(id: String, document: String = "1"): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(StartVoiceAudioArgs::class.java) } returns StartVoiceAudioArgs().apply {
            sessionId = id; callId = "call-one"; documentId = document
        }
    }
    private fun document(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(VoiceDocumentArgs::class.java) } returns VoiceDocumentArgs().apply { documentId = id }
    }
    private fun stop(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(StopVoiceAudioArgs::class.java) } returns StopVoiceAudioArgs().apply { sessionId = id }
    }
    private fun release(id: String): Invoke = mockk<Invoke>(relaxed = true).also { invoke ->
        every { invoke.parseArgs(ReleaseVoiceSegmentsArgs::class.java) } returns ReleaseVoiceSegmentsArgs().apply { sessionId = id }
    }

    @Test fun releaseBeforeStopIsFinalForThatCaptureAndDoesNotTouchTheNextSession() {
        val old = start("old-segments")
        owner.startVoiceAudio(old)
        owner.releaseVoiceSegments(release("old-segments"))
        owner.stopVoiceAudio(stop("old-segments"))
        owner.onPermissionResult(old)
        val next = start("next-segments")
        owner.startVoiceAudio(next)
        owner.releaseVoiceSegments(release("old-segments"))
        verify(exactly = 0) { next.reject(any<String>()) }
        val late = start("old-segments")
        owner.startVoiceAudio(late)
        verify(exactly = 1) { late.reject("cancelled:voice_session_inactive") }
        assertEquals(2, permissionRequests.size)
    }

    @Test fun absentEventChannelDoesNotRequestPermissionOrAcknowledgeCapture() {
        eventsReady = false
        val start = start("no-event-channel")
        owner.startVoiceAudio(start)
        verify(exactly = 1) { start.reject("voice_events_unavailable") }
        verify(exactly = 0) { start.resolve() }
        assertEquals(0, permissionRequests.size)
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
        verify(exactly = 1) { stop.resolve(match<JSObject> { it.getLong("sampleIndex") == 0L && it.getLong("audioEpoch") == 1L }) }
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
        verify(exactly = 0) { audioSession.endCall(any()) }
        verify(exactly = 0) { output.endCall(any(), any()) }
    }

    @Test fun endingACallStopsCaptureNowButKeepsItsRouteUntilOutputRelease() {
        var outputReleased: (() -> Unit)? = null
        every { output.endCall("call-one", any()) } answers { outputReleased = secondArg(); Unit }
        val capture = start("ending-call")
        owner.startVoiceAudio(capture)
        val ended = mockk<Invoke>(relaxed = true)
        every { ended.parseArgs(VoiceCallArgs::class.java) } returns VoiceCallArgs().apply { callId = "call-one" }
        owner.endVoiceCall(ended)
        verify(exactly = 1) { capture.reject("cancelled:call_ended") }
        verify(exactly = 0) { audioSession.endCall(any()) }
        verify(exactly = 0) { ended.resolve() }
        checkNotNull(outputReleased).invoke()
        verify(exactly = 1) { audioSession.endCall("call-one") }
        verify(exactly = 1) { ended.resolve() }
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

    @Test fun leavingDuringPermissionWaitStopsOnlyCaptureAndTheCallCanOpenOnANewDocument() {
        val old = start("departing-input")
        owner.startVoiceAudio(old)
        owner.invalidateVoiceDocument(document("1"))
        owner.onPermissionResult(old)
        verify(exactly = 1) { old.reject("cancelled:document_left") }
        verify(exactly = 0) { old.resolve() }
        verify(exactly = 0) { audioSession.endCall(any()) }
        verify(exactly = 0) { output.endCall(any(), any()) }

        val retired = document("1")
        owner.connectVoiceDocument(retired)
        verify(exactly = 1) { retired.reject("cancelled:voice_document") }
        val late = start("late-old-document")
        owner.startVoiceAudio(late)
        verify(exactly = 1) { late.reject("cancelled:voice_document") }

        owner.connectVoiceDocument(document("2"))
        val next = start("replacement-input", "2")
        owner.startVoiceAudio(next)
        owner.invalidateVoiceDocument(document("1"))
        owner.onPermissionResult(old)
        verify(exactly = 0) { next.reject(any<String>()) }
        assertEquals(listOf(old, next), permissionRequests)
        assertEquals("document_left", events.single().getString("reason"))
    }

    @Test fun connectingTheSameDocumentIsReadOnlyButANewerDocumentStopsItsPredecessor() {
        val old = start("still-current")
        owner.startVoiceAudio(old)
        owner.connectVoiceDocument(document("1"))
        verify(exactly = 0) { old.reject(any<String>()) }
        assertEquals(0, events.size)

        owner.connectVoiceDocument(document("2"))
        verify(exactly = 1) { old.reject("cancelled:document_replaced") }
        assertEquals("document_replaced", events.single().getString("reason"))
        verify(exactly = 0) { audioSession.endCall(any()) }
        verify(exactly = 0) { output.endCall(any(), any()) }
    }

    @Test fun invalidationBeforeConnectRetiresTheTicketAndStopsOlderCapture() {
        val old = start("before-new-connect")
        owner.startVoiceAudio(old)
        owner.invalidateVoiceDocument(document("3"))
        owner.onPermissionResult(old)
        verify(exactly = 1) { old.reject("cancelled:document_left") }
        for (id in listOf("1", "2", "3")) {
            val late = document(id)
            owner.connectVoiceDocument(late)
            verify(exactly = 1) { late.reject("cancelled:voice_document") }
        }
        owner.connectVoiceDocument(document("4"))
        val next = start("after-retired-connect", "4")
        owner.startVoiceAudio(next)
        verify(exactly = 0) { next.reject(any<String>()) }
        assertEquals(listOf(old, next), permissionRequests)
    }

    @Test fun documentComparisonUsesTheFullUnsignedTicketAndRejectsNoncanonicalIds() {
        for (id in listOf("0", "01", "+1", "-1", "18446744073709551616")) {
            val malformed = document(id)
            owner.connectVoiceDocument(malformed)
            verify(exactly = 1) { malformed.reject("invalid_voice_document_id") }
        }
        owner.connectVoiceDocument(document("18446744073709551614"))
        owner.invalidateVoiceDocument(document("18446744073709551615"))
        val retired = document("18446744073709551615")
        owner.connectVoiceDocument(retired)
        verify(exactly = 1) { retired.reject("cancelled:voice_document") }
        val late = start("full-width-ticket", "18446744073709551614")
        owner.startVoiceAudio(late)
        verify(exactly = 1) { late.reject("cancelled:voice_document") }
        assertEquals(0, permissionRequests.size)
    }
}
