---
id: VUE-CORE-002
title: 状态管理总览与Store全景图
description: VCP Mobile 前端 18 个 Pinia Store 的职责分类、依赖关系与状态设计全景
version: 0.9.14
date: 2026-05-27
---

# 02. 状态管理总览与Store全景图

## 1. 概述

### 1.1 领域定位

`src/core/stores/` 是 VCP Mobile Vue 3 前端层的**状态中枢目录**，负责承载全部 18 个 Pinia Store。这些 Store 按领域边界划分为 5 大类，覆盖会话消息、UI 覆盖层、Agent/群组管理、系统设置与同步五大业务域。

与 Rust 后端不同，前端 Store 不负责数据持久化（SQLite 由后端托管），而是承担以下职责：

- **运行时状态聚合**：当前选中的 Agent/Group、活跃话题、流式消息池、抽屉开关等
- **IPC 调用编排**：封装 Tauri `invoke()` / `Channel` / `listen()` 调用，提供类型安全的 Action 接口
- **跨组件状态共享**：通过 Pinia 的响应式系统，让分散在 `features/` 与 `components/` 中的 Vue 组件共享同一状态源
- **UI 乐观更新**：在异步回写后端前，先更新本地状态以提供即时视觉反馈

该目录**不**涉及：
- 原始 SQL 查询（由 Rust 后端 `db_manager.rs` 负责）
- HTTP 网络请求底层（由 Rust 后端 `vcp_client.rs` 负责）
- 文件系统读写（由 Rust 后端 `file_manager.rs` 负责）

### 1.2 模块构成表（18 个 Store）

| 分类 | 文件名 | Store 名 | 职责简述 | 持久化 |
|------|--------|----------|----------|--------|
| 会话与消息 | `chatSessionStore.ts` | `useChatSessionStore` | 当前选中的 Agent/Group 与话题 ID 管理 | ✅ |
| 会话与消息 | `chatHistoryStore.ts` | `useChatHistoryStore` | 聊天历史加载、消息发送/删除/重生成 | ❌ |
| 会话与消息 | `chatStreamStore.ts` | `useChatStreamStore` | SSE 流式事件处理与活跃消息池管理 | ❌ |
| 会话与消息 | `attachmentStore.ts` | `useAttachmentStore` | 附件上传、暂存、文档 JIT 预处理 | ❌ |
| UI 与覆盖层 | `layout.ts` | `useLayoutStore` | 左右抽屉（侧边栏）开关状态 | ❌ |
| UI 与覆盖层 | `overlay.ts` | `useOverlayStore` | 页面栈（SlidePage）、Prompt、ContextMenu、Editor | ❌ |
| UI 与覆盖层 | `notification.ts` | `useNotificationStore` | Toast 气泡、通知中心历史、系统状态栏 | ❌ |
| UI 与覆盖层 | `theme.ts` | `useThemeStore` | 主题模式（light/dark/system）、主题加载与变量注入 | ⚠️ localStorage |
| Agent 与群组 | `assistant.ts` | `useAssistantStore` | Agent/Group CRUD、未读计数聚合 | ❌ |
| Agent 与群组 | `avatar.ts` | `useAvatarStore` | 头像二进制缓存、Blob URL 管理、主色调同步提取 | ❌ |
| Agent 与群组 | `modelStore.ts` | `useModelStore` | VCP 模型列表、热门模型、收藏管理 | ❌ |
| Agent 与群组 | `topicListManager.ts` | `useTopicStore` | 话题列表流式加载、CRUD、未读/消息计数乐观更新 | ❌ |
| 系统与设置 | `appLifecycle.ts` | `useAppLifecycleStore` | 应用启动状态机（BOOTING → READY）、预加载编排 | ❌ |
| 系统与设置 | `settings.ts` | `useSettingsStore` | 全局配置读写（后端 SQLite 持久化） | ❌ |
| 系统与设置 | `rebuildSession.ts` | `useRebuildSessionStore` | 预渲染重建任务会话（状态机 + 进度监听） | ❌ |
| 同步与分布式 | `syncSession.ts` | `useSyncSessionStore` | 手动同步会话（WebSocket 连接、日志、进度） | ❌ |
| 系统与设置 | `tarvenStore.ts` | `useTarvenStore` | Tarven 注入规则列表、启用状态、选择器开关 | ❌ |

> **持久化说明**：
> - ✅ = `pinia-plugin-persistedstate` 自动持久化到 `localStorage`
> - ⚠️ = 自行通过 `localStorage` 管理（themeStore 因 Vite HMR 需求独立处理）
> - ❌ = 纯运行时状态，页面刷新即重置，依赖后端 SQLite 作为唯一持久源

### 1.3 在整体架构中的位置

```
┌─────────────────────────────────────────────────────────────┐
│                      Vue 3 前端层                            │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              src/core/stores/ (本域)                   │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐   │  │
│  │  │ 会话与消息   │  │ UI 与覆盖层  │  │ Agent/群组  │   │  │
│  │  │  chat*      │  │ layout/     │  │ assistant/  │   │  │
│  │  │  attachment │  │ overlay/    │  │ avatar/     │   │  │
│  │  │             │  │ notification│  │ model/      │   │  │
│  │  │             │  │ theme       │  │ topic       │   │  │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘   │  │
│  │         │                │                │          │  │
│  │  ┌─────────────────────────────────────────────────┐  │  │
│  │  │          系统与设置 / 同步与分布式                │  │  │
│  │  │     appLifecycle / settings / syncSession        │  │  │
│  │  └─────────────────────────────────────────────────┘  │  │
│  └──────────────────────────┬────────────────────────────┘  │
│                             │ Pinia (Composition API)        │
│                             │ ref / computed / reactive      │
└─────────────────────────────┼───────────────────────────────┘
                              │ IPC (Tauri Commands / Events)
┌─────────────────────────────▼───────────────────────────────┐
│                   src-tauri (Rust 核心层)                    │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │db_manager   │  │vcp_client   │  │ sync_service        │  │
│  │(SQLite)     │  │(HTTP/SSE)   │  │ (WebSocket + HTTP)  │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 核心设计原则

| 原则 | 说明 |
|------|------|
| **Composition API 风格** | 全部 Store 使用 `defineStore('id', () => { ... })` 定义，state/getters/actions 通过 `ref`/`computed`/`function` 自然表达，而非 Options API 的 `state`/`getters`/`actions` 三段式 |
| **数据在后端，状态在前端** | 前端 Store 不持久化业务数据（消息、话题、Agent 配置等），仅持久化极小量的 UI 恢复状态（如 `lastActiveTopicMap`）。后端 SQLite 是唯一持久化源 |
| **流式感知** | 支持 Channel 的 Store（`chatHistoryStore`、`topicListManager`）在加载大数据量时使用流式分块接收，避免 IPC 传输阻塞 |
| **会话隔离** | `chatStreamStore` 使用 `"itemId:topicId"` 作为流状态键，确保多标签/多话题的并发流互不干扰 |
| **乐观更新** | `topicListManager` 在消息发送/删除时立即调整 `msgCount`/`unreadCount`，不等待后端确认 |
| **错误降级** | 所有 `invoke()` 调用均包裹 `try/catch`，失败时打印日志并可能触发 Toast 通知，但极少阻断用户操作 |

---

## 2. Store 分类体系

### 2.1 会话与消息类

本类 4 个 Store 构成聊天核心链路，是用户交互频率最高的状态集合。

#### chatSessionStore —— 会话导航锚点

> 文件位置：`src/core/stores/chatSessionStore.ts`

`useChatSessionStore` 是整个聊天 UI 的**导航锚点**，它决定了"当前在看谁、当前在看哪个话题"。

**核心状态**（3 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `currentSelectedItem` | `any \| null` | 当前选中的 Agent 或 Group 对象（含 `type: 'agent' \| 'group'`） |
| `currentTopicId` | `string \| null` | 当前活跃话题 ID |
| `lastActiveTopicMap` | `Record<string, string>` | 每个 itemId 最后一次选中的话题 ID，用于会话恢复 |

**持久化策略**：通过 `pinia-plugin-persistedstate` 持久化全部 3 个字段。这意味着应用重启后，用户会自动回到上次离开的 Agent/Group 与话题。

**关键设计**：`selectItem` 方法实现了"智能话题恢复"——优先使用 `lastActiveTopicMap` 中缓存的话题 ID，若无缓存则调用后端 `get_topics` 获取最新话题。回调函数 `loadHistoryCallback` 的设计将 HistoryStore 的加载动作以函数参数形式注入，避免了 SessionStore 对 HistoryStore 的直接依赖（反向依赖）。

#### chatHistoryStore —— 历史记录与消息操作

> 文件位置：`src/core/stores/chatHistoryStore.ts`

`useChatHistoryStore` 是聊天内容的核心持有者，管理当前话题下的消息列表及其生命周期。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `currentChatHistory` | `ChatMessage[]` | 当前话题的全部已加载消息 |
| `historyOffset` | `number` | 分页加载偏移量（下次请求起点） |
| `hasMoreHistory` | `boolean` | 是否还有更多旧消息可加载 |
| `editMessageContent` | `string` | 重新生成时用于回填输入框的内容 |
| `editingOriginalMessageId` | `string \| null` | 标记当前是否处于"编辑重发"模式 |

**依赖注入**：该 Store 依赖了 6 个其他 Store，是**依赖最多的 Store**——`sessionStore`、`streamStore`、`attachmentStore`、`assistantStore`、`settingsStore`、`topicStore`。这种高耦合是必要的：发送消息需要知道当前会话（session）、需要调用流式处理（stream）、需要读取附件（attachment）、需要用户名称（settings）、需要更新话题计数（topic）。

**Object Hydration 机制**：在 `loadHistory` 的 Channel `onmessage` 回调中，存在一段关键代码：

```typescript
const activeMsg = streamStore.activeStreamMessages.get(chunk.message.id);
const msgToUse = activeMsg || chunk.message;
```

此机制称为 **Object Hydration**：当从数据库流式加载历史时，若某消息正在活跃生成中（存在于 `streamStore.activeStreamMessages`），则用"活的"响应式对象替换数据库拉回的"死的"骨架对象。这确保了流式动画不会因历史加载而断裂。

#### chatStreamStore —— 流式消息池与事件调度

> 文件位置：`src/core/stores/chatStreamStore.ts`

`useChatStreamStore` 是 Backend-Driven Streaming 架构在前端的**核心落地层**。它不再预创建 thinking 占位消息，而是接收后端 SSE 事件并驱动 UI 更新。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `streamingMessageId` | `string \| null` | 当前正在流式输出的消息 ID |
| `sessionActiveStreams` | `Record<string, string[]>` | 按 `"itemId:topicId"` 索引的活跃流消息 ID 列表 |
| `activeStreamMessages` | `Map<string, ChatMessage>` | 全局活跃流消息池（响应式 Map），所有正在生成的消息对象存放于此 |
| `activeStreamingIds` | `ComputedRef<Set<string>>` | 当前选中会话下的活跃流 ID 集合 |
| `isGroupGenerating` | `ComputedRef<boolean>` | 当前群组是否有任何成员正在生成 |

**全局流消息池**：`activeStreamMessages` 使用 `reactive(new Map())` 构造，确保 Map 内部对象的变化能被 Vue 追踪。池上限为 `MAX_STREAM_MESSAGES = 100`，超出时按插入顺序清理最旧的非活跃消息，防止 OOM。

**computeShell**：在前端本地计算 `MessageShell`（替代 Rust 的 `precompute_shell`），需要读取 `avatarStore.dominantColors` 和 `assistantStore.agents`，实现零 IPC 开销的 UI 着色。

#### attachmentStore —— 附件上传与预处理

> 文件位置：`src/core/stores/attachmentStore.ts`

`useAttachmentStore` 管理消息发送前的附件暂存、上传与 JIT 文档预处理。

**核心状态**（1 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `stagedAttachments` | `Attachment[]` | 准备随下一条消息发送的暂存附件列表 |

**双轨上传策略**：
- **Android 端**：通过 `plugin:vcp-mobile|pick_file` 调用原生 Kotlin File Picker，配合 `vcp-mobile-file-*` 自定义事件完成沙盒拷贝与哈希计算
- **非 Android 端**：标准 HTML `<input type="file">`，小文件（< 2MB）走 IPC `store_file`，大文件走高速 TCP 链路 `prepare_vcp_upload` + XHR

**JIT 文档预处理**：`preProcessDocuments` 调用 `useDocumentProcessor` 组合式函数，对 `txt`/`md`/`csv`/`json`/`docx`/`pdf` 附件提取文本内容，在发送前完成。

#### 会话与消息类 Store 协作流程

4 个 Store 并非独立工作，而是在一次完整的"发送消息 → 接收流式回复"流程中紧密协作：

```
用户点击发送
    │
    ▼
┌─────────────────────────────────────┐
│ chatHistoryStore.sendMessage()      │
│ 1. 读取 stagedAttachments           │
│ 2. 构造 userMsg 对象                │
│ 3. push 到 currentChatHistory       │
│ 4. 调用 triggerGeneration()         │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│ chatHistoryStore.triggerGeneration()│
│ 1. invoke('append_single_message')  │
│ 2. 创建 streamChannel               │
│ 3. 将 streamChannel.onmessage 委托  │
│    给 chatStreamStore.processStreamEvent
│ 4. invoke('handle_agent_chat_message')│
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│ chatStreamStore.processStreamEvent()│
│ 1. 收到 thinking → 创建骨架消息     │
│    写入 activeStreamMessages        │
│ 2. 收到 data → 追加 content         │
│ 3. 收到 end → 调用 process_message_content
│    生成 blocks，清理流状态           │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│ chatHistoryStore.onMessageCreated   │
│ (回调) 将 msg push 到               │
│ currentChatHistory (若 tid 匹配)    │
└─────────────────────────────────────┘
```

**流程要点**：
- `chatHistoryStore` 负责消息的生命周期管理（创建、追加、删除），但不直接解析流式事件
- `chatStreamStore` 负责解析 SSE 事件并维护响应式消息对象，但不持有历史列表
- 两者通过 `processStreamEvent` 的回调参数 `onMessageCreated` 衔接
- `chatSessionStore` 提供整个流程的上下文（`currentSelectedItem` + `currentTopicId`）
- `attachmentStore` 在流程起点提供附件数据，在 `sendMessage()` 后被清空

---

### 2.2 UI 与覆盖层类

#### layout —— 抽屉布局管理

> 文件位置：`src/core/stores/layout.ts`

极简 Store，仅管理左右两个抽屉的开关状态。关键设计：两个抽屉**互斥**——打开左侧时自动关闭右侧，反之亦然。通过 `useModalHistory` 组合式函数注册/注销模态历史，支持 Android 物理返回键关闭抽屉。

**核心状态**（2 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `leftDrawerOpen` | `boolean` | Agent 侧边栏开关 |
| `rightDrawerOpen` | `boolean` | 话题列表侧边栏开关 |

#### overlay —— 页面栈与模态框管理

> 文件位置：`src/core/stores/overlay.ts`

`useOverlayStore` 是 VCP Mobile "无路由导航"架构的**核心基础设施**。由于应用本质是单页应用（仅 `/chat` 一条路由），所有"页面切换"通过 `pageStack` 实现的虚拟页面栈完成。

**核心状态**（4 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `pageStack` | `PageStackItem[]` | 虚拟页面栈，每项含 `type`/`id`/`modalId` |
| `promptConfig` | `PromptConfig \| null` | 当前 Prompt 弹窗配置 |
| `contextMenuConfig` | `ContextMenuConfig \| null` | 当前上下文菜单配置（`shallowRef`，避免深层响应式开销） |
| `editorConfig` | `EditorConfig \| null` | 当前全屏编辑器配置 |

**页面栈与 Z-Index**：`getPageZIndex(type)` 根据页面在栈中的索引动态计算 `z-index`，公式为 `LAYER_PAGE_BASE + min(index, LAYER_PAGE_MAX_OFFSET)`。这确保了后打开的页面始终覆盖在先打开的页面之上，与 `docs/UI_LAYER_ARCHITECTURE.md` 中的语义化层级规范一致。

**特殊页面托管**：`syncSession` 与 `rebuildSession` 两个页面由于内部有独立的状态机，overlayStore 在 `openSyncSession`/`openRebuildSession` 中直接操作对应 Store 的状态（调用 `syncStore.open()` / `rebuildStore.open()`），并将它们的关闭动作与页面栈弹栈绑定。

#### notification —— 通知中心与 Toast

> 文件位置：`src/core/stores/notification.ts`

`useNotificationStore` 是全局通知设施的**唯一真相源**，所有成功/错误/警告信息均通过 `addNotification()` 统一入口流入。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `historyList` | `VcpNotification[]` | 通知中心历史列表（上限 100 条） |
| `activeToasts` | `VcpNotification[]` | 当前悬浮显示的 Toast 气泡 |
| `unreadCount` | `number` | 未读通知计数 |
| `vcpStatus` | `VcpStatus` | VCPLog（WebSocket）连接状态 |
| `vcpCoreStatus` | `VcpStatus` | Rust 核心引擎状态 |

**单例抑制机制**：若传入固定 `id`（如 `vcp_sync_connection_status`），Store 会先在当前活动 Toast 中查找并原地更新，避免同一状态反复弹出多个气泡。若该 ID 在 30 秒内已在历史中出现过，则仅更新时间戳而不弹出新 Toast。

**幽灵清理**：每 30 秒运行一次定时器，清理因页面切换或逻辑异常而残留的过期 Toast（`duration !== 0` 的条目在 `duration + 5000ms` 后强制移除）。

#### theme —— 主题系统

> 文件位置：`src/core/stores/theme.ts`

`useThemeStore` 管理 VCP Mobile 的全局视觉主题，支持 `light`/`dark`/`system` 三种模式与动态 CSS 变量注入。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `mode` | `ThemeMode` | 当前主题模式（`light`/`dark`/`system`） |
| `isDarkResolved` | `boolean` | 实际解析后的暗色状态（`system` 模式下跟随 OS） |
| `currentTheme` | `string` | 当前主题文件名（如 `themes-bear-holiday.css`） |
| `availableThemes` | `ThemeInfo[]` | 扫描到的全部可用主题列表 |
| `currentThemeInfo` | `ThemeInfo \| null` | 当前主题的完整元数据 |

**Vite HMR 感知**：通过 `import.meta.hot` 检测开发环境热更新，主题 TS 文件修改后 100ms 自动重新加载并注入变量，实现样式的实时无刷新生效。

**Legacy 主题映射**：`LEGACY_THEME_MAP` 将旧版中文文件名（如 `themes冰火魔歌.css`）映射到新版英文文件名，保证旧用户升级后主题不丢失。

---

### 2.3 Agent 与群组类

#### assistant —— Agent/Group CRUD

> 文件位置：`src/core/stores/assistant.ts`

`useAssistantStore` 是 Agent 与 Group 两个实体的**统一前端门面**。虽然名称是 "assistant"，但内部同时管理 `agents` 和 `groups` 两个数组，并通过 `combinedItems` 计算属性将两者合并为统一的侧边栏数据源。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `agents` | `AgentConfig[]` | 全部 Agent 配置列表 |
| `groups` | `GroupConfig[]` | 全部 Group 配置列表 |
| `combinedItems` | `ComputedRef` | agents + groups 的合并视图，含 `type` 标记 |
| `loading` | `boolean` | 全局加载状态 |
| `unreadCounts` | `Record<string, number>` | 按 item ID 索引的未读消息计数 |

**批量未读计数**：`refreshUnreadCounts` 调用后端 `get_unread_counts` 一次性获取全部 item 的未读状态，替代 N+1 逐个查询，是移动端性能优化的典型实践。

#### avatar —— 头像缓存与主色调

> 文件位置：`src/core/stores/avatar.ts`

`useAvatarStore` 负责头像二进制数据的内存级缓存管理，以及主色调（Dominant Color）的前端提取与回写。

**核心状态**（3 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `cache` | `Map<string, AvatarCache>` | 头像 Blob URL 缓存，键格式 `"ownerType:ownerId"`，上限 50 条 |
| `dominantColors` | `Map<string, string>` | 主色调同步缓存，供 `computeShell` 等同步场景读取 |
| `pending` | `Map<string, Promise<string>>` | 进行中的头像请求去重映射 |

**版本号防死循环**：缓存条目含 `version` 字段（来自后端 `updated_at`）。`getAvatarUrl` 比较逻辑为 `version === 0 || existing.version >= version`，确保请求版本非强制刷新时，只要缓存存在即可直接返回，切断潜在的死循环。

**前端主色调兜底**：当后端返回的 `dominant_color` 为 `null` 时，前端通过 Canvas 16×16 降采样 + 512-bin 相似色归纳量化自主计算颜色，并异步调用 `store_dominant_color` 回写后端，实现前后端互补。

#### modelStore —— 模型列表

> 文件位置：`src/core/stores/modelStore.ts`

`useModelStore` 管理 VCP 服务器返回的模型列表，支持缓存、热门排序与收藏功能。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `models` | `ModelInfo[]` | 全部可用模型列表 |
| `hotModels` | `string[]` | 热门模型 ID 列表（后端按使用频率统计） |
| `favorites` | `string[]` | 用户收藏的模型 ID 列表 |
| `isLoading` | `boolean` | 刷新中标志（带锁频防护） |
| `lastRefreshed` | `number` | 上次成功刷新时间戳 |

**平滑转圈**：强制刷新时，若实际请求耗时不足 800ms，则人为延迟到 800ms 才清除 `isLoading`，避免用户产生"点击了但没反应"的困惑。

#### topicListManager —— 话题列表

> 文件位置：`src/core/stores/topicListManager.ts`

`useTopicStore` 管理单个 Agent/Group 下的话题列表，是右侧话题侧边栏的数据源。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `topics` | `Topic[]` | 当前 item 的话题列表 |
| `loading` | `boolean` | 加载状态 |
| `searchTerm` | `string` | 话题搜索关键词 |
| `filteredTopics` | `ComputedRef` | 按标题和创建日期过滤后的列表 |
| `currentAgentId` | `string \| null` | 当前正在加载话题的 item ID（用于竞态丢弃） |

**流式加载**：通过 `get_topics_streamed` + `Channel<Topic[]>` 实现话题列表的增量加载，每收到一个 chunk 即 `push` 到数组并强制触发重绘（`topics.value = [...topics.value]`），配合虚拟列表实现平滑的渐进式渲染。

**删除话题后的自动导航**：若删除的是当前选中话题，自动选择列表中的下一个话题；若列表为空，则清空 `sessionStore.currentTopicId`，由 `chatHistoryStore` 的 watch 监听自动清空历史。

---

### 2.4 系统与设置类

#### appLifecycle —— 应用启动状态机

> 文件位置：`src/core/stores/appLifecycle.ts`

`useAppLifecycleStore` 是应用冷启动的**总指挥**，将启动流程建模为严格的状态机。

**核心状态**（6 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `state` | `AppState` | 当前状态：`PERMISSIONS` → `BOOTING` → `CONNECTING` → `PRELOADING` → `READY`/`ERROR` |
| `isBootstrapping` | `boolean` | 是否正在启动中（防止重复触发） |
| `hasBootstrapped` | `boolean` | 是否已成功完成过启动 |
| `currentPhaseLabel` | `string` | 当前预加载阶段的人类可读描述 |
| `errorMsg` | `string \| null` | 启动失败时的错误信息 |
| `statusText` | `ComputedRef<string>` | 根据当前状态自动生成的状态描述文本 |

**启动流程编排**：

```
PERMISSIONS (检查 Android 权限)
    │
    ▼
BOOTING (初始化主题资源)
    │
    ▼
CONNECTING (等待 Rust 核心就绪)
    │
    ▼
PRELOADING (顺序加载 Settings → 并发加载 Agents/Groups)
    │
    ▼
READY (应用可用)
```

**并发预加载**：Settings 为顺序任务（其他任务依赖其完成），Agents 与 Groups 为并发任务组，整体有 20 秒硬超时保护。每个任务组完成后若耗时不足 150ms，则进行毫秒级视觉补白，避免用户感知到"闪一下"。

#### settings —— 全局配置

> 文件位置：`src/core/stores/settings.ts`

`useSettingsStore` 是前端访问 Rust 后端 `settings_manager` 的**统一封装层**。

**核心状态**（3 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `settings` | `AppSettings \| null` | 完整的全局配置对象（含 VCP URL、API Key、主题设置等） |
| `loading` | `boolean` | 读写加载状态 |
| `error` | `string \| null` | 最近一次操作错误 |

**增量更新**：`updateSettings` 调用后端 `update_settings`，支持 JSON Patch 风格的增量合并，避免前端构造完整配置对象。

#### rebuildSession —— 预渲染重建

> 文件位置：`src/core/stores/rebuildSession.ts`

`useRebuildSessionStore` 管理消息预渲染（Block 解析）的批量重建任务，通常在后端数据结构升级后触发。

**核心状态**（5 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `isOpen` | `boolean` | 重建会话面板是否打开 |
| `status` | `'idle' \| 'running' \| 'completed' \| 'error'` | 任务状态机 |
| `progress` | `{ current: number; total: number }` | 进度计数 |
| `needsReload` | `boolean` | 完成后是否需要刷新页面 |
| `canDismiss` | `boolean` | 是否允许关闭面板（运行中禁止关闭） |

---

### 2.5 同步与分布式类

#### syncSession —— 手动同步会话

> 文件位置：`src/core/stores/syncSession.ts`

`useSyncSessionStore` 管理 VCP Mobile 三阶段增量同步协议的手动触发会话。

**核心状态**（6 个）：

| 状态 | 类型 | 说明 |
|------|------|------|
| `isOpen` | `boolean` | 同步面板是否打开 |
| `status` | `'idle' \| 'connecting' \| 'connected' \| 'error' \| 'completed'` | 连接状态机 |
| `activeTab` | `'live' \| 'history'` | 当前面板标签页 |
| `logs` | `{ id, level, message, time }[]` | 实时同步日志（上限 200 条） |
| `progressData` | `{ phase, total, completed, message }` | 同步进度数据 |
| `needsReload` | `boolean` | 同步完成后是否需要刷新数据 |

**事件监听**：注册 4 个 Tauri 事件监听器——`vcp-log`（日志）、`vcp-sync-progress`（进度）、`vcp-sync-status`（状态变更）、`vcp-sync-completed`（完成）。监听器在 `open()` 时注册、`close()` 时清理，防止内存泄漏。

**屏幕常亮**：同步期间调用 `acquireScreenKeep()`，确保 Android 不因息屏而中断长时同步；完成后或出错时释放。

#### 同步与重建会话的托管关系

`syncSessionStore` 与 `rebuildSessionStore` 是两个**自包含状态机**的 Store，它们不引用任何其他业务 Store。但与 `overlayStore` 之间存在特殊的**托管关系**：

```
overlayStore.openSyncSession()
    │
    ├──► syncSessionStore.open() ──► 重置状态机、注册监听器
    │
    └──► pageStack.push('syncSession') ──► 注册返回键回调
                                              │
                                              ▼
                                    用户点击物理返回键
                                              │
                                              ▼
                                    overlayStore.popPageInternal()
                                              │
                                              ▼
                                    syncSessionStore.close() ──► 清理监听器
```

这种设计的优点是：
- `syncSessionStore` 和 `rebuildSessionStore` 可独立测试（无外部 Store 依赖）
- `overlayStore` 统一管理页面生命周期（Z-Index、返回键、弹栈）
- 关闭动作双向绑定：用户既可以通过 UI 按钮关闭，也可以通过物理返回键关闭

---

## 3. Store 间依赖关系

### 3.1 依赖关系图（ASCII）

```
                          ┌─────────────────┐
                          │  notification   │
                          │  (公共通知设施)  │
                          │   零 Store 依赖  │
                          └────────┬────────┘
                                   │ 被 5 个 Store 依赖
           ┌───────────────────────┼───────────────────────┐
           │           │           │           │           │
           ▼           ▼           ▼           ▼           ▼
      ┌────────┐  ┌────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
      │assistant│  │settings│  │appLifecycle│ │modelStore│ │topicListManager│
      │        │  │        │  │          │  │         │  │          │
      └────┬───┘  └───┬────┘  └────┬─────┘  └─────────┘  └────┬─────┘
           │          │            │                          │
           │    ┌─────┘            │                          │
           │    │         ┌────────┘                          │
           │    │         │                                   │
           ▼    ▼         ▼                                   ▼
      ┌─────────────────────────────────────────────────────────────┐
      │                     chatSessionStore                         │
      │  (currentSelectedItem / currentTopicId / lastActiveTopicMap) │
      │                     【pinia-plugin-persistedstate】           │
      └───────────────────────────┬─────────────────────────────────┘
                                  │ 被 3 个 Store 依赖
                    ┌─────────────┼─────────────┐
                    │             │             │
                    ▼             ▼             ▼
           ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
           │chatHistoryStore│ │chatStreamStore│ │topicListManager│
           │             │ │             │ │ (二次依赖)   │
           └──────┬──────┘ └──────┬──────┘ └─────────────┘
                  │               │
        ┌─────────┼─────────┐     │
        │         │         │     │
        ▼         ▼         ▼     ▼
   ┌─────────┐ ┌─────────┐ ┌─────────┐
   │streamStore│ │assistant│ │ avatar  │
   │(Object   │ │(summarize│ │(shell)  │
   │Hydration)│ │ title)  │ │         │
   └─────────┘ └─────────┘ └─────────┘
        │
        │    ┌──────────────────────────────────┐
        │    │  topicStore / attachmentStore    │
        │    │  (msgCount / preProcessDocuments)│
        │    └──────────────────────────────────┘
        │
        └──►  settings (vcpUrl / vcpApiKey)


┌─────────────────────────────────────────────────────────────────┐
│                      独立子系统                                  │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────────┐ │
│  │  theme  │  │ layout  │  │  avatar │  │    overlay          │ │
│  │(无依赖) │  │(ModalHist│  │(无依赖) │  │(syncSession/       │ │
│  │         │  │ory only)│  │         │  │ rebuildSession)     │ │
│  └─────────┘  └─────────┘  └─────────┘  └─────────────────────┘ │
│                                                                  │
│  ┌─────────────┐  ┌─────────────┐                                │
│  │syncSession  │  │rebuildSession│   (被 overlayStore 直接托管)  │
│  │(自包含状态机)│  │(自包含状态机)│                                │
│  └─────────────┘  └─────────────┘                                │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 依赖方向说明（禁止循环依赖）

当前 16 个 Store 的依赖图**无循环依赖**，方向始终为：

```
notification → (被所有业务 Store 引用，但自身不引用任何 Store)
     │
     ▼
assistant / settings / theme → (被 appLifecycle 引用)
     │
     ▼
chatSessionStore → (被 chatHistoryStore / chatStreamStore / topicListManager 引用)
     │
     ▼
chatHistoryStore → streamStore / assistant / attachment / settings / topicStore
chatStreamStore → sessionStore / assistant / avatar / topicStore
```

**关键单向依赖验证**：

| 被验证的依赖对 | 方向 | 是否单向 |
|----------------|------|----------|
| `chatHistoryStore` ↔ `chatSessionStore` | History → Session（通过注入） | ✅ 单向 |
| `chatStreamStore` ↔ `chatHistoryStore` | History → Stream（调用 `processStreamEvent`） | ✅ 单向 |
| `overlayStore` ↔ `syncSession`/`rebuildSession` | Overlay → Sync/Rebuild（打开时托管） | ✅ 单向 |
| `assistantStore` ↔ `topicListManager` | Topic → Assistant（通过 `combinedItems` 不直接引用） | ✅ 无直接依赖 |
| `notification` ↔ 任意 Store | 全部单向指向 Notification | ✅ 单向 |

**overlayStore 的独立性**：`overlayStore` 不依赖任何其他业务 Store（除 `syncSessionStore` 和 `rebuildSessionStore` 的托管关系外），也不被任何业务 Store 依赖。它是纯 UI 层基础设施，与业务状态完全解耦。这种设计确保了无论聊天状态如何变化，页面栈和模态框的管理始终稳定可靠。

### 3.3 依赖数量统计

| Store | 依赖的其他 Store 数 | 被其他 Store 依赖数 | 角色定位 |
|-------|-------------------|-------------------|----------|
| `notification` | 0 | 5 | 公共设施（底层） |
| `assistant` | 1 | 3 | 业务数据源 |
| `settings` | 1 | 2 | 配置源 |
| `theme` | 0 | 1 | 视觉配置 |
| `appLifecycle` | 4 | 0 | 启动编排（顶层） |
| `chatSessionStore` | 1 | 3 | 导航锚点（枢纽） |
| `chatHistoryStore` | 6 | 0 | 内容核心（叶子） |
| `chatStreamStore` | 4 | 1 | 流式引擎（枢纽） |
| `attachmentStore` | 1* | 1 | 资产工具 |
| `topicListManager` | 2 | 2 | 话题管理 |
| `overlay` | 2 | 0 | UI 基础设施 |
| `layout` | 0 | 0 | 布局状态 |
| `avatar` | 0 | 1 | 视觉资产缓存 |
| `modelStore` | 1 | 0 | 模型数据源 |
| `syncSession` | 0 | 1 | 同步状态机 |
| `rebuildSession` | 0 | 1 | 重建状态机 |

> *`attachmentStore` 仅在 Android 文件选择分支中局部创建 `useNotificationStore`，非顶部依赖。

**枢纽 Store**（依赖与被依赖数均 ≥ 2）：`chatSessionStore`、`chatStreamStore`、`topicListManager`。它们处于数据流的核心交汇点，修改时需特别谨慎。

---

## 4. 核心 Store 设计模式

### 4.1 Composition API 风格定义

全部 16 个 Store 统一使用 Pinia 的 **Setup Store**（Composition API 风格）定义：

```typescript
export const useXxxStore = defineStore('storeId', () => {
  // State: ref / reactive
  const state = ref<...>(...);
  
  // Getters: computed
  const derived = computed(() => ...);
  
  // Actions: plain functions
  const doSomething = async () => { ... };
  
  return { state, derived, doSomething };
});
```

与 Options Store 相比，Setup Store 的优势在于：

1. **类型推断更自然**：`ref`、`computed`、`function` 的类型由 TypeScript 自动推导，无需额外的类型声明文件
2. **逻辑复用更灵活**：可在 Store 内部直接使用 `watch`、`onScopeDispose` 等 Vue 生命周期钩子（如 `chatStreamStore` 的定时器清理、`appLifecycle` 的 `unwatchVcpStatus`）
3. **与 Vue 组件代码一致**：开发者无需在 Options 和 Composition 两种心智模型间切换

> 示例：`chatStreamStore.ts` 第 335–338 行使用 `onScopeDispose` 清理 `cleanupTimers`，确保 Store 被销毁（如 HMR 热更新）时不会留下孤儿定时器。

#### ref 与 reactive 的选择策略

16 个 Store 在状态声明时混合使用了 `ref` 和 `reactive`，选择依据是数据结构的访问模式：

| 场景 | 推荐方案 | 典型 Store |
|------|----------|-----------|
| 标量或简单对象 | `ref` | `streamingMessageId` (`ref<string \| null>`)、`settings` (`ref<AppSettings \| null>`) |
| 数组（需要整体替换） | `ref` | `currentChatHistory` (`ref<ChatMessage[]>`)、`topics` (`ref<Topic[]>`) |
| Map（需要响应式键值对） | `reactive(new Map())` | `activeStreamMessages` (`reactive<Map<string, ChatMessage>>`)、`cache` (`reactive(new Map())`) |
| 普通对象（固定键集合） | `reactive` 或 `ref` 均可 | `sessionActiveStreams` (`ref<Record<string, string[]>>`) |

**为何 `activeStreamMessages` 使用 `reactive(new Map())` 而非 `ref(new Map())`？**

因为 Vue 3 的 `reactive()` 对原生 `Map` 有特殊的响应式代理支持：`map.set(key, value)` 和 `map.get(key)` 都能触发依赖追踪。若使用 `ref(new Map())`，每次 `map.value.set()` 后需要手动确保依赖更新，且深层对象（如 `Map` 中存储的 `ChatMessage`）的字段变更无法被自动追踪。`reactive(new Map())` 让 `chatStreamStore` 可以直接写 `activeStreamMessages.set(id, msg)` 和 `activeStreamMessages.get(id).content += chunk`，无需解包 `.value`。

### 4.2 持久化策略（pinia-plugin-persistedstate）

项目中仅 **`chatSessionStore`** 使用 `pinia-plugin-persistedstate` 进行自动持久化：

```typescript
{
  persist: {
    pick: ['currentSelectedItem', 'currentTopicId', 'lastActiveTopicMap'],
  },
}
```

**为何只有 chatSessionStore 需要持久化？**

| Store | 不持久化的原因 |
|-------|---------------|
| `chatHistoryStore` | 消息数据量可能很大，不适合 `localStorage`；后端 SQLite 是持久源 |
| `chatStreamStore` | 流状态是瞬时的，页面刷新后必然重置 |
| `assistant` | Agent/Group 列表由后端托管，前端仅做缓存 |
| `topicListManager` | 话题列表按需加载，持久化无意义 |
| `notification` | Toast 和通知历史是易失的会话级信息 |
| `settings` | 后端 `settings_manager` 已将配置写入 SQLite，前端仅做镜像 |

`themeStore` 不使用 Pinia 持久化插件，而是直接操作 `localStorage`，原因是：
- 主题需要在 HTML `<html>` 标签渲染前即生效（防止 FOUC——Flash of Unstyled Content）
- Vite HMR 场景下需要更精细的持久化/恢复控制
- `LEGACY_THEME_MAP` 需要在读取时进行文件名迁移

### 4.3 跨 Store 通信模式

前端 Store 间通信采用三种模式，按优先级递减排列：

#### 模式 A：直接 Store 依赖（最常用）

在一个 Store 的 Action 中直接 `useOtherStore()` 获取实例并读取状态或调用方法。

```typescript
// chatHistoryStore.ts 第 28–33 行
const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();
const attachmentStore = useAttachmentStore();
const assistantStore = useAssistantStore();
const settingsStore = useSettingsStore();
const topicStore = useTopicStore();
```

**适用场景**：强业务耦合的 Store 间调用，如发送消息需要同时读取会话、设置、附件等多个 Store。

#### 模式 B：回调函数注入（解耦专用）

`chatSessionStore` 的 `selectItem` 和 `selectTopicById` 接受 `loadHistoryCallback` 回调参数：

```typescript
const selectTopicById = async (
  itemId: string, 
  topicId: string, 
  loadHistoryCallback?: (itemId: string, ownerType: string, topicId: string) => Promise<void>
) => { ... };
```

**适用场景**：需要反转依赖方向以避免循环依赖时。SessionStore 不应直接依赖 HistoryStore，否则两者形成双向依赖；通过回调注入，SessionStore 保持对 HistoryStore 的无知。

#### 模式 C：Tauri 事件广播（跨层通信）

Rust 后端通过 Tauri 事件通道向前端广播状态变更，前端 Store 通过 `listen()` 订阅：

| 事件名 | 订阅 Store | 用途 |
|--------|-----------|------|
| `vcp-log` | `syncSession` | 同步日志实时推送 |
| `vcp-sync-progress` | `syncSession` | 同步进度更新 |
| `vcp-sync-status` | `syncSession` | 同步连接状态变更 |
| `vcp-sync-completed` | `syncSession` | 同步完成通知 |
| `render_rebuild_progress` | `rebuildSession` | 预渲染重建进度 |
| `vcp-file-register-progress` | `attachmentStore` | 大文件注册进度 |
| `onThemeUpdated` | `theme` | 后端触发主题变更 |

### 4.4 Store 内事件监听生命周期管理

使用 `listen()` 订阅 Tauri 事件的 Store 必须严格管理监听器的生命周期，否则会导致：
1. **内存泄漏**：事件监听器在 Store 销毁后仍持有闭包引用
2. **重复触发**：同一事件被多个孤儿监听器重复处理
3. **状态污染**：已关闭面板的监听器仍修改响应式状态

**正确模式**（以 `syncSessionStore` 为例）：

```typescript
// src/core/stores/syncSession.ts 第 26–35 行、第 79–108 行
let unlistenFns: UnlistenFn[] = [];

const registerListeners = () => {
  cleanupListeners(); // 先清理，防止重复注册
  listen('vcp-log', (event) => { ... }).then(fn => unlistenFns.push(fn));
  listen('vcp-sync-progress', (event) => { ... }).then(fn => unlistenFns.push(fn));
  // ...
};

const cleanupListeners = () => {
  unlistenFns.forEach(fn => fn());
  unlistenFns = [];
};

const close = () => {
  // ...
  cleanupListeners();
};
```

**Promise 型监听器陷阱**：`listen()` 返回的是 `Promise<UnlistenFn>`，而非直接的 `UnlistenFn`。若 Store 在监听器注册完成前被销毁（如 HMR 热更新），必须存储 Promise 并在 `onScopeDispose` 中 `await` 其完成后再调用解注册：

```typescript
// theme.ts 第 245–255 行
const unlistenThemePromise = listen('onThemeUpdated', (event) => { ... });
onScopeDispose(() => {
  unlistenThemePromise.then((fn: UnlistenFn) => fn()).catch(() => {});
});
```

---

## 5. 与 Rust 后端的 IPC 交互

### 5.1 Store 初始化时的数据加载

各 Store 在应用启动或用户交互时，通过 `invoke()` 从 Rust 后端加载初始数据：

```
appLifecycle.bootstrap()
    │
    ├──► settingsStore.fetchSettings() ──► invoke('read_settings')
    │
    ├──► assistantStore.fetchAgents() ──► invoke('get_agents')
    │                                    └── 内部循环调用 read_agent_config
    │                                        (三级缓存策略: 内存 → SQLite → 默认)
    │
    ├──► assistantStore.fetchGroups() ──► invoke('get_groups')
    │
    └──► themeStore.initTheme() ──► Vite dynamic import + CSS 变量注入
```

**懒加载策略**：`modelStore.fetchModels()` 不在启动时加载，而是在用户首次打开模型选择器时触发。它先调用 `get_cached_models` 获取内存缓存，若缓存为空或强制刷新则调用 `refresh_models`。

### 5.2 Store 变更时的数据回写

前端 Store 不直接操作 SQLite，所有写操作通过 Tauri Command 委托给 Rust 后端：

| 前端操作 | Tauri Command | 后端负责模块 |
|----------|---------------|-------------|
| 发送消息 | `handle_agent_chat_message` / `handle_group_chat_message` | `agent_chat_application_service` |
| 删除消息 | `delete_messages` / `truncate_history_after_timestamp` | `message_service` |
| 保存 Agent 配置 | `save_agent_config` | `agent_service` |
| 保存 Group 配置 | `save_group_config` | `group_service` |
| 更新设置 | `update_settings` | `settings_manager` |
| 保存头像 | `save_avatar_data` | `avatar_service` |
| 创建话题 | `create_topic` | `topic_service` |
| 流式历史加载 | `load_chat_history_streamed` | `message_service` |

**流式写入的特例**：`chatStreamStore` 在收到 `thinking` 事件时，会立即调用 `append_single_message` 将骨架消息持久化到 SQLite（第 184–203 行）。这使得用户即使中途切换话题，重新加载历史时也能看到该消息的占位符，触发 Object Hydration 完美接续动画。

### 5.3 Channel 流式传输详解

Tauri v2 的 `Channel` 是前端 Store 与 Rust 后端进行**高吞吐量、低延迟**双向通信的核心机制。相比传统的 `invoke()` 一次请求-一次响应模式，Channel 允许 Rust 后端在单个命令执行期间向前端推送任意次数的消息。

**Channel 在 chatHistoryStore 中的使用**：

```typescript
// src/core/stores/chatHistoryStore.ts 第 97–163 行
const channel = new Channel<HistoryChunk>();
const buffer: ChatMessage[] = [];
let resolveComplete: (() => void) | null = null;
const completePromise = new Promise<void>((resolve) => { resolveComplete = resolve; });

channel.onmessage = (chunk) => {
  // 1. 会话一致性校验
  if (sessionStore.currentTopicId !== requestedTopicId && requestedTopicId !== null) {
    return; // 用户中途切换话题，丢弃后续消息
  }
  // 2. Object Hydration
  const activeMsg = streamStore.activeStreamMessages.get(chunk.message.id);
  const msgToUse = activeMsg || chunk.message;
  // 3. 累加或缓冲
  if (offset === 0) {
    currentChatHistory.value.push(msgToUse);
  } else {
    buffer.push(msgToUse);
  }
  // 4. 完成信号
  if (chunk.is_last) {
    resolveComplete?.();
  }
};

await invoke<number>('load_chat_history_streamed', {
  ownerId, ownerType, topicId, limit, offset, onMessage: channel,
});
await completePromise; // 等待全部 chunk 接收完毕
```

**关键设计点**：
- **会话一致性校验**：若用户在加载中途切换话题，后续 chunk 被丢弃，避免旧话题消息污染新话题列表
- **Buffer 模式**：分页加载时（`offset > 0`）使用 `buffer` 数组暂存，最后一次性 `unshift` 到头部，减少中间态的 DOM 重绘次数
- **完成 Promise**：`chunk.is_last` 信号与 `total === 0` 的兜底分支共同驱动 `completePromise`，确保无论有无数据都能正确释放等待

**Channel 在 topicListManager 中的使用**：

```typescript
// src/core/stores/topicListManager.ts 第 95–124 行
const channel = new Channel<Topic[]>();
channel.onmessage = (chunk) => {
  if (currentAgentId.value !== ownerId) return; // 竞态丢弃
  topics.value.push(...chunk.map(...));
  topics.value = [...topics.value]; // 强制触发虚拟列表重绘
};
await invoke('get_topics_streamed', { ownerId, ownerType, onChunk: channel });
```

与 `chatHistoryStore` 不同，`topicListManager` 的 Channel 接收的是 `Topic[]` 数组而非单个对象，这意味着 Rust 后端可以按批次（如每 20 个话题为一组）推送，前端每批只更新一次 DOM。

---

## 6. 设计决策与注意事项

### 6.1 为何使用 Composition API 风格定义 Store？

项目早期曾使用 Options API 风格定义部分 Store，但在 2025 年的重构中全面迁移到 Setup Store。原因包括：

1. **与 Vue 3.5 `<script setup>` 保持一致**：项目前端组件全部使用 `<script setup>`，Store 也使用相同风格可降低认知负担
2. **`onScopeDispose` 支持**：Setup Store 支持 Vue 作用域生命周期钩子，对 Tauri 事件监听器的清理至关重要（如 `syncSession` 的 `cleanupListeners`）
3. **更好的类型推导**：无需 `mapStores` 或 `storeToRefs` 的辅助类型，直接使用 `ref`/`computed` 即可获得完整类型推断

### 6.2 哪些 Store 需要持久化、哪些不需要？

持久化的决策遵循"**最小持久化原则**"：

- **持久化**：仅 `chatSessionStore`（3 个字段）和 `themeStore`（通过 `localStorage`）。这两者代表用户的"回到上次离开的地方"和"视觉偏好"，属于跨会话的个性化状态。
- **不持久化**：其余 14 个 Store。业务数据（消息、Agent、话题）由后端 SQLite 作为唯一持久源；运行时状态（流、Toast、抽屉）是瞬时的，无需恢复。

这一决策避免了 `localStorage` 容量限制（通常 5MB）和版本兼容性问题，同时确保多端同步时前端不会持有过期的本地缓存。

### 6.3 Store 分层的设计逻辑

16 个 Store 并非随意拆分，而是遵循**领域边界 + 变化频率**双重标准：

| 分层 | 变化频率 | 代表 Store |
|------|----------|-----------|
| **导航层** | 低（用户切换会话） | `chatSessionStore` |
| **内容层** | 高（每发送一条消息） | `chatHistoryStore`、`chatStreamStore` |
| **资产层** | 中（上传附件、切换头像） | `attachmentStore`、`avatarStore` |
| **配置层** | 低（应用设置变更） | `settings`、`theme` |
| **基础设施层** | 极低（应用启动一次） | `appLifecycle` |

将 `chatSessionStore` 从 `chatHistoryStore` 中独立出来，是因为会话选择（"看哪个 Agent"）与消息内容（"看到了什么"）是两个完全不同的变化频率和作用域。分离后，话题切换时只需重置 `currentTopicId`，而不影响历史加载的分页状态。

### 6.4 Object Hydration 的权衡

`chatHistoryStore` 中的 Object Hydration 是一个精妙的妥协设计：

**收益**：流式消息在历史加载时不会"闪烁"或"重置"，动画连续性得到保证。
**代价**：`chatHistoryStore` 必须依赖 `chatStreamStore`，增加了耦合度。
**替代方案评估**：若将活跃消息池下沉到 `chatHistoryStore`，则 `chatStreamStore` 会变得过于单薄；若完全取消 Hydration，则流式消息在历史加载后会出现内容跳跃。当前方案在耦合与体验之间取得了平衡。

### 6.5 attachmentStore 的 Android 特殊性

`attachmentStore` 是前端 Store 中**平台感知最强**的一个。它在运行时检测 `navigator.userAgent` 是否包含 `android`，如果是，则走原生 Kotlin File Picker 路径（通过 `evaluateJavascript` 注入的自定义事件）；否则走标准 HTML Input 路径。这种分支逻辑集中在 Store 中而非分散到组件，确保了附件上传行为的全局一致性。

### 6.6 性能考量与内存管理

前端 Store 在移动端 WebView 环境中运行，内存和 CPU 资源受限。以下是在 Store 设计中贯彻的性能策略：

**响应式对象上限控制**：
- `chatStreamStore.activeStreamMessages` 上限 100 条，超出时清理最旧的非活跃消息
- `notification.historyList` 上限 100 条，超出时从尾部弹出
- `syncSession.logs` 上限 200 条，超出时从头部 shift（保留最新日志）
- `avatar.cache` 上限 50 条，超出时按 FIFO 清理并 `URL.revokeObjectURL()` 释放物理内存

**shallowRef 的使用**：`overlayStore.contextMenuConfig` 使用 `shallowRef` 而非 `ref`。因为上下文菜单的配置对象（含 `actions` 数组）不会被深层修改，使用 `shallowRef` 可避免 Vue 对整个数组进行深层响应式代理，减少运行时开销。

**防抖与锁频**：
- `modelStore.fetchModels` 在 `isLoading` 期间拒绝新的刷新请求
- `attachmentStore` 的 XHR `onprogress` 限制刷新频率为 ~30fps（每 33ms 一次），避免高频重绘导致卡顿
- `themeStore.setMode` 有 100ms 的防抖窗口，防止快速连续点击导致主题闪烁

**懒加载与空闲回调**：
- `themeStore.initTheme()` 优先只加载当前主题，全量主题扫描推迟到 `requestIdleCallback`
- `modelStore.fetchModels()` 不在启动时加载，而是按需触发
- `assistantStore` 的 `fetchAgents`/`fetchGroups` 仅在生命周期启动或同步恢复后触发

### 6.7 前端 Store 的测试现状

根据 `AGENTS.md` 的明确记录，**VCP Mobile 当前没有前端自动化测试**：

- `package.json` 不含任何测试脚本（无 `test`、`test:unit`、`test:e2e`）
- 未安装 Vitest、Jest、Cypress、Playwright 等测试框架
- 16 个 Pinia Store 均无任何单元测试覆盖

**验证手段**：以 `pnpm check`（`vue-tsc --noEmit` 前端类型检查 + `cargo check` Rust 编译检查）和真机/模拟器手动测试为主。

这种现状的成因是：
1. **Tauri IPC  mocking 成本高**：所有 Store 的核心逻辑都围绕 `invoke()` 展开，搭建稳定的 IPC mock 环境需要大量基础设施投入
2. **UI 与状态高度耦合**：许多 Store 的 Action 同时触发 Toast 通知、DOM 操作（如 `themeStore` 的 CSS 变量注入）、物理返回键注册，难以在 headless 环境中验证
3. **流式行为的时间敏感性**：`chatStreamStore` 的事件处理涉及定时器、Promise 竞态、状态机转换，单元测试的断言时机难以把握

**未来改进方向**（非当前优先级）：
- 为纯计算逻辑（如 `avatarStore` 的 `extractDominantColorFromBlob`、`modelStore` 的 `sortedModels`）引入 Vitest 单元测试
- 为 `notificationStore` 的 `addNotification` 单例抑制逻辑编写边界条件测试
- 使用 `@tauri-apps/api/mocks`（若可用）搭建最小 IPC mock 环境，覆盖 `settingsStore` 的读写流程

---

## 7. 术语速查表

| 术语 | 英文/缩写 | 定义 | 相关 Store |
|------|----------|------|-----------|
| Setup Store | — | Pinia 的 Composition API 风格 Store 定义方式，使用 `defineStore('id', () => { ... })` | 全部 |
| Object Hydration | — | 从历史加载的死消息对象替换为流式消息池中的活响应式对象，保证动画连续性 | `chatHistoryStore` |
| 全局流消息池 | Active Stream Pool | `chatStreamStore.activeStreamMessages`，存放所有正在生成的 Assistant 消息的响应式对象 | `chatStreamStore` |
| MessageShell | — | 消息气泡的 UI 元数据（头像色、边框色、显示名等），在前端本地计算 | `chatStreamStore` |
| 乐观更新 | Optimistic Update | 在异步后端确认前，先更新本地 UI 状态的操作模式 | `topicListManager` |
| 虚拟页面栈 | Virtual Page Stack | `overlayStore.pageStack`，在单页应用中模拟多页面导航的栈结构 | `overlayStore` |
| 单例抑制 | Singleton Suppression | 通过固定 ID 避免同一通知反复弹出多个 Toast 的机制 | `notification` |
| 幽灵清理 | Ghost Cleanup | 定期扫描并移除残留过期 Toast 的后台机制 | `notification` |
| JIT 预处理 | Just-In-Time Preprocessing | 消息发送前对文档附件进行文本提取的即时处理 | `attachmentStore` |
| 高速链路 | High-Speed Link | 大文件（≥ 2MB）上传时绕开 IPC 的临时本地 TCP 接收器方案 | `attachmentStore` |
| 视觉补白 | Visual Padding | 加载任务完成后若耗时过短，人为延迟至最低时长的 UX 技巧 | `appLifecycle` |
| 三级缓存 | Three-Level Cache | Rust 后端 `agent_service` 的读取策略：内存 DashMap → SQLite → 默认配置 | `assistant`（消费端） |
| 主色调 | Dominant Color | 从头像提取的代表性颜色，用于 UI 主题色 | `avatar` |
| 会话隔离 | Session Isolation | 流状态按 `"itemId:topicId"` 索引，确保多话题并发流互不干扰 | `chatStreamStore` |
| Backend-Driven Streaming | — | 由后端 SSE 事件驱动消息生命周期，前端不再预创建 thinking 占位消息 | `chatStreamStore` |
| 增量同步 | Incremental Sync | 基于 SHA-256 Hash 差异的三阶段同步协议 | `syncSession` |
| 热更新感知 | HMR Awareness | Store 检测 Vite HMR 并自动重新加载状态的机制 | `theme` |
| 智能话题恢复 | Smart Topic Recovery | 启动时优先使用缓存的 `lastActiveTopicMap` 恢复上次话题 | `chatSessionStore` |
| 流式分块 | Streamed Chunk | 通过 Tauri `Channel` 分批次接收的大量数据（历史消息、话题列表） | `chatHistoryStore`、`topicListManager` |
| 锁频防护 | Debounce Guard | 防止并发重复触发同一异步请求的状态检查（如 `isLoading`） | `modelStore` |

---

*最后更新：2026-05-27 | VCP Mobile v0.9.14*
