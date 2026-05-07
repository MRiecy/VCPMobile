package com.vcp.avatar.insets

import android.util.Log
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.vcp.avatar.bridge.FrontendBridge

/**
 * 键盘 IME Insets 管理器
 *
 * 职责：
 * 1. 监听系统 WindowInsets 中 IME（键盘）区域的变化
 * 2. 将键盘高度与安全区域信息实时推送到前端，驱动 CSS 变量 --keyboard-offset
 * 3. 不再通过 setPadding 干预 WebView 布局，完全交由前端 CSS 接管
 *
 * 适用场景：Android 15+ Edge-to-Edge 强制模式下，系统已禁用 adjustResize 的
 * 自动布局行为，必须由应用层手动处理 Insets。
 */
class KeyboardInsetsManager(private val frontendBridge: FrontendBridge) {

    /**
     * 将 Insets 监听绑定到指定根视图。
     *
     * @param rootView 通常是 window.decorView.rootView
     */
    fun attach(rootView: View) {
        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())

            val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0
            Log.d("VCPKeyboard", "Native inset event: height=$keyboardHeight, visible=$isKeyboardVisible, safeArea=${systemBars.bottom} [setPadding REMOVED]")
            frontendBridge.emit(
                "vcp-keyboard-inset",
                mapOf(
                    "height" to keyboardHeight,
                    "visible" to isKeyboardVisible,
                    "safeAreaBottom" to systemBars.bottom
                )
            )

            // 继续向下传递 insets，确保 WebView 能正确解析 env(safe-area-inset-*)
            // 注意：不再通过 setPadding 干预布局，键盘偏移完全由前端 CSS 接管
            insets
        }
    }

    /**
     * 手动查询当前键盘状态（供前端 focus 兜底时调用）。
     */
    fun queryCurrentState(rootView: View): KeyboardState {
        val insets = ViewCompat.getRootWindowInsets(rootView)
            ?: return KeyboardState(0, false)
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        val visible = insets.isVisible(WindowInsetsCompat.Type.ime())
        return KeyboardState(
            height = if (visible) ime.bottom else 0,
            visible = visible
        )
    }

    data class KeyboardState(val height: Int, val visible: Boolean)
}
