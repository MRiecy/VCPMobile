package com.vcp.mobile.haptics

import android.app.Activity
import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import android.view.HapticFeedbackConstants
import app.tauri.annotation.InvokeArg
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import com.vcp.mobile.runtime.PluginTaskRuntime
import java.util.Locale

internal class NativeHaptics(private val activity: Activity, private val runtime: PluginTaskRuntime) {
    private fun getSystemServiceVibrator(): Vibrator? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val vm = activity.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager
            vm?.defaultVibrator ?: (activity.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator)
        } else {
            @Suppress("DEPRECATION")
            activity.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
    }

    private fun vibrateTouch(vibrator: Vibrator, effect: VibrationEffect) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            vibrator.vibrate(effect, android.os.VibrationAttributes.createForUsage(android.os.VibrationAttributes.USAGE_TOUCH))
        } else {
            @Suppress("DEPRECATION")
            vibrator.vibrate(effect, android.media.AudioAttributes.Builder()
                .setUsage(android.media.AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                .setContentType(android.media.AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build())
        }
    }

    fun triggerHapticFeedback(invoke: Invoke) {
        if (runtime.isDestroying) {
            invoke.reject("插件已在销毁过程中")
            return
        }
        val args = try {
            invoke.parseArgs(TriggerHapticFeedbackArgs::class.java)
        } catch (e: Exception) {
            invoke.reject("参数解析失败: ${e.message}")
            return
        }

        val type = args.type ?: "view"
        val effect = args.effect ?: "keyboard_tap"
        val durationMs = (args.durationMs ?: 20L).coerceIn(1L, 5000L)
        val amplitude = args.amplitude ?: -1

        val result = JSObject()
        result.put("type", type)
        result.put("effect", effect)
        result.put("submitted", false)

        activity.runOnUiThread {
            try {
                if (runtime.isDestroying) {
                    result.put("reason", "plugin_destroyed")
                    invoke.resolve(result)
                    return@runOnUiThread
                }
                if (android.provider.Settings.System.getInt(activity.contentResolver,
                        android.provider.Settings.System.HAPTIC_FEEDBACK_ENABLED, 1) == 0) {
                    result.put("reason", "system_haptics_disabled")
                    invoke.resolve(result)
                    return@runOnUiThread
                }
                val resolvedType = if (type == "message") "predefined" else type
                val resolvedEffect = if (type == "message") when (effect) {
                    "send" -> "tick"
                    "complete" -> "click"
                    else -> throw IllegalArgumentException("未知消息触感: $effect")
                } else effect.lowercase(Locale.ROOT)
                when (resolvedType) {
                    "view" -> {
                        val constant = when (resolvedEffect) {
                            "keyboard_tap" -> HapticFeedbackConstants.KEYBOARD_TAP
                            "virtual_key" -> HapticFeedbackConstants.VIRTUAL_KEY
                            "clock_tick" -> HapticFeedbackConstants.CLOCK_TICK
                            "context_click" -> {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) HapticFeedbackConstants.CONTEXT_CLICK else HapticFeedbackConstants.LONG_PRESS
                            }
                            "confirm" -> {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.VIRTUAL_KEY
                            }
                            "reject" -> {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.REJECT else HapticFeedbackConstants.LONG_PRESS
                            }
                            "gesture_start" -> {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.GESTURE_START else HapticFeedbackConstants.VIRTUAL_KEY
                            }
                            "gesture_end" -> {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.GESTURE_END else HapticFeedbackConstants.VIRTUAL_KEY
                            }
                            "long_press" -> HapticFeedbackConstants.LONG_PRESS
                            else -> throw IllegalArgumentException("未知视图触感: $effect")
                        }

                        val submitted = activity.window.decorView.performHapticFeedback(constant)
                        result.put("submitted", submitted)
                        result.put("constant", constant)
                        result.put("resolvedEffect", resolvedEffect)
                        if (!submitted) result.put("reason", "system_suppressed")
                    }
                    "predefined" -> {
                        val vibrator = getSystemServiceVibrator()
                        if (vibrator?.hasVibrator() != true) {
                            result.put("reason", "no_vibrator")
                            invoke.resolve(result)
                            return@runOnUiThread
                        }
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                            val effectId = when (resolvedEffect) {
                                "tick" -> VibrationEffect.EFFECT_TICK
                                "click" -> VibrationEffect.EFFECT_CLICK
                                "heavy_click" -> VibrationEffect.EFFECT_HEAVY_CLICK
                                "double_click" -> VibrationEffect.EFFECT_DOUBLE_CLICK
                                else -> throw IllegalArgumentException("未知预定义触感: $effect")
                            }
                            val vibrationEffect = VibrationEffect.createPredefined(effectId)
                            vibrateTouch(vibrator, vibrationEffect)
                            result.put("submitted", true)
                            result.put("effectId", effectId)
                            result.put("resolvedEffect", resolvedEffect)
                        } else {
                            // API 26–28 cannot play predefined effects. Keep this a system
                            // touch cue instead of replacing it with an arbitrary timed buzz.
                            val constant = when (resolvedEffect) {
                                "tick" -> HapticFeedbackConstants.CLOCK_TICK
                                "click" -> HapticFeedbackConstants.VIRTUAL_KEY
                                else -> {
                                    result.put("reason", "effect_unavailable")
                                    invoke.resolve(result)
                                    return@runOnUiThread
                                }
                            }
                            val submitted = activity.window.decorView.performHapticFeedback(constant)
                            result.put("submitted", submitted)
                            result.put("fallback", true)
                            result.put("constant", constant)
                            result.put("resolvedEffect", if (resolvedEffect == "tick") "clock_tick" else "virtual_key")
                            if (!submitted) result.put("reason", "system_suppressed")
                        }
                    }
                    "one_shot" -> {
                        val vibrator = getSystemServiceVibrator()
                        if (vibrator?.hasVibrator() != true) {
                            result.put("reason", "no_vibrator")
                            invoke.resolve(result)
                            return@runOnUiThread
                        }
                        result.put("hasAmplitudeControl", vibrator.hasAmplitudeControl())
                        val amp = if (amplitude in 1..255) amplitude else VibrationEffect.DEFAULT_AMPLITUDE
                        vibrateTouch(vibrator, VibrationEffect.createOneShot(durationMs, amp))
                        result.put("submitted", true)
                        result.put("durationMs", durationMs)
                        result.put("amplitude", amp)
                    }
                    else -> {
                        result.put("reason", "unsupported_haptic_type")
                    }
                }
                invoke.resolve(result)
            } catch (e: Exception) {
                Log.e("VcpMobilePlugin", "triggerHapticFeedback 失败", e)
                invoke.reject("触发触感失败: ${e.message}")
            }
        }
    }
}

@InvokeArg
class TriggerHapticFeedbackArgs {
    var type: String? = "view"
    var effect: String? = null
    var durationMs: Long? = 20L
    var amplitude: Int? = null
    var intensity: Double? = null
}
