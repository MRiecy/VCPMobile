---
id: MOD-INFRA-023
title: RAG灵视中心
description: VCP Mobile 认知广播观察器，WebSocket 连接桌面 VCP /vcpinfo 端点，监听并可视化 AI 认知事件流
version: "1.1.0"
date: 2026-06-15
module: vcp_info_service.rs
scope: src-tauri/src/vcp_modules/infra/
related: [RagObserver.vue, ragObserver.ts, lifecycle_manager.rs, settings_manager.rs, RightSidebar.vue]
---

# 23_RAG灵视中心（RAG Observer Center）

> 源文件：
> - `src-tauri/src/vcp_modules/infra/vcp_info_service.rs`（765 行）
> - `src/features/rag/RagObserver.vue`（~1290 行）
> - `src/core/stores/ragObserver.ts`（135 行）
>
> 本文档覆盖 VCP Mobile 的"认知广播观察器"模块——一条从桌面 VCP 到移动端的 WebSocket 实时通道，让用户在手机上观察 AI 的知识检索、元思考链、记忆回溯、Agent 梦境等认知活动。

---

## 1. 概述

### 1.1 模块定位

`vcp_info_service.rs` 是一个**WebSocket 客户端服务**，连接到桌面 VCP 应用的 `/vcpinfo` WebSocket 端点，监听 AI 认知事件广播。它被设计为"被动观察者"——只接收、缓存、转发，不发送任何业务请求。

该模块解决的核心需求：
- 用户在手机上实时看到桌面 AI 正在"想什么"（RAG 检索了哪些知识库、元思考链走到了哪一步、Agent 梦境中联想到了什么记忆）
- 所有认知事件在移动端以卡片形式可视化展示，支持按类型过滤和懒加载详情

### 1.2 系统位置

```
┌──────────────────────────────────────────────────────────────┐
│                     桌面 VCP 应用                              │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  VCP Info Broadcaster (内置 WebSocket 服务器)              │ │
│  │  广播 AI 认知事件: RAG知识库、元思考链、Agent会话、      │ │
│  │                   记忆回溯、Agent梦境、DailyNote          │ │
│  └──────────────────────┬───────────────────────────────────┘ │
└─────────────────────────┼────────────────────────────────────┘
                          │ wss://host:port/vcpinfo/VCP_Key=xxx
                          │
┌─────────────────────────▼────────────────────────────────────┐
│                   VCP Mobile (本模块)                          │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  vcp_info_service.rs (Rust WS 客户端)                     │ │
│  │  - 连接管理 + 自动重连（指数退避 1s→60s）                  │ │
│  │  - zstd 压缩缓存（500条 FIFO）                             │ │
│  │  - Ping 心跳（15s 间隔）                                   │ │
│  │  - 5 个 Tauri Command 供前端调用                           │ │
│  └──────────────────────┬───────────────────────────────────┘ │
│                         │ Tauri Events (vcp-info-event)       │
│  ┌──────────────────────▼───────────────────────────────────┐ │
│  │  ragObserver.ts (Pinia Store)                             │ │
│  │  - 响应式 metadataList (上限 500)                          │ │
│  │  - connectionStatus: closed/connecting/connected/error    │ │
│  │  - 按需 fetchPayload (zstd 解压 + JSON.parse)             │ │
│  └──────────────────────┬───────────────────────────────────┘ │
│                         │ Vue reactive binding                │
│  ┌──────────────────────▼───────────────────────────────────┐ │
│  │  RagObserver.vue (SlidePage 全屏面板)                      │ │
│  │  - 折叠卡片列表 + 6 个过滤标签                             │ │
│  │  - 频谱动画 Canvas                                         │ │
│  │  - 子卡片展开 + Markdown 渲染                              │ │
│  └──────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

---

## 2. 核心类型与消息分类

### 2.1 VcpInfoMetadata（前端类型）

```typescript
interface VcpInfoMetadata {
  id: string;        // 格式: "vcp_info_{timestamp}_{counter}"
  type: string;      // 消息类型枚举值
  title: string;     // 卡片主标题
  subtitle?: string; // 卡片副标题（详细信息）
  summary: string;   // 摘要文本（截断到 50 字符）
  timestamp: string; // ISO 8601 时间戳
  hasDetails: boolean; // 是否可展开查看详情
}
```

### 2.2 ConnectionStatus

```typescript
type ConnectionStatus = 'closed' | 'connecting' | 'connected' | 'error';
```

状态在连接生命周期中自动流转：
```
closed → connecting → connected → (断开) → closed → (重连) → connecting → ...
                              ↘ error → (指数退避重试) → connecting → ...
```

### 2.3 消息类型枚举（6 类）

| 类型 | type 常量 | 说明 | hasDetails |
|------|----------|------|:----------:|
| **RAG 知识库检索** | `RAG_RETRIEVAL_DETAILS`（兜底匹配）| AI 查询知识库，返回召回结果 | true |
| **元思考链** | `META_THINKING_CHAIN` | AI 的推理阶段、K 序列、激活分组 | true |
| **Agent 会话预览** | `AGENT_PRIVATE_CHAT_PREVIEW` | Agent 私聊的查询/响应摘要 | true |
| **记忆回溯** | `AI_MEMO_RETRIEVAL` | 日记/文件记忆检索，含 TagMemo 召回 | true |
| **Agent 梦境** | `AGENT_DREAM_*`（5 个子类型）| 梦境全生命周期广播 | 部分 |
| **DailyNote 日记** | `DailyNote` | 日记直接召回通知 | false |

Agent 梦境子类型：

| 子类型 | 说明 | hasDetails |
|--------|------|:----------:|
| `AGENT_DREAM_START` | Agent 入梦开始 | false |
| `AGENT_DREAM_ASSOCIATIONS` | 梦境共鸣联想（种子数、联想数） | true |
| `AGENT_DREAM_NARRATIVE` | 梦叙事文本（含完整叙事 JSON） | true |
| `AGENT_DREAM_OPERATIONS` | 梦操作（合并/删除/感悟的待审核列表） | true |
| `AGENT_DREAM_END` | 出梦（成功/失败状态） | false |
| `AGENT_DREAM_SCHEDULE` | 梦境自动调度（定时触发） | false |

---

## 3. Rust 后端（vcp_info_service.rs）

### 3.1 全局静态状态

```rust
lazy_static! {
    static ref INFO_CONNECTION_ACTIVE: Arc<AtomicBool>;      // 防重复连接
    static ref METADATA_LIST: Arc<RwLock<VecDeque<Value>>>;  // 元数据 FIFO 队列
    static ref COMPRESSED_PAYLOADS: Arc<RwLock<HashMap<String, Vec<u8>>>>; // id → zstd 压缩字节
    static ref WS_INFO_URL_CHANNEL: (watch::Sender<Option<Url>>, watch::Receiver<Option<Url>>);
    static ref CURRENT_INFO_STATUS: Arc<RwLock<String>>;      // "closed"|"connecting"|"connected"|"error"
}
```

关键设计决策：
- 全部使用 `lazy_static!` + `Arc<RwLock<>>`，确保在 Tauri 的多线程异步运行时中安全共享
- `METADATA_LIST` 和 `COMPRESSED_PAYLOADS` 分离存储——元数据轻量常驻内存，完整载荷压缩后按需解压
- `WS_INFO_URL_CHANNEL` 使用 `watch::channel`，URL 变更时自动触发重连

### 3.2 连接建立与重连策略

`init_vcp_info_connection(url, key)` 是连接入口：

```
1. parse_info_url(): 将 VCP Server URL 转为 ws://host/vcpinfo/VCP_Key=xxx
2. 写入 WS_INFO_URL_CHANNEL，通知监听器线程
3. 若 INFO_CONNECTION_ACTIVE 已为 true，跳过（防重复）
4. spawn start_vcp_info_listener() 异步任务
```

`start_vcp_info_listener()` 是持久化监听循环：

```
loop {
    │
    ├── 从 url_rx 读取当前 URL（无则等待 changed）
    │
    ├── 构建请求：添加 Host/Origin/User-Agent 头
    │
    ├── tokio::time::timeout(10s, connect_async(request))
    │   ├── 成功 → 进入消息循环
    │   └── 失败/超时 → 指数退避重试
    │
    ├── 消息循环（tokio::select!）:
    │   ├── url_rx.changed() → URL 变更，断开重连
    │   ├── heartbeat_timer (15s) → 发送 Ping 帧
    │   └── ws_read.next() → 处理文本消息
    │
    └── 断开后：指数退避 (1s → 2s → 4s → ... → 60s 上限)
}
```

重试延迟计算：`retry_delay = (retry_delay * 2).min(Duration::from_secs(60))`，初始值 1 秒。

### 3.3 zstd 压缩缓存与 FIFO 淘汰

`process_incoming_vcp_info()` 处理每条到达的消息：

```
1. extract_metadata(&msg_id, &payload)
   ├── 根据 type 提取 title / subtitle / summary
   └── 无法识别的消息 → 返回 None，直接丢弃

2. compress_payload(raw_text) → zstd::encode_all(level=3)

3. 存入内存:
   ├── METADATA_LIST.push_front(metadata)
   └── COMPRESSED_PAYLOADS.insert(msg_id, compressed_data)

4. FIFO 淘汰: 当 METADATA_LIST 超过 500 条时:
   ├── pop_back() 逐条淘汰
   └── 同步清理 COMPRESSED_PAYLOADS 对应条目

5. 向前端广播 vcp-info-event { type: "vcp-info-message", data: metadata }
```

zstd 压缩级别 3 是速度与压缩比的折中选择——移动端解压开销低，同时 500 条消息的总体内存占用可控。

### 3.4 元数据提取器（extract_metadata）

位于文件 393–764 行，是一个大型 `match msg_type` 分支：

```
match msg_type {
    "AGENT_PRIVATE_CHAT_PREVIEW" → 提取 agentName / sessionId / query / response
    "META_THINKING_CHAIN"        → 提取 chainName / totalStages / kSequence / activatedGroups
    "AI_MEMO_RETRIEVAL"          → 提取 diaryCount / fileCount / mode / chunkCount / extractedMemories
    "DailyNote"                  → 提取 dbName / action / message
    t if t.starts_with("AGENT_DREAM_") → 5 个子类型分支处理
    "AGENT_DREAM_SCHEDULE"       → 提取 message / currentHour / agents
    _ (兜底 RAG)                  → dbName + K + 策略标签 (Time|Rerank|TagMemo|GeoRerank|Associate|Group)
}
```

所有类型的 summary 字段均截断到 50 字符（`.chars().take(50)`），防止卡片列表页面溢出。

### 3.5 5 个 Tauri Command

| Command | 参数 | 返回 | 说明 |
|---------|------|------|------|
| `get_vcp_info_connection_status` | — | `String` | 返回当前连接状态：`"closed"` / `"connecting"` / `"connected"` / `"error"` |
| `get_vcp_info_metadata_list` | — | `Vec<Value>` | 返回内存中全部元数据列表（最多 500 条） |
| `get_vcp_info_payload` | `id: String` | `String` | 按 id 从压缩缓存中获取完整载荷，返回解压后 JSON 字符串 |
| `clear_vcp_info` | — | `()` | 清空内存中的元数据列表和压缩载荷缓存，广播 `vcp-info-clear` 事件 |
| `init_vcp_info_connection` | `url: String, key: String` | `()` | 初始化/更新 WebSocket 连接 |

### 3.6 Ping 心跳

连接建立后每 15 秒发送一次 WebSocket Ping 帧：
```rust
heartbeat_timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(15));
```
Ping 失败时退出消息循环，进入重连流程。这确保了长时间空闲连接不会被中间代理关闭。

---

## 4. 前端 RagObserver（RagObserver.vue）

### 4.1 入口与布局

`RagObserver.vue` 是一个全屏 SlidePage 面板，从 `RightSidebar.vue` 的"灵视"按钮触发打开：

```html
<button @click="overlayStore.openRagObserver()">灵视</button>
```

面板结构：
```
┌──────────────────────────────────────────┐
│  [X 关闭]  RAG 灵视中心  [连接状态指示器]  │  ← Header
│  [🗂️ 全部] [📚 RAG] [🔗 思考链]           │  ← 6 个过滤标签
│  [💬 会话] [🧠 记忆] [🌙 梦境] [🗑️ 清空]   │
├──────────────────────────────────────────┤
│  ┌──────────────────────────────────────┐ │
│  │ 🎨 频谱动画 Canvas (连接时播放)       │ │
│  ├──────────────────────────────────────┤ │
│  │ 卡片 1: RAG知识库: MyDB              │ │  ← 折叠卡片列表
│  │   K: 5 | [Time | Rerank]            │ │     (filtered)
│  │   [召回 3 项] 什么是...       ▼      │ │
│  │   ┌─ 子卡片: 详细信息 (展开状态) ──┐ │ │
│  │   │ Markdown 渲染的完整载荷        │ │ │
│  │   └──────────────────────────────┘ │ │
│  ├──────────────────────────────────────┤ │
│  │ 卡片 2: 元思考链: 核心推理           │ │
│  │   阶段: 3 | K序列: [1,2,1]   ▼     │ │
│  │ ...                                  │ │
│  └──────────────────────────────────────┘ │
└──────────────────────────────────────────┘
```

### 4.2 6 个过滤标签

```typescript
const activeFilter = ref<'all' | 'rag' | 'chain' | 'chat' | 'memo' | 'dream'>('all');
```

| 标签 | 值 | 过滤逻辑 |
|------|-----|----------|
| 全部 | `'all'` | 显示所有消息 |
| RAG | `'rag'` | `type` 匹配 RAG_RETRIEVAL_DETAILS（兜底分支） |
| 思考链 | `'chain'` | `type === 'META_THINKING_CHAIN'` |
| 会话 | `'chat'` | `type === 'AGENT_PRIVATE_CHAT_PREVIEW'` |
| 记忆 | `'memo'` | `type === 'AI_MEMO_RETRIEVAL'` 或 `type === 'DailyNote'` |
| 梦境 | `'dream'` | `type.startsWith('AGENT_DREAM_')` 或 `type === 'AGENT_DREAM_SCHEDULE'` |

### 4.3 频谱动画 Canvas

组件内使用 Canvas 2D 绘制音频频谱风格的实时动画：

- **触发条件**：`store.triggerSpectrumAnimation === true`（每次 `vcp-info-message` 事件到达时触发 1.5 秒动画）
- **实现方式**：`requestAnimationFrame` 驱动，绘制 64 根频率柱
- **视觉风格**：渐变色填充（`createLinearGradient`），柱高随机波动模拟频谱分析仪效果

### 4.4 卡片展开与子卡片

- **卡片展开**：`expandedCardIds: Set<string>` 控制顶层卡片的展开/折叠，点击切换
- **子卡片**：当 `hasDetails === true` 时，展开后可查看详情；详情通过 `store.fetchPayload(id)` 按需加载（懒加载），加载期间显示 Spinner
- **Markdown 渲染**：子卡片内使用 `RagPayloadDetail.vue` 子组件渲染完整 JSON 载荷，支持 Markdown 文本格式化

### 4.5 左右滑动手势

组件实现了触摸手势切换过滤标签：

```
touchstart → 记录 startX
touchmove  → 计算 deltaX (需 >30px 阈值)
           → 限制偏角在 ±25 度以内（防止垂直滚动误触）
touchend   → deltaX < -50 → next tab
           → deltaX > 50  → prev tab
```

---

## 5. 集成点

### 5.1 lifecycle_manager.rs：启动时自动连接

在 `bootstrap()` 的 P2 后台任务中（`lifecycle_manager.rs:266`）：

```rust
if !s_url.is_empty() && !s_key.is_empty() {
    // 自动连接 VCP Log（前面）
    init_vcp_log_connection_internal(h.clone(), s_url.clone(), s_key.clone()).await;
    // 自动连接 VCP Info
    let _ = super::vcp_info_service::init_vcp_info_connection(h.clone(), s_url, s_key).await;
}
```

VCP Info 连接与 VCP Log 连接在同一代码块中顺序初始化，共享相同的 URL 和 API Key。

### 5.2 settings_manager.rs：设置变更时自动重连

`settings_manager.rs:258` 在 VCP 服务器 URL 或 API Key 变更时调用 `init_vcp_info_connection_internal`：

- 若新 URL/Key 为空，发送 `None` 到 `WS_INFO_URL_CHANNEL`，触发监听器断开当前连接
- 若 URL/Key 有效，`watch::channel` 自动通知监听器以新 URL 重连

### 5.3 RightSidebar.vue：UI 入口按钮

右侧抽屉中"灵视"按钮（`RightSidebar.vue:182`）：

```html
<button @click="overlayStore.openRagObserver()">灵视</button>
```

点击后 `overlayStore` 将 `RagObserver` 的 `isOpen` 设为 `true`，SlidePage 面板从右侧滑入。

### 5.4 Tauri Command 注册

5 个命令在 `lib.rs` 中注册为 `#[tauri::command]`，前端通过标准 `invoke()` 调用。

---

## 6. 性能考量

| 维度 | 策略 | 说明 |
|------|------|------|
| **内存上限** | 500 条 FIFO 淘汰 | `METADATA_LIST` 和 `COMPRESSED_PAYLOADS` 同步淘汰，防止无限增长 |
| **载荷存储** | zstd level 3 压缩 | 原始 JSON 压缩后存入 `HashMap`，按需解压 |
| **元数据/载荷分离** | 懒加载 | 列表页仅传输轻量 metadata（~200 bytes/条），详情页按需调用 `get_vcp_info_payload` |
| **前端渲染** | `content-visibility: auto` | CSS 属性让屏幕外的卡片跳过渲染，减少首屏布局开销 |
| **连接开销** | 单 WebSocket 连接 | 全局共享一条 WS 连接，不随卡片数量增长 |
| **心跳间隔** | 15 秒 Ping | 平衡保活效果与流量开销 |
| **重连退避** | 1s → 60s 指数上限 | 避免对 VCP 服务器造成连接风暴 |
| **前端列表上限** | 500 条（前端再截断） | `metadataList.value.length > 500` 时 `pop()` |

---

## 7. 交叉引用

| 关联模块 | 文档 | 关系 |
|----------|------|------|
| `ragObserver.ts` | —（本文档覆盖） | Pinia Store，前端状态管理 |
| `RagObserver.vue` | —（本文档覆盖） | 全屏 SlidePage UI 面板 |
| `RagPayloadDetail.vue` | —（本文档覆盖） | 子卡片详情 + Markdown 渲染 |
| `lifecycle_manager.rs` | MOD-INFRA-021 基础设施杂项 | Bootstrap 阶段自动连接 VCP Info |
| `settings_manager.rs` | MOD-INFRA-006 设置管理系统 | URL/Key 变更时自动重连 |
| `RightSidebar.vue` | VUE-COMP-020 布局外壳与 UI 原语 | "灵视"入口按钮 |
| `overlayStore` | VUE-CORE-004 UI 状态与覆盖层管理 | `openRagObserver()` 方法 |

---

*最后更新：2026-06-15 | VCP Mobile v1.1.0*
