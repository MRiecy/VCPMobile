package com.vcp.mobile.haptics

import android.app.Activity
import android.content.Context
import android.media.AudioAttributes
import android.os.Build
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View
import android.view.Window
import androidx.test.core.app.ApplicationProvider
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.fasterxml.jackson.annotation.JsonAutoDetect
import com.fasterxml.jackson.annotation.PropertyAccessor
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.databind.ObjectMapper
import com.vcp.mobile.runtime.PluginTaskRuntime
import io.mockk.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class NativeHapticsTest {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private lateinit var activity: Activity
    private lateinit var view: View
    private lateinit var vibrator: Vibrator
    private lateinit var runtime: PluginTaskRuntime
    private lateinit var haptics: NativeHaptics
    // Match the locked Tauri PluginManager's argument mapper rather than mocking parseArgs.
    private val mapper = ObjectMapper()
        .disable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES)
        .enable(DeserializationFeature.FAIL_ON_NULL_FOR_PRIMITIVES)
        .setVisibility(PropertyAccessor.FIELD, JsonAutoDetect.Visibility.ANY)

    @Before fun setUp() {
        activity = mockk(relaxed = true)
        view = mockk(relaxed = true)
        vibrator = mockk(relaxed = true)
        val window = mockk<Window>()
        every { activity.window } returns window
        every { window.decorView } returns view
        every { activity.contentResolver } returns context.contentResolver
        every { activity.runOnUiThread(any()) } answers { firstArg<Runnable>().run() }
        every { activity.getSystemService(Context.VIBRATOR_SERVICE) } returns vibrator
        if (Build.VERSION.SDK_INT >= 31) {
            val manager = mockk<VibratorManager>()
            every { manager.defaultVibrator } returns vibrator
            every { activity.getSystemService("vibrator_manager") } returns manager
        }
        every { vibrator.hasVibrator() } returns true
        every { vibrator.hasAmplitudeControl() } returns true
        runtime = PluginTaskRuntime()
        haptics = NativeHaptics(activity, runtime)
    }

    @After fun tearDown() {
        runtime.beginDestroy()
        runtime.shutdown()
        unmockkAll()
    }

    private fun touchSetting(enabled: Boolean) {
        @Suppress("DEPRECATION")
        Settings.System.putInt(context.contentResolver, Settings.System.HAPTIC_FEEDBACK_ENABLED, if (enabled) 1 else 0)
    }

    private fun trigger(json: String): JSObject {
        val responses = mutableListOf<Pair<Long, String>>()
        val invoke = Invoke(
            id = 1L, command = "triggerHapticFeedback", callback = 2L, error = 3L,
            sendResponse = { callback, data -> responses.add(callback to data) },
            argsJson = json, jsonMapper = mapper,
        )
        haptics.triggerHapticFeedback(invoke)
        assertEquals("one terminal response", 1, responses.size)
        assertEquals(responses.single().second, 2L, responses.single().first)
        return JSObject(responses.single().second)
    }

    // The repository preloads API 28 android-all for tests. Resolve newer APIs
    // against the selected Robolectric SDK without changing that shared classpath.
    private fun predefined(id: Int): VibrationEffect =
        VibrationEffect::class.java.getMethod("createPredefined", Int::class.javaPrimitiveType)
            .invoke(null, id) as VibrationEffect

    @Test fun modernMessageAndExplicitEffectsReachTouchServiceDespiteObsoleteSetting() {
        touchSetting(false)
        val effects = mutableListOf<VibrationEffect>()
        val attributes = mutableListOf<VibrationAttributes>()
        every { vibrator["vibrate"](capture(effects), capture(attributes)) } returns Unit
        val requests = listOf(
            """{"type":"message","effect":"send","durationMs":null,"amplitude":null,"intensity":null}""",
            """{"type":"predefined","effect":"double_click","durationMs":null,"amplitude":null,"intensity":null}""",
            """{"type":"one_shot","effect":null,"durationMs":37,"amplitude":81,"intensity":null}""",
        )
        requests.forEach { assertTrue(trigger(it).getBoolean("submitted")) }
        assertEquals(listOf(
            predefined(VibrationEffect.EFFECT_TICK),
            predefined(VibrationEffect.EFFECT_DOUBLE_CLICK),
            VibrationEffect.createOneShot(37, 81),
        ), effects)
        assertEquals(3, attributes.size)
        attributes.forEach {
            assertEquals(VibrationAttributes.USAGE_TOUCH, it.usage)
            assertEquals(0, it.flags)
        }
    }

    @Test fun viewFeedbackReportsTheActualSystemDecision() {
        touchSetting(false)
        every { view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP) } returnsMany listOf(false, true)
        val blocked = trigger("""{"type":"view","effect":"keyboard_tap"}""")
        assertFalse(blocked.getBoolean("submitted"))
        assertEquals("system_suppressed", blocked.getString("reason"))
        assertTrue(trigger("""{"type":"view","effect":"keyboard_tap"}""").getBoolean("submitted"))
        verify(exactly = 2) { view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP) }
        verify { vibrator wasNot Called }
    }

    @Test
    @Config(sdk = [28])
    fun legacyDirectVibrationStillHonorsTouchSettingAndUsesSonification() {
        val effect = slot<VibrationEffect>()
        val attributes = slot<AudioAttributes>()
        every { vibrator.vibrate(capture(effect), capture(attributes)) } just Runs
        touchSetting(false)
        val blocked = trigger("""{"type":"one_shot","durationMs":37,"amplitude":81}""")
        assertFalse(blocked.getBoolean("submitted"))
        assertEquals("system_haptics_disabled", blocked.getString("reason"))
        verify(exactly = 0) { vibrator.vibrate(any<VibrationEffect>(), any<AudioAttributes>()) }

        touchSetting(true)
        assertTrue(trigger("""{"type":"one_shot","durationMs":37,"amplitude":81}""").getBoolean("submitted"))
        assertEquals(VibrationEffect.createOneShot(37, 81), effect.captured)
        assertEquals(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION, attributes.captured.usage)
        assertEquals(AudioAttributes.CONTENT_TYPE_SONIFICATION, attributes.captured.contentType)
    }

    @Test
    @Config(sdk = [28])
    fun legacyMessageFallbackUsesTheViewDecision() {
        touchSetting(false)
        every { view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY) } returns false
        val response = trigger("""{"type":"message","effect":"complete"}""")
        assertFalse(response.getBoolean("submitted"))
        assertTrue(response.getBoolean("fallback"))
        assertEquals("system_suppressed", response.getString("reason"))
        verify(exactly = 1) { view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY) }
        verify(exactly = 0) { vibrator.vibrate(any<VibrationEffect>(), any<AudioAttributes>()) }
    }
}
