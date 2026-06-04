---
id: VUE-CORE-005
title: Agent与群组状态
description: VCP Mobile 前端 Agent/群组 列表管理、配置缓存与话题联动的状态设计
version: 1.0.3
date: 2026-06-04
---

# 05. Agent与群组状态

## 1. 概述

### 1.1 领域定位

`Agent与群组状态`是 VCP Mobile 前端核心数据层中负责**智能体（Agent）与群组（Group）生命周期管理**的领域。它覆盖从侧边栏列表展示、配置编辑、话题联动到头像视觉资产缓存的完整链路，是用户与 AI 交互的入口状态枢纽。

该领域**不**涉及：
- 聊天消息的具体渲染与流式解析（由 `chatHistoryStore` / `chatStreamStore` 负责）
- 模型推理与 HTTP 请求（由 Rust 后端 `vcp_client.rs` 负责）
- 全局应用设置（由 `settingsStore` 负责，但排序数据存储于此）

### 1.2 模块构成表

| 文件 | 类型 | 行数 | 职责 |
|------|------|------|------|
| `src/core/stores/assistant.ts` | Pinia Store | 261 | Agent/Group 列表管理、CRUD、未读计数、头像上传封装 |
| `src/core/stores/avatar.ts` | Pinia Store | 250 | 头像二进制缓存（Blob URL）、Dominant Color 计算与同步 |
| `src/core/stores/topicListManager.ts` | Pinia Store | 414 | 话题列表的流式加载、搜索筛选、乐观更新 |
| `src/core/stores/chatSessionStore.ts` | Pinia Store | 103 | 当前选中项（Agent/Group）、当前话题 ID、最后活跃话题映射 |
| `src/features/agent/AgentList.vue` | Vue 组件 | 439 | Agent/Group 侧边栏渲染、拖拽排序、右滑手势 |
| `src/features/agent/AgentSettingsView.vue` | Vue 组件 | 400 | Agent 配置全屏编辑页、自动保存、头像裁剪 |
| `src/features/agent/GroupSettingsView.vue` | Vue 组件 | 445 | Group 配置全屏编辑页、成员管理、模型统一设置 |
| `src/features/agent/SidebarTabs.vue` | Vue 组件 | 28 | 侧边栏「助手 / 话题」Tab 切换 |
| `src/features/agent/SidebarSearch.vue` | Vue 组件 | 29 | 动态占位符搜索输入框 |
| `src/components/ui/VcpAvatar.vue` | Vue 组件 | 111 | 头像展示组件（缓存感知、Fallback、Dominant Color 边框） |

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                    Vue 3 前端层                              │
│                                                              │
│   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│   │ AgentList.vue│  │AgentSettings │  │GroupSettings │     │
│   │  (侧边栏)     │  │   View.vue   │  │   View.vue   │     │
│   └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
│          │                 │                 │              │
│          ▼                 ▼                 ▼              │
│   ┌──────────────────────────────────────────────────────┐  │
│   │              assistantStore (assistant.ts)            │  │
│   │  agents[] / groups[] / combinedItems / unreadCounts  │  │
│   └──────────────┬───────────────────────┬───────────────┘  │
│                  │                       │                  │
│          ┌───────┴───────┐       ┌───────┴───────┐         │
│          ▼               ▼       ▼               ▼         │
│   ┌────────────┐  ┌────────────┐  ┌────────────────────┐  │
│   │ avatarStore│  │ topicStore │  │ chatSessionStore   │  │
│   │ (avatar.ts)│  │(topicList..)│  │(chatSessionStore.) │  │
│   └──────┬─────┘  └──────┬─────┘  └─────────┬──────────┘  │
│          │               │                  │              │
│          └───────────────┼──────────────────┘              │
│                          ▼                                 │
│              ┌─────────────────────┐                       │
│              │   VcpAvatar.vue     │                       │
│              │  (Blob URL / Canvas)│                       │
│              └─────────────────────┘                       │
└──────────────────────────┬─────────────────────────────────┘
                           │ IPC (Tauri Commands)
┌──────────────────────────▼─────────────────────────────────┐
│              src-tauri (Rust 核心层)                        │
│   agent_service / avatar_service / topic_service / ...     │
└────────────────────────────────────────────────────────────┘
```

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **单一 Store 收敛** | Agent 与 Group 的列表状态统一收敛在 `assistantStore`，避免拆分导致的 N+1 查询和状态同步问题 |
| **选中状态独立** | `currentSelectedItem` 与 `currentTopicId` 由 `chatSessionStore` 独立管理，解耦列表管理与会话路由 |
| **懒加载 + 流式** | 话题列表通过 Tauri `Channel` 流式接收，避免大数据量阻塞主线程；头像按需获取并 LRU 缓存 |
| **乐观更新** | 话题的未读计数、消息计数、锁定状态在 UI 上先变更，后同步到后端 |
| **排序外置持久化** | 拖拽排序结果保存在 `settingsStore.settings.agentOrder / groupOrder`，而非 Agent/Group 配置本身，保持后端数据纯净 |
| **防抖自动保存** | Agent/Group 设置页监听配置变化，800ms–1000ms 防抖后自动落盘，减少用户心智负担 |

---

## 2. Agent 状态（assistant.ts / assistantStore）

### 2.1 Agent 列表状态

`assistantStore` 使用 Pinia Composition API 风格定义，核心状态如下：

> 文件位置：`src/core/stores/assistant.ts` 第 49–59 行

```typescript
const agents = ref<AgentConfig[]>([]);
const groups = ref<GroupConfig[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);
const unreadCounts = ref<Record<string, number>>({});
```

| 状态 | 类型 | 说明 |
|------|------|------|
| `agents` | `Ref<AgentConfig[]>` | 全部 Agent 列表，由 `get_agents` 填充 |
| `groups` | `Ref<GroupConfig[]>` | 全部 Group 列表，由 `get_groups` 填充 |
| `loading` | `Ref<boolean>` | 全局加载标志，CRUD 操作期间置为 true |
| `error` | `Ref<string \| null>` | 最近一次操作的错误信息 |
| `unreadCounts` | `Ref<Record<string, number>>` | 每个 item（agent 或 group）的未读消息数，-1 表示"有新消息但未计具体数量" |

`combinedItems` 是一个计算属性，将 `agents` 和 `groups` 合并为统一列表，并附加 `type: "agent" | "group"` 标记：

> 文件位置：`src/core/stores/assistant.ts` 第 76–79 行

```typescript
const combinedItems = computed(() => [
  ...agents.value.map((agent) => ({ ...agent, type: "agent" as const })),
  ...groups.value.map((group) => ({ ...group, type: "group" as const })),
]);
```

### 2.2 当前选中 Agent

`assistantStore` **不**维护当前选中状态。当前选中的 Agent/Group 及其话题由 `chatSessionStore` 独立管理：

> 文件位置：`src/core/stores/chatSessionStore.ts` 第 6–10 行

```typescript
const currentSelectedItem = ref<any>(null);   // 当前选中的 Agent 或 Group 对象
const currentTopicId = ref<string | null>(null); // 当前话题 ID
const lastActiveTopicMap = ref<Record<string, string>>({}); // 每个 item 最后打开的话题
```

`chatSessionStore` 通过 Pinia `persist` 插件将这三个字段持久化到 `localStorage`，保证应用重启后能恢复上次会话上下文：

> 文件位置：`src/core/stores/chatSessionStore.ts` 第 99–103 行

```typescript
{
  persist: {
    pick: ['currentSelectedItem', 'currentTopicId', 'lastActiveTopicMap'],
  },
}
```

### 2.3 Agent 配置缓存策略

`assistantStore` 本身不做配置级缓存（配置缓存由 Rust 后端 `AgentConfigState` 的 `DashMap` 承担）。前端采用**列表级整刷**策略：

| 操作 | 前端行为 | 后端命令 |
|------|---------|----------|
| 加载列表 | `fetchAgents()` → 填充 `agents` | `get_agents` |
| 创建 Agent | `createAgent(name)` → 返回新对象，**不自动 fetch** | `create_agent` |
| 删除 Agent | `deleteAgent(id)` → 成功后 `fetchAgents()` | `delete_agent` |
| 保存配置 | `saveAgent(agent)` → 成功后 `fetchAgents()` | `save_agent_config` |
| 保存头像 | `saveAvatar(...)` → 返回 hash，**不自动 fetch** | `save_avatar_data` |

> 文件位置：`src/core/stores/assistant.ts` 第 81–96 行、第 187–201 行

**设计意图**：创建和头像保存后不自动全局 fetch，是因为调用方（如 `AgentSettingsView`）通常会在本地增量更新 UI，避免不必要的全量列表重刷。

### 2.4 Agent 排序与拖拽

排序逻辑不在 Store 中，而在消费组件 `AgentList.vue` 内，通过 **SortableJS** 实现：

> 文件位置：`src/features/agent/AgentList.vue` 第 70–136 行

```typescript
Sortable.create(agentListRef.value, {
  animation: 150,
  handle: ".drag-handle",      // 仅拖拽手柄响应
  delay: 200,                  // 移动端延迟，避免与点击/滑动冲突
  delayOnTouchOnly: true,
  touchStartThreshold: 3,
  direction: "vertical",
  forceFallback: true,
  ghostClass: "opacity-50",
  onEnd: (evt) => {
    const newOrder = orderedAgents.value.map((a) => a.id);
    const [movedItem] = newOrder.splice(evt.oldIndex!, 1);
    newOrder.splice(evt.newIndex!, 0, movedItem);
    settingsStore.updateSettings({ agentOrder: newOrder });
  },
});
```

排序结果通过 `settingsStore.updateSettings` 持久化到 Rust 后端的 `settings` 表：`agentOrder` 和 `groupOrder` 是两个字符串数组，分别保存 Agent ID 和 Group ID 的展示顺序。

列表渲染时，`orderedAgents` / `orderedGroups` 计算属性根据 `settings.agentOrder` / `settings.groupOrder` 对原始数组重排：

> 文件位置：`src/features/agent/AgentList.vue` 第 46–60 行

```typescript
const orderedAgents = computed(() => {
  const agents = assistantStore.agents;
  const order = settingsStore.settings?.agentOrder || [];
  if (order.length === 0) return agents;
  return [...agents].sort((a, b) => {
    const indexA = order.indexOf(a.id);
    const indexB = order.indexOf(b.id);
    if (indexA === -1 && indexB === -1) return 0;
    if (indexA === -1) return 1;   // 未在 order 中的置尾
    if (indexB === -1) return -1;
    return indexA - indexB;
  });
});
```

### 2.5 侧边栏卡片 Swipe 手势

`AgentList.vue` 为每个 Agent/Group 卡片实现了**向右滑动展开快捷操作**的移动端原生手势。该手势与 SortableJS 拖拽共享同一 DOM 区域，因此需要精细的手势状态机避免冲突：

> 文件位置：`src/features/agent/AgentList.vue` 第 62–260 行

| 状态/变量 | 说明 |
|-----------|------|
| `isSorting` | SortableJS 拖拽期间置为 `true`，Swipe 手势完全禁用 |
| `activeSwipeId` | 当前已展开卡片的 ID，同一时间仅允许一张卡片展开 |
| `currentSwipeX` | 卡片当前的水平位移（px），最大 `MAX_SWIPE = 80` |
| `isStartedAsSwiped` | 手势开始时卡片是否已处于展开状态，决定后续行为分支 |

**手势状态分流**：

```
触摸开始
    │
    ▼
已处于展开状态?
    │
    ├──► Yes ──► 锁定为水平手势（禁止垂直滚动）
    │            左滑：阻尼缩回，右滑：微阻尼延伸
    │
    └──► No  ──► 判断滑动方向（斜率阈值 tan(30°) ≈ 0.577）
                 │
                 ├──► 垂直 ──► 放弃，允许页面滚动
                 │
                 └──► 水平 ──► 左滑：放弃，允许关闭侧边栏
                                右滑：响应卡片展开
```

核心交互约束：

- **拖拽期间**：`isSorting = true` 时 Swipe 完全禁用，且通过 `e.preventDefault()` + `e.stopPropagation()` 防止手指滑动误触外层容器
- **向左滑动**：若卡片处于折叠状态，不阻止事件冒泡，让 touch 事件流传递给外层侧边栏以执行「左滑关闭侧边栏」
- **展开后二次触摸**：强制锁定水平方向，确保用户能顺滑地「收回」快捷操作面板，而不会误入垂直滚动

---

## 3. Avatar 状态（avatar.ts / avatarStore）

### 3.1 头像数据缓存

`avatarStore` 是独立于 `assistantStore` 的专用缓存层，负责头像二进制数据的获取、Blob URL 构造与生命周期管理。

> 文件位置：`src/core/stores/avatar.ts` 第 112–125 行

```typescript
const cache = reactive(new Map<string, AvatarCache>());
const pending = new Map<string, Promise<string>>();
const inFlightCompute = new Set<string>();
```

| 状态 | 类型 | 说明 |
|------|------|------|
| `cache` | `Reactive<Map<string, AvatarCache>>` | 头像 Blob URL 缓存，key = `${ownerType}:${ownerId}` |
| `pending` | `Map<string, Promise<string>>` | 防并发重复请求：同一 ID 正在加载时复用 Promise |
| `inFlightCompute` | `Set<string>` | 防止 Dominant Color 重复计算 |

**`getAvatarUrl` 缓存策略**：

> 文件位置：`src/core/stores/avatar.ts` 第 127–222 行

```
输入: ownerType, ownerId, version=0
       │
       ▼
┌─────────────────────────┐
│ 1. 同步检查 cache        │──> key = `${ownerType}:${ownerId}`
│    existing?             │
└──────────┬──────────────┘
      Yes  │    No / 版本旧
      ┌────┘        └────────┐
      ▼                      ▼
 直接返回 Blob URL      ┌─────────────────────────┐
                       │ 2. 检查 pending Map      │
                       │    是否已有同 ID 请求?   │
                       └──────────┬──────────────┘
                             Yes  │    No
                             ┌────┘     └────────┐
                             ▼                   ▼
                        返回现有 Promise    发起 invoke("get_avatar")
                                                 │
                                                 ▼
                                        ┌─────────────────────────┐
                                        │ 3. 构建 Blob URL         │
                                        │    URL.createObjectURL   │
                                        │ 4. LRU 淘汰 (上限 50)    │
                                        │ 5. cache.set(key, {...}) │
                                        └─────────────────────────┘
```

**LRU 淘汰**：当缓存条目数达到 `MAX_AVATAR_CACHE = 50` 时，淘汰最早插入的条目并 `URL.revokeObjectURL` 释放物理内存：

> 文件位置：`src/core/stores/avatar.ts` 第 197–205 行

```typescript
const MAX_AVATAR_CACHE = 50;
if (cache.size >= MAX_AVATAR_CACHE) {
  const firstKey = cache.keys().next().value;
  if (firstKey) {
    const old = cache.get(firstKey);
    if (old) URL.revokeObjectURL(old.blobUrl);
    cache.delete(firstKey);
  }
}
```

**版本号机制**：缓存条目的 `version` 取 `Math.max(result.updated_at, 请求传入的 version)`。当组件传入 `version = Date.now()`（如头像上传后）时，旧缓存会因 `existing.version < version` 而失效，强制重新获取。

### 3.2 Dominant Color 管理

`dominantColors` 是一个独立的 reactive Map，提供**同步读取**能力，供 `VcpAvatar.vue` 的边框染色和 `computeShell` 等同步场景使用：

> 文件位置：`src/core/stores/avatar.ts` 第 122 行

```typescript
const dominantColors = reactive(new Map<string, string>());
```

**后端缺失时的前端兜底计算**：

当 `get_avatar` 返回的 `dominant_color` 为 `null` 时（多见于旧数据迁移或头像刚上传尚未提取），`avatarStore` 在前端通过 **Canvas 16×16 降采样 + 512-bin 颜色量化** 自主计算：

> 文件位置：`src/core/stores/avatar.ts` 第 20–110 行

| 步骤 | 技术细节 |
|------|----------|
| ① 降采样 | Canvas 绘制 16×16，降低计算量 |
| ② 透明过滤 | 忽略 `a < 128` 的像素 |
| ③ 灰度过滤 | 排除纯黑（`max < 30`）、纯白（`min > 225`）、低饱和度（`chroma < 25`） |
| ④ 512-bin 量化 | 每通道 32 为粒度，统计最多像素 bin |
| ⑤ 回退链 | bin 平均 → 全局平均 → `#808080` |
| ⑥ 回写后端 | `invoke("store_dominant_color", { ownerType, ownerId, color })` |

### 3.3 与 AgentStore 的联动

`assistantStore` 提供 `saveAvatar` 方法作为上传入口，但实际的 Blob URL 生成与缓存完全由 `avatarStore` 自治：

> 文件位置：`src/core/stores/assistant.ts` 第 219–241 行

```typescript
const saveAvatar = async (ownerType, ownerId, mimeType, imageData) => {
  const hash = await invoke<string>("save_avatar_data", { ... });
  return hash;
};
```

消费链路：

```
AgentSettingsView / GroupSettingsView
       │
       ▼ 头像裁剪确认
assistantStore.saveAvatar() ──invoke──> Rust save_avatar_data
       │
       ▼ 返回 hash
avatarVersion = Date.now()  (props 传入 VcpAvatar)
       │
       ▼
VcpAvatar.vue
       │
       ▼ watchEffect 检测到 version 变化
avatarStore.getAvatarUrl(type, id, version)
       │
       ▼ version > cache.version，强制刷新
更新 cache + 返回新 Blob URL
```

> 文件位置：`src/components/ui/VcpAvatar.vue` 第 54–80 行

---

## 4. Topic 列表管理（topicListManager.ts / topicStore）

### 4.1 话题列表状态

`topicStore`（`useTopicStore`）管理当前选中 Agent/Group 下的话题列表，采用**流式增量加载**而非一次性全量返回。

> 文件位置：`src/core/stores/topicListManager.ts` 第 26–35 行

```typescript
const topics = ref<Topic[]>([]);
const loading = ref(false);
const searchTerm = ref("");
const currentAgentId = ref<string | null>(null);
```

**流式加载机制**：

> 文件位置：`src/core/stores/topicListManager.ts` 第 86–134 行

```typescript
const channel = new Channel<Topic[]>();
topics.value = [];

channel.onmessage = (chunk) => {
  // 竞态检查：若用户已切换其他 Agent，丢弃旧结果
  if (currentAgentId.value !== ownerId) return;

  const mappedChunk = chunk.map((t) => ({
    ...t,
    ownerId: ownerId,
    ownerType: owner_type,
    name: t.name || t.title || t.id,
  }));

  topics.value.push(...mappedChunk);
  topics.value = [...topics.value]; // 强制触发虚拟列表重绘
};

await invoke("get_topics_streamed", { ownerId, ownerType: owner_type, onChunk: channel });
```

Rust 后端通过 Tauri `Channel` 分批次推送话题块，前端每收到一批即增量追加到 `topics.value`。`currentAgentId` 用于**竞态防护**：若用户在加载过程中切换了侧边栏选中项，旧请求的返回数据会被丢弃。

### 4.2 与 Agent 的从属关系

话题在数据模型上从属于某个 `owner`（Agent 或 Group）。`topicStore` 本身不感知「当前选中的是 Agent 还是 Group」，它只接收 `ownerId` 和 `ownerType` 参数：

| 场景 | 调用方 | 行为 |
|------|--------|------|
| 切换 Agent/Group | `chatSessionStore.selectItem()` → 触发外部 watcher | `topicStore.loadTopicList(ownerId, ownerType)` |
| 创建话题 | 话题列表 UI | `topicStore.createTopic(ownerId, ownerType, name)`，本地 `unshift` 乐观更新 |
| 删除话题 | 话题列表 UI | `topicStore.deleteTopic(...)`，若删除的是当前话题则自动切换至下一个 |
| 同步完成后 | 外部生命周期 | `topicStore.invalidateAllTopicCaches()` → `topics.value = []` |

**删除话题的级联处理**：

> 文件位置：`src/core/stores/topicListManager.ts` 第 194–227 行

```typescript
if (sessionStore.currentTopicId === topicId) {
  const nextTopic = topics.value[0];
  if (nextTopic) {
    await sessionStore.selectTopicById(ownerId, nextTopic.id);
  } else {
    sessionStore.currentTopicId = null;
    // chatHistoryStore 监听 currentTopicId 变化，自动清空历史
  }
}
```

### 4.3 话题排序与筛选

**排序**：话题默认按后端返回顺序排列（通常为 `updated_at DESC`），前端不做二次排序。

**搜索筛选**：`filteredTopics` 计算属性支持按标题和创建日期搜索：

> 文件位置：`src/core/stores/topicListManager.ts` 第 54–77 行

```typescript
const filteredTopics = computed(() => {
  const term = searchTerm.value.toLowerCase().trim();
  if (!term) return topics.value;

  return topics.value.filter((topic) => {
    const nameMatch = topic.name.toLowerCase().includes(term);
    // 日期匹配：支持 YYYY-MM-DD 和 YYYY-MM-DD HH:MM 格式
    let dateMatch = false;
    const createdAt = topic.createdAt || topic.created_at;
    if (createdAt) {
      const date = new Date(createdAt > 1e11 ? createdAt : createdAt * 1000);
      const fullDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")} ${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
      const shortDateStr = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
      dateMatch = fullDateStr.includes(term) || shortDateStr.includes(term);
    }
    return nameMatch || dateMatch;
  });
});
```

---

## 5. 群组状态

### 5.1 群组列表与当前选中

VCP Mobile 前端**没有独立的 `groupStore`**。群组的列表状态与 Agent 统一收敛在 `assistantStore`：

> 文件位置：`src/core/stores/assistant.ts` 第 49–51 行

```typescript
const agents = ref<AgentConfig[]>([]);
const groups = ref<GroupConfig[]>([]);
```

`chatSessionStore.selectItem` 通过判断传入对象是否具有 `members` 字段来区分 Agent 与 Group：

> 文件位置：`src/core/stores/chatSessionStore.ts` 第 49–90 行

```typescript
const selectItem = async (item: any, loadHistoryCallback?) => {
  const ownerId = item.id;
  const ownerType = item.members ? 'group' : 'agent';
  // ...
};
```

当前选中的 Group 与 Agent 在 `currentSelectedItem` 中以统一形态存储，通过 `type: "agent" | "group"` 区分。

### 5.2 群组成员管理

群组成员管理由 `GroupSettingsView.vue` 负责：

> 文件位置：`src/features/agent/GroupSettingsView.vue` 第 127–138 行、第 204–215 行

```typescript
const fetchAgents = async () => {
  const agents = await invoke<any[]>("get_agents");
  allAgents.value = agents.map(a => ({ id: a.id, name: a.name, avatar: a.avatar }));
};

const toggleMember = (agentId: string) => {
  const index = groupConfig.value.members.indexOf(agentId);
  if (index === -1) {
    groupConfig.value.members.push(agentId);
    if (!groupConfig.value.memberTags[agentId]) {
      groupConfig.value.memberTags[agentId] = agent?.name || agentId;
    }
  } else {
    groupConfig.value.members.splice(index, 1);
  }
};
```

成员勾选与标签编辑均为本地状态变更，最终通过 `autoSave` → `assistantStore.saveGroup()` → `save_group_config` 统一落盘。

---

## 6. 数据流时序

### 6.1 应用启动时 Agent 列表加载时序

```
main.ts 应用初始化
    │
    ▼
assistantStore.fetchAgents()
    │
    ▼ invoke("get_agents")
Rust agent_service::get_agents
    │
    ▼ 返回 Vec<AgentConfig>
agents.value = fetchedAgents
    │
    ▼
assistantStore.refreshUnreadCounts()
    │
    ▼ invoke("get_unread_counts")
Rust 返回 Record<ownerId, number>
    │
    ▼
unreadCounts.value = counts
    │
    ▼
AgentList.vue 渲染列表 + 未读红点
```

### 6.2 切换 Agent 时的状态联动

```
用户点击 AgentList 中某 Agent
    │
    ▼ emit("select-agent", agent)
父组件（AgentSidebar / App.vue）
    │
    ▼ chatSessionStore.selectItem(agent, loadHistory)
    │
    ├──► 若已选中且 currentTopicId 存在，直接返回（防重复）
    │
    ├──► lastActiveTopicMap[agent.id]?
    │       Yes ──► 使用该 topicId
    │       No  ──► invoke("get_topics") 取最新话题
    │
    ▼ selectTopicById(agent.id, topicId, loadHistory)
    │
    ├──► currentTopicId = topicId
    ├──► lastActiveTopicMap[agent.id] = topicId
    ├──► currentSelectedItem = { ...agent, type: "agent" }
    │
    ▼ loadHistoryCallback(itemId, ownerType, topicId)
    │
    ▼ topicStore.loadTopicList(agent.id, "agent")
        │
        ▼ invoke("get_topics_streamed") + Channel
        │
        ▼ 增量 push 到 topics.value
```

### 6.3 保存 Agent 设置后的本地更新

```
用户在 AgentSettingsView 修改字段（如 name / temperature）
    │
    ▼ watch(agentConfig, { deep: true })
    │
    ├──► 与 originalConfig 快照 JSON 对比
    │       相同 ──► 跳过
    │       不同 ──► 800ms debounce
    │
    ▼ autoSave()
    │
    ▼ assistantStore.saveAgent(agentConfig.value)
    │
    ▼ invoke("save_agent_config")
Rust 事务写入 SQLite
    │
    ▼ 成功后 fetchAgents()
    │
    ▼ agents.value 刷新
    │
    ▼ AgentList.vue 自动更新显示
    │
    ▼ saveSuccess = true（2s 后自动清除）
```

---

## 7. 与 Rust 后端的 IPC 交互

### 7.1 Tauri Commands 调用表

| 命令 | 调用方 | 参数 | 返回 | 用途 |
|------|--------|------|------|------|
| `get_agents` | `assistantStore.fetchAgents` | — | `AgentConfig[]` | 获取全部 Agent 列表 |
| `get_groups` | `assistantStore.fetchGroups` | — | `GroupConfig[]` | 获取全部 Group 列表 |
| `create_agent` | `assistantStore.createAgent` | `{ name }` | `AgentConfig` | 新建 Agent |
| `delete_agent` | `assistantStore.deleteAgent` | `{ agentId }` | — | 删除 Agent |
| `create_group` | `assistantStore.createGroup` | `{ name }` | `GroupConfig` | 新建 Group |
| `delete_group` | `assistantStore.deleteGroup` | `{ groupId }` | — | 删除 Group |
| `save_agent_config` | `assistantStore.saveAgent` | `{ agent }` | — | 完整保存 Agent 配置 |
| `save_group_config` | `assistantStore.saveGroup` | `{ group }` | — | 完整保存 Group 配置 |
| `read_agent_config` | `AgentSettingsView.loadConfig` | `{ agentId, allowDefault }` | `AgentConfig` | 读取单个 Agent 配置 |
| `read_group_config` | `GroupSettingsView.fetchGroupConfig` | `{ groupId }` | `GroupConfig` | 读取单个 Group 配置 |
| `save_avatar_data` | `assistantStore.saveAvatar` | `{ ownerType, ownerId, mimeType, imageData }` | `string` (hash) | 保存头像二进制 |
| `get_avatar` | `avatarStore.getAvatarUrl` | `{ ownerType, ownerId }` | `AvatarResult \| null` | 获取头像数据 |
| `store_dominant_color` | `avatarStore` (前端计算后) | `{ ownerType, ownerId, color }` | — | 回写主色调到后端 |
| `get_unread_counts` | `assistantStore.refreshUnreadCounts` | — | `Record<string, number>` | 批量获取未读计数 |
| `get_topics_streamed` | `topicStore.loadTopicList` | `{ ownerId, ownerType, onChunk }` | — | 流式获取话题列表 |
| `get_topics` | `chatSessionStore.selectItem` | `{ ownerId, ownerType }` | `any[]` | 获取话题列表（非流式 fallback） |
| `create_topic` | `topicStore.createTopic` | `{ ownerId, ownerType, name }` | `Topic` | 创建话题 |
| `delete_topic` | `topicStore.deleteTopic` | `{ ownerId, ownerType, topicId }` | — | 删除话题 |
| `update_topic_title` | `topicStore.updateTopicTitle` | `{ ownerId, ownerType, topicId, title }` | — | 更新话题标题 |
| `toggle_topic_lock` | `topicStore.toggleTopicLock` | `{ ownerId, ownerType, topicId, locked }` | — | 切换话题锁定状态 |
| `set_topic_unread` | `topicStore.setTopicUnread` / `markTopicAsRead` | `{ ownerId, ownerType, topicId, unread }` | — | 设置话题未读状态 |
| `read_settings` | `settingsStore.fetchSettings` | — | `AppSettings` | 读取全局设置（含排序数组） |
| `update_settings` | `settingsStore.updateSettings` | `{ updates }` | `AppSettings` | 增量更新全局设置 |

### 7.2 事件监听表

当前 `assistantStore` 与 `topicStore` **不**依赖 Tauri 事件通道（`listen`）进行状态同步。所有状态变更均通过以下路径处理：

| 事件源 | 处理方式 | 说明 |
|--------|----------|------|
| 同步完成 | `main.ts` 统一 `window.location.reload()` | 全量刷新，避免逐事件处理同步冲突 |
| 话题列表变更 | 命令调用后本地乐观更新 | 不监听后端推送 |
| Agent/Group 配置变更 | 命令调用后本地 `fetchAgents()` / `fetchGroups()` | 由调用方主动刷新 |
| 头像变更 | `version` prop 驱动 `VcpAvatar` 强制刷新 | 无全局事件广播 |

> 注：`topicListManager.ts` 中曾存在对 `topic-index-updated` 事件的监听代码，因 Rust 侧未实际 emit 已被移除。
> 文件位置：`src/core/stores/topicListManager.ts` 第 37 行注释

---

## 8. 设计决策与注意事项

### 8.1 为何 Agent 与 Group 共用同一个 Store？

在早期的迭代中，Agent 与 Group 的列表分别由不同模块管理。但随着侧边栏需要同时展示「Individual Agents」与「Agent Groups」两个区块，并支持统一搜索与交叉导航，拆分管理导致了大量状态同步代码。将两者收敛到 `assistantStore` 后：

- `combinedItems` 提供统一的遍历视图
- `unreadCounts` 以同一本字典覆盖两种实体
- CRUD 模式完全一致（`fetchXxx / createXxx / deleteXxx / saveXxx`）

### 8.2 为何 `currentTopicId` 不在 `topicStore` 而在 `chatSessionStore`？

话题列表（`topicStore.topics`）是**从属数据**：它依附于当前选中的 Agent/Group，切换选中项后旧列表即失效。而 `currentTopicId` 是**会话路由状态**：它决定聊天窗口展示哪条话题的历史记录，且需要跨组件（侧边栏高亮、聊天区加载、输入框归属）共享。若将 `currentTopicId` 放入 `topicStore`，会导致「列表数据失效但路由状态仍需保留」的语义冲突。

### 8.3 为何 `avatarStore` 使用 `reactive(new Map())` 而非 `ref({})`？

头像缓存需要高频的「key 查找」和「精确删除」。`reactive(new Map())` 相比 `ref<Record<string, T>>({})` 的优势：

- `Map.get()` / `Map.delete()` 在 Vue 响应式系统下仍保持 O(1)
- `Map.keys().next().value` 可直接获取最早插入项，实现 LRU
- `reactive` 包裹后，外部组件（如 `VcpAvatar.vue`）可通过 `avatarStore.cache.get(key)` 做**同步缓存命中检查**，消除异步获取导致的闪烁

### 8.4 为何话题列表使用 Channel 流式加载？

移动端场景下，一个长期使用的 Agent 可能积累数百个话题。一次性 `invoke` 返回全部数据会产生以下问题：

1. **JSON 序列化/反序列化阻塞**：大数据量阻塞主线程
2. **内存峰值**：完整数组在 Rust 与 JS 两侧各存在一份
3. **白屏时间**：用户必须等待全部话题返回才能看到列表

通过 `Channel<Topic[]>` 分块推送，前端每收到一批即可渲染一批，实现渐进式加载。

### 8.5 为何排序保存在 `settingsStore` 而非 Agent/Group 配置？

若将 `sort_order` 字段加入 `AgentConfig` / `GroupConfig`：

- 每次拖拽后需要修改 N 个对象的配置并保存，产生 N 次 DB 写入
- 同步领域需要将排序变更扩散到多端，增加冲突概率
- 排序是**视图层偏好**，不应污染业务实体

将 `agentOrder` / `groupOrder` 保存在 `settings` 表中，一次 `update_settings` 即可完成持久化，且天然复用 Settings 的同步机制。

### 8.6 自动保存的防抖与快照策略

`AgentSettingsView` 和 `GroupSettingsView` 均实现了基于「原始快照对比」的防抖保存：

> 文件位置：`src/features/agent/AgentSettingsView.vue` 第 119–187 行

```typescript
const originalConfig = ref<AgentConfig | null>(null);

watch(agentConfig, () => {
  if (!originalConfig.value) return;
  if (JSON.stringify(agentConfig.value) === JSON.stringify(originalConfig.value)) return;
  if (saveTimeout) clearTimeout(saveTimeout);
  saveTimeout = setTimeout(() => autoSave(), 800);
}, { deep: true });
```

- **800ms / 1000ms 防抖**：避免用户连续输入时触发大量后端请求
- **JSON 快照对比**：仅当配置真正变更时才调用 `autoSave`，防止无意义写盘
- **保存后更新快照**：`originalConfig.value = JSON.parse(JSON.stringify(agentConfig.value))`，确保后续变更能被正确检测

---

## 9. 术语速查表

| 术语 | 英文/缩写 | 定义 | 相关文件 |
|------|----------|------|----------|
| assistantStore | — | 前端 Agent/Group 列表与 CRUD 的 Pinia Store | `assistant.ts` |
| avatarStore | — | 头像 Blob URL 缓存与 Dominant Color 管理的 Pinia Store | `avatar.ts` |
| topicStore | — | 话题列表流式加载与乐观更新的 Pinia Store | `topicListManager.ts` |
| chatSessionStore | — | 当前选中项、当前话题 ID、最后活跃话题映射的 Pinia Store | `chatSessionStore.ts` |
| Combined Item | — | Agent 与 Group 合并后的统一列表项，带 `type` 标记 | `assistant.ts` |
| Blob URL | — | 通过 `URL.createObjectURL(blob)` 创建的内存级对象 URL，用于展示头像 | `avatar.ts` |
| Dominant Color | 主色调 | 从头像中提取的代表性颜色，用于 UI 边框与 Glassmorphism 背景 | `avatar.ts`, `VcpAvatar.vue` |
| Channel | Tauri Channel | Tauri v2 的流式通信机制，Rust 可多次 `send()`，前端通过 `onmessage` 接收 | `topicListManager.ts` |
| 竞态防护 | Race Condition Guard | 通过 `currentAgentId` 比对，丢弃非当前选中项的异步返回数据 | `topicListManager.ts` |
| 乐观更新 | Optimistic Update | UI 先变更本地状态，再异步同步到后端，提升交互响应感 | `topicListManager.ts` |
| 原始快照 | Original Snapshot | 配置加载后深克隆一份 JSON 快照，用于后续变更检测 | `AgentSettingsView.vue` |
| SortableJS | — | 第三方拖拽排序库，用于 Agent/Group 侧边栏的手动排序 | `AgentList.vue` |
| agentOrder / groupOrder | — | 保存在 `AppSettings` 中的 ID 数组，定义侧边栏展示顺序 | `settings.ts`, `AgentList.vue` |
| lastActiveTopicMap | — | 记录每个 Agent/Group 最后一次打开的话题 ID，会话恢复时使用 | `chatSessionStore.ts` |
| allowDefault | — | `read_agent_config` 的参数，为 `true` 时若查不到记录返回默认配置而非报错 | `AgentSettingsView.vue` |
| mobileSystemPrompt | — | 仅本机生效、不参与同步的系统提示词，实现移动端差异化行为 | `AgentSettingsView.vue` |
| memberTags | — | Group 中每个成员对应的触发标签，用于自然随机发言模式 | `GroupSettingsView.vue` |
| useUnifiedModel | — | Group 设置项，为 `true` 时所有成员强制使用同一模型 | `GroupSettingsView.vue` |
| LRU 淘汰 | Least Recently Used | 头像缓存上限 50 条，超出时淘汰最早插入项并释放 Blob URL | `avatar.ts` |
| 流式加载 | Streaming Load | 通过 Channel 分块接收话题数据，前端渐进式渲染 | `topicListManager.ts` |

---
*最后更新：2026-06-04 | VCP Mobile v1.0.3*
