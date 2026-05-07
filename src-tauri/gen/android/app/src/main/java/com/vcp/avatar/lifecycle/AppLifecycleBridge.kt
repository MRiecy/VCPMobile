package com.vcp.avatar.lifecycle

import android.content.res.Configuration
import com.vcp.avatar.bridge.FrontendBridge

/**
 * 应用生命周期桥接器
 *
 * 集中处理 Android → 前端的生命周期事件，便于后续扩展：
 * - 前台/后台切换
 * - 深色/浅色模式跟随系统
 * - 低内存警告
 * - 省电模式变化
 * - 屏幕旋转 / 尺寸变化
 *
 * 当前仅提供基础框架，需要时可在 MainActivity 对应生命周期钩子中调用。
 */
class AppLifecycleBridge(private val frontendBridge: FrontendBridge) {

    fun notifyResume() {
        frontendBridge.emit("vcp-lifecycle", mapOf("state" to "resume"))
    }

    fun notifyPause() {
        frontendBridge.emit("vcp-lifecycle", mapOf("state" to "pause"))
    }

    fun notifyConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        frontendBridge.emit(
            "vcp-lifecycle",
            mapOf(
                "state" to "config-changed",
                "isDarkMode" to isDark
            )
        )
    }

    fun notifyLowMemory() {
        frontendBridge.emit("vcp-lifecycle", mapOf("state" to "low-memory"))
    }
}
