package com.vcp.mobile

import android.app.Activity
import android.content.res.Configuration
import android.webkit.WebView
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.JSObject
import java.lang.ref.WeakReference
import java.util.concurrent.atomic.AtomicLong

/**
 * 应用生命周期桥接器
 *
 * 通过 DefaultLifecycleObserver 自动监听 Activity 生命周期，
 * 使用 evaluateJavascript 直接注入 window.CustomEvent，保持与前端 window.addEventListener 兼容。
 */
class LifecycleBridge : DefaultLifecycleObserver {

    private var activityRef: WeakReference<Activity>? = null
    private var pluginRef: WeakReference<VcpMobilePlugin>? = null
    private val transitionEpoch = AtomicLong(0)
    private var lastForegroundState: Boolean? = null

    fun attach(activity: Activity, plugin: VcpMobilePlugin) {
        activityRef = WeakReference(activity)
        pluginRef = WeakReference(plugin)
        // 升级为进程级生命周期监听，完美防抖，免疫 Activity 重建与切换
        activity.runOnUiThread {
            androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.addObserver(this)
        }
    }

    fun detach() {
        val activity = activityRef?.get()
        if (activity != null) {
            activity.runOnUiThread {
                try {
                    androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
                } catch (_: Exception) {}
            }
        } else {
            try {
                androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
            } catch (_: Exception) {}
        }
        activityRef = null
        pluginRef = null
        lastForegroundState = null
    }

    override fun onDestroy(owner: LifecycleOwner) {
        detach()
        super.onDestroy(owner)
    }

    override fun onResume(owner: LifecycleOwner) {
        emitTransition("resume", true)
    }

    override fun onPause(owner: LifecycleOwner) {
        emitTransition("pause", false)
    }

    override fun onStop(owner: LifecycleOwner) {
        emitTransition("stop", false)
    }

    fun onConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        emit(mapOf(
            "state" to "config-changed",
            "isDarkMode" to isDark
        ))
    }

    fun onLowMemory() {
        emit(mapOf("state" to "low-memory"))
    }

    private fun emitTransition(state: String, foreground: Boolean) {
        if (lastForegroundState == foreground) return
        lastForegroundState = foreground
        emit(mapOf("state" to state, "epoch" to transitionEpoch.incrementAndGet()))
    }

    private fun emit(detail: Map<String, Any?>) {
        // 向 Rust 侧派发强类型的原生生命周期事件，规避 WebView 被冻结时 JS 无法执行的痛点
        val plugin = pluginRef?.get()
        if (plugin != null) {
            val triggerData = JSObject()
            for ((key, value) in detail) {
                when (value) {
                    is String -> triggerData.put(key, value)
                    is Boolean -> triggerData.put(key, value)
                    is Int -> triggerData.put(key, value)
                    is Double -> triggerData.put(key, value)
                    is Long -> triggerData.put(key, value)
                }
            }
            plugin.trigger("lifecycle", triggerData)
        }
    }
}
