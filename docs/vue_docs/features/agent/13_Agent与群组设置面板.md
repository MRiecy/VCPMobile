---
id: VUE-AGEN-013
title: Agent与群组设置面板
description: VCP Mobile 前端 AgentSettingsView 与 GroupSettingsView 的表单设计、头像裁剪与配置持久化
version: 1.0.3
date: 2026-06-04
---

# 13. Agent与群组设置面板

## 1. 概述

### 1.1 领域定位

`AgentSettingsView.vue` 与 `GroupSettingsView.vue` 是 VCP Mobile 前端**智能体（Agent）与群组（Group）的配置入口**。它们负责将用户输入的表单数据通过 Tauri IPC 桥接层持久化到 Rust 核心层的 SQLite 数据库，并管理头像裁剪、模型选择、话题列表等附属交互。

该领域**不**涉及：
- 聊天消息的实际发送与渲染（由 `ChatView` / `chatStreamStore` 负责）
- 模型推理本身（由 Rust 侧 `vcp_client.rs` 负责）
- 侧边栏的列表展示与排序（由 `AgentList.vue` / `AgentSidebar.vue` 负责）

### 1.2 模块构成表

| 文件 | 行数 | 职责 |
|------|------|------|
| `AgentSettingsView.vue` | 400 | Agent 个人设置面板：基本信息、模型参数、提示词 |
| `GroupSettingsView.vue` | 445 | 群组设置面板：成员管理、发言策略、统一模型、提示词 |
| `AgentsCreator.vue` | 107 | Agent / Group 创建触发器，创建成功后自动打开对应设置面板 |
| `assistant.ts` | 261 | Pinia Store：封装 IPC 调用，提供 `saveAgent` / `saveGroup` / `saveAvatar` 等 Action |
| `avatar.ts` | 250 | 头像缓存、Blob URL 管理、Canvas 主色调提取 |
| `chatSessionStore.ts` | 103 | 会话切换，删除 Agent/Group 时清空当前选中项 |
| `AvatarCropper.vue` | 144 | `vue-cropper` 封装：圆形裁剪、旋转、缩放、确认输出 Blob |
| `ModelSelector.vue` | 420 | BottomSheet 风格模型选择器：搜索、Tag 过滤、虚拟滚动 |
| `VcpAvatar.vue` | 111 | 头像展示组件：缓存优先加载、Fallback 首字母、主色调边框 |

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                      Vue 3 前端层                            │
│                                                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │           features/agent/ (本域)                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│  │  │AgentSettings│  │GroupSettings│  │AgentsCreator│   │  │
│  │  │   View.vue  │  │   View.vue  │  │   .vue      │   │  │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘   │  │
│  │         │                │                │          │  │
│  │         └────────────────┼────────────────┘          │  │
│  │                          ▼                           │  │
│  │              ┌───────────────────────┐               │  │
│  │              │  assistant.ts (Store) │               │  │
│  │              │  saveAgent/saveAvatar │               │  │
│  │              └───────────┬───────────┘               │  │
│  │                          ▼                           │  │
│  │              ┌───────────────────────┐               │  │
│  │              │  avatar.ts (Store)    │               │  │
│  │              │  getAvatarUrl /       │               │  │
│  │              │  extractDominantColor │               │  │
│  │              └───────────┬───────────┘               │  │
│  └──────────────────────────┼───────────────────────────┘  │
│                             │ IPC (Tauri Commands)         │
├─────────────────────────────┼──────────────────────────────┤
│         src-tauri (Rust)    ▼                              │
│  ┌───────────────────────────────────────────────────────┐ │
│  │  agent_service.rs  │  group_service.rs  │ avatar_     │ │
│  │  read/save_agent   │  read/save_group   │ service.rs  │ │
│  │  _config           │  _config           │             │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **SlidePage 导航** | 设置面板不使用路由跳转，而是通过 `SlidePage` 组件从右侧滑入，保持主聊天视图的 DOM 活性 |
| **全量保存，防抖触发** | 单个字段变更不调用 `update_agent_config`，而是触发防抖后的 `save_agent_config` 全量写入，简化前端逻辑 |
| **快照比对** | 通过 `originalConfig` 深拷贝快照，仅当用户真正修改内容后才发起后端调用，避免无意义 IO |
| **Store 化 IPC** | 所有 IPC 调用封装在 `assistant.ts` Pinia Store 中，视图层不直接 `invoke`，便于统一通知与错误处理 |
| **头像版本戳** | 头像上传成功后通过 `avatarVersion = Date.now()` 强制 `VcpAvatar` 重新加载，绕过浏览器缓存 |

---

## 2. Agent 设置面板（AgentSettingsView.vue）

> 文件位置：`src/features/agent/AgentSettingsView.vue`

### 2.1 打开方式（SlidePage）

`AgentSettingsView` 并非通过 Vue Router 打开，而是以**全局常驻组件**的形式挂载在 `FeatureOverlays.vue` 中，通过 `overlayStore` 的 **Page Stack** 机制控制显隐：

```ts
// AgentList.vue / AgentsCreator.vue
overlayStore.openAgentSettings(agentId);   // 打开
overlayStore.closeAgentSettings();         // 关闭（点击返回按钮）
```

`FeatureOverlays.vue` 中保持 `AgentSettingsView` **常驻 DOM**（不使用 `v-if`），以确保：
1. `SlidePage` 的 `leave` 动画（向右滑出）能正常完成
2. 表单草稿状态在关闭后得以保留，下次打开时无需重新加载

```vue
<!-- FeatureOverlays.vue -->
<AgentSettingsView
  :is-open="overlayStore.isAgentSettingsOpen"
  :id="overlayStore.agentSettingsId"
  :z-index="overlayStore.getPageZIndex('agentSettings')"
  @close="overlayStore.closeAgentSettings()"
/>
```

`z-index` 由 `overlayStore.getPageZIndex('agentSettings')` 动态计算，基于 `LAYER_PAGE_BASE + stackIndex`，确保多层页面叠加时层级正确。

### 2.2 组件结构

```
┌────────────────────────────────────────────┐
│  ← 助手设置        [保存中... / 已保存 ✅]   │  Header
├────────────────────────────────────────────┤
│  ┌──────────────────────────────────────┐  │
│  │           [头像] (点击更换)            │  │  Identity Section
│  │            Agent 名称                  │  │
│  └──────────────────────────────────────┘  │
│  ┌──────────────────────────────────────┐  │
│  │  系统提示词 (System Prompt)            │  │  Prompt Section
│  │  [textarea: mobileSystemPrompt]       │  │
│  └──────────────────────────────────────┘  │
│  ┌──────────────────────────────────────┐  │
│  │  ▼ 模型参数配置 (可折叠)               │  │  Parameters Section
│  │  ├─ 模型名称 [input + 选择器按钮]      │  │
│  │  ├─ Temperature (0-2)                 │  │
│  │  ├─ 上下文 Token 上限 / 最大输出 Token │  │
│  │  ├─ [开关] 流式输出                   │  │
│  │  └─ [开关] 发送温度参数               │  │
│  └──────────────────────────────────────┘  │
│  ┌──────────────────────────────────────┐  │
│  │         [删除此 Agent]                 │  │  Actions
│  └──────────────────────────────────────┘  │
└────────────────────────────────────────────┘
```

### 2.3 表单字段与数据绑定

组件内部定义了一个本地 `AgentConfig` 接口（与 Rust 侧字段名保持一致，camelCase）：

```ts
interface AgentConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  systemPrompt: string;
  mobileSystemPrompt?: string;
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
  useTemperature: boolean;
}
```

所有表单字段均通过 `v-model` 双向绑定到 `agentConfig.value`：

| Vue 模板字段 | 绑定路径 | 输入类型 |
|-------------|---------|---------|
| Agent 名称 | `agentConfig.name` | `input[type=text]` |
| 系统提示词 | `agentConfig.mobileSystemPrompt` | `textarea` |
| 模型名称 | `agentConfig.model` | `input[type=text]` + 选择器按钮 |
| Temperature | `agentConfig.temperature` | `input[type=number]` min=0 max=2 step=0.1 |
| 上下文 Token 上限 | `agentConfig.contextTokenLimit` | `input[type=number]` |
| 最大输出 Token | `agentConfig.maxOutputTokens` | `input[type=number]` |
| 流式输出 | `agentConfig.streamOutput` | 自定义 Toggle Switch |
| 发送温度参数 | `agentConfig.useTemperature` | 自定义 Toggle Switch |

### 2.4 配置分类（基本信息/模型参数/提示词/话题）

| 分类 | 对应 UI 区域 | 说明 |
|------|-------------|------|
| **基本信息** | Identity Section（头像 + 名称） | `name` 直接展示在聊天界面顶部 |
| **提示词** | Prompt Section | `mobileSystemPrompt` 仅本机生效，不同步到桌面端；留空则回退到 `systemPrompt` |
| **模型参数** | Parameters Section（可折叠） | 默认折叠，减少视觉噪音；展开后显示 model/temperature/token/streamOutput/useTemperature |
| **话题** | 不在本视图内 | 话题列表由独立的 `TopicList.vue` 在聊天侧边栏管理，但 Rust 侧 `read_agent_config` 会一并返回 `topics` 数组 |

---

## 3. 群组设置面板（GroupSettingsView.vue）

> 文件位置：`src/features/agent/GroupSettingsView.vue`

### 3.1 与 AgentSettingsView 的差异

| 维度 | AgentSettingsView | GroupSettingsView |
|------|-------------------|-------------------|
| 标题 | "助手设置" | "群组设置" |
| 身份字段 | 仅 name | name |
| 提示词 | `mobileSystemPrompt` | `groupPrompt` + `invitePrompt` |
| 模型参数 | 直接配置 | `useUnifiedModel` 开关 + `unifiedModel` 选择器 |
| 成员管理 | 无 | 复选框选择 Agent + Tag 输入 |
| 发言策略 | 无 | `mode`（sequential/naturerandom/invite_only） |
| Tag 匹配 | 无 | `tagMatchMode`（strict/natural） |
| 防抖延迟 | 800ms | 1000ms |
| 组件复用 | 使用原始 DOM + `card-modern` | 引入 `SettingsSection` / `SettingsRow` / `SettingsSwitch` |

### 3.2 群组成员管理

群组设置面板的核心差异在于**成员选择器**：

```vue
<!-- GroupSettingsView.vue: Members Section -->
<div v-for="agent in allAgents" :key="agent.id"
  class="flex items-center gap-3 p-3 ...">
  <input type="checkbox" :checked="isMember(agent.id)" @change="toggleMember(agent.id)" />
  <VcpAvatar owner-type="agent" :owner-id="agent.id" ... />
  <div>
    <div>{{ agent.name }}</div>
    <div v-if="isMember(agent.id)">
      <input v-model="groupConfig.memberTags[agent.id]" placeholder="设置触发标签..." />
    </div>
  </div>
</div>
```

数据流：
1. 打开面板时调用 `invoke("get_agents")` 拉取全部 Agent 列表（`allAgents`）
2. 勾选 Agent 时将其 ID 加入 `groupConfig.members`，并自动以 Agent 名称初始化 `memberTags[agentId]`
3. 取消勾选时从 `members` 和 `memberTags` 中移除
4. `memberTags` 的值将作为该 Agent 在群组中的**触发标签**（如 `@Planner`），供 `naturerandom` 模式匹配

### 3.3 群组专属配置

| 字段 | 类型 | 说明 |
|------|------|------|
| `mode` | `string` | 发言逻辑：`sequential`（顺序轮流）、`naturerandom`（基于标签智能选择）、`invite_only`（用户手动邀请） |
| `tagMatchMode` | `string` | Tag 匹配严格度：`strict`（原始严格匹配）、`natural`（区分 Tag 来源，避免自引用循环触发） |
| `useUnifiedModel` | `boolean` | 是否强制所有成员使用同一模型 |
| `unifiedModel` | `string` | 统一模型 ID，仅在 `useUnifiedModel = true` 时生效 |
| `groupPrompt` | `string` | 群组全局背景提示词 |
| `invitePrompt` | `string` | 邀请某位助手发言时的提示语模板，支持 `{{VCPChatAgentName}}` 占位符 |

---

## 4. 头像系统

### 4.1 头像展示与编辑触发

头像展示由 `VcpAvatar.vue` 统一封装：

```vue
<!-- AgentSettingsView.vue -->
<VcpAvatar
  owner-type="agent"
  :owner-id="props.id || ''"
  :version="avatarVersion"
  :fallback-name="agentConfig.name"
  size="w-24 h-24"
  rounded="rounded-full"
  :dominant-color="agentConfig.avatarCalculatedColor"
/>
```

编辑触发：头像区域包裹在 `<div @click="triggerFileInput">` 中，点击后触发隐藏的 `<input type="file" accept="image/*">`，选择图片后进入裁剪流程。

### 4.2 头像裁剪（vue-cropper）

裁剪由 `AvatarCropper.vue` 封装 `vue-cropper` 实现：

```ts
// AvatarCropper.vue 核心配置
const options = reactive({
  img: props.img,           // FileReader 读取的 DataURL
  outputType: 'png',
  fixedBox: true,           // 锁定裁剪框比例
  autoCropWidth: 360,
  autoCropHeight: 360,
  canMoveBox: false,
  centerBox: true,
  enlarge: 1,               // 禁止根据 DPR 放大输出
  mode: 'contain'
});
```

UI 层级：`AvatarCropper` 通过 `<Teleport to="#vcp-feature-overlays">` 渲染到全局遮罩容器，使用 `z-viewer`（70）层级，独立于设置面板的 DOM 树之外，避免 `Transition` / `v-if` 干扰。

用户可执行的操作：
- **移动图片**：调整裁剪区域（touch-action: none 禁用默认滚动）
- **放大 / 缩小**：`changeScale(+1)` / `changeScale(-1)`
- **旋转**：`rotateLeft()`
- **确认**：调用 `getCropBlob((blob) => emit('confirm', blob))`

样式覆盖：通过 `:deep(.cropper-view-box)` 将裁剪预览设为圆形，与最终展示效果一致。

### 4.3 头像上传流程

```
用户点击头像
    │
    ▼
触发 file input ──→ 选择图片文件
    │
    ▼
FileReader.readAsDataURL ──→ 得到 Base64 DataURL
    │
    ▼
isCropping = true ──→ AvatarCropper 显示
    │
    ▼
用户调整裁剪区域 → 点击"完成"
    │
    ▼
getCropBlob() ──→ Blob (image/png, ~360x360)
    │
    ▼
Blob.arrayBuffer() ──→ Uint8Array ──→ number[]
    │
    ▼
assistantStore.saveAvatar("agent"|"group", id, blob.type, bytes)
    │
    ▼
invoke("save_avatar_data", { ownerType, ownerId, mimeType, imageData })
    │
    ▼
Rust 端：SHA-256 哈希 → SQLite `avatars` 表 upsert
    │
    ▼
avatarVersion = Date.now()  ──→ VcpAvatar 强制刷新
```

> 源码位置：`src/features/agent/AgentSettingsView.vue:87-107`

### 4.4 Dominant Color 提取与展示

Dominant Color（主色调）有两条计算路径：

**路径 A：后端预计算（优先）**
- Rust 侧 `avatar_service.rs` 在保存头像时通过颜色量化算法提取主色调，存入 `avatars` 表
- 前端调用 `get_avatar` 时，若 `dominant_color` 字段非空，直接同步缓存到 `avatarStore.dominantColors`

**路径 B：前端 Canvas 兜底（异步）**
- 若后端返回的 `dominant_color` 为 `null`（历史头像或旧数据），`avatarStore.getAvatarUrl()` 会触发前端 Canvas 提取：

```ts
// src/core/stores/avatar.ts
const extractDominantColorFromBlob = (blobUrl: string): Promise<string> => {
  // 1. 绘制到 16x16 Canvas
  // 2. 遍历像素，排除透明/黑白/低饱和度像素
  // 3. 512-bin 量化聚类，取最大桶的平均色
  // 4. 回写后端：invoke("store_dominant_color", { ownerType, ownerId, color })
};
```

展示效果：`VcpAvatar.vue` 根据 `dominantColor` prop 动态生成 `borderColor`（80% 透明度混合）和 `boxShadow`（弱发光），使头像边缘与聊天界面的色调自然融合。

---

## 5. 配置保存策略

### 5.1 全量保存（save_agent_config / save_group_config）

虽然 Rust 侧提供了 `update_agent_config`（JSON Patch 风格合并），但**前端设置面板统一使用全量保存**，以简化逻辑：

```ts
// assistant.ts
const saveAgent = async (agent: AgentConfig) => {
  await invoke("save_agent_config", { agent });
  await fetchAgents();  // 刷新列表，确保侧边栏同步
};
```

全量保存的优劣：
- **优点**：前端无需维护字段级 diff 逻辑；Rust 侧通过事务化写入保证原子性
- **缺点**：每次保存传输完整配置对象；但在移动端 WiFi/USB 调试环境下，对象体积（<1KB）可忽略

### 5.2 防抖与自动保存

两个视图均实现了**深度 Watch + 防抖**的自动保存机制：

```ts
// AgentSettingsView.vue (防抖 800ms)
watch(agentConfig, () => {
  if (!originalConfig.value) return;
  if (JSON.stringify(agentConfig.value) === JSON.stringify(originalConfig.value)) return;
  if (saveTimeout) clearTimeout(saveTimeout);
  saveTimeout = setTimeout(() => autoSave(), 800);
}, { deep: true });

// GroupSettingsView.vue (防抖 1000ms)
watch(groupConfig, () => {
  if (!originalConfig.value) return;
  if (JSON.stringify(groupConfig.value) === JSON.stringify(originalConfig.value)) return;
  if (saveTimeout) clearTimeout(saveTimeout);
  saveTimeout = setTimeout(() => autoSave(), 1000);
}, { deep: true });
```

**为什么 GroupSettingsView 使用更长的 1000ms？**
- 群组配置包含成员列表的频繁勾选操作（checkbox toggle），稍长的防抖可降低连续勾选时的后端调用频率。

### 5.3 保存状态反馈

Header 右上角提供实时保存状态指示：

```vue
<div :class="{ 'opacity-100': isSaving || saveSuccess, 'opacity-0': !isSaving && !saveSuccess }">
  <span v-if="isSaving" class="text-blue-400 animate-pulse">保存中...</span>
  <span v-else-if="saveSuccess" class="text-green-500">已自动保存 ✅</span>
</div>
```

- `saveSuccess` 持续 2 秒后自动淡出（通过 `saveSuccessTimer` 控制）
- 无保存操作时完全隐藏，避免界面常驻噪音

### 5.4 保存失败的回滚策略

当前实现**不自动回滚**：若 `autoSave()` 抛出异常，仅通过 `console.error` 输出并触发 `assistant.ts` 中的 Toast 错误通知。用户输入保留在本地 `agentConfig` / `groupConfig` 中，下次 Watch 触发时会再次尝试保存。

> 注意：由于 `originalConfig` 仅在保存**成功**后才更新，保存失败时下一次 Watch 仍会检测到差异并重新触发保存，形成隐式的重试机制。

---

## 6. 话题管理（设置面板内）

### 6.1 话题列表展示

**话题列表不在设置面板内展示**，而是由 `TopicList.vue` 在聊天界面侧边栏独立管理。但 Rust 侧的 `read_agent_config` / `read_group_config` 会随配置一并返回 `topics` 数组，因此前端 Store 中可直接访问：

```ts
// assistant.ts 中的 AgentConfig / GroupConfig 接口
export interface AgentConfig {
  // ... 其他字段
  topics?: Topic[];
}
```

### 6.2 新建/删除/重命名话题

话题操作由 `topicListManager.ts`（`useTopicStore`）统一封装：

| 操作 | Store Action | Rust Command | 说明 |
|------|-------------|--------------|------|
| 新建 | `createTopic(ownerId, ownerType, name)` | `create_topic` | 新话题默认 `locked = true` |
| 删除 | `deleteTopic(ownerId, ownerType, topicId)` | `delete_topic` | 删除当前话题后自动切换到最新话题 |
| 重命名 | `updateTopicTitle(ownerId, ownerType, topicId, title)` | `update_topic_title` | 即时更新本地列表 |

创建入口通常位于聊天界面的标题栏下拉菜单中，不在设置面板内。

### 6.3 话题锁定与未读状态

| 状态 | 管理方 | 说明 |
|------|--------|------|
| `locked` | Rust + Store | 锁定的话题不再自动追加新消息，适合作为"归档" |
| `unread` | Rust + Store | 话题级别的未读标记，与 `unreadCount` 配合显示红点 |
| `msgCount` | Store（乐观更新） | 前端实时递增/递减，无需等待后端确认 |

```ts
// topicListManager.ts
const toggleTopicLock = async (ownerId, ownerType, topicId) => {
  const target = !topics.value[index].locked;
  await invoke("toggle_topic_lock", { ownerId, ownerType, topicId, locked: target });
  // 乐观更新
  topics.value[index] = { ...topics.value[index], locked: target };
};
```

### 6.4 默认话题

每个 Agent / Group 在创建时由 Rust 后端自动初始化一个**默认话题**（通常为 "默认话题" 或 "General"），该话题**锁定不可删除**，确保用户始终至少有一个可用对话容器。默认话题的 ID 由后端生成，并在 `create_agent` / `create_group` 的返回值中随 `topics` 数组一并返回。
```

---

## 7. 数据流时序

### 7.1 打开 Agent 设置的时序

```
用户点击 Agent 列表项（长按/设置按钮）
    │
    ▼
overlayStore.openAgentSettings(agentId)
    │
    ▼
pageStack.push({ type: 'agentSettings', id: agentId })
    │
    ▼
AgentSettingsView.props.isOpen = true
    │
    ▼
watch(isOpen) ──→ loadConfig()
    │
    ▼
invoke("read_agent_config", { agentId, allowDefault: true })
    │
    ▼
Rust: 查缓存 → 查 SQLite → 返回 AgentConfig (含 topics)
    │
    ▼
agentConfig.value = config
originalConfig.value = JSON.parse(JSON.stringify(config))
    │
    ▼
VcpAvatar 自动加载头像（cache hit 时同步显示）
```

### 7.2 修改配置后的保存时序

```
用户修改 input (如 name)
    │
    ▼
v-model ──→ agentConfig.name 变更
    │
    ▼
deep watch 触发
    │
    ▼
JSON.stringify 比对 originalConfig → 发现差异
    │
    ▼
clearTimeout(saveTimeout)
setTimeout(autoSave, 800)
    │
    ▼
(800ms 内无新输入)
    │
    ▼
autoSave() ──→ isSaving = true
    │
    ▼
assistantStore.saveAgent(agentConfig.value)
    │
    ▼
invoke("save_agent_config", { agent })
    │
    ▼
Rust: 按 Agent ID 取锁 → BEGIN TRANSACTION → UPSERT → COMMIT
    │
    ▼
返回成功
    │
    ▼
saveSuccess = true; originalConfig = 最新快照
fetchAgents() ──→ 刷新侧边栏列表
Toast 通知: "Agent 配置保存成功"
```

### 7.3 头像上传的时序

```
用户点击头像区域
    │
    ▼
fileInput.click() ──→ 选择图片
    │
    ▼
FileReader.onload ──→ cropImg = dataURL; isCropping = true
    │
    ▼
AvatarCropper 渲染（Teleport 到全局遮罩）
    │
    ▼
用户调整 → 点击"完成"
    │
    ▼
getCropBlob((blob) => onCropConfirm(blob))
    │
    ▼
blob.arrayBuffer() → Uint8Array → Array.from(bytes)
    │
    ▼
assistantStore.saveAvatar("agent", id, blob.type, bytes)
    │
    ▼
invoke("save_avatar_data", { ownerType, ownerId, mimeType, imageData })
    │
    ▼
Rust: 计算 SHA-256 → upsert avatars 表 → 返回 hash
    │
    ▼
avatarVersion = Date.now()
    │
    ▼
VcpAvatar.watchEffect: cache miss → getAvatarUrl(version=new)
    │
    ▼
invoke("get_avatar", { ownerType, ownerId })
    │
    ▼
Rust: 读取 avatars 表 → 返回 { mime_type, image_data, dominant_color }
    │
    ▼
前端: Blob URL → 显示新头像 + dominantColor 边框
```

---

## 8. 与 Rust 后端的 IPC 交互

### 8.1 Tauri Commands 调用表

| Command | 调用方 | 参数 | 返回值 | 说明 |
|---------|--------|------|--------|------|
| `read_agent_config` | `AgentSettingsView.loadConfig` | `{ agentId, allowDefault }` | `AgentConfig` | 读取单个 Agent 配置，含话题列表 |
| `save_agent_config` | `assistant.saveAgent` | `{ agent }` | `boolean` | 全量写入 Agent 配置（事务化） |
| `update_agent_config` | *(前端未直接调用)* | `{ agentId, updates }` | `AgentConfig` | JSON Patch 风格增量更新，保留供外部使用 |
| `read_group_config` | `GroupSettingsView.fetchGroupConfig` | `{ groupId }` | `GroupConfig` | 读取群组配置 |
| `save_group_config` | `assistant.saveGroup` | `{ group }` | `boolean` | 全量写入群组配置 |
| `get_agents` | `GroupSettingsView.fetchAgents` | 无 | `AgentConfig[]` | 获取全部 Agent 摘要（用于成员选择器） |
| `save_avatar_data` | `assistant.saveAvatar` | `{ ownerType, ownerId, mimeType, imageData }` | `string` | 返回 SHA-256 哈希 |
| `get_avatar` | `avatarStore.getAvatarUrl` | `{ ownerType, ownerId }` | `AvatarResult` | 含 `mime_type`, `image_data`, `dominant_color` |
| `store_dominant_color` | `avatarStore`（兜底计算后） | `{ ownerType, ownerId, color }` | `void` | 回写前端计算的主色调 |
| `create_agent` | `assistant.createAgent` | `{ name }` | `AgentConfig` | 创建新 Agent |
| `delete_agent` | `assistant.deleteAgent` | `{ agentId }` | `void` | 软删除 Agent |
| `create_group` | `assistant.createGroup` | `{ name }` | `GroupConfig` | 创建新群组 |
| `delete_group` | `assistant.deleteGroup` | `{ groupId }` | `void` | 软删除群组 |
| `create_topic` | `topicStore.createTopic` | `{ ownerId, ownerType, name }` | `Topic` | 新建话题 |
| `delete_topic` | `topicStore.deleteTopic` | `{ ownerId, ownerType, topicId }` | `void` | 删除话题 |
| `update_topic_title` | `topicStore.updateTopicTitle` | `{ ownerId, ownerType, topicId, title }` | `void` | 重命名话题 |
| `toggle_topic_lock` | `topicStore.toggleTopicLock` | `{ ownerId, ownerType, topicId, locked }` | `void` | 切换锁定状态 |
| `set_topic_unread` | `topicStore.setTopicUnread` | `{ ownerId, ownerType, topicId, unread }` | `void` | 设置未读标记 |

### 8.2 事件监听表

设置面板本身**不直接监听** Tauri 事件。所有状态更新均通过：
1. **同步调用**：`invoke()` 的返回值直接更新本地状态
2. **Store 刷新**：`saveAgent()` / `saveGroup()` 成功后调用 `fetchAgents()` / `fetchGroups()`，更新全局列表
3. **Watch 驱动**：`avatarVersion` 变更驱动 `VcpAvatar` 重新加载

---

## 9. 设计决策与注意事项

### 9.1 为什么设置面板不使用路由？

VCP Mobile 采用**单页 Hash 路由**（仅 `/chat` 一条主路由），所有"子页面"通过 SlidePage 堆叠实现。这确保了：
- 聊天主视图始终存活于 DOM，消息流不被打断
- 返回手势（右滑）可直接映射到 `overlayStore.popPage()`
- 设置面板与聊天视图共享同一个 Pinia 状态树，无需跨路由同步

### 9.2 为什么 `update_agent_config` 在前端未被使用？

Rust 侧虽提供了 `update_agent_config`（JSON 合并写入），但前端统一走 `save_agent_config` 全量保存，原因如下：
- 前端表单采用 `v-model` 全量绑定，天然维护完整对象
- 800ms 防抖已足够降低保存频率，无需字段级 diff 优化
- 减少前端逻辑分支：Agent 与 Group 使用同一套保存范式

### 9.3 头像裁剪输出 360x360 的考量

`AvatarCropper.vue` 固定输出 360x360 像素：
- 该尺寸在移动端 2x/3x DPR 屏幕上作为 120px 展示尺寸足够清晰
- `enlarge: 1` 禁止 `vue-cropper` 根据 DPR 自动放大，确保输出体积可控
- 输出格式固定为 PNG，避免 JPEG 压缩导致的透明背景黑边问题

### 9.4 离开页面时是否需要确认未保存变更？

**当前实现无确认弹窗**。原因：
- 自动保存防抖仅 800ms/1000ms，用户正常操作（点击返回）前几乎必然已完成保存
- `originalConfig` 快照机制确保保存成功后不再重复触发，降低"正在保存中"的概率
- 若极端情况下在防抖窗口内关闭页面，未保存内容保留在本地 `ref` 中，下次打开同一 Agent 时会重新加载最新数据库状态（不会丢失上次已保存的内容，仅可能丢失最后 1 秒内的输入）

### 9.5 模型选择器的数据来源

`ModelSelector.vue` 的数据来自 `modelStore`（`src/core/stores/modelStore.ts`），该 Store 通过 `fetchModels()` 从 Rust 侧拉取可用模型列表。设置面板中的模型输入框允许**自由编辑**（非只读），用户可直接粘贴模型 ID，选择器仅作为便捷浏览工具。这一设计兼容了私有部署和自定义模型路径的场景。

### 9.6 成员 Tag 的默认值陷阱

`GroupSettingsView` 在勾选 Agent 时，若 `memberTags[agentId]` 不存在，会以 `agent?.name || agentId` 自动初始化：

```ts
if (!groupConfig.value.memberTags[agentId]) {
  groupConfig.value.memberTags[agentId] = agent?.name || agentId;
}
```

这可能导致：
- Agent 改名后，已存在的 Tag 不会自动跟随更新
- 用户需手动进入群组设置修改 Tag

### 9.7 `card-modern` 样式类的一致性

两个设置面板均使用 `.card-modern` 定义卡片容器，但实现方式略有差异：`AgentSettingsView` 使用 `<style scoped>` 内联定义，`GroupSettingsView` 同样使用 scoped 定义但圆角为 `rounded-2xl`（Agent 为 `rounded-xl`）。这种微观差异是渐进式 UI 迭代的结果，不影响功能，但新开发者应注意不要假设两者视觉完全一致。

### 9.8 `mobileSystemPrompt` 的同步边界

`mobileSystemPrompt` 字段在 Rust 侧 `agent_types.rs` 中标记为 `#[serde(default)]`，且数据库表中有独立字段。该字段**不参与桌面端同步**，实现了 Agent 的移动端差异化行为。用户可在提示词区域看到明确说明：

> "此处编辑的提示词仅在本机生效，不会同步到桌面端。"

---

## 10. 术语速查表

| 术语 | 含义 | 对应文件/字段 |
|------|------|--------------|
| **SlidePage** | 从右侧滑入的全屏覆盖层组件 | `src/components/ui/SlidePage.vue` |
| **Page Stack** | OverlayStore 维护的虚拟页面栈，支持多层叠加 | `src/core/stores/overlay.ts` |
| **originalConfig** | 配置加载时的深拷贝快照，用于 Watch 比对 | `AgentSettingsView.vue:120` |
| **avatarVersion** | 时间戳，用于强制 VcpAvatar 跳过缓存重新加载 | `AgentSettingsView.vue:69` |
| **Dominant Color** | 头像主色调，用于边框发光与 Fallback 背景 | `avatar.ts` / `avatar_service.rs` |
| **ModelSelector** | BottomSheet 风格的模型选择抽屉 | `src/components/ModelSelector.vue` |
| **SettingsSection** | 群组设置中的分类标题原子组件 | `src/components/settings/SettingsSection.vue` |
| **memberTags** | 群组成员的触发标签映射 | `groupConfig.memberTags` |
| **tagMatchMode** | Tag 匹配严格度：`strict` / `natural` | `GroupConfig.tag_match_mode` |
| **useUnifiedModel** | 是否强制群组内所有 Agent 使用同一模型 | `GroupConfig.use_unified_model` |
| **mobileSystemPrompt** | 移动端专用提示词，不参与桌面端同步 | `AgentConfig.mobile_system_prompt` |
| **JSON Patch 风格** | `update_agent_config` 的更新方式：读取 → 合并 → 写入 | `agent_service.rs:193` |

---
*最后更新：2026-06-04 | VCP Mobile v1.0.3*
