---
id: PLUGIN-KEYBOARD-004
title: 键盘 Insets 管理
description: 通过 WindowInsetsCompat 监听键盘状态并实时推送到前端
version: 0.9.14
date: 2026-05-21
related_files:
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/KeyboardInsetsManager.kt
---

# 键盘 Insets 管理

## 1. 功能概述

监听 Android 系统软键盘的弹出与收起事件，将键盘高度、可见性状态及安全区域底部距离通过 `evaluateJavascript` 实时注入前端，使 Vue 层能够动态调整输入框和消息列表的布局，避免键盘遮挡内容。

> **设计决策**：使用 `evaluateJavascript` + `CustomEvent` 而非 Tauri 官方事件通道（`Plugin.trigger()`），因为前端使用 `window.addEventListener` 监听，与 `vcp-lifecycle` 事件保持一致的接收范式。参见 `docs/ANDROID_PLUGIN_MANAGEMENT.md` §4.1。

---

## 2. 代码结构

```
src-tauri/plugins/vcp-mobile/android/.../KeyboardInsetsManager.kt (92 lines)
├── KeyboardInsetsManager(activity: Activity)
│   ├── attach(webView: WebView)
│   ├── queryCurrentState(): KeyboardState
│   ├── emit(eventName, detail)
│   ├── serializeValue(value): String
│   └── escapeJson(s): String
│
└── data class KeyboardState(val height: Int, val visible: Boolean)
```

---

## 3. 核心机制

### 3.1 Insets 监听注册

```kotlin
fun attach(webView: WebView) {
    webViewRef = webView
    val rootView = activity.window.decorView.rootView

    ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
        val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
        val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0

        emit("vcp-keyboard-inset", mapOf(
            "height" to keyboardHeight,
            "visible" to isKeyboardVisible,
            "safeAreaBottom" to systemBars.bottom
        ))

        insets  // 必须返回 insets，否则其他监听器收不到
    }
}
```

| Insets 类型 | 含义 | 用途 |
|-------------|------|------|
| `WindowInsetsCompat.Type.systemBars()` | 系统栏（状态栏 + 导航栏）区域 | 获取 `safeAreaBottom`，用于计算底部安全距离 |
| `WindowInsetsCompat.Type.ime()` | 输入法（软键盘）区域 | 获取键盘高度 `ime.bottom` |
| `isVisible(Type.ime())` | 键盘当前是否可见 | 区分"键盘收起"与"键盘高度为 0" |

### 3.2 事件格式

```kotlin
val script = """
    window.dispatchEvent(
        new CustomEvent('vcp-keyboard-inset', {
            detail: {
                height: 876,
                visible: true,
                safeAreaBottom: 42
            }
        })
    )
"""
webViewRef?.evaluateJavascript(script, null)
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `height` | `number` | 键盘高度（px）。键盘收起时为 `0` |
| `visible` | `boolean` | 键盘是否可见 |
| `safeAreaBottom` | `number` | 底部系统导航栏高度（px） |

---

## 4. 前端接收方式

```typescript
window.addEventListener('vcp-keyboard-inset', (e: CustomEvent) => {
    const { height, visible, safeAreaBottom } = e.detail;
    // 动态调整输入栏 padding-bottom 或消息列表高度
});
```

> **注意**：前端不使用 `@tauri-apps/api/event` 的 `listen()`，因为 Kotlin 侧未使用 `Plugin.trigger()` 发射事件。两者通道不互通。

---

## 5. 状态查询（同步）

```kotlin
fun queryCurrentState(): KeyboardState {
    val rootView = activity.window.decorView.rootView
    val insets = ViewCompat.getRootWindowInsets(rootView) ?: return KeyboardState(0, false)
    val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
    val visible = insets.isVisible(WindowInsetsCompat.Type.ime())
    return KeyboardState(
        height = if (visible) ime.bottom else 0,
        visible = visible
    )
}
```

用于前端初始化时获取当前键盘状态（而非等待下一次 Insets 变化事件）。

---

## 6. JSON 序列化实现

由于 `evaluateJavascript` 需要注入完整的 JavaScript 对象字面量，Kotlin 侧手写了一个轻量级 JSON 序列化器：

```kotlin
private fun serializeValue(value: Any?): String {
    return when (value) {
        null -> "null"
        is String -> "\"${escapeJson(value)}\""
        is Boolean -> value.toString()
        is Number -> value.toString()
        is Map<*, *> -> {
            val entries = value.entries.joinToString(", ") { (k, v) ->
                "\"$k\": ${serializeValue(v)}"
            }
            "{ $entries }"
        }
        else -> "\"${escapeJson(value.toString())}\""
    }
}

private fun escapeJson(s: String): String {
    return s
        .replace("\\", "\\\\")
        .replace("\"", "\\\"")
        .replace("\b", "\\b")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
}
```

> **为什么不使用 Gson/Moshi？**
> - Tauri 插件的 Kotlin 侧默认不引入 Gson。
> - 序列化对象结构简单（仅单层 Map），手写序列化器避免增加依赖体积。

---

## 7. 生命周期绑定

```kotlin
// VcpMobilePlugin.kt
override fun load(webView: WebView) {
    super.load(webView)
    keyboardInsetsManager.attach(webView)
    // ...
}
```

`attach()` 在 `Plugin.load(webView)` 时调用，确保 WebView 初始化完成后立即注册 Insets 监听器。

---

## 8. 关键约束

1. **必须返回 `insets`**：`setOnApplyWindowInsetsListener` 的 lambda 必须返回传入的 `insets` 对象，否则系统栏的内边距计算会被中断，导致布局异常。

2. **不干预 WebView 布局**：`KeyboardInsetsManager` 仅负责**信息推送**，不通过 `setPadding` 修改 WebView 布局。前端通过 CSS `env(safe-area-inset-bottom)` 和动态计算完全接管布局调整。

3. **`safeAreaBottom` 的用途**：部分设备使用手势导航栏（无实体按钮），`safeAreaBottom` 帮助前端区分"键盘高度"和"导航栏高度"，避免双重 padding。
