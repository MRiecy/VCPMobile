package com.vcp.mobile.voice

import android.app.Activity
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.media.MediaRouter
import com.google.common.collect.ImmutableList
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28])
class VoiceAudioSessionTest {
    private lateinit var activity: Activity
    private lateinit var session: VoiceAudioSession

    @Before fun setUp() {
        mockkConstructor(VoiceAudioRouting::class)
        every { anyConstructed<VoiceAudioRouting>().open() } just Runs
        every { anyConstructed<VoiceAudioRouting>().close() } just Runs
        every { anyConstructed<VoiceAudioRouting>().awaitReady(any()) } just Runs
        activity = Robolectric.buildActivity(Activity::class.java).setup().get()
        session = VoiceAudioSession(activity)
    }
    @After fun tearDown() { session.destroy(); unmockkAll() }

    private fun port(number: Int, portType: Int, deviceAddress: String = ""): AudioDeviceInfo = mockk {
        every { id } returns number
        every { type } returns portType
        every { address } returns deviceAddress
        every { productName } returns "device-$number"
    }

    @Test @Config(sdk = [33]) fun selectedBluetoothMicReconfiguresLiveOutputAndRequiresItsMatchingSource() {
        unmockkConstructor(VoiceAudioRouting::class)
        val audio = activity.getSystemService(AudioManager::class.java)
        val mic = port(2, AudioDeviceInfo.TYPE_BUILTIN_MIC)
        val media = port(10, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP)
        val a = port(11, AudioDeviceInfo.TYPE_BLUETOOTH_SCO, "AA")
        val b = port(12, AudioDeviceInfo.TYPE_BLUETOOTH_SCO, "BB")
        val inputA = port(21, AudioDeviceInfo.TYPE_BLUETOOTH_SCO, "AA")
        val inputB = port(22, AudioDeviceInfo.TYPE_BLUETOOTH_SCO, "BB")
        shadowOf(audio).setInputDevices(listOf(mic, inputA, inputB))
        shadowOf(audio).setOutputDevices(listOf(media, a, b))
        shadowOf(audio).setAvailableCommunicationDevices(listOf(a, b))
        val attributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
        shadowOf(audio).setAudioDevicesForAttributes(attributes, ImmutableList.of(media))
        session.beginCall("switch")
        val initial = session.acquireInput("switch", {}, {})
        val output = session.acquireOutput {}
        val original = output.awaitReady { false }
        initial.close()
        var quiet: (() -> Unit)? = null
        session.setOutputBarrier { quiet = it }
        val selected = session.acquireInput("switch", {}, {}, "bluetooth:12")
        assertNotNull(quiet)
        assertFalse(output.isCurrent(original))
        quiet!!.invoke()
        val communication = output.awaitReady { false }
        assertEquals(AudioAttributes.USAGE_VOICE_COMMUNICATION, communication.usage)
        assertEquals(b.id, communication.bluetoothSink?.id)
        assertTrue(communication.acceptsOutput(b))
        assertFalse(communication.acceptsOutput(a))
        assertTrue(selected.input!!.matches(audio, inputB))
        assertFalse(selected.input.matches(audio, inputA))
        assertFalse(selected.input.matches(audio, mic))
        selected.close()
        session.setOutputBarrier { it() }
        val speaker = port(1, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER)
        shadowOf(audio).setAudioDevicesForAttributes(attributes, ImmutableList.of(speaker))
        val builtin = session.acquireInput("switch", {}, {}, "builtin")
        assertTrue(builtin.input!!.matches(audio, mic))
        assertEquals(AudioManager.MODE_NORMAL, audio.mode)
        assertFalse(output.isCurrent(communication))
        shadowOf(audio).setAudioDevicesForAttributes(attributes, ImmutableList.of(media))
        shadowOf(android.os.Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(100))
        assertEquals(AudioAttributes.USAGE_MEDIA, output.awaitReady { false }.usage)
        assertEquals(AudioManager.MODE_NORMAL, audio.mode)
        assertFalse(runCatching { session.acquireInput("switch", {}, {}, "bluetooth:99") }.isSuccess)
    }

    @Test fun legacyScoOffersOnlyOneAddressableHeadset() {
        val audio = activity.getSystemService(AudioManager::class.java)
        val a = port(1, AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
        val b = port(2, AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
        val input = port(3, AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
        shadowOf(audio).setIsBluetoothScoAvailableOffCall(true)
        shadowOf(audio).setInputDevices(listOf(input))
        shadowOf(audio).setOutputDevices(listOf(a, b))
        assertTrue(VoiceInputDevice.available(audio).none { it.kind == "bluetooth" })
        shadowOf(audio).setOutputDevices(listOf(a))
        val choice = VoiceInputDevice.available(audio).single()
        assertTrue(choice.matches(audio, input))
        shadowOf(audio).setOutputDevices(listOf(b))
        assertFalse(choice.matches(audio, input))
    }

    @Test @Config(sdk = [33]) fun explicitPhoneMicDoesNotAcquireACommunicationOnlyBluetoothAccessory() {
        unmockkConstructor(VoiceAudioRouting::class)
        val audio = activity.getSystemService(AudioManager::class.java)
        val mic = port(2, AudioDeviceInfo.TYPE_BUILTIN_MIC)
        val sco = port(4, AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
        val speaker = port(5, AudioDeviceInfo.TYPE_BUILTIN_SPEAKER)
        shadowOf(audio).setInputDevices(listOf(mic))
        shadowOf(audio).setAvailableCommunicationDevices(listOf(sco, speaker))
        shadowOf(audio).setAudioDevicesForAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build(), ImmutableList.of(speaker))
        session.beginCall("phone")
        var route = ""
        val input = session.acquireInput("phone", { route = it }, {}, "builtin")
        assertEquals(AudioAttributes.USAGE_VOICE_COMMUNICATION, input.awaitReady { false }.usage)
        assertEquals("speaker", route)
        assertTrue(input.input!!.matches(audio, mic))
    }

    @Test @Config(sdk = [33]) fun bluetoothMediaCallUsesTheSelectedHeadphonesAcrossMicMute() {
        unmockkConstructor(VoiceAudioRouting::class)
        val audio = activity.getSystemService(AudioManager::class.java)
        val headset = mockk<AudioDeviceInfo> {
            every { type } returns AudioDeviceInfo.TYPE_BLUETOOTH_A2DP
            every { id } returns 24
        }
        val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
        shadowOf(audio).setAudioDevicesForAttributes(media, ImmutableList.of(headset))
        session.beginCall("headphones")
        val input = session.acquireInput("headphones", {}, {})
        val output = session.acquireOutput {}
        val profile = input.awaitReady { false }
        assertEquals(AudioAttributes.USAGE_MEDIA, profile.usage)
        assertEquals(headset.id, profile.headphones?.id)
        assertFalse(profile.legacySco)
        assertEquals(AudioManager.MODE_NORMAL, audio.mode)
        input.close()
        assertEquals(profile, output.awaitReady { false })
    }

    @Test fun legacyBluetoothMediaSelectionDoesNotStartSco() {
        unmockkConstructor(VoiceAudioRouting::class)
        val audio = activity.getSystemService(AudioManager::class.java)
        val headset = mockk<AudioDeviceInfo> {
            every { type } returns AudioDeviceInfo.TYPE_BLUETOOTH_A2DP
            every { id } returns 31
        }
        shadowOf(audio).setOutputDevices(listOf(headset))
        shadowOf(activity.getSystemService(MediaRouter::class.java)).addBluetoothRoute()
        session.beginCall("legacy-headphones")
        val profile = session.acquireInput("legacy-headphones", {}, {}).awaitReady { false }
        assertEquals(AudioAttributes.USAGE_MEDIA, profile.usage)
        assertEquals(headset.id, profile.headphones?.id)
        assertFalse(profile.legacySco)
        assertEquals(AudioManager.MODE_NORMAL, audio.mode)
    }

    @Test @Config(sdk = [33]) fun connectedHeadphonesDoNotOverrideAnExplicitMediaSpeaker() {
        unmockkConstructor(VoiceAudioRouting::class)
        val audio = activity.getSystemService(AudioManager::class.java)
        val headset = mockk<AudioDeviceInfo> { every { type } returns AudioDeviceInfo.TYPE_BLUETOOTH_A2DP }
        val handsFree = mockk<AudioDeviceInfo> {
            every { type } returns AudioDeviceInfo.TYPE_BLUETOOTH_SCO
            every { id } returns 12
        }
        val speaker = mockk<AudioDeviceInfo> {
            every { type } returns AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
            every { id } returns 1
        }
        shadowOf(audio).setOutputDevices(listOf(headset, speaker))
        shadowOf(audio).setAvailableCommunicationDevices(listOf(handsFree, speaker))
        val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build()
        shadowOf(audio).setAudioDevicesForAttributes(media, ImmutableList.of(speaker))
        session.beginCall("speaker")
        var selected = ""
        val profile = session.acquireInput("speaker", { selected = it }, {}).awaitReady { false }
        assertEquals(AudioAttributes.USAGE_VOICE_COMMUNICATION, profile.usage)
        assertEquals("speaker", selected)
    }

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
