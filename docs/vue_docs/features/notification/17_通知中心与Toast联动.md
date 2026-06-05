---
id: VUE-NOTI-017
title: 通知中心与Toast联动
description: VCP Mobile 前端通知列表、Toast 气泡、剪贴板联动与 VCP System Event 处理
version: 1.0.3
date: 2026-06-05
---

# 17. 通知中心与Toast联动

## 1. 概述

### 1.1 领域定位

通知中心（Notification Center）与 Toast 气泡是 VCP Mobile 前端**全局反馈层**的核心组件，负责将后端 Rust 事件、系统状态变更、工具执行结果以非阻塞方式送达用户。该模块横跨 Vue 渲染层与 Tauri IPC 桥接层，是用户感知后端活动的首要通道。

### 1.2 模块构成表

| 文件路径 | 类型 | 职责 |
|---------|------|------|
| `src/core/stores/notification.ts` | Pinia Store | 全局通知状态：历史队列、活动 Toast、未读计数、状态栏 |
| `src/core/composables/useNotificationProcessor.ts` | Composable | VCP System Event 解析、过滤引擎、Payload → 通知对象转换 |
| `src/features/notification/NotificationList.vue` | 组件 | 右抽屉内的通知历史列表渲染 |
| `src/features/notification/NotificationCard.vue` | 组件 | 单条通知卡片：图标、内容、操作按钮、滑动删除 |
| `src/features/notification/NotificationEmptyState.vue` | 组件 | 空态占位图（Bell 图标 + 文案） |
| `src/features/notification/NotificationStatusBar.vue` | 组件 | 连接状态栏（VCPLog / Core 状态可视化） |
| `src/features/notification/composables/useNotificationClipboard.ts` | Composable | 通知内容复制、复制状态管理 |
| `src/features/notification/composables/useNotificationPresentation.ts` | Composable | 通知图标/颜色/按钮样式映射 |
| `src/components/ui/ToastManager.vue` | 组件 | Toast 容器：固定定位、TransitionGroup、z-toast 层级 |
| `src/components/ui/ToastItem.vue` | 组件 | 单个 Toast 气泡：滑动/点击消失、图标、标题、内容 |
| `src/components/layout/RightSidebar.vue` | 组件 | 通知抽屉外壳：打开/关闭、调试注入、全部已读/清空 |
| `src/components/GlobalOverlayManager.vue` | 组件 | 全局覆盖层管理器：挂载 ToastManager 与 Prompt |

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                        Vue 3 渲染层                          │
│  ┌──────────────┐  ┌─────────────┐  ┌───────────────────┐   │
│  │ ToastManager │  │ RightSidebar│  │ GlobalOverlayMgr  │   │
│  │  (z-toast)   │  │ (z-drawer)  │  │   (z-overlay)     │   │
│  └──────┬───────┘  └──────┬──────┘  └───────────────────┘   │
│         │                 │                                   │
│  ┌──────▼───────┐  ┌──────▼──────┐                           │
│  │  ToastItem   │  │Notification │                           │
│  │  (active)    │  │   List      │                           │
│  └──────────────┘  └──────┬──────┘                           │
│                           │                                   │
│                    ┌──────▼──────┐                            │
│                    │Notification │                            │
│                    │    Card     │                            │
│                    └─────────────┘                            │
├─────────────────────────────────────────────────────────────┤
│                     Tauri IPC 桥接层                          │
│         listen("vcp-system-event", ...)                      │
│         invoke("send_vcp_log_message", ...)                  │
├─────────────────────────────────────────────────────────────┤
│                      Rust 核心层                              │
│  ┌─────────────────┐  ┌──────────────────────────────────┐  │
│  │ vcp_log_service │  │ distributed/tools/notification.rs│  │
│  │ (WebSocket/SSE) │  │ (MobileNotification OneShot)     │  │
│  └─────────────────┘  └──────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 核心设计原则

1. **双轨存储**：历史列表（`historyList`）与活动 Toast（`activeToasts`）物理分离。同一条通知可同时存在于两处，也可通过 `toastOnly` / `historyOnly` 标记选择性进入单轨。
2. **单例抑制（Singleton Suppression）**：固定 ID 的通知（如 `vcp_sync_connection_status`）在同一 Toast 上原地更新，避免视觉轰炸。
3. **降噪过滤**：内置过滤引擎拦截心跳、Ping/Pong、冗余连接成功等噪声消息。
4. **审批不消失**：`duration = 0` 的审批类通知永不自动消失，必须用户显式操作。
5. **移动端优先交互**：Toast 支持横向滑动 dismiss；通知卡片支持右滑删除；全部交互附带触觉反馈（`navigator.vibrate`）。

---

## 2. 通知状态（notificationStore）

### 2.1 通知队列设计

`src/core/stores/notification.ts` 采用 Pinia Composition API 风格，暴露两个核心队列：

```ts
const historyList = ref<VcpNotification[]>([]);   // 通知中心历史（上限 100）
const activeToasts = ref<VcpNotification[]>([]);  // 当前悬浮 Toast
const unreadCount = ref(0);                       // 未读计数（badge）
const isDrawerOpen = ref(false);                  // 抽屉打开状态（抑制 Toast）
```

`VcpNotification` 接口完整字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `string` | 唯一标识；后端可传递固定 ID 用于去重 |
| `type` | `'info' \| 'success' \| 'warning' \| 'error' \| 'tool' \| 'agent'` | 视觉类型 |
| `title` | `string` | 通知标题 |
| `message` | `string` | 通知正文 |
| `timestamp` | `number` | 创建时间戳（ms） |
| `duration` | `number?` | 显示时长（ms）；`0` 为永不消失；默认 `3000` / `7000` |
| `isPreformatted` | `boolean?` | 是否使用等宽字体 + 滚动容器渲染（JSON/代码内容） |
| `actions` | `{label, value, color}[]?` | 操作按钮数组（如审批的"允许"/"拒绝"） |
| `silent` | `boolean?` | 静默模式：不展示任何 UI |
| `toastOnly` | `boolean?` | 仅作为 Toast 显示，**不进入**通知中心历史 |
| `historyOnly` | `boolean?` | 仅进入历史记录，**不弹出** Toast |
| `read` | `boolean?` | 是否已读 |
| `rawPayload` | `any?` | 原始 payload，用于 action 处理或剪贴板复制 |

### 2.2 去重策略

`addNotification()` 实现三层去重：

```
┌────────────────────────────────────────────────────────────┐
│  传入 payload                                              │
│        │                                                   │
│        ▼                                                   │
│  ┌─────────────┐    Y    ┌─────────────────────────────┐   │
│  │ payload.id? │────────▶│ 检查 activeToasts 同 ID     │   │
│  └─────────────┘         │ 存在 → 原地更新 + 重置计时器  │   │
│        │ N               │ 返回（不产生新 Toast）        │   │
│        ▼                 └─────────────────────────────┘   │
│  ┌─────────────┐    Y    ┌─────────────────────────────┐   │
│  │ 30s 冷却窗  │────────▶│ 检查 historyList 同 ID      │   │
│  │ 内存在？    │         │ 存在 → 更新时间戳，抑制 Toast │   │
│  └─────────────┘         │ 返回                        │   │
│        │ N               └─────────────────────────────┘   │
│        ▼                                                   │
│  生成新 notification，继续入队逻辑                           │
└────────────────────────────────────────────────────────────┘
```

1. **活动 Toast 原地更新**：若同 ID 已在 `activeToasts` 中，直接合并更新并重置 `setTimeout`。
2. **历史冷却抑制**：若同 ID 在 `historyList` 的 30 秒窗口内已出现过，仅更新时间戳与消息内容，不弹新 Toast。
3. **历史列表 ID 查重**：入 `historyList` 前再次查重，防止列表膨胀；上限 100 条，超限时尾部弹出。

### 2.3 Toast-only 模式

```ts
// 仅显示 Toast，不进入历史（适合即时反馈如"复制成功"）
store.addNotification({ message: 'Copied!', type: 'success', toastOnly: true });

// 仅进入历史，不弹 Toast（适合后台同步错误如连接断开）
store.addNotification({ id: 'vcp_sync_connection_status', message: '...', historyOnly: true });
```

- **`toastOnly`**：跳过 `historyList` 写入与 `unreadCount` 递增。
- **`historyOnly`**：跳过 `activeToasts` 推入；典型场景为 Sync 连接失败，避免用户操作时弹出干扰。

### 2.4 持久化与清理

- **自动移除**：`duration !== 0` 时，`setTimeout` 在到期后从 `activeToasts` 过滤掉该 ID。
- **幽灵清理**：每 30 秒运行一次 interval，强制清理残留 Toast（`duration === 0` 的审批类通知豁免）。
- **Store 销毁**：`onScopeDispose` 清除全部 timer 与 interval，防止内存泄漏。
- **清空历史**：`clearHistory()` 一键清空历史与未读计数；`removeHistoryItem(id)` 单条删除并扣减未读。

---

## 3. 通知列表（NotificationList.vue）

### 3.1 打开方式

通知列表嵌入 `RightSidebar.vue` 抽屉内，通过 `layoutStore.isRightSidebarOpen` 控制显隐：

```
App.vue ──▶ layoutStore.toggleRightSidebar() ──▶ RightSidebar.vue ──▶ NotificationList.vue
```

抽屉打开时：`isDrawerOpen = true`，此时所有新通知被抑制 Toast 弹出（仅入历史），且自动调用 `markAllRead()`。

### 3.2 列表结构

```vue
<!-- src/features/notification/NotificationList.vue -->
<div class="flex-1 overflow-y-auto vcp-scrollable no-rubber-band">
  <TransitionGroup name="list" tag="div" class="flex flex-col">
    <NotificationCard
      v-for="item in props.items"
      :key="item.id"
      :item="item"
      :copy-icon="getCopyIcon(item.id)"
      @copy="copyContent(item)"
      @remove="store.removeHistoryItem(item.id)"
    />
  </TransitionGroup>
</div>
```

- 使用 `TransitionGroup` 实现进入/离开动画。
- 列表项通过 `props.items`（即 `store.historyList`）传入，保持组件纯展示。

### 3.3 空态处理

当 `props.items.length === 0` 时渲染 `NotificationEmptyState.vue`：

- Bell 图标（`lucide-vue-next`），`size=48`，`stroke-width=1`
- 文案：`"No notifications yet"`，大写 + 字间距加宽，透明度 20%

### 3.4 与 NotificationCard 的联动

`NotificationList` 仅负责列表容器与动画；每条通知的完整交互由 `NotificationCard` 实现。`@copy` 与 `@remove` 事件通过 Emits 回传至列表层执行 Store 操作。

---

## 4. 通知卡片（NotificationCard.vue）

### 4.1 Props 与样式

```ts
const props = defineProps<{
  item: VcpNotification;
  copyIcon: any;      // 动态图标：Copy 或 Check
}>();

const emit = defineEmits<{
  copy: [];
  remove: [];
}>();
```

卡片采用高密度线性布局：
- 水平 flex，左侧类型图标（`size=13`），中间内容区，右侧复制按钮
- 底部分隔线：`border-b border-black/5 dark:border-white/5`
- 时间戳：`HH:mm:ss`，等宽字体，透明度 30%

### 4.2 类型区分

通过 `useNotificationPresentation.ts` 映射：

| type | 图标 | 颜色类 | 语义 |
|------|------|--------|------|
| `info` | `Info` | `text-blue-400` | 普通信息 |
| `success` | `CheckCircle` | `text-green-500` | 成功（如 DailyNote） |
| `warning` | `AlertTriangle` | `text-amber-500` | 警告（如审批请求） |
| `error` | `X` | `text-red-500` | 错误 |
| `tool` | `Cpu` | `text-purple-500` | 工具执行结果 |
| `agent` | `User` | `text-blue-500` | Agent 相关 |

`isPreformatted = true` 时，消息体使用等宽字体 + 深色背景块 + 最大高度 100px 滚动容器，适合展示 JSON 或命令输出。

### 4.3 操作按钮

当 `item.actions` 存在时渲染按钮行：

```vue
<div v-if="item.actions && item.actions.length > 0" class="mt-2 flex gap-1.5">
  <button v-for="action in item.actions" :key="action.label" @click="handleAction(action)">
    {{ action.label }}
  </button>
</div>
```

按钮样式通过 `getActionButtonClass(action)` 动态生成：
- 标签为 `"允许"` / `"Approve"` → `bg-green-600`
- 标签为 `"拒绝"` / `"Deny"` → `bg-red-600`
- 否则使用 `action.color`
- 统一附加：`rounded-md`、`active:scale-95`、`transition-all duration-100`

### 4.4 点击与滑动交互

**滑动删除（Swipe to Dismiss）**：

```
TouchStart ──▶ 记录 startX/startY
     │
     ▼
TouchMove ────▶ 方向判定（absY/absX > 0.577 → 垂直滚动，放弃）
     │            水平右滑：translateX 跟随手指（上限 200px）
     ▼
TouchEnd ─────▶ swipeX > 120px ? navigator.vibrate(40) + emit('remove') : 回弹归零
```

- 复制按钮平时 `opacity-0`，`group-hover:opacity-30`，点击区域独立，不触发滑动。

---

## 5. 状态栏集成（NotificationStatusBar.vue）

### 5.1 状态栏通知展示

`NotificationStatusBar.vue` 常驻于 `RightSidebar` 顶部、`NotificationList` 上方，实时展示 VCPLog 连接状态：

```vue
<div class="w-full text-center py-1.5 text-[10px] font-black uppercase tracking-[0.2em] ...">
  {{ store.vcpStatus.source || 'VCPLog' }}: {{ store.vcpStatus.message || 'IDLE' }}
</div>
```

### 5.2 与 VCPLog 连接的联动

状态栏颜色映射 `store.vcpStatus.status`：

| status | 背景色 | 文字色 | 语义 |
|--------|--------|--------|------|
| `connected` / `open` | `#2e7d32` | 白 | 已连接 |
| `disconnected` / `closed` | `#c62828` | 白 | 断开 |
| `error` | `#b71c1c` | 白 | 错误 |
| `connecting` | `#f9a825` | 黑 | 连接中 |
| 其他 | `bg-black/20` | 主题色 | 未知 |

### 5.3 状态栏图标与文字

- 无独立图标，纯文字色块表达状态。
- 状态来源由 `useNotificationProcessor` 解析 `vcp-log-status` / `vcp-core-status` 事件后写入 `vcpStatus` / `vcpCoreStatus`。
- **注意**：同步状态不再渲染到全局状态栏（同步已改为完全手动触发，避免状态栏干扰）。

---

## 6. Toast 系统

### 6.1 Toast 与通知的关系

Toast 是通知的**瞬时投影**：同一条 `VcpNotification` 对象既可入 `historyList`，也可被压入 `activeToasts`。两者的视觉形态不同：

| 维度 | 通知中心（NotificationCard） | Toast（ToastItem） |
|------|---------------------------|-------------------|
| 定位 | 右抽屉内（`z-drawer`） | 全局固定顶部（`z-toast`） |
| 背景 | 透明/主题底色 | `bg-white/90 dark:bg-zinc-900/90` + `backdrop-blur-md` |
| 阴影 | 无 | `shadow-[0_8px_30px_rgba(0,0,0,0.12)]` |
| 宽度 | 填满抽屉 | `max-w-[90vw] w-[320px]` |
| 圆角 | 无（线性列表） | `rounded-xl` |
| 消失 | 手动滑动/清空 | 自动倒计时 + 点击/滑动 dismiss |

### 6.2 Toast 的自动消失机制

```ts
// ToastManager.vue 容器
<div class="fixed top-safe left-0 right-0 z-toast pointer-events-none ...">
  <TransitionGroup name="toast">
    <ToastItem v-for="toast in store.activeToasts" :key="toast.id" :toast="toast" />
  </TransitionGroup>
</div>
```

`ToastItem.vue` 的 dismiss 方式：
1. **点击整卡**：`handleClick()` → `dismissToast(toast.id)`
2. **点击 × 按钮**：`@click.stop` 阻止冒泡，仅关闭当前
3. **横向滑动**：`@vueuse/core/useSwipe`，`|lengthX| > 60` 且方向为 left/right 时 dismiss
4. **自动超时**：由 `notificationStore.addNotification()` 内 `setTimeout` 触发

动画关键帧（`ToastManager.vue`）：
- 进入：`translateY(-40px) scale(0.8)` → 正常，`cubic-bezier(0.18, 0.89, 0.32, 1.28)`（弹性回弹）
- 离开：`translateY(-20px) scale(0.9)` → 透明，`ease-in`

### 6.3 Toast 的堆叠与替换

- **堆叠**：多个 Toast 垂直排列（`flex-col items-center gap-2.5`），新 Toast 从顶部推入。
- **替换**：同 ID Toast 在 `addNotification()` 中被原地替换（数组索引赋值），视觉上表现为内容刷新而非新增。
- **抽屉抑制**：`isDrawerOpen === true` 时，`addNotification()` 不将任何通知推入 `activeToasts`，避免抽屉打开期间被 Toast 遮挡。

---

## 7. 剪贴板联动

### 7.1 useNotificationClipboard 的职责

`src/features/notification/composables/useNotificationClipboard.ts` 封装通知内容的复制逻辑：

```ts
export const useNotificationClipboard = () => {
  const copiedId = ref<string | null>(null);   // 当前处于"已复制"状态的 ID
  let copyTimer: ReturnType<typeof setTimeout> | null = null;

  const buildCopyText = (item: VcpNotification) => {
    return item.rawPayload
      ? JSON.stringify(item.rawPayload, null, 2)   // 优先复制完整原始数据
      : `${item.title}\n${item.message}`;           // Fallback：标题 + 正文
  };

  const copyContent = async (item: VcpNotification) => { ... };
  const getCopyIcon = (itemId: string) => copiedId.value === itemId ? Check : Copy;

  return { copiedId, buildCopyText, copyContent, getCopyIcon };
};
```

### 7.2 通知内容复制

复制按钮位于 `NotificationCard` 右侧，平时 `opacity-0`，hover 时显示：

```vue
<button @click="$emit('copy')" class="opacity-0 group-hover:opacity-30 hover:!opacity-80 ...">
  <component :is="props.copyIcon" :size="13" />
</button>
```

- 成功复制后图标从 `Copy` 变为 `Check`（绿色对钩语义）
- 2 秒后自动恢复为 `Copy`

### 7.3 剪贴板写入策略

- 使用 `navigator.clipboard.writeText()`（Web Standard API），无需 Tauri 原生权限。
- 优先复制 `rawPayload`（完整 JSON），便于开发者/用户获取后端原始数据。
- 失败时 catch 并打印 `[useNotificationClipboard] Copy failed`。

---

## 8. VCP System Event 处理

### 8.1 useNotificationProcessor 的职责

`src/core/composables/useNotificationProcessor.ts` 是通知系统的**解析中枢**，对标桌面端的 `notificationRenderer.js` + `filterManager.js`：

```
Rust emit("vcp-system-event", payload)
         │
         ▼
App.vue listen("vcp-system-event", ...)
         │
         ▼
useNotificationProcessor.processPayload(payload)
         │
         ├──▶ vcp-log-status  ──▶ updateStatus() ──▶ 状态栏更新（silent）
         ├──▶ vcp-core-status ──▶ updateCoreStatus() ──▶ 核心状态 + 错误弹窗
         ├──▶ vcp-log-message ──▶ 工具结果/错误/分布式消息解析
         ├──▶ tool_approval_request ──▶ 审批通知（duration=0, actions）
         ├──▶ video_generation_status ──▶ 视频生成状态
         ├──▶ daily_note_created ──▶ 日记创建（遗留兼容）
         └──▶ 默认回退 ──▶ Generic 类型 + JSON 字符串化
```

### 8.2 事件 payload 解析

`processPayload()` 对主要类型的解析逻辑：

**vcp-log-message（工具执行结果）**：

```ts
if (vcpData.tool_name && vcpData.status) {
  type = vcpData.status === 'error' ? 'error'
       : (vcpData.tool_name === 'DailyNote' ? 'success' : 'tool');
  title = `${vcpData.tool_name} ${vcpData.status}`;
  message = String(vcpData.content || '');
  isPreformatted = true;

  // 深层解析：提取 original_plugin_output、MaidName、timestamp
  try {
    const inner = JSON.parse(rawContent);
    if (inner.original_plugin_output) {
      message = JSON.stringify(inner.original_plugin_output, null, 2);
    }
    // DailyNote 成功 Fallback
    if (vcpData.tool_name === 'DailyNote' && vcpData.status === 'success' && !hasValidOutput) {
      message = "✅ 日记内容已成功记录到本地知识库。";
    }
  } catch (e) { /* 保持 rawContent */ }
}
```

**tool_approval_request（审批请求）**：

```ts
type = 'warning';
title = `🛠️ 审核请求: ${approvalData.toolName}`;
message = `助手: ${approvalData.maid}\n命令: ${approvalData.args?.command}\n时间: ...`;
duration = 0;  // 永不消失
actions = [
  { label: '允许', value: true,  color: 'bg-green-500 shadow-lg shadow-green-500/20' },
  { label: '拒绝', value: false, color: 'bg-red-500   shadow-lg shadow-red-500/20'   }
];
```

### 8.3 通知生成规则

解析完成后，`processPayload()` 执行全局过滤引擎 `checkMessageFilter()`：

| 规则名 | 匹配条件 | 动作 | duration 覆盖 |
|--------|---------|------|--------------|
| Heartbeat/Ping/Pong Noise Reduction | `type` 或内容含 heartbeat/ping/pong | `hide` | — |
| Redundant Connection Success | `connection_ack` + 含 successful | `hide` | — |
| Important Error Duration Extension | 标题/内容含 error/failed 或 `status === 'error'` | `show` | `15000` |
| DistPluginManager Noise Reduction | `source === 'DistPluginManager'` + heartbeat/checking | `hide` | — |

通过过滤后，返回 `Partial<VcpNotification>`；若 `action === 'hide'` 则返回 `{ silent: true }`。

---

## 9. 数据流时序

### 9.1 VCP System Event 到达后的处理时序

```
Rust Backend                    Tauri IPC                    Vue Frontend
     │                              │                              │
     │  emit("vcp-system-event")    │                              │
     │─────────────────────────────▶│                              │
     │                              │  WebView 事件投递              │
     │                              │─────────────────────────────▶│
     │                              │                              │
     │                              │              listen() 回调触发 │
     │                              │                              ▼
     │                              │                   ┌────────────────────┐
     │                              │                   │ processPayload()   │
     │                              │                   │  - 类型分支解析     │
     │                              │                   │  - 过滤引擎检查     │
     │                              │                   └──────────┬─────────┘
     │                              │                              │
     │                              │                              ▼
     │                              │                   ┌────────────────────┐
     │                              │              N    │ silent?            │
     │                              │         ┌────────│                    │
     │                              │         │        └────────────────────┘
     │                              │         │ Y
     │                              │         ▼
     │                              │   丢弃（不进入 Store）
     │                              │         │
     │                              │         ▼
     │                              │   ┌──────────────────────────────────┐
     │                              │   │ addNotification()                │
     │                              │   │  - 去重（ID 更新 / 冷却抑制）    │
     │                              │   │  - 入 historyList（!toastOnly）  │
     │                              │   │  - 入 activeToasts（!historyOnly）│
     │                              │   │  - setTimeout 自动移除           │
     │                              │   └──────────────────────────────────┘
```

### 9.2 用户点击审批操作后的回传时序

```
User
 │
 │ 点击"允许" / "拒绝"
 ▼
NotificationCard.handleAction(action)
 │
 ▼
notificationStore.executeAction(notificationId, action)
 │
 ├──▶ 查找 historyList 中对应 item
 │
 ├──▶ 构造 approval response payload
 │     {
 │       type: 'tool_approval_response',
 │       data: { requestId, approved: action.value }
 │     }
 │
 ├──▶ invoke('send_vcp_log_message', { payload })
 │     ─────────────────────────────────────────▶ Rust vcp_log_service
 │                                                   通过 WebSocket 回传后端
 │
 └──▶ UI 反馈
       - item.actions = []（按钮置空）
       - item.message = `[已处理] 操作: ${action.label}`
       - 从 activeToasts 过滤掉该通知
```

---

## 10. 与 Rust 后端的 IPC 交互

### 10.1 Tauri Commands 调用表

| Command | 调用方 | 参数 | 用途 |
|---------|--------|------|------|
| `send_vcp_log_message` | `notificationStore.executeAction()` | `{ payload: JSON }` | 向前端 VCPLog WebSocket 通道回传审批响应 |
| `init_vcp_log_connection` | `App.vue` bootstrap | `{ url: string, key: string }` | 初始化 WebSocket/SSE 连接，后续事件通过 `vcp-system-event` 推送 |
| `set_vcp_log_heartbeat` | 设置页面 | `{ interval_ms: number }` | 调整心跳间隔 |

### 10.2 事件监听表

| 事件名 | 监听方 | 发射方 | Payload 结构 | 用途 |
|--------|--------|--------|-------------|------|
| `vcp-system-event` | `App.vue` | `vcp_log_service.rs` | `{ type, data?, ... }` | 核心通知通道：状态、日志、审批、视频状态等 |
| `distributed-notification` | `ToolInteractionOverlay.vue` | `distributed/tools/notification.rs` | `{ title, body }` | AI 触发的移动设备本地通知（浏览器 Notification API） |

**Android 原生通知联动**：

- `tauri-plugin-vcp-mobile` 的 Kotlin 侧声明了 `POST_NOTIFICATIONS` 权限（Android 13+）。
- `StreamKeepaliveService.kt` 使用 `startForeground(NOTIFICATION_ID, notification)` 维持前台服务通知，确保流式会话期间不被 OEM 杀后台。
- `distributed/tools/notification.rs` 中的 `NotificationTool` 通过 `app.emit("distributed-notification")` 将事件投递到前端，前端再调用浏览器 `Notification` API 或降级为 console 日志。

---

## 11. 设计决策与注意事项

1. **为什么 Toast 和通知中心使用同一数据模型？**
   - 减少转换层。`VcpNotification` 同时驱动两种视觉形态，保证数据一致性。`toastOnly` / `historyOnly` 提供灵活的分流能力。

2. **为什么 `duration = 0` 表示永不消失？**
   - 审批类通知必须等待用户操作。`0` 作为哨兵值，在 `setTimeout` 逻辑和幽灵清理逻辑中均被豁免。

3. **为什么过滤引擎放在 `useNotificationProcessor` 而非 Store？**
   - Store 只负责存储与生命周期；过滤属于"解析"职责。分离后便于单元测试和规则扩展。

4. **为什么状态栏不展示同步状态？**
   - 同步已改为完全手动触发。自动同步的状态抖动会造成状态栏视觉干扰，因此同步错误以 `historyOnly` 方式静默入历史。

5. **滑动删除的方向判定算法**
   - `absY / absX > 0.577`（即 tan(30°)）作为垂直滚动判定阈值。高于此值认定为上下滚动，立即放弃手势接管，避免与列表滚动冲突。

6. **剪贴板优先复制 `rawPayload`**
   - 工具输出常包含嵌套 JSON，用户需要原始结构进行调试。Fallback 到 `title\nmessage` 保证纯文本通知也能复制。

7. **Toast 容器使用 `pointer-events-none` + 子元素 `pointer-events-auto`**
   - 确保 Toast 不阻挡下方页面点击，同时 Toast 自身可交互（点击、滑动、关闭按钮）。

8. **右抽屉打开时自动 markAllRead**
   - 用户主动打开通知中心即视为已读。`unreadCount` 归零，但历史项的 `read` 标记同时被设为 `true`，便于后续扩展"未读高亮"样式。

---

## 12. 术语速查表

| 术语 | 说明 |
|------|------|
| `VcpNotification` | 前端通知数据模型，同时驱动 Toast 与通知中心 |
| `activeToasts` | 当前屏幕上悬浮显示的 Toast 数组 |
| `historyList` | 通知中心抽屉内的历史记录数组，上限 100 |
| `toastOnly` | 标记：仅显示 Toast，不进入历史 |
| `historyOnly` | 标记：仅进入历史，不弹出 Toast |
| `silent` | 标记：完全静默，不展示任何 UI |
| `isPreformatted` | 标记：使用等宽字体 + 滚动块渲染消息 |
| `rawPayload` | 原始后端 payload，用于剪贴板复制和 action 处理 |
| `vcp-system-event` | Tauri 事件名，Rust → Vue 的核心通知通道 |
| `useNotificationProcessor` | VCP 事件解析器：payload → VcpNotification |
| `checkMessageFilter` | 内置降噪过滤引擎 |
| `StreamKeepaliveService` | Android 前台服务，依赖系统通知维持存活 |
| `POST_NOTIFICATIONS` | Android 13+ 必需权限，由 `tauri-plugin-vcp-mobile` 管理 |

---
*最后更新：2026-06-05 | VCP Mobile v1.0.3*
