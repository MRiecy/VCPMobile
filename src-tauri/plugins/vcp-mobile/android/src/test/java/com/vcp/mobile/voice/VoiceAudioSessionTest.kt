package com.vcp.mobile.voice

import android.app.Activity
import android.media.AudioAttributes
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class VoiceAudioSessionTest {
    private lateinit var session: VoiceAudioSession

    @Before fun setUp() {
        mockkConstructor(VoiceAudioRouting::class)
        every { anyConstructed<VoiceAudioRouting>().open() } just Runs
        every { anyConstructed<VoiceAudioRouting>().close() } just Runs
        every { anyConstructed<VoiceAudioRouting>().awaitReady(any()) } just Runs
        session = VoiceAudioSession(Robolectric.buildActivity(Activity::class.java).setup().get())
    }
    @After fun tearDown() { session.destroy(); unmockkAll() }

    @Test fun callReservationIsLogicalAndMicrophoneMuteKeepsTheCommunicationRoute() {
        session.beginCall("call-one")
        verify(exactly = 0) { anyConstructed<VoiceAudioRouting>().open() }
        val input = session.acquireInput("call-one", {}, {})
        val output = session.acquireOutput {}
        val profile = output.awaitReady { false }
        assertEquals(AudioAttributes.USAGE_VOICE_COMMUNICATION, profile.usage)
        input.close()
        verify(exactly = 0) { anyConstructed<VoiceAudioRouting>().close() }
        assertTrue(output.isCurrent(profile))
        output.close()
        verify(exactly = 0) { anyConstructed<VoiceAudioRouting>().close() }
        session.endCall("call-one")
        verify(exactly = 1) { anyConstructed<VoiceAudioRouting>().close() }
    }

    @Test fun standaloneOutputUsesMediaAndCallBoundaryWaitsForOutputQuiescence() {
        val output = session.acquireOutput {}
        val media = output.awaitReady { false }
        assertEquals(AudioAttributes.USAGE_MEDIA, media.usage)
        var quiesced: (() -> Unit)? = null
        session.setOutputBarrier { quiesced = it }
        session.beginCall("call-two")
        assertNotNull(quiesced)
        verify(exactly = 1) { anyConstructed<VoiceAudioRouting>().open() }
        quiesced!!.invoke()
        val communication = output.awaitReady { false }
        assertEquals(AudioAttributes.USAGE_VOICE_COMMUNICATION, communication.usage)
        assertFalse(output.isCurrent(media))
        session.setOutputBarrier { it() }
        output.close()
        session.endCall("call-two")
    }

    @Test fun oldEndAndEndBeforeBeginCannotTakeOverANewerCall() {
        session.endCall("cancelled-call")
        assertTrue(runCatching { session.beginCall("cancelled-call") }.isFailure)
        session.beginCall("old-call")
        session.endCall("old-call")
        session.beginCall("new-call")
        session.endCall("old-call")
        assertTrue(session.hasCall("new-call"))
    }
}
