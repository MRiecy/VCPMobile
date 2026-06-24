---
id: PLUGIN-GUARDIAN-011
title: ForegroundGuardian 前台守护者
description: 进程级单例，统一管理 WakeLock + WifiLock + 前台服务的引用计数与优先级调度。v1.1.2 新增
version: 1.1.2
date: 2026-06-24
related_files:
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
  - src-tauri/plugins/vcp-mobile/src/stream.rs
  - src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml
---

# ForegroundGuardian 前台守护者

## 1. 功能概述

`ForegroundGuardian` 是 v1.1.2 引入的**进程级 Kotlin 单例**，将此前分散在 `VcpMobileState.active_streams`（Rust 侧）与 `StreamKeepaliveService` / `VcpMobilePlugin`（Kotlin 侧）中的**双锁 + 前台服务**管理逻辑统一收敛到一个入口，实现以下三大能力的生命周期协同：

| 能力 | API | 作用 |
|------|-----|------|
| **CPU WakeLock** | `PowerManager.PARTIAL_WAKE_LOCK` | 保持 CPU 持续运转，防止深度休眠中断流式连接 |
| **WifiLock** | `WifiManager.WIFI_MODE_FULL_HIGH_PERF` | 防止后台 Wi-Fi 睡眠或降速，保障 SSE/WebSocket 长连接 |
| **前台服务 (FGS)** | `StreamKeepaliveService` 通知栏常驻 | 向 Android 系统声明"用户感知的重要任务"，大幅降低 OEM 杀后台概率 |

> **核心设计哲学**：引用计数 + 优先级调度。多模块（同步、流式对话、预渲染、分布式保活）可并发申请锁，互不冲突；通知栏文案始终展示优先级最高的活跃消费者；最后一个消费者退出时自动释放全部物理资源，绝不空占。

### 与旧架构的对比

| 维度 | v1.1.1 及以前 | v1.1.2 (ForegroundGuardian) |
|------|--------------|---------------------------|
| 引用计数位置 | Rust `active_streams: Mutex<Vec<(String, u32)>>` | Kotlin `ConcurrentHashMap<String, ConsumerEntry>` |
| 锁管理 | `StreamKeepaliveService` 内部自行管理 WakeLock/WifiLock | `ForegroundGuardian` 统一管理，Service 不再持有锁 |
| 前台服务启动 | `VcpMobilePlugin.startStreamingService()` 直接创建 Intent | 全部经由 `ForegroundGuardian.acquire()` → `startFgs()` |
| 通知文案 | Agent 名称字符串拼接（"A、B、C..."） | 按优先级自动选定最高优先级的 `displayLabel` |
| 屏幕常亮 | `VcpMobilePlugin` 中 `isScreenKeepOnActive` 布尔开关 | `isScreenKeepOnRequired` 动态计算（任一消费者需求即 true） |
| 分布式保活 | Rust `set_keepalive_mode()` 独立 Tauri 命令 | 作为 `PRIORITY_DISTRIBUTED(10)` 消费者接入同一机制 |

---

## 2. 代码结构

```
ForegroundGuardian.kt (211 lines)
├── 优先级常量 (4 级)
├── consumers: ConcurrentHashMap<String, ConsumerEntry>   // 消费者注册表
├── wakeLock: PowerManager.WakeLock?                      // 全局 CPU 锁
├── wifiLock: WifiManager.WifiLock?                        // 全局 Wi-Fi 锁
├── ConsumerEntry(priority, displayLabel, screenKeepOn)    // 消费者数据结构
│
├── acquire(context, tag, priority, label, screenKeepOn)   // 申请持锁（幂等）
├── release(context, tag)                                   // 释放持锁（幂等）
├── releaseAllLocks()                                       // 紧急自毁（清空全部）
│
├── isActive: Boolean                                       // 是否有活跃消费者
├── isScreenKeepOnRequired: Boolean                         // 是否需要屏幕常亮
├── getNotificationLabel(): String                          // 最高优先级者通知文案
│
├── acquireLocks(context)      [private]                   // 物理获取双锁
├── releaseLocks()              [private]                   // 物理释放双锁
├── startFgs(context)           [private]                   // 启动前台服务
├── updateFgs(context)          [private]                   // 更新前台服务通知
└── stopFgs(context)            [private]                   // 停止前台服务
```

---

## 3. 优先级系统

### 3.1 四级常量

```kotlin
const val PRIORITY_SYNC = 40        // 数据同步 — 最高，需屏幕常亮
const val PRIORITY_PRERENDER = 30   // 预渲染重建 — 高，需屏幕常亮
const val PRIORITY_STREAM = 20      // 普通流式对话 — 中，不保持屏幕
const val PRIORITY_DISTRIBUTED = 10 // 分布式后台保活 — 低，不保持屏幕
```

### 3.2 消费者分配表

| 消费者 Tag | 优先级 | screenKeepOn | 通知 Label | 发起方 |
|-----------|:------:|:------------:|-----------|--------|
| `"sync"` | 40 | ✅ | `[数据同步]` | `VcpMobilePlugin.startStreamingService()` 识别人工 Agent 名 |
| `"prerender"` | 30 | ✅ | `[预渲染重建]` | `VcpMobilePlugin.startStreamingService()` 识别人工 Agent 名 |
| `"stream:{AgentName}"` | 20 | ❌ | 用户可见 Agent 名 | `stream.rs` → `start_stream_service_inner()` |
| `"distributed"` | 10 | ❌ | `[分布式后台保活]` | `stream.rs` → `set_keepalive_mode_inner()` / `vcp_log_service` linger |
| `"manual_keepalive"` | 10 | ❌ | `[后台保活]` | `VcpMobilePlugin.acquireWakeLock()`（旧 API 兼容） |

> **通知文案选择逻辑**：`getNotificationLabel()` 遍历 `consumers.values`，返回 `maxByOrNull { it.priority }?.displayLabel`。若有同步任务在运行，通知始终展示"正在与云端服务器进行高精度同步..."而非普通 Agent 名。

---

## 4. 引用计数机制

### 4.1 数据结构

```kotlin
private val consumers = ConcurrentHashMap<String, ConsumerEntry>()

data class ConsumerEntry(
    val priority: Int,        // 优先级（数值越大越高）
    val displayLabel: String, // 通知栏中展示的文案
    val screenKeepOn: Boolean // 是否需要保持屏幕常亮
)
```

- **线程安全**：使用 `ConcurrentHashMap`，支持多线程并发 acquire/release，无需外部加锁。
- **同步保护**：`acquire()` 和 `release()` 方法本身标记 `@Synchronized`，确保"判空 → 获取锁"的原子性（防止两个消费者同时作为"首个"进入时重复获取物理锁）。
- **Tag 唯一性**：每个消费者通过唯一的 `tag` 标识。重复调用 `acquire` 同一 tag 会**覆盖更新**（幂等），而不会重复计数——这点与旧 Rust 侧的 `<String, u32>` 计数模型不同。

### 4.2 Acquire 流程

```
acquire(context, tag, priority, label, screenKeepOn)
  │
  ├── wasEmpty = consumers.isEmpty()
  ├── consumers[tag] = ConsumerEntry(priority, label, screenKeepOn)
  │
  ├── if wasEmpty:                // 首个消费者
  │   ├── acquireLocks(context)   //    → 物理获取 WakeLock + WifiLock
  │   └── startFgs(context)       //    → 启动 StreamKeepaliveService 前台服务
  │
  └── else:                       // 已有消费者
      └── updateFgs(context)      //    → 仅触发 onStartCommand 更新通知
```

### 4.3 Release 流程

```
release(context, tag)
  │
  ├── consumers.containsKey(tag)? → No → return (忽略未注册 tag)
  ├── consumers.remove(tag)
  │
  ├── if consumers.isEmpty():     // 最后一个消费者退出
  │   ├── releaseLocks()          //    → 物理释放 WakeLock + WifiLock
  │   └── stopFgs(context)        //    → 停止前台服务
  │
  └── else:                       // 仍有消费者
      └── updateFgs(context)      //    → 更新通知为次高优先级者的文案
```

### 4.4 紧急自毁

```kotlin
@Synchronized
fun releaseAllLocks() {
    consumers.clear()
    releaseLocks()
}
```

`releaseAllLocks()` 被以下路径调用：
- `StreamKeepaliveService.onDestroy()`：前台服务被系统杀死或 `stopFgs()` 触发的自杀，作为兜底清扫防止锁泄漏。
- `VcpMobilePlugin.stopStreamingService()` 空参数分支：前端发出"全部停止"信号时。

---

## 5. 物理锁管理

### 5.1 WakeLock

```kotlin
val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
wakeLock = powerManager?.newWakeLock(
    PowerManager.PARTIAL_WAKE_LOCK,
    "VCP:ForegroundGuardian"
)
```

- **类型**：`PARTIAL_WAKE_LOCK` — 保持 CPU 运行，但不强制屏幕和键盘背光亮起。
- **标签**：`"VCP:ForegroundGuardian"` — 在 `dumpsys power` 中可识别。
- **安全保护**：获取前检查 `isHeld` 防重复获取；释放后置 `null` 防二次释放。

### 5.2 WifiLock

```kotlin
val wifiManager = appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
    wifiManager.createWifiLock(
        WifiManager.WIFI_MODE_FULL_HIGH_PERF,
        "VCP:ForegroundGuardianWifi"
    )
} else {
    @Suppress("DEPRECATION")
    wifiManager.createWifiLock(
        WifiManager.WIFI_MODE_FULL,
        "VCP:ForegroundGuardianWifi"
    )
}
```

| API 级别 | 模式 | 行为 |
|----------|------|------|
| Android 10+ (API 29+) | `WIFI_MODE_FULL_HIGH_PERF` | 满性能 Wi-Fi 锁，延迟最低、不降速 |
| Android 9 及以下 | `WIFI_MODE_FULL` (deprecated) | 传统满性能 Wi-Fi 锁 |

- **标签**：`"VCP:ForegroundGuardianWifi"` — 在 `dumpsys wifi` 中可识别。
- 同样实现获取前 `isHeld` 检查和释放后 `null` 置空保护。

---

## 6. 前台服务集成

### 6.1 服务启动 / 更新 / 停止

全部通过 `Intent` 路由到 `StreamKeepaliveService`：

```kotlin
// 启动
private fun startFgs(context: Context) {
    val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
    context.applicationContext.startForegroundService(intent)  // API 26+
}

// 更新（复用启动路径触发 onStartCommand）
private fun updateFgs(context: Context) {
    val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
    context.applicationContext.startForegroundService(intent)  // 轻量刷新通知
}

// 停止
private fun stopFgs(context: Context) {
    val intent = Intent(context.applicationContext, StreamKeepaliveService::class.java)
    context.applicationContext.stopService(intent)
}
```

> **设计意图**：`updateFgs()` 复用 `startForegroundService()` 而非直接操作 `NotificationManager`。这利用了 Android 的机制：对已在运行的前台服务再次调用 `startForegroundService()` 会触发 `onStartCommand()`，Service 在其中通过 `ForegroundGuardian.getNotificationLabel()` 获取最新文案并更新通知——一条路径同时覆盖启动和更新。

### 6.2 StreamKeepaliveService 的角色转变

在 v1.1.2 中，`StreamKeepaliveService` 从"锁管理者"降级为"通知壳"：

| 职责 | v1.1.1 | v1.1.2 |
|------|--------|--------|
| WakeLock 管理 | ✅ 自己管理 | ❌ 委托 ForegroundGuardian |
| WifiLock 管理 | ✅ 自己管理 | ❌ 委托 ForegroundGuardian |
| 通知构建 | ✅ 根据 Agent 名 | ✅ 根据 ForegroundGuardian.getNotificationLabel() |
| startForeground() | ✅ | ✅（仅此一项保留） |
| onDestroy 锁清扫 | ❌ 无 | ✅ 调用 ForegroundGuardian.releaseAllLocks() |

---

## 7. 屏幕常亮协同

```kotlin
val isScreenKeepOnRequired: Boolean
    get() = consumers.values.any { it.screenKeepOn }
```

`VcpMobilePlugin` 的 Activity 生命周期回调中，每次 `onResume` / `onPause` 均查询此属性：

- `onResume` + `isScreenKeepOnRequired == true` → JNI 添加 `FLAG_KEEP_SCREEN_ON`
- `onPause` 或 `isScreenKeepOnRequired` 变为 `false` → JNI 清除 `FLAG_KEEP_SCREEN_ON`

这替代了旧版 `isScreenKeepOnActive` 布尔开关，使屏幕常亮与具体业务（同步/预渲染）精确绑定，而非笼统的"流式进行中"。

---

## 8. 数据流完整链路

```
┌──────────────────────────────────────────────────────────────┐
│ Rust stream.rs                                               │
│                                                              │
│ start_stream_service_inner("Agent-A")                        │
│   → tag = "stream:Agent-A", priority = 20, screenKeepOn = F  │
│   → acquire_foreground_inner(tag, 20, "Agent-A", false)      │
│       → run_mobile_plugin("acquireForeground", {...})        │
└──────────────────────┬───────────────────────────────────────┘
                       │ Tauri IPC
                       ▼
┌──────────────────────────────────────────────────────────────┐
│ VcpMobilePlugin.kt                                           │
│                                                              │
│ @Command fun acquireForeground(invoke) {                     │
│     val args = parseArgs(AcquireForegroundArgs)              │
│     ForegroundGuardian.acquire(                              │
│         context, args.tag, args.priority,                    │
│         args.label, args.screenKeepOn                        │
│     )                                                        │
│ }                                                            │
└──────────────────────┬───────────────────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────────────────┐
│ ForegroundGuardian.kt                                        │
│                                                              │
│ acquire(tag="stream:Agent-A", priority=20, label="Agent-A")  │
│   ├── consumers["stream:Agent-A"] = ConsumerEntry(20, ...)   │
│   ├── wasEmpty? → YES (首个消费者)                            │
│   │   ├── acquireLocks()  → WakeLock + WifiLock 物理获取      │
│   │   └── startFgs()      → startForegroundService(intent)   │
│   │                           │                              │
│   │                           ▼                              │
│   │              StreamKeepaliveService.onStartCommand()     │
│   │                ├── label = getNotificationLabel()        │
│   │                │     → "Agent-A" (唯一消费者)             │
│   │                ├── buildNotification(label)              │
│   │                └── startForeground(NOTIFICATION_ID, ...) │
│   │                                                          │
│   └── (后续消费者: Agent-B 进入)                               │
│       ├── consumers["stream:Agent-B"] = ConsumerEntry(20, ...)│
│       └── wasEmpty? → NO → updateFgs()                       │
│           → getNotificationLabel() → 最高优先级者             │
└──────────────────────────────────────────────────────────────┘
```

---

## 9. 调用方整合

### 9.1 Rust `stream.rs` — Agent 流式对话

```rust
// start_stream_service_inner() 根据 Agent 名自动推导优先级
let priority = if agent_name.contains("[数据同步]") {
    40  // PRIORITY_SYNC
} else if agent_name.contains("[预渲染重建]") {
    30  // PRIORITY_PRERENDER
} else {
    20  // PRIORITY_STREAM
};
let screen_keep_on = priority >= 30;  // 同步和预渲染保持屏幕常亮
let tag = format!("stream:{}", agent_name);
acquire_foreground_inner(app, &tag, priority, agent_name, screen_keep_on)
```

### 9.2 Rust `stream.rs` — 分布式保活

```rust
pub fn set_keepalive_mode_inner(app, is_keepalive) {
    if is_keepalive {
        acquire_foreground_inner(app, "distributed", 10, "distributed", false)
    } else {
        release_foreground_inner(app, "distributed")
    }
}
```

### 9.3 Kotlin `VcpMobilePlugin.kt` — startStreamingService 重构

```kotlin
@Command fun startStreamingService(invoke: Invoke) {
    val args = parseArgs(StartStreamArgs)
    val name = args.agentName

    when {
        // 空名 + 保活模式 → 分布式标签
        name.isEmpty() && args.isKeepalive == true ->
            ForegroundGuardian.acquire(context, "distributed", PRIORITY_DISTRIBUTED, "distributed")

        // 空名 + 非保活 → 释放全部同步/渲染/流式标签
        name.isEmpty() -> {
            ForegroundGuardian.release(context, "sync")
            ForegroundGuardian.release(context, "prerender")
            ForegroundGuardian.release(context, "stream_default")
        }

        // 同步 Agent → PRIORITY_SYNC + 屏幕常亮
        name.contains("[数据同步]") ->
            ForegroundGuardian.acquire(context, "sync", PRIORITY_SYNC, name, true)

        // 预渲染 Agent → PRIORITY_PRERENDER + 屏幕常亮
        name.contains("[预渲染重建]") ->
            ForegroundGuardian.acquire(context, "prerender", PRIORITY_PRERENDER, name, true)

        // 普通 Agent → PRIORITY_STREAM
        else -> ForegroundGuardian.acquire(context, "stream:$name", PRIORITY_STREAM, name, false)
    }
}
```

### 9.4 Kotlin `VcpMobilePlugin.kt` — 手动保活（旧 API 兼容）

```kotlin
// acquireWakeLock → 委托给 ForegroundGuardian
ForegroundGuardian.acquire(context, "manual_keepalive", 10, "[后台保活]")

// releaseWakeLock → 委托给 ForegroundGuardian
ForegroundGuardian.release(context, "manual_keepalive")
```

---

## 10. `stopWithTask` 行为变更

`AndroidManifest.xml` 中 `StreamKeepaliveService` 的 `android:stopWithTask` 从 **`false` → `true`**：

| 版本 | 值 | 行为 |
|------|-----|------|
| v1.1.1 | `false` | 用户从最近任务划掉应用后，前台服务继续存活 |
| v1.1.2 | `true` | 用户划掉应用后，服务随 Task 一同停止 |

**变更理由**：ForegroundGuardian 已接管全部锁管理。进程被用户主动划掉后，继续维持前台服务变得没有意义——锁在进程内，进程死则锁消亡。设为 `true` 可避免残留的"僵尸通知"，改善用户体验。

---

## 11. 关键约束与血训

1. **`@Synchronized` 是必需的**：`acquire()` 和 `release()` 必须互斥执行。若不加同步，两个线程可能同时读到 `consumers.isEmpty() == true`，双双调用 `acquireLocks()` 导致重复获取锁（虽然 `isHeld` 检查能防住二次物理获取，但语义上不正确）。

2. **`releaseAllLocks()` 不能省略 `consumers.clear()`**：若不清空注册表而只释放物理锁，后续 `acquire()` 将看到 `consumers.isNotEmpty()` 从而跳过 `acquireLocks()`，导致传入的消费者永远拿不到物理锁。

3. **物理锁置 null 是防御性编程**：`releaseLocks()` 在 `release()` 后立即 `wakeLock = null`。如果 Kotlin 的 `PowerManager.WakeLock` 被 GC 回收时未 release，可能导致系统 `PowerManagerService` 中残留引用。置 null 配合 `isHeld` 检查形成双保险。

4. **不要跨进程共享 ForegroundGuardian**：`WakeLock` 和 `WifiLock` 是进程级资源，Binder 无法传递。ForegroundGuardian 必须始终作为调用进程内的单例使用。`distributed/` 的保活调用通过 `set_keepalive_mode_inner()` 而非跨进程方式正是基于此原则。

5. **通知文案语义化**：`displayLabel` 直接出现在通知栏，需确保中文文本精炼（≤ 15 字）且不含技术内部 ID。`StreamKeepaliveService.buildNotification()` 会基于 label 内容做二次语义化（如包含"数据同步"→显示"正在与云端服务器进行高精度同步..."）。

6. **updateFgs 不是免费操作**：每次 `updateFgs` 都通过 `startForegroundService()` 触发完整的 `onStartCommand` → `startForeground()` 链路。高频 acquire/release 场景下应考虑合并更新。当前设计中，Rust 侧的 `start_stream_service_inner` / `stop_stream_service_inner` 已做了去重优化（同一 Agent 名重复调用仅更新计数而非重复通知 Service）。

---

## 12. 与其他文档的关系

| 文档 | 关系 |
|------|------|
| [03_流式前台保活服务](03_流式前台保活服务.md) | ForegroundGuardian 的消费者；StreamKeepaliveService 是 FGS 壳 |
| [01_插件初始化与命令路由](01_插件初始化与命令路由.md) | 注册 acquire/release_foreground 等 6 条新命令 |
| [06_权限与系统控制](06_权限与系统控制.md) | OEM 自启动/电源管理权限为 ForegroundGuardian 保活提供硬件环境 |
| [05_生命周期桥接](05_生命周期桥接.md) | `set_app_foreground_state` 是 FGS 保活的后台触发源之一 |
| `docs/ANDROID_PLUGIN_MANAGEMENT.md` | 前台服务声明规范、`stopWithTask` 行为准则 |

---

> **维护提示**：ForegroundGuardian 是进程级单例，新增消费者 tag 时请遵循优先级分层：40=同步，30=预渲染，20=流式，10=后台/分布式。不要创造新的"优先级 50"破坏现有调度逻辑。
