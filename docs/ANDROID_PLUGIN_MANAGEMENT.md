# Android 插件管理体系规范

> **文档编号**: ARCH-ANDROID-001  
> **版本**: 0.9.14  
> **状态**: 已批准（Plan B: Plugin-based Refactoring）  
> **适用范围**: `src-tauri/` 及 Android 原生层全部自定义代码  

---

## 目录

1. [现状与问题诊断](#1-现状与问题诊断)
2. [插件总览表](#2-插件总览表)
3. [自定义插件架构：`tauri-plugin-vcp-mobile`](#3-自定义插件架构-tauri-plugin-vcp-mobile)
4. [插件开发工作流](#4-插件开发工作流)
5. [迁移路线图](#5-迁移路线图)
6. [官方插件采用准则](#6-官方插件采用准则)
7. [未来 iOS 扩展路径](#7-未来-ios-扩展路径)
8. [附录：参考链接](#8-附录参考链接)

---

## 1. 现状与问题诊断

### 1.1 当前架构风险

VCP Mobile（`com.vcp.avatar`）当前采用**非标准 Android 架构**：所有自定义 Kotlin 代码直接写入 Tauri CLI 自动生成的目录 `src-tauri/gen/android/app/src/main/java/com/vcp/avatar/`。

| 风险项 | 严重度 | 说明 |
|--------|--------|------|
| `gen/android` 覆盖 | **高** | 运行 `tauri android init` 或升级 Tauri CLI 时，自动生成逻辑会覆盖 `gen/android`，导致自定义 Kotlin 代码丢失 |
| 版本控制污染 | 中 | `gen/android` 本不应入版本库（内含绝对路径、本地 SDK 配置），但当前必须提交以保留自定义代码 |
| 职责混杂 | 中 | MainActivity.kt 同时承担 Tauri 框架职责与业务编排职责，难以单元测试 |
| 事件通道碎片化 | 中 | 原生→前端事件通过 `WebView.evaluateJavascript` 手动注入（`FrontendBridge.kt`），与 Tauri 的 `Plugin.trigger()` 体系割裂 |
| JNI 裸调 | 低 | Rust 侧通过 `jni 0.21` 直接操作 `Activity`/`Window`/`Intent`，代码冗长且类型安全差 |

### 1.2 当前 Android 自定义功能清单

| 功能 | 实现位置（Rust） | 实现位置（Kotlin） | 通信方式 |
|------|------------------|-------------------|----------|
| 屏幕常亮（Wake Lock） | `vcp_modules/screen_wake_manager.rs` | — | Rust JNI 直接调用 `activity.getWindow().addFlags()` |
| 流式前台保活服务 | `vcp_modules/stream_service_manager.rs` | `service/StreamKeepaliveService.kt` | Rust JNI `startForegroundService()` + Kotlin 通知管理 |
| 键盘 Insets 管理 | — | `insets/KeyboardInsetsManager.kt` | `evaluateJavascript` 注入 `vcp-keyboard-inset` 事件 |
| 返回键拦截 | — | `BackNavigationManager.kt` | 纯 Kotlin，`OnBackPressedDispatcher` |
| 生命周期事件桥接 | — | `lifecycle/AppLifecycleBridge.kt` | `evaluateJavascript` 注入 `vcp-lifecycle` 事件 |
| 附件外部存储路径 | `vcp_modules/file_manager.rs` | — | 纯 Rust，`app_handle.path().document_dir()` |

### 1.3 已集成的官方插件

- `tauri-plugin-log` v2.8.0 — 日志双写（stdout + 文件）
- `tauri-plugin-opener` v2.5.4 — 唤起系统应用打开文件/URL

---

## 2. 插件总览表

### 2.1 官方插件（Official）

| 插件名 | Android 支持 | 当前状态 | 备注 |
|--------|-------------|----------|------|
| `tauri-plugin-log` | ✅ | **已用** v2.8.0 | 日志输出到 stdout 与 `LogDir` |
| `tauri-plugin-opener` | ✅ | **已用** v2.5.4 | 替代旧版 `open` shell 命令 |
| `tauri-plugin-http` | ✅ | 待评估 | 若需替换 `reqwest` 为统一 HTTP 客户端可考虑 |
| `tauri-plugin-websocket` | ✅ | 不需要 | 当前使用 `tokio-tungstenite` 直接管理 WebSocket，控制粒度足够 |
| `tauri-plugin-fs` | ✅ | 不需要 | 附件管理已自研（含 SHA-256 内容寻址、缩略图、文本提取），无需替换 |
| `tauri-plugin-store` | ✅ | 不需要 | 配置已用 SQLite + Pinia persistedstate 覆盖 |
| `tauri-plugin-sql` | ✅ | 不需要 | 已用 `sqlx` + `rusqlite` 双驱动 |
| `tauri-plugin-clipboard-manager` | ⚠️ 部分 | 不需要 | 前端已用原生 Clipboard API |
| `tauri-plugin-dialog` | ⚠️ 部分 | 不需要 | 无桌面端文件选取需求；大文件走高速分片上传 |
| `tauri-plugin-barcode-scanner` | ✅ | 不需要 | 当前无扫码需求 |
| `tauri-plugin-biometric` | ✅ | 待评估 | 未来敏感操作（如导出密钥）可引入 |
| `tauri-plugin-geolocation` | ✅ | 不需要 | `AndroidManifest.xml` 已声明定位权限，但当前仅作分布式节点信息展示，未调用原生 GPS |
| `tauri-plugin-haptics` | ✅ | 待引入 | 建议引入以替代前端 `navigator.vibrate()`，提供一致的触觉反馈 |
| `tauri-plugin-nfc` | ✅ | 不需要 | 当前无 NFC 需求 |
| `tauri-plugin-deep-link` | ⚠️ 部分 | 待评估 | 若未来支持分享链接唤起应用，可引入 |
| `tauri-plugin-upload` | ✅ | 不需要 | 已自研分片上传（`init_chunked_upload` / `append_chunk` / `finish_chunked_upload`） |
| `tauri-plugin-os` | ✅ | 不需要 | 前端已用 `@vueuse/core` 获取平台信息 |
| `tauri-plugin-localhost` | ✅ | 不需要 | 未使用 localhost server 方案 |

### 2.2 自定义插件（Custom）

| 插件名 | Android 支持 | 当前状态 | 备注 |
|--------|-------------|----------|------|
| `tauri-plugin-vcp-mobile` | ✅ | **待开发** | 统一管理全部 Android 原生能力，见第 3 节 |

### 2.3 不适用于 Android 的官方插件

以下插件**明确不支持 Android**，禁止在移动端引入：

- `tauri-plugin-autostart`
- `tauri-plugin-notification`（Tauri v2 通知插件仅限桌面端）
- `tauri-plugin-updater`（移动端使用应用商店或 OTA 自研方案）
- `tauri-plugin-global-shortcut`
- `tauri-plugin-shell`（移动端受限，使用 `opener` 替代）

---

## 3. 自定义插件架构：`tauri-plugin-vcp-mobile`

### 3.1 设计哲学：Single Unified Plugin

采用**单一插件统一管理**（single plugin统一管理）策略，而非为每个功能拆分为独立插件。理由：

1. **减少维护面**：一个 Cargo 依赖、一个 npm 包、一组权限声明
2. **共享基础设施**：Kotlin 侧的 `FrontendBridge` 替换为 Tauri 原生的 `Plugin.trigger()`，统一事件通道
3. **iOS 扩展友好**：未来只需在插件内新增 `ios/` 目录，前端 API 完全复用
4. **避免 gen/android 污染**：插件源码位于 `src-tauri/plugins/`（或独立仓库），永不触碰 `gen/android`

### 3.2 目录结构

```
src-tauri/plugins/vcp-mobile/
├── Cargo.toml                          # 插件 Rust 清单
├── README.md
├── build.rs                            # Tauri 插件构建脚本
├── android/
│   ├── build.gradle.kts
│   ├── src/main/AndroidManifest.xml
│   └── src/main/java/com/vcp/mobile/
│       ├── VcpMobilePlugin.kt          # 插件入口（继承 Plugin(activity)）
│       ├── ScreenManager.kt            # 屏幕常亮管理
│       ├── StreamServiceManager.kt     # 前台服务启动/停止/通知
│       ├── NavigationManager.kt        # 返回键拦截
│       ├── KeyboardInsetsManager.kt    # 键盘 Insets 监听
│       └── LifecycleBridge.kt          # 生命周期事件发射
├── ios/                                # （未来）Swift 实现占位
│   └── README.md
├── permissions/
│   ├── default.toml                    # 默认权限组（聚合所有子权限）
│   ├── screen.toml
│   ├── stream.toml
│   ├── navigation.toml
│   ├── keyboard.toml
│   └── lifecycle.toml
├── src/
│   ├── lib.rs                          # 插件入口：Tauri Plugin Builder
│   ├── screen.rs                       # `set_keep_screen_on` / `clear_keep_screen_on`
│   ├── stream.rs                       # `start_stream_service` / `stop_stream_service`
│   ├── navigation.rs                   # `attach_back_navigation`
│   ├── keyboard.rs                     # `query_keyboard_state`
│   └── lifecycle.rs                    # `register_lifecycle_listener`
└── guest-js/
    ├── index.ts                        # 前端统一导出
    ├── screen.ts
    ├── stream.ts
    ├── navigation.ts
    ├── keyboard.ts
    └── lifecycle.ts
```

### 3.3 Rust 侧模块设计

#### `src/lib.rs`

```rust
use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod screen;
mod stream;
mod navigation;
mod keyboard;
mod lifecycle;

/// 插件状态：持有 Android 侧可能需要的共享计数器/配置
pub struct VcpMobilePluginState {
    pub streaming_count: std::sync::atomic::AtomicU32,
}

impl Default for VcpMobilePluginState {
    fn default() -> Self {
        Self {
            streaming_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

/// 初始化插件
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vcp-mobile")
        .invoke_handler(tauri::generate_handler![
            screen::set_keep_screen_on,
            screen::clear_keep_screen_on,
            stream::start_stream_service,
            stream::stop_stream_service,
            navigation::attach_back_navigation,
            keyboard::query_keyboard_state,
            lifecycle::register_lifecycle_listener,
        ])
        .setup(|app, api| {
            app.manage(VcpMobilePluginState::default());

            // Android 侧初始化钩子
            #[cfg(target_os = "android")]
            {
                let handle = app.clone();
                api.register_android_plugin(
                    "com.vcp.mobile",
                    "VcpMobilePlugin",
                )?;
            }

            Ok(())
        })
        .build()
}
```

#### `src/screen.rs`

将现有 `screen_wake_manager.rs` 的 JNI 裸调迁移为插件 Command。Tauri v2 插件模式下，仍可通过 `api` 与 Kotlin 通信，但推荐**简单 Window 操作继续保留 Rust 侧 JNI**（见第 6 节“Raw JNI 适用场景”），或封装为插件内部辅助函数。

```rust
/// 设置屏幕常亮
#[tauri::command]
pub async fn set_keep_screen_on<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.vcp_mobile().set_keep_screen_on()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 清除屏幕常亮
#[tauri::command]
pub async fn clear_keep_screen_on<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        app.vcp_mobile().clear_keep_screen_on()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
```

> **注意**：若选择保留 Rust 侧直接 JNI（不通过 Kotlin 中转），则 `screen.rs` 可维持当前 `with_webview` + `jni_handle().exec()` 实现，仅需将 `#[tauri::command]` 注册点从 `lib.rs` 移至插件内。

#### `src/stream.rs`

将 `stream_service_manager.rs` 的计数器逻辑保留在 Rust，但把 Android 服务启动/停止委托给 Kotlin 侧插件方法：

```rust
use tauri::{AppHandle, Manager, Runtime};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct StreamingState {
    pub active_count: AtomicU32,
}

impl Default for StreamingState {
    fn default() -> Self {
        Self { active_count: AtomicU32::new(0) }
    }
}

#[tauri::command]
pub async fn start_stream_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
) -> Result<(), String> {
    let state = app.state::<StreamingState>();
    let count = state.active_count.fetch_add(1, Ordering::SeqCst);

    if count == 0 {
        #[cfg(target_os = "android")]
        {
            app.vcp_mobile()
                .start_streaming_service(agent_name)
                .map_err(|e| e.to_string())?;
        }
    }

    log::info!("[VcpMobilePlugin] Stream active count: {}", count + 1);
    Ok(())
}

#[tauri::command]
pub async fn stop_stream_service<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    let state = app.state::<StreamingState>();
    let count = state.active_count.fetch_sub(1, Ordering::SeqCst);

    if count <= 1 {
        state.active_count.store(0, Ordering::SeqCst);
        #[cfg(target_os = "android")]
        {
            app.vcp_mobile()
                .stop_streaming_service()
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}
```

### 3.4 Android 侧模块设计

#### `android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt`

Tauri v2 插件要求 Kotlin 入口类继承 `Plugin(activity)`，并使用 `@Command` / `@TauriPlugin` 注解：

```kotlin
package com.vcp.mobile

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import app.tauri.plugin.Invoke

@TauriPlugin
class VcpMobilePlugin(private val activity: Activity) : Plugin(activity) {

    private val screenManager = ScreenManager(activity)
    private val streamServiceManager = StreamServiceManager(activity)
    private val navigationManager = NavigationManager(activity)
    private val keyboardInsetsManager = KeyboardInsetsManager(activity)
    private val lifecycleBridge = LifecycleBridge { event, payload ->
        // 替代旧的 evaluateJavascript，使用 Tauri 原生事件通道
        trigger(event, payload)
    }

    // ==================================================================
    // Screen
    // ==================================================================
    @Command
    fun setKeepScreenOn(invoke: Invoke) {
        screenManager.setKeepScreenOn()
        invoke.resolve()
    }

    @Command
    fun clearKeepScreenOn(invoke: Invoke) {
        screenManager.clearKeepScreenOn()
        invoke.resolve()
    }

    // ==================================================================
    // Stream Service
    // ==================================================================
    @Command
    fun startStreamingService(invoke: Invoke) {
        val args = invoke.parseArgs(StartStreamArgs::class.java)
        streamServiceManager.start(args.agentName)
        invoke.resolve()
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        streamServiceManager.stop()
        invoke.resolve()
    }

    // ==================================================================
    // Keyboard
    // ==================================================================
    @Command
    fun queryKeyboardState(invoke: Invoke) {
        val state = keyboardInsetsManager.queryCurrentState()
        val ret = JSObject()
        ret.put("height", state.height)
        ret.put("visible", state.visible)
        invoke.resolve(ret)
    }

    // ==================================================================
    // Lifecycle & Navigation 的绑定在 load() 中完成
    // ==================================================================
    override fun load(webView: android.webkit.WebView) {
        super.load(webView)
        navigationManager.attach(webView)
        keyboardInsetsManager.attach(webView)
        lifecycleBridge.attach(activity)
    }

    override fun onDestroy() {
        lifecycleBridge.detach()
        navigationManager.detach()
        super.onDestroy()
    }
}

@InvokeArg
class StartStreamArgs {
    lateinit var agentName: String
}
```

> **关键变更**：`FrontendBridge.emit()` 的 `evaluateJavascript` 调用方式全面替换为 `Plugin.trigger(eventName, JSObject)`。前端监听方式从 `window.addEventListener("vcp-lifecycle", ...)` 改为 Tauri 标准 `listen("vcp-lifecycle", ...)`。

#### `android/src/main/java/com/vcp/mobile/KeyboardInsetsManager.kt`

迁移后的核心变化：

```kotlin
package com.vcp.mobile

import android.app.Activity
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.tauri.plugin.JSObject

class KeyboardInsetsManager(private val activity: Activity) {

    private var webViewRef: WebView? = null

    fun attach(webView: WebView) {
        webViewRef = webView
        val rootView = activity.window.decorView.rootView

        ViewCompat.setOnApplyWindowInsetsListener(rootView) { _, insets ->
            val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val isKeyboardVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            val keyboardHeight = if (isKeyboardVisible) ime.bottom else 0

            // 获取插件实例并触发事件（需在 VcpMobilePlugin 中提供引用）
            val payload = JSObject().apply {
                put("height", keyboardHeight)
                put("visible", isKeyboardVisible)
                put("safeAreaBottom", systemBars.bottom)
            }

            // 通过 Plugin.trigger 发射到前端
            // 注意：此处需持有 Plugin 实例引用，或通过回调委托
            onKeyboardChanged?.invoke(payload)

            insets
        }
    }

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

    data class KeyboardState(val height: Int, val visible: Boolean)

    internal var onKeyboardChanged: ((JSObject) -> Unit)? = null
}
```

#### `android/src/main/java/com/vcp/mobile/LifecycleBridge.kt`

```kotlin
package com.vcp.mobile

import android.app.Activity
import android.content.res.Configuration
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.JSObject

class LifecycleBridge(
    private val emit: (String, JSObject) -> Unit
) : DefaultLifecycleObserver {

    fun attach(activity: Activity) {
        if (activity is LifecycleOwner) {
            activity.lifecycle.addObserver(this)
        }
    }

    fun detach() {
        // lifecycle 会自动清理
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
```

### 3.5 权限设计

Tauri v2 插件使用基于能力的权限系统（Capability-based Permission）。`tauri-plugin-vcp-mobile` 采用**分组权限**策略：

#### `permissions/default.toml`

```toml
[[permission]]
identifier = "default"
description = "默认权限组，包含所有 VCP Mobile 插件能力"
permissions = [
    "screen",
    "stream",
    "navigation",
    "keyboard",
    "lifecycle",
]
```

#### `permissions/screen.toml`

```toml
[[permission]]
identifier = "screen"
description = "允许控制屏幕常亮状态"
commands.allow = ["set_keep_screen_on", "clear_keep_screen_on"]
```

#### `permissions/stream.toml`

```toml
[[permission]]
identifier = "stream"
description = "允许启动/停止流式前台保活服务"
commands.allow = ["start_stream_service", "stop_stream_service"]
```

#### Capability 注册（`src-tauri/capabilities/mobile.json`）

```json
{
  "identifier": "mobile-capability",
  "platforms": ["android"],
  "permissions": [
    "vcp-mobile:default"
  ]
}
```

### 3.6 前端绑定（`guest-js/`）

```typescript
// guest-js/index.ts
export * from './screen';
export * from './stream';
export * from './keyboard';
export * from './lifecycle';
export * from './navigation';

// guest-js/screen.ts
import { invoke } from '@tauri-apps/api/core';

export function setKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|set_keep_screen_on');
}

export function clearKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|clear_keep_screen_on');
}

// guest-js/stream.ts
import { invoke } from '@tauri-apps/api/core';

export function startStreamService(agentName: string): Promise<void> {
  return invoke('plugin:vcp-mobile|start_stream_service', { agentName });
}

export function stopStreamService(): Promise<void> {
  return invoke('plugin:vcp-mobile|stop_stream_service');
}

// guest-js/keyboard.ts
import { invoke } from '@tauri-apps/api/core';

export interface KeyboardState {
  height: number;
  visible: boolean;
}

export function queryKeyboardState(): Promise<KeyboardState> {
  return invoke('plugin:vcp-mobile|query_keyboard_state');
}

// guest-js/lifecycle.ts
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface LifecycleEvent {
  state: 'resume' | 'pause' | 'stop' | 'config-changed' | 'low-memory';
  isDarkMode?: boolean;
}

export function onLifecycleEvent(
  handler: (event: LifecycleEvent) => void
): Promise<UnlistenFn> {
  return listen<LifecycleEvent>('vcp-lifecycle', (e) => handler(e.payload));
}

// guest-js/navigation.ts
import { invoke } from '@tauri-apps/api/core';

export function attachBackNavigation(): Promise<void> {
  return invoke('plugin:vcp-mobile|attach_back_navigation');
}
```

前端使用方式（对齐现有 Pinia store / composable）：

```typescript
// 替换现有 import { invoke } from '@tauri-apps/api/core' 中的裸调
import { setKeepScreenOn, clearKeepScreenOn } from 'tauri-plugin-vcp-mobile';

// syncSession.ts
const startSync = () => {
  setKeepScreenOn().catch(() => {});
  // ...
};
```

---

## 4. 插件开发工作流

### 4.1 脚手架创建

```bash
# 1. 在 src-tauri/plugins/ 目录下创建插件
npx @tauri-apps/cli plugin new --name vcp-mobile --android

# 2. 若需同时预留 iOS 扩展（当前不实现，但保留目录）
npx @tauri-apps/cli plugin new --name vcp-mobile --android --ios
```

生成的目录结构：

```
src-tauri/plugins/vcp-mobile/
├── Cargo.toml
├── package.json          # 前端绑定 npm 包配置
├── build.rs
├── android/
│   ├── build.gradle.kts
│   └── src/main/java/[package]/VcpMobilePlugin.kt
├── permissions/
│   └── default.toml
├── src/lib.rs
└── guest-js/index.ts
```

### 4.2 加入 Workspace

#### `src-tauri/Cargo.toml` — 工作区成员

```toml
[workspace]
members = [".", "plugins/vcp-mobile"]
```

#### `src-tauri/Cargo.toml` — 主应用依赖

```toml
[dependencies]
tauri-plugin-vcp-mobile = { path = "./plugins/vcp-mobile" }
```

#### `src-tauri/src/lib.rs` — 注册插件

```rust
.plugin(tauri_plugin_vcp_mobile::init())
```

#### `package.json` — 前端依赖

```json
{
  "dependencies": {
    "tauri-plugin-vcp-mobile": "file:src-tauri/plugins/vcp-mobile"
  }
}
```

执行 `pnpm install` 后，前端即可通过包名引用。

### 4.3 Kotlin `@Command` 注解规范

Tauri v2 Android 插件通过注解反射绑定 Rust invoke → Kotlin 方法。

| 规则 | 说明 |
|------|------|
| 类注解 | `@TauriPlugin` 必须标注在 `Plugin(activity)` 子类上 |
| 方法注解 | `@Command` 标注公开方法，方法名即为 Rust 调用名（camelCase 自动映射） |
| 参数解析 | 方法参数固定为 `Invoke`，通过 `invoke.parseArgs(ArgsClass::class.java)` 获取结构化参数 |
| 参数类 | 参数类必须标注 `@InvokeArg`，字段名与 JSON 键一致 |
| 返回值 | 成功调用必须执行 `invoke.resolve(JSObject?)`；失败执行 `invoke.reject(message)` |
| 线程安全 | `@Command` 默认在主线程执行；耗时操作需自行切至后台线程 |

### 4.4 事件发射：`Plugin.trigger()` vs 旧 `evaluateJavascript`

| 维度 | 旧 `FrontendBridge.emit()` | 新 `Plugin.trigger()` |
|------|---------------------------|----------------------|
| 前端监听 | `window.addEventListener("name", ...)` | `listen("name", ...)` from `@tauri-apps/api/event` |
| 类型安全 | 手动 JSON 序列化，无类型校验 | Tauri IPC 通道，TS 类型自动生成 |
| 生命周期 | WebView  detach 后可能 NPE | Plugin 自动管理，与 WebView 生命周期绑定 |
| 多 WebView | 需手动维护 WebView 引用 | Tauri 框架自动分发至所有 WebView |
| 性能 | `evaluateJavascript` 有额外字符串解析开销 | 原生 JNI 直接调用，开销更低 |

**迁移示例**：

```kotlin
// 旧方式（FrontendBridge.kt）
val script = "window.dispatchEvent(new CustomEvent('vcp-lifecycle', { detail: { state: 'resume' } }))"
webView?.evaluateJavascript(script, null)

// 新方式（VcpMobilePlugin.kt）
val payload = JSObject().apply { put("state", "resume") }
trigger("vcp-lifecycle", payload)
```

前端迁移：

```typescript
// 旧方式
window.addEventListener('vcp-lifecycle', (e: CustomEvent) => { ... });

// 新方式
import { listen } from '@tauri-apps/api/event';
const unlisten = await listen<{ state: string }>('vcp-lifecycle', (event) => { ... });
```

### 4.5 权限声明与 Capability 注册

1. 在 `permissions/` 目录下为每个子功能创建独立的 `.toml` 文件
2. 在 `default.toml` 中聚合为默认权限组
3. 在 `src-tauri/capabilities/mobile.json` 中为 Android 平台声明 `vcp-mobile:default`
4. 构建时 Tauri CLI 自动将权限元数据编译进应用

### 4.6 构建与测试

```powershell
# 1. 插件 Rust 侧编译检查
cd src-tauri/plugins/vcp-mobile
cargo check

# 2. 主应用编译（含插件）
cd ../../
cargo check

# 3. Android 真机/模拟器热重载开发
pnpm tauri android dev

# 4. Release APK 构建（确认插件已正确打包）
pnpm tauri android build --apk --target aarch64
```

---

## 5. 迁移路线图

### 5.1 代码迁移矩阵

| 当前文件/模块 | 迁移目标（插件内） | 迁移策略 | 优先级 |
|--------------|-------------------|----------|--------|
| `gen/android/.../MainActivity.kt` | 精简为默认 Tauri Activity | 删除所有自定义初始化，仅保留 `enableEdgeToEdge()` | P0 |
| `gen/android/.../BackNavigationManager.kt` | `android/.../NavigationManager.kt` | 整体迁移，改为 Plugin 生命周期绑定 | P0 |
| `gen/android/.../insets/KeyboardInsetsManager.kt` | `android/.../KeyboardInsetsManager.kt` | 替换 `evaluateJavascript` 为 `trigger()` | P0 |
| `gen/android/.../lifecycle/AppLifecycleBridge.kt` | `android/.../LifecycleBridge.kt` | 替换事件通道，使用 `DefaultLifecycleObserver` | P0 |
| `gen/android/.../service/StreamKeepaliveService.kt` | `android/.../StreamServiceManager.kt` + `service/` | Service 类保留，启动入口改为 Plugin Command | P0 |
| `gen/android/.../service/StreamingActionReceiver.kt` | `android/.../service/StreamingActionReceiver.kt` | 保留，但注册方式改为 Plugin `load()` 中动态注册 | P1 |
| `gen/android/.../bridge/FrontendBridge.kt` | **删除** | 被 `Plugin.trigger()` 完全替代 | P0 |
| `vcp_modules/screen_wake_manager.rs` | `plugins/vcp-mobile/src/screen.rs` | 整体迁移，Command 注册点移至插件 | P0 |
| `vcp_modules/stream_service_manager.rs` | `plugins/vcp-mobile/src/stream.rs` | 保留 Rust 计数器逻辑，JNI 部分替换为 Plugin 调用 | P0 |
| `vcp_modules/file_manager.rs` | **无需迁移** | 纯 Rust 实现，不依赖 Android 原生层 | — |
| `core/composables/useKeyboardInsets.ts` | 更新事件监听方式 | `window.addEventListener` → `listen()` | P1 |
| `features/chat/ChatView.vue` | 更新生命周期事件监听 | 同上 | P1 |
| `core/stores/syncSession.ts` | 更新 invoke 目标 | `invoke('set_keep_screen_on')` → `setKeepScreenOn()` from plugin | P1 |

### 5.2 需要删除的内容

- `gen/android/app/src/main/java/com/vcp/avatar/bridge/` 整个目录
- `gen/android/app/src/main/java/com/vcp/avatar/MainActivity.kt` 中的自定义模块组装代码（保留类定义和 `enableEdgeToEdge()`）
- Rust `lib.rs` 中裸调的 `set_keep_screen_on`、`clear_keep_screen_on` Command 注册
- Rust `lib.rs` 中对 `StreamingServiceState` 的 `manage()`（移至插件内）
- 前端中所有 `window.addEventListener('vcp-keyboard-inset', ...)` 和 `window.addEventListener('vcp-lifecycle', ...)`

### 5.3 需要替换为官方插件的内容

| 当前实现 | 替换目标 | 时机 |
|---------|---------|------|
| 前端 `navigator.vibrate()` | `tauri-plugin-haptics` | P2（体验优化） |
| 未来 URL Scheme 唤起 | `tauri-plugin-deep-link` | P2（需求驱动） |
| 未来生物识别（密钥导出） | `tauri-plugin-biometric` | P3（安全增强） |

### 5.4 迁移执行顺序（建议）

```
Phase 1: 基建（不改动现有功能）
  ├─ 创建 tauri-plugin-vcp-mobile 脚手架
  ├─ 配置 workspace / Cargo.toml / package.json
  └─ 验证空插件能编译并打包进 APK

Phase 2: 核心迁移（功能对等）
  ├─ 迁移 screen_wake_manager.rs → screen.rs（保留 JNI 或改为 Kotlin 中转）
  ├─ 迁移 stream_service_manager.rs → stream.rs
  ├─ 迁移 KeyboardInsetsManager.kt（替换事件通道）
  ├─ 迁移 AppLifecycleBridge.kt（替换事件通道）
  ├─ 迁移 BackNavigationManager.kt
  └─ 更新前端所有监听与 invoke 调用

Phase 3: 清理与验证
  ├─ 删除 FrontendBridge.kt
  ├─ 精简 MainActivity.kt
  ├─ 运行 pnpm check
  ├─ 真机测试：键盘 Insets、返回键、生命周期、流式服务、屏幕常亮
  └─ git commit 归档

Phase 4: 优化（可选）
  ├─ 引入 tauri-plugin-haptics
  └─ 评估 deep-link / biometric 需求
```

---

## 6. 官方插件采用准则

### 6.1 决策树

```
是否需要与 Android 原生层交互？
├── 否 → 纯 Rust/TS 实现，不引入任何插件
│
└── 是 → 该功能是否已有官方插件覆盖？
    ├── 是 → 优先使用官方插件
    │       └── 官方插件是否满足全部需求？
    │           ├── 是 → 直接引入
    │           └── 否 → 评估 Fork 官方插件 vs 自定义插件
    │               （原则：修改量 < 20% 则 Fork + PR；否则自定义插件）
    │
    └── 否 → 调用复杂度如何？
        ├── 单次简单 API 调用（如 addFlags / clearFlags）
        │   └── 允许保留 Rust 侧 Raw JNI，不创建插件
        │       （screen_wake_manager.rs 即属此类）
        │
        └── 复杂交互（生命周期监听、服务管理、事件发射）
            └── 必须封装为自定义插件（tauri-plugin-vcp-mobile）
```

### 6.2 三条铁律

| 优先级 | 规则 | 示例 |
|--------|------|------|
| 1 | **官方插件优先** | 文件打开用 `tauri-plugin-opener`（已执行），触觉反馈用 `tauri-plugin-haptics` |
| 2 | **自定义插件兜底** | Android 特有的前台服务、键盘 Insets、生命周期桥接，官方无覆盖，必须自建插件 |
| 3 | **Raw JNI 仅限一次性调用** | `FLAG_KEEP_SCREEN_ON` 只需一次 `addFlags()`/`clearFlags()`，维持 Rust 侧 JNI 裸调比新增 Kotlin 方法更简洁 |

### 6.3 Raw JNI 保留场景

以下情况**不需要**将 Rust JNI 代码迁移到 Kotlin 插件：

1. **调用次数极少**（仅 1~2 个 JNI 方法调用）
2. **不涉及前端事件**（无数据需要推送到前端）
3. **无状态管理**（不需要 Android 侧持有计数器/监听器）
4. **调用对象为 Android 框架类**（`Window`, `PowerManager` 等），而非应用自定义类

当前 `screen_wake_manager.rs` 完全符合以上条件，迁移时可选择：
- **方案 A**：维持 Rust 侧 JNI 裸调（推荐，改动最小）
- **方案 B**：封装为 `ScreenManager.kt` + `@Command`（更统一，但增加一层 JNI 往返）

**决策**：采用方案 A，`screen.rs` 仅作为 Command 注册入口保留在插件内，实际 JNI 逻辑不迁移。

---

## 7. 未来 iOS 扩展路径

### 7.1 单插件结构的跨平台优势

`tauri-plugin-vcp-mobile` 的目录结构已天然预留 iOS 扩展能力：

```
src-tauri/plugins/vcp-mobile/
├── android/          # Android Kotlin 实现
├── ios/              # （未来）Swift 实现
│   ├── Package.swift
│   └── Sources/
│       └── VcpMobilePlugin.swift
├── src/
│   └── lib.rs        # 跨平台 Rust 入口（#[cfg] 条件编译）
└── guest-js/
    └── index.ts      # 前端 API 完全复用，零改动
```

### 7.2 各功能 iOS 映射

| Android 功能 | iOS 等价实现 | 复杂度 |
|-------------|-------------|--------|
| 屏幕常亮 | `UIApplication.isIdleTimerDisabled = true` | 低 |
| 流式前台服务 | iOS 不支持前台 Service 概念；改为 `BGTaskScheduler` 或保持后台任务 | 中 |
| 键盘 Insets | `UIKeyboardFrameBeginUserInfoKey` 通知 + `safeAreaInsets` | 低 |
| 返回键拦截 | iOS 无物理返回键；对应手势返回由系统管理，前端路由处理即可 | 无需实现 |
| 生命周期事件 | `UIApplicationDelegate` / `SceneDelegate` 生命周期方法 | 低 |
| 附件外部存储 | `FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)` | 低 |

### 7.3 Rust 侧条件编译

```rust
// src/lib.rs
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("vcp-mobile")
        .invoke_handler(tauri::generate_handler![
            // 跨平台通用命令
            screen::set_keep_screen_on,
            screen::clear_keep_screen_on,
            lifecycle::register_lifecycle_listener,
        ])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            api.register_android_plugin("com.vcp.mobile", "VcpMobilePlugin")?;

            #[cfg(target_os = "ios")]
            api.register_ios_plugin("VcpMobilePlugin")?;

            Ok(())
        })
        .build()
}
```

### 7.4 前端无感知切换

由于前端 API 完全通过 `guest-js/index.ts` 封装，无论底层是 Android Kotlin 还是 iOS Swift，前端调用方式完全一致：

```typescript
import { setKeepScreenOn, onLifecycleEvent } from 'tauri-plugin-vcp-mobile';

// Android 和 iOS 通用
await setKeepScreenOn();
await onLifecycleEvent((e) => console.log(e.state));
```

---

## 8. 附录：参考链接

| 资源 | URL |
|------|-----|
| Tauri v2 Plugin 开发文档 | https://v2.tauri.app/develop/plugins/ |
| Tauri Android Plugin 详解 | https://v2.tauri.app/develop/plugins/develop-mobile/#android |
| Tauri Plugin Permissions | https://v2.tauri.app/security/permissions/ |
| `@tauri-apps/cli plugin new` | https://v2.tauri.app/reference/cli/#plugin-new |
| Tauri v2 官方插件仓库 | https://github.com/tauri-apps/plugins-workspace |
| VCP Mobile 同步架构文档 | `./SYNC_ARCHITECTURE.md` |
| VCP Mobile UI 层级规范 | `./UI_LAYER_ARCHITECTURE.md` |

---

*本文档遵循 VCP Mobile 工程纪律：任何涉及 `src-tauri/` 核心架构的修改前，必须先执行 `git add . && git commit -m "save"` 进行存档。*
