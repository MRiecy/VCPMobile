---
id: PLUGIN-GUESTJS-007
title: Guest JS API
description: tauri-plugin-vcp-mobile 的前端调用封装层
version: 1.0.3
date: 2026-06-05
related_files:
  - src-tauri/plugins/vcp-mobile/guest-js/index.ts
  - src-tauri/plugins/vcp-mobile/src/lib.rs
---

# Guest JS API

## 1. 功能概述

`guest-js/index.ts` 是 `tauri-plugin-vcp-mobile` 暴露给前端 Vue/TS 代码的**唯一官方调用入口**。将所有 Tauri 命令字符串、参数结构、返回类型封装为类型安全的 TypeScript 函数，避免前端直接手写命令字符串导致的拼写错误。

---

## 2. 代码结构

```
src-tauri/plugins/vcp-mobile/guest-js/index.ts (42 lines)
├── Screen
│   ├── setKeepScreenOn(): Promise<void>
│   └── clearKeepScreenOn(): Promise<void>
├── Stream Service
│   ├── startStreamService(agentName: string): Promise<void>
│   └── stopStreamService(): Promise<void>
└── Native File Picker
    └── pickFile(): Promise<PickedFile>
```

> **注意**：`system` 模块的部分命令（权限检查、请求、返回桌面、电池状态、原生打开文件）尚未在 `guest-js/index.ts` 中封装，前端目前直接通过 `invoke()` 调用。

---

## 3. 接口详解

### 3.1 屏幕控制

```typescript
export function setKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|set_keep_screen_on');
}

export function clearKeepScreenOn(): Promise<void> {
  return invoke('plugin:vcp-mobile|clear_keep_screen_on');
}
```

| 函数 | Tauri 命令 | 参数 | 返回值 | 对应 Rust 函数 |
|------|-----------|------|--------|---------------|
| `setKeepScreenOn()` | `plugin:vcp-mobile\|set_keep_screen_on` | 无 | `Promise<void>` | `screen::set_keep_screen_on` |
| `clearKeepScreenOn()` | `plugin:vcp-mobile\|clear_keep_screen_on` | 无 | `Promise<void>` | `screen::clear_keep_screen_on` |

---

### 3.2 流式保活服务

```typescript
export function startStreamService(agentName: string): Promise<void> {
  return invoke('plugin:vcp-mobile|start_streaming_service', { agentName });
}

export function stopStreamService(): Promise<void> {
  return invoke('plugin:vcp-mobile|stop_streaming_service');
}
```

| 函数 | Tauri 命令 | 参数 | 返回值 | 对应 Rust 函数 |
|------|-----------|------|--------|---------------|
| `startStreamService(agentName)` | `plugin:vcp-mobile\|start_streaming_service` | `{ agentName: string }` | `Promise<void>` | `stream::start_streaming_service` |
| `stopStreamService()` | `plugin:vcp-mobile\|stop_streaming_service` | 无 | `Promise<void>` | `stream::stop_streaming_service` |

#### 参数说明

- **`agentName`**：当前正在流式输出的 Agent 名称。支持多 Agent 同时流式输出，Rust 侧通过引用计数管理。
- **调用约定**：`startStreamService` 与 `stopStreamService` 必须成对调用。异常路径中应确保 `stopStreamService` 被调用，否则服务将永久驻留。

---

### 3.3 原生文件选择器

```typescript
export interface PickedFile {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

export function pickFile(): Promise<PickedFile> {
  return invoke<PickedFile>('plugin:vcp-mobile|pick_file');
}
```

| 函数 | Tauri 命令 | 参数 | 返回值 | 对应 Rust 函数 |
|------|-----------|------|--------|---------------|
| `pickFile()` | `plugin:vcp-mobile\|pick_file` | 无 | `Promise<PickedFile>` | `system::pick_file` |

#### 返回值说明

- **`path`**：文件在应用私有目录中的绝对路径。
- **`name`**：原始文件名。
- **`mime`**：MIME 类型（如 `image/jpeg`、`application/pdf`）。
- **`size`**：文件大小（字节）。
- **`hash`**：文件内容的 SHA-256 哈希值，用于去重与完整性校验。
- **`thumbnailPath`**（可选）：当选择图片/视频时，系统生成的缩略图路径。

> **平台限制**：该接口仅在 Android 物理端可用；桌面端调用将抛出错误。

---

## 4. 命令命名映射规则

Tauri v2 插件命令的完整格式为：

```
plugin:<plugin-name>|<command-name>
```

| 层级 | 规则 | 示例 |
|------|------|------|
| 插件名 | `Builder::new("vcp-mobile")` 中定义 | `vcp-mobile` |
| 命令名 | Rust `#[tauri::command]` 函数名**原样传递** | `set_keep_screen_on` |

> **注意**：命令名**不使用 camelCase**。前端 `invoke()` 中必须完全匹配 Rust 函数名的下划线命名。

---

## 5. 前端使用示例

### 5.1 屏幕常亮

```typescript
import { setKeepScreenOn, clearKeepScreenOn } from 'tauri-plugin-vcp-mobile-api';

async function beginLongRunningTask() {
  await setKeepScreenOn();
  try {
    // ... 耗时操作
  } finally {
    await clearKeepScreenOn();
  }
}
```

### 5.2 流式输出保活

```typescript
import { startStreamService, stopStreamService } from 'tauri-plugin-vcp-mobile-api';

async function streamResponse(agentName: string) {
  await startStreamService(agentName);
  try {
    // ... SSE 流式读取与渲染
  } finally {
    await stopStreamService();
  }
}
```

### 5.3 权限检查（直接 invoke）

```typescript
import { invoke } from '@tauri-apps/api/core';

const status = await invoke<{
  notification: boolean;
  storage: boolean;
  battery: boolean;
  microphone: boolean;
}>('plugin:vcp-mobile|check_all_permissions');

if (!status.notification) {
  await invoke('plugin:vcp-mobile|request_android_permission', {
    p_type: 'notification'
  });
}
```

---

## 6. 未封装命令清单

以下命令已在 Rust `lib.rs` 的 `invoke_handler` 中注册，但**尚未在 `guest-js/index.ts` 中封装**。前端需直接通过 `invoke()` 调用：

| 命令 | 参数 | 返回类型 | 说明 |
|------|------|----------|------|
| `plugin:vcp-mobile\|check_all_permissions` | 无 | `{ notification: boolean, storage: boolean, battery: boolean, microphone: boolean }` | 检查四项权限状态 |
| `plugin:vcp-mobile\|request_android_permission` | `{ p_type: string }` | `void` | 请求指定权限（`notification` / `storage` / `microphone` 等） |
| `plugin:vcp-mobile\|move_task_to_back` | 无 | `void` | 将应用移至后台 |
| `plugin:vcp-mobile\|get_battery_status` | 无 | `{ level: number, isPowerSaveMode: boolean }` | 获取电池电量与省电模式状态 |
| `plugin:vcp-mobile\|open_file_native` | `{ path: string }` | `void` | 调用系统原生应用打开指定路径文件 |

> **建议**：后续应在 `guest-js/index.ts` 中补充这些函数的 TS 封装，以保持一致性。参数名 `p_type` 也应在前端枚举化，避免字符串硬编码。

---

## 7. 与 `ANDROID_PLUGIN_MANAGEMENT.md` 的交叉引用

- 命令静默拒绝排查：参见 `docs/ANDROID_PLUGIN_MANAGEMENT.md` §7.1（检查 `capabilities/default.json` 是否包含 `"vcp-mobile:default"`）。
- 插件开发规范（Guest JS 侧）：参见 `docs/ANDROID_PLUGIN_MANAGEMENT.md` §5.1 / §5.2。
