---
id: PLUGIN-HARDWARE-009
title: 硬件状态与 Root 访问
description: 设备硬件状态采集（CPU 热分级、GPU 型号、网络连接）与 Root 权限访问体系的实现细节。v1.0.3 新增
version: 1.0.3
date: 2026-06-05
related_files:
  - src-tauri/plugins/vcp-mobile/src/system.rs
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/CpuStatusManager.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/GpuStatusManager.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/NetworkStatusManager.kt
  - src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml
---

# 硬件状态与 Root 访问

## 1. 功能概述

提供设备硬件状态采集与 Root 权限访问体系，涵盖 CPU 热状态分级、GPU 渲染器型号读取、网络连接状态与带宽检测，以及基于 libsu 的 Root 权限检测、命令执行和管理器启动。所有模块均遵循高解耦、模块化设计，通过 Plugin IPC 通道向 Rust 工具层和前端提供数据。

---

## 2. 代码结构

### Rust 侧

```
src-tauri/plugins/vcp-mobile/src/system.rs（硬件与 Root 相关部分）
├── NetworkStatus { connected, type, downSpeedKbps, upSpeedKbps, ip }
├── GpuStatus { renderer, restricted }
├── RootAccessStatus { is_root }
├── RootCommandResult { success, output }
├── LaunchRootManagerResult { success, manager, message }
│
├── get_cpu_thermal_status<R>(app) -> Result<String, String>
├── get_gpu_status<R>(app) -> Result<GpuStatus, String>
├── get_network_status<R>(app) -> Result<NetworkStatus, String>
├── check_root_access<R>(app) -> Result<RootAccessStatus, String>
├── run_root_command<R>(app, command) -> Result<RootCommandResult, String>
└── launch_root_manager<R>(app) -> Result<LaunchRootManagerResult, String>
```

### Kotlin 侧

```
VcpMobilePlugin.kt（硬件状态与 Root 相关部分）
├── 管理器持有
│   ├── cpuStatusManager: CpuStatusManager
│   ├── gpuStatusManager: GpuStatusManager
│   └── networkStatusManager: NetworkStatusManager
│
├── 硬件状态命令
│   ├── getCpuThermalStatus(invoke)  → CpuStatusManager.getThermalStatus()
│   ├── getGpuStatus(invoke)         → GpuStatusManager.getGpuStatusJson()
│   └── getNetworkStatus(invoke)     → NetworkStatusManager.getNetworkStatus()
│
├── Root 访问命令
│   ├── checkRootAccess(invoke)      → Shell.getShell().isRoot
│   ├── runRootCommand(invoke)       → Shell.cmd(command).exec()
│   └── launchRootManager(invoke)    → packageManager.getLaunchIntentForPackage()
│
└── @InvokeArg class RunRootCommandArgs { lateinit var command: String }
```

---

## 3. CpuStatusManager —— CPU 热状态分级

> 源码：`src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/CpuStatusManager.kt`（38 行）

### 3.1 概述

`CpuStatusManager` 是独立的 CPU 热状态检测模块，通过 Android `PowerManager.currentThermalStatus` API（API 29+）获取设备当前 CPU 热状态，并以中文描述返回 7 级热分级。该模块零依赖、无状态，每次调用直接查询系统 API。

### 3.2 Rust 接口

```rust
#[tauri::command]
pub fn get_cpu_thermal_status<R: Runtime>(app: AppHandle<R>) -> Result<String, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|get_cpu_thermal_status` |
| 参数 | 无 |
| 返回值 | `String` —— 中文热状态描述 |
| 桌面端行为 | 始终返回 `"正常"` |

### 3.3 7 级热分级映射

```kotlin
class CpuStatusManager(private val context: Context) {
    fun getThermalStatus(): JSObject {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val powerManager = context.getSystemService(Context.POWER_SERVICE) as PowerManager
            val status = powerManager.currentThermalStatus
            val statusStr = when (status) {
                PowerManager.THERMAL_STATUS_NONE      -> "正常"
                PowerManager.THERMAL_STATUS_LIGHT     -> "轻微发热"
                PowerManager.THERMAL_STATUS_MODERATE  -> "中等发热"
                PowerManager.THERMAL_STATUS_SEVERE    -> "严重发热"
                PowerManager.THERMAL_STATUS_CRITICAL  -> "极热(限频)"
                PowerManager.THERMAL_STATUS_EMERGENCY -> "紧急(限流)"
                PowerManager.THERMAL_STATUS_SHUTDOWN  -> "即将关机"
                else -> "未知"
            }
            result.put("status", statusStr)
        } else {
            result.put("status", "不支持(API<29)")
        }
    }
}
```

| 热状态常量 | 中文描述 | 系统行为 |
|-----------|----------|----------|
| `THERMAL_STATUS_NONE` | 正常 | 无限制 |
| `THERMAL_STATUS_LIGHT` | 轻微发热 | 轻度限制，用户不可感知 |
| `THERMAL_STATUS_MODERATE` | 中等发热 | 开始降低 CPU/GPU 频率 |
| `THERMAL_STATUS_SEVERE` | 严重发热 | 大幅降频，屏幕亮度可能降低 |
| `THERMAL_STATUS_CRITICAL` | 极热(限频) | CPU 被强制限频，UI 卡顿 |
| `THERMAL_STATUS_EMERGENCY` | 紧急(限流) | 严重限制，后台任务被暂停 |
| `THERMAL_STATUS_SHUTDOWN` | 即将关机 | 系统即将因过热关机 |

> **API 版本要求**：`currentThermalStatus` 需要 API 29 (Android 10 Q) 及以上。低于此版本返回 `"不支持(API<29)"`。

### 3.4 数据流

```
前端: invoke('plugin:vcp-mobile|get_cpu_thermal_status')
    │
    ▼
system.rs :: get_cpu_thermal_status(app)
    │
    ▼
run_mobile_plugin("getCpuThermalStatus", {})
    │
    ▼
Kotlin: VcpMobilePlugin.getCpuThermalStatus(invoke)
    │
    ▼
CpuStatusManager.getThermalStatus() → JSObject { status: "正常" }
    │
    ▼
Rust 解包 ThermalResponse { status } → Result<String, String>
```

---

## 4. GpuStatusManager —— GPU 硬件信息拉取

> 源码：`src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/GpuStatusManager.kt`（84 行）

### 4.1 概述

`GpuStatusManager` 通过**动态创建临时 EGL 上下文**读取 GPU 渲染器型号（例如 "Adreno (TM) 740"）。GPU 型号信息仅需查询一次，使用双重检查锁定（Double-Checked Locking）+ `@Volatile` 缓存避免重复的 EGL 初始化开销。

### 4.2 Rust 接口

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStatus {
    pub renderer: String,
    pub restricted: bool,
}

#[tauri::command]
pub fn get_gpu_status<R: Runtime>(app: AppHandle<R>) -> Result<GpuStatus, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|get_gpu_status` |
| 参数 | 无 |
| 返回值 | `GpuStatus { renderer, restricted }` —— `restricted` 固定为 `true`，表示 GPU 信息受限（无 sysfs 功耗/温度数据） |
| 桌面端行为 | 返回 `{ renderer: "PC Mock GPU", restricted: true }` |

### 4.3 EGL 临时上下文技术

GpuStatusManager 采用轻量级 EGL Pbuffer Surface 方案读取 GPU 型号，避免需要 GLSurfaceView 或 Activity：

```
fetchGpuRendererFromEgl()
    │
    ├── EGLContext.getEGL() as EGL10
    ├── eglGetDisplay(EGL_DEFAULT_DISPLAY)
    ├── eglInitialize(dpy, vers)
    │
    ├── eglChooseConfig(dpy, configAttr, configs, 1, numConfig)
    │   └── configAttr: [RED_SIZE=8, GREEN_SIZE=8, BLUE_SIZE=8]
    │
    ├── eglCreatePbufferSurface(dpy, config, [WIDTH=1, HEIGHT=1])
    │   └── 1x1 像素 Pbuffer Surface，最小化 GPU 资源占用
    │
    ├── eglCreateContext(dpy, config, EGL_NO_CONTEXT, [EGL_CONTEXT_CLIENT_VERSION=2])
    │   └── GLES 2.0 上下文
    │
    ├── eglMakeCurrent(dpy, surf, surf, ctx)
    │
    ├── GLES20.glGetString(GLES20.GL_RENDERER) → "Adreno (TM) 740"
    │
    └── 清理（严格按顺序）
        ├── eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT)
        ├── eglDestroyContext(dpy, ctx)
        ├── eglDestroySurface(dpy, surf)
        └── eglTerminate(dpy)
```

**为什么 `restricted` 固定为 `true`？**
在大多数 Android 设备上，`/sys/class/kgsl/` 或 `/sys/class/devfreq/` 等 GPU 性能计数器路径需要 Root 权限读取。`GpuStatusManager` 仅通过 EGL 读取渲染器名称字符串，无法获取 GPU 使用率、频率、温度等运行时指标。`restricted: true` 向前端明确告知此限制。

### 4.4 双重检查锁定缓存

```kotlin
@Volatile
private var cachedGpuRenderer: String? = null

fun getGpuRenderer(): String {
    cachedGpuRenderer?.let { return it }     // 第一次检查（无锁，快速路径）
    synchronized(this) {
        cachedGpuRenderer?.let { return it } // 第二次检查（持锁，防止竞争）
        val renderer = fetchGpuRendererFromEgl()
        cachedGpuRenderer = renderer
        return renderer
    }
}
```

| 场景 | 行为 |
|------|------|
| 首次调用 | `cachedGpuRenderer == null` → 进入 `synchronized` → 创建 EGL → 读取 GL_RENDERER → 写入缓存 |
| 并发调用（首次） | 第二个线程在 `synchronized` 外的 `?.let` 返回 `null` → 等待锁 → 持锁后第二次检查 → 发现已缓存 → 直接返回 |
| 后续调用 | `@Volatile` 保证可见性 → 无锁快速路径返回缓存值 → 零 EGL 开销 |

---

## 5. NetworkStatusManager —— 网络连接与带宽状态

> 源码：`src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/NetworkStatusManager.kt`（86 行）

### 5.1 概述

`NetworkStatusManager` 通过 Android `ConnectivityManager` API 获取当前活跃网络连接的类型、链路带宽估计值和本地 IPv4 地址。该模块零状态，每次调用即时查询系统 API。

### 5.2 Rust 接口

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub connected: bool,
    pub r#type: String,
    pub down_speed_kbps: i32,
    pub up_speed_kbps: i32,
    pub ip: String,
}

#[tauri::command]
pub fn get_network_status<R: Runtime>(app: AppHandle<R>) -> Result<NetworkStatus, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|get_network_status` |
| 参数 | 无 |
| 返回值 | `NetworkStatus` JSON 对象（camelCase 序列化） |
| 桌面端行为 | 返回 `{ connected: true, type: "以太网", downSpeedKbps: 100000, upSpeedKbps: 100000, ip: "127.0.0.1" }` |

### 5.3 网络类型与带宽检测

```kotlin
fun getNetworkStatus(): JSObject {
    val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    val activeNetwork = cm.activeNetwork
    if (activeNetwork == null) {
        return { connected: false, type: "未连接", downSpeedKbps: 0, upSpeedKbps: 0, ip: "未分配" }
    }

    val capabilities = cm.getNetworkCapabilities(activeNetwork) ?: return /* 同上 */

    val type = when {
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)     -> "WiFi"
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "移动数据"
        capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "以太网"
        else -> "未知"
    }

    val downSpeed = capabilities.linkDownstreamBandwidthKbps  // 下行估计带宽 (Kbps)
    val upSpeed   = capabilities.linkUpstreamBandwidthKbps    // 上行估计带宽 (Kbps)
    val ip        = getLocalIpAddress()                       // 本地 IPv4 地址
}
```

| 字段 | 来源 | 说明 |
|------|------|------|
| `connected` | `ConnectivityManager.activeNetwork` | `null` 表示无网络连接 |
| `type` | `NetworkCapabilities.hasTransport()` | 优先级：WiFi > 移动数据 > 以太网 |
| `downSpeedKbps` | `linkDownstreamBandwidthKbps` | 链路层估计值，非实时测速 |
| `upSpeedKbps` | `linkUpstreamBandwidthKbps` | 链路层估计值，非实时测速 |
| `ip` | `NetworkInterface.getNetworkInterfaces()` | 非回环 `Inet4Address` 的第一个地址 |

### 5.4 本地 IP 获取

```kotlin
private fun getLocalIpAddress(): String? {
    val en = NetworkInterface.getNetworkInterfaces()
    while (en.hasMoreElements()) {
        val intf = en.nextElement()
        val enumIpAddr = intf.inetAddresses
        while (enumIpAddr.hasMoreElements()) {
            val inetAddress = enumIpAddr.nextElement()
            if (!inetAddress.isLoopbackAddress && inetAddress is Inet4Address) {
                return inetAddress.hostAddress
            }
        }
    }
    return null
}
```

- 遍历所有网络接口，返回**第一个非回环 IPv4 地址**。
- 如无可用 IPv4 地址，`ip` 字段设为 `"未分配"`。

### 5.5 桌面端与异常 fallback

| 场景 | connected | type | downSpeedKbps | upSpeedKbps | ip |
|------|-----------|------|---------------|-------------|-----|
| 正常连接（WiFi） | true | "WiFi" | 链路估计值 | 链路估计值 | 实际 IPv4 |
| 正常连接（移动数据） | true | "移动数据" | 链路估计值 | 链路估计值 | 实际 IPv4 |
| 无活跃网络 | false | "未连接" | 0 | 0 | "未分配" |
| capabilities 为 null | false | "未连接" | 0 | 0 | "未分配" |
| 异常 | false | "未连接" | 0 | 0 | "未分配" |
| 桌面端 | true | "以太网" | 100000 | 100000 | "127.0.0.1" |

---

## 6. Root 访问体系

> 源码：`VcpMobilePlugin.kt` (lines 313--390), `system.rs` (lines 649--747)

### 6.1 概述

Root 访问体系基于 [libsu](https://github.com/topjohnwu/libsu)（`com.topjohnwu.superuser`）库提供三项能力：Root 状态检测、Root 命令执行、Root 管理器应用启动。所有操作在 `fileIoExecutor` 单线程池上执行，避免阻塞主线程。

### 6.2 check_root_access —— Root 状态检测

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootAccessStatus {
    pub is_root: bool,
}

#[tauri::command]
pub fn check_root_access<R: Runtime>(app: AppHandle<R>) -> Result<RootAccessStatus, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|check_root_access` |
| 参数 | 无 |
| 返回值 | `RootAccessStatus { is_root: bool }` |
| 桌面端行为 | 返回 `{ isRoot: false }` |

**Kotlin 实现**：

```kotlin
@Command
fun checkRootAccess(invoke: Invoke) {
    fileIoExecutor.execute {
        try {
            val isRoot = Shell.getShell().isRoot
            val result = JSObject().apply { put("isRoot", isRoot) }
            invoke.resolve(result)
        } catch (e: Exception) {
            invoke.resolve(JSObject().apply { put("isRoot", false) })
        }
    }
}
```

- 使用 `Shell.getShell().isRoot`（libsu API）检测设备是否已获取 Root 权限。
- 异常时安全降级为 `{ isRoot: false }`。

### 6.3 run_root_command —— 以 Root 身份执行 Shell 命令

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCommandResult {
    pub success: bool,
    pub output: String,
}

#[tauri::command]
pub fn run_root_command<R: Runtime>(
    app: AppHandle<R>,
    command: String,
) -> Result<RootCommandResult, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|run_root_command` |
| 参数 | `command: String` —— 要执行的 Shell 命令 |
| 返回值 | `RootCommandResult { success, output }` —— `output` 为命令的标准输出（多行用 `\n` 拼接） |
| 桌面端行为 | 返回 `{ success: false, output: "非Android物理端无法运行Root指令" }` |

**Kotlin 实现**：

```kotlin
@Command
fun runRootCommand(invoke: Invoke) {
    val args = invoke.parseArgs(RunRootCommandArgs::class.java)
    fileIoExecutor.execute {
        try {
            val output = Shell.cmd(args.command).exec().out
            val result = JSObject().apply {
                put("success", true)
                put("output", output.joinToString("\n"))
            }
            invoke.resolve(result)
        } catch (e: Exception) {
            val result = JSObject().apply {
                put("success", false)
                put("output", e.message ?: "Unknown Shell execution error")
            }
            invoke.resolve(result)
        }
    }
}
```

- `Shell.cmd(command).exec().out`：通过 libsu 以 Root 身份执行命令，返回标准输出行列表。
- 在 `fileIoExecutor` 上运行，避免阻塞 UI 线程。
- 参数解析失败直接 `invoke.reject()`，不上报执行级错误。

### 6.4 launch_root_manager —— 启动 Root 管理器应用

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRootManagerResult {
    pub success: bool,
    pub manager: Option<String>,
    pub message: Option<String>,
}

#[tauri::command]
pub fn launch_root_manager<R: Runtime>(
    app: AppHandle<R>,
) -> Result<LaunchRootManagerResult, String>;
```

| 属性 | 说明 |
|------|------|
| 前端命令 | `plugin:vcp-mobile\|launch_root_manager` |
| 参数 | 无 |
| 返回值 | `LaunchRootManagerResult { success, manager, message }` |
| 桌面端行为 | 返回 `{ success: false, manager: null, message: "该接口仅在 Android 物理端可用" }` |

**Kotlin 实现**：

```kotlin
@Command
fun launchRootManager(invoke: Invoke) {
    val managers = listOf(
        "com.topjohnwu.magisk" to "Magisk",
        "me.weishu.kernelsu" to "KernelSU",
        "me.tool.apatch" to "APatch"
    )
    for ((pkg, name) in managers) {
        try {
            val intent = activity.packageManager.getLaunchIntentForPackage(pkg)
            if (intent != null) {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                invoke.resolve(JSObject().apply {
                    put("success", true)
                    put("manager", name)
                })
                return
            }
        } catch (e: Exception) { /* 尝试下一个 */ }
    }
    // 未找到任何已安装的 Root 管理器
    invoke.resolve(JSObject().apply {
        put("success", false)
        put("message", "未找到支持的 Root 管理器 (Magisk, KernelSU, APatch)。")
    })
}
```

**支持的 Root 管理器**：

| 包名 | 显示名称 | 说明 |
|------|---------|------|
| `com.topjohnwu.magisk` | Magisk | 最主流的 Android Root 方案 |
| `me.weishu.kernelsu` | KernelSU | 基于内核的 Root 方案 |
| `me.tool.apatch` | APatch | 新一代内核级 Root 方案 |

- 按顺序遍历三个包名，使用 `packageManager.getLaunchIntentForPackage()` 查找启动 Intent。
- 找到第一个已安装的管理器即启动并立即返回。
- 全部未找到返回 `success: false` + 错误消息。

### 6.5 AndroidManifest.xml 包可见性声明

Android 11 (API 30) 引入了包可见性限制，默认情况下应用无法查询其他已安装应用。为支持 `launch_root_manager` 中通过 `getLaunchIntentForPackage` 查询 Root 管理器，需在 `AndroidManifest.xml` 中声明 `<queries>`：

```xml
<queries>
    <package android:name="com.topjohnwu.magisk" />
    <package android:name="me.weishu.kernelsu" />
    <package android:name="me.tool.apatch" />
</queries>
```

> **不声明 `<queries>` 的后果**：`getLaunchIntentForPackage()` 始终返回 `null`，`launch_root_manager` 会错误报告"未找到支持的 Root 管理器"，即使设备上已安装了 Magisk/KernelSU/APatch。

### 6.6 Root 访问数据流总览

```
前端调用
    │
    ├── invoke('plugin:vcp-mobile|check_root_access')
    │   → system.rs :: check_root_access(app)
    │   → run_mobile_plugin("checkRootAccess", {})
    │   → Kotlin: checkRootAccess(invoke) on fileIoExecutor
    │   → Shell.getShell().isRoot
    │   → JSObject { isRoot: bool }
    │
    ├── invoke('plugin:vcp-mobile|run_root_command', { command: "..." })
    │   → system.rs :: run_root_command(app, command)
    │   → run_mobile_plugin("runRootCommand", { "command": "..." })
    │   → Kotlin: runRootCommand(invoke) on fileIoExecutor
    │   → Shell.cmd(command).exec().out
    │   → JSObject { success, output }
    │
    └── invoke('plugin:vcp-mobile|launch_root_manager')
        → system.rs :: launch_root_manager(app)
        → run_mobile_plugin("launchRootManager", {})
        → Kotlin: launchRootManager(invoke)
        → packageManager.getLaunchIntentForPackage(pkg)
        → JSObject { success, manager, message }
```

---

## 7. 关键约束

1. **CpuStatusManager API 版本门控**：`currentThermalStatus` 需要 API 29+。低于此版本返回 `"不支持(API<29)"`，前端应根据此字符串进行 UI 降级。

2. **GpuStatusManager 资源安全**：
   - EGL 上下文在每次调用后立即完全销毁（`eglMakeCurrent(NO_SURFACE)` → `eglDestroyContext` → `eglDestroySurface` → `eglTerminate`）。
   - 双重检查锁定确保只初始化一次 EGL，后续调用零开销。
   - 如 EGL 初始化失败，返回 `"Unknown GPU"`。

3. **NetworkStatus 的带宽值为估计值**：`linkDownstreamBandwidthKbps` / `linkUpstreamBandwidthKbps` 是链路层的**理论估计值**，非实时测速结果。实际速度受信号强度、拥塞程度等因素影响。

4. **Root 命令安全**：
   - 所有 Root 命令在 `fileIoExecutor` 单线程池上执行，避免并发 Root Shell 冲突。
   - `run_root_command` 完全信任前端传入的命令字符串，**不做任何命令校验或沙盒化**。调用方需自行确保命令安全性。
   - 异常时 `success: false` + 异常消息，不会因 Root 命令执行失败导致应用崩溃。

5. **包可见性依赖**：`launch_root_manager` 依赖 `AndroidManifest.xml` 中的 `<queries>` 声明。如果构建时遗漏 `<queries>`，该功能在 Android 11+ 设备上将静默失败。

6. **桌面端安全降级**：所有 Root 和管理器相关命令在桌面端返回安全默认值（`isRoot: false`、`success: false`、错误提示消息），不暴露任何实际系统能力。

## 8. WakeLock/WifiLock 保活机制（交叉引用）

> **注意**：WakeLock/WifiLock 双锁保活机制的完整文档（包含 `acquire_wake_lock`、`release_wake_lock` 的 Kotlin 实现细节、超时安全策略及生命周期清理）已统一收录于 [06_权限与系统控制.md](./06_权限与系统控制.md) 第 15 节「WakeLock/WifiLock 保活机制」。

### 8.1 概述

`acquire_wake_lock` / `release_wake_lock` 是防止 Android 设备在长连接场景下进入深度休眠的保活指令：

| 命令 | Rust 接口 | 锁类型 | 用途 |
|------|-----------|--------|------|
| `acquire_wake_lock` | `system.rs :: acquire_wake_lock()` | CPU WakeLock (`PARTIAL_WAKE_LOCK`) + WifiLock (`WIFI_MODE_FULL_LOW_LATENCY`) | 防止 CPU 休眠与 WiFi 断开 |
| `release_wake_lock` | `system.rs :: release_wake_lock()` | — (释放上述双锁) | 成对释放，恢复系统正常休眠策略 |

这两个命令在分布式计算节点（`distributed/client.rs`）中被自动成对调用，覆盖 WebSocket 连接、工具执行、占位推送三个关键阶段：

| 阶段 | 位置 | 锁持有时长 |
|------|------|------------|
| TCP 连接 | `connect_async` 前后 | 毫秒级 |
| 工具执行 | `execute_tool` 前后 | 取决于工具耗时（硬上限 5 分钟） |
| 占位推送 | `push_static_placeholders` 前后 | 毫秒级 |

前端一般无需直接调用这两个命令，它们由 Rust 后端在 `DistributedNode` 生命周期中自动管理。

### 8.2 交叉引用

- **完整实现**：[06_权限与系统控制.md 第 15 节](./06_权限与系统控制.md) — 含 Kotlin 源码、双锁参数详情、`onDestroy()` 清理逻辑
- **使用场景**：[03_流式前台保活服务.md](./03_流式前台保活服务.md) — 前台 Service + WakeLock 双重保活在流式对话中的应用
- **调用模式**：`src-tauri/src/distributed/client.rs` (`acquire_wake_lock_helper` / `release_wake_lock_helper` 在 connect / tool_exec / placeholder_push 三个 phase 中成对调用)

---

*最后更新：2026-06-05 | VCP Mobile v1.0.3*
