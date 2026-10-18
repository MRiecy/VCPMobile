package com.vcp.mobile.lifecycle

import android.app.Activity
import android.content.res.Configuration
import android.os.Handler
import android.os.Looper
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.tauri.plugin.JSObject
import java.util.concurrent.atomic.AtomicLong

/**
 * 应用生命周期桥接器
 *
 * 监听进程级前后台状态，通过插件持有的原生 Channel 送到 Rust。
 * 订阅者可读取当前快照，避免冷启动发生在 Channel 注册前的事件丢失。
 */
class LifecycleBridge : DefaultLifecycleObserver {

    @Volatile private var eventSink: ((String, JSObject) -> Unit)? = null
    // Process-wide sequence survives a plugin instance/Activity replacement.
    private companion object {
        val nativeSequence = AtomicLong(0)
    }
    private var lastForegroundState: Boolean? = null

    fun attach(activity: Activity, eventSink: (String, JSObject) -> Unit) {
        this.eventSink = eventSink
        // ProcessLifecycleOwner 合并 Activity 切换，避免短暂重建触发后台保活。
        activity.runOnUiThread {
            if (this.eventSink === eventSink) {
                ProcessLifecycleOwner.get().lifecycle.addObserver(this)
            }
        }
    }

    fun detach() {
        eventSink = null
        lastForegroundState = null
        if (Looper.myLooper() == Looper.getMainLooper()) {
            ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
        } else {
            Handler(Looper.getMainLooper()).post {
                // A newer attachment may already be using this same observer.
                if (eventSink == null) ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
            }
        }
    }

    /** 必须在主线程与 observer 回调、Channel 注册一起串行调用。 */
    fun snapshot(): JSObject {
        val foreground = lastForegroundState
            ?: ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)
        return JSObject().apply {
            put("phase", if (foreground) "active" else "background")
            put("nativeSequence", nativeSequence.get())
            put("snapshot", true)
        }
    }

    override fun onDestroy(owner: LifecycleOwner) {
        detach()
        super.onDestroy(owner)
    }

    override fun onResume(owner: LifecycleOwner) {
        emitTransition(true)
    }

    override fun onPause(owner: LifecycleOwner) {
        emitTransition(false)
    }

    override fun onStop(owner: LifecycleOwner) {
        emitTransition(false)
    }

    fun onConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        emit("configuration", mapOf(
            "isDarkMode" to isDark
        ))
    }

    fun onLowMemory() {
        emit("memory-pressure", emptyMap())
    }

    private fun emitTransition(foreground: Boolean) {
        if (lastForegroundState == foreground) return
        lastForegroundState = foreground
        // Preserve Android's process pause/stop coalescing. Only iOS supplies inactive.
        emit("lifecycle", mapOf(
            "phase" to if (foreground) "active" else "background",
            "nativeSequence" to nativeSequence.incrementAndGet(),
            "snapshot" to false
        ))
    }

    private fun emit(event: String, detail: Map<String, Any?>) {
        // 向 Rust 侧派发强类型的原生生命周期事件，规避 WebView 被冻结时 JS 无法执行的痛点
        val sink = eventSink
        if (sink != null) {
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
            sink(event, triggerData)
        }
    }
}
