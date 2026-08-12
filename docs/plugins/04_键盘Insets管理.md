---
id: PLUGIN-KEYBOARD-004
title: 键盘 Insets 管理
description: 通过 WindowInsetsCompat 监听键盘状态并实时推送到前端
version: 1.1.4
date: 2026-08-13
related_files:
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/KeyboardInsetsManager.kt
---

# 键盘 Insets 管理

## 1. 功能概述

监听 Android `WindowInsetsCompat`，把 system bars 与 display cutout 合并后的**四边安全区**、原始 IME bottom 与可见性通过 `evaluateJavascript` 实时注入前端。Vue 根桥接统一转换为 CSS px，并只应用扣除底部安全区后的 IME 净增量，避免导航栏重复计入。

> **设计决策**：使用 `evaluateJavascript` + `CustomEvent` 而非 Tauri 官方事件通道（`Plugin.trigger()`），因为前端使用 `window.addEventListener` 监听，与 `vcp-hardware-back`、`vcp-exit-requested` 等窗口级事件保持一致的接收范式。v1.1.3 起生命周期事件已迁移至 Tauri Event `vcp-lifecycle-changed`。参见 `docs/ANDROID_PLUGIN_MANAGEMENT.md` §4.1。

---

## 2. 代码结构

```
src-tauri/plugins/vcp-mobile/android/.../KeyboardInsetsManager.kt
├── KeyboardInsetsManager(activity: Activity)
│   ├── attach(webView: WebView)
│   ├── detach()
│   ├── schedule(snapshot): 同帧合并
│   ├── emitSnapshot(snapshot)
│   └── snapshotFrom(insets)
├── EdgeInsetsPx(top, right, bottom, left)
├── InsetSnapshot(safeTopPx, safeRightPx, safeBottomPx, safeLeftPx,
│                imeBottomPx, imeVisible)
├── mergeInsetSnapshot(...)
├── netImeBottomPx(snapshot)
└── buildInsetsJavascript(snapshot)
```

---

## 3. 核心机制

### 3.1 Insets 监听注册

```kotlin
fun attach(webView: WebView) {
    webViewRef = webView
    val rootView = activity.window.decorView.rootView
    this.rootView = rootView

    ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
        schedule(snapshotFrom(insets))
        insets
    }
    ViewCompat.getRootWindowInsets(rootView)?.let { schedule(snapshotFrom(it)) }
    ViewCompat.requestApplyInsets(rootView)
}
```

| Insets 类型 | 含义 | 用途 |
|-------------|------|------|
| `WindowInsetsCompat.Type.systemBars()` | 系统栏（状态栏 + 导航栏）区域 | 四边安全区候选 |
| `WindowInsetsCompat.Type.displayCutout()` | 刘海/挖孔/侧边切口 | 与 system bars 逐边取最大值 |
| `WindowInsetsCompat.Type.ime()` | 输入法（软键盘）区域 | 保留原始 `ime.bottom` 物理像素 |
| `isVisible(Type.ime())` | 键盘当前是否可见 | 区分"键盘收起"与"键盘高度为 0" |

### 3.2 事件格式

```javascript
window.__VCP_NATIVE_INSETS__ = {
  safeTopPx: 96,
  safeRightPx: 0,
  safeBottomPx: 72,
  safeLeftPx: 0,
  imeBottomPx: 876,
  imeVisible: true
};
window.dispatchEvent(new CustomEvent('vcp-keyboard-inset', {
  detail: window.__VCP_NATIVE_INSETS__
}));
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `safeTopPx` / `safeRightPx` / `safeBottomPx` / `safeLeftPx` | `number` | system bars 与 display cutout 逐边最大值，Android 物理像素 |
| `imeBottomPx` | `number` | IME 原始 bottom，Android 物理像素；收起时为 0 |
| `imeVisible` | `boolean` | IME 是否可见 |

先赋值 `window.__VCP_NATIVE_INSETS__` 再发事件是冷启动防丢契约：若 Kotlin 在 Vue listener 安装前发射，App 根桥接仍能立即重放缓存快照。

---

## 4. 前端接收方式

```typescript
const release = retainNativeInsetsBridge();
// App 根桥在整个应用生命周期持有 listener，并立即重放
// window.__VCP_NATIVE_INSETS__。

// 卸载时对称释放；内部引用计数避免多个编辑器重复绑定。
onUnmounted(release);
```

`useKeyboardInsets()` 复用同一个引用计数桥，并为 Web/host 环境保留 Virtual Keyboard API 与 focus/scroll fallback。收到过原生快照后 fallback 不再覆盖原生真相源。前端兼容解析旧 `height/visible/safeAreaBottom` 字段，但当前 Kotlin 只发新六字段格式。

---

## 5. 快照重放与单位转换

```typescript
const css = nativeInsetsToCss(snapshot, window.devicePixelRatio || 1);
// imeExtraBottom = max(0, imeBottomPx - safeBottomPx) / DPR

document.documentElement.style.setProperty('--vcp-safe-top', `${css.safeTop}px`);
document.documentElement.style.setProperty('--vcp-safe-right', `${css.safeRight}px`);
document.documentElement.style.setProperty('--vcp-safe-bottom', `${css.safeBottom}px`);
document.documentElement.style.setProperty('--vcp-safe-left', `${css.safeLeft}px`);
document.documentElement.style.setProperty('--vcp-ime-offset', `${css.imeExtraBottom}px`);
```

旧文档中的 `queryCurrentState(): KeyboardState` 已删除。当前策略是 Android attach 时主动读取 `getRootWindowInsets` 并请求 apply；每次注入都更新 window cache，前端 listener retain 时同步重放。原生字段始终是物理像素，DPR 转换只在前端唯一桥接点执行一次。

---

## 6. 有界 JavaScript 构造

当前 payload 只有六个已归一化的 Int/Boolean 字段，`buildInsetsJavascript` 直接构造固定 schema，不再存在通用 `serializeValue` / `escapeJson`：

```kotlin
internal fun buildInsetsJavascript(snapshot: InsetSnapshot): String
```

字段没有用户字符串输入，schema 可由 JVM 契约测试直接断言；不要重新引入通用 JSON serializer 或任意事件名。

---

## 7. 生命周期绑定

```kotlin
// VcpMobilePlugin.kt
override fun load(webView: WebView) {
    super.load(webView)
    keyboardInsetsManager.attach(webView)
    // ...
}

override fun onDestroy(activity: AppCompatActivity) {
    keyboardInsetsManager.detach()
    // ...
}
```

`attach()` 在 `Plugin.load(webView)` 时调用，确保 WebView 初始化完成后立即注册并主动请求 Insets；`onDestroy()` 对称 `detach()`，撤销 listener 与 frame callback，并在需要时先发出 IME 已收起快照。

---

## 8. 关键约束

1. **必须返回 `insets`**：`setOnApplyWindowInsetsListener` 的 lambda 必须返回传入的 `insets` 对象，否则系统栏的内边距计算会被中断，导致布局异常。

2. **不干预 WebView 布局**：`KeyboardInsetsManager` 仅负责信息推送，不通过 `setPadding` 修改 WebView。前端统一写入 `--vcp-safe-top/right/bottom/left` 与 `--vcp-ime-offset`；`env()` 仅是无原生快照时的 Web fallback。

3. **IME 只应用净增量**：组件不得把 raw `imeBottomPx` 与 safe bottom 直接相加；统一使用 `max(0, imeBottomPx - safeBottomPx)` 转换后的 `--vcp-ime-offset`。

4. **同帧只提交最新快照**：Insets 高频回调先写入 `pending`，再通过 `postOnAnimation` 合并到下一帧；相同 `InsetSnapshot` 不重复执行 `evaluateJavascript`。`detach()` 必须移除已排队的 frame callback 并清空 `pending/lastSent`，防止 Activity 重建后旧 WebView 收到迟到事件。

5. **四边同源**：system bars 与 display cutout 必须逐边取最大值；横屏 left/right 切口不能由 bottom-only 逻辑代替。

6. **根桥长期持有**：App 不应等待某个输入组件挂载才安装 listener；根级 `retainNativeInsetsBridge()` 负责全生命周期接收与冷启动重放。

---

*最后更新：2026-08-13 | VCP Mobile v1.1.4*
