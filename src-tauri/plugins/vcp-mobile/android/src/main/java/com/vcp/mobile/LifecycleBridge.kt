package com.vcp.mobile

import android.app.Activity
import android.content.res.Configuration
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.JSObject

/**
 * 应用生命周期桥接器
 *
 * 通过 DefaultLifecycleObserver 自动监听 Activity 生命周期，
 * 使用 Plugin.trigger() 替代旧的 evaluateJavascript 向前端推送事件。
 */
class LifecycleBridge(
    private val emit: (String, JSObject) -> Unit
) : DefaultLifecycleObserver {

    fun attach(activity: Activity) {
        if (activity is LifecycleOwner) {
            activity.lifecycle.addObserver(this)
        }
    }

    override fun onResume(owner: LifecycleOwner) {
        emit("vcp-lifecycle", JSObject().apply { put("state", "resume") })
    }

    override fun onPause(owner: LifecycleOwner) {
        emit("vcp-lifecycle", JSObject().apply { put("state", "pause") })
    }

    override fun onStop(owner: LifecycleOwner) {
        emit("vcp-lifecycle", JSObject().apply { put("state", "stop") })
    }

    fun onConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        emit("vcp-lifecycle", JSObject().apply {
            put("state", "config-changed")
            put("isDarkMode", isDark)
        })
    }

    fun onLowMemory() {
        emit("vcp-lifecycle", JSObject().apply { put("state", "low-memory") })
    }
}
