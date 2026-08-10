---
id: MOD-VCP-CLI-009
version: "1.1.3"
date: 2026-08-10
module: vcp_client.rs
scope: src-tauri/src/vcp_modules/
related: [aurora_pipeline.rs, media_processor/, content_parser.rs, agent_chat_application_service.rs, group_chat_application_service.rs, message_service.rs]
---

# 09_VCP 请求客户端（VCP Client）

## 1. 概述

### 1.1 模块定位

`vcp_client.rs` 是 VCP Mobile 核心层（Rust 后端）的**统一 VCP 请求处理模块**，位于 `src-tauri/src/vcp_modules/infra/vcp_client.rs`（约 2100 行）。该模块对应原桌面端项目的 `modules/vcpClient.js`，负责处理所有与 VCP 服务器的通信，是前端对话引擎与后端网络层之间的唯一 HTTP 出入口。

> **2026-08-10 生命周期收口**：Android SSE Helper 仍负责缓存与按事件索引续接，但恢复入口已合并为单个 `recover_active_generation`。它从 claim、query/cache、resume 一直持有同一 attempt lease 到 terminal DB commit，不再暴露独立 `resume_stream` Tauri Command。

其核心职责包括：
- 将前端传入的 `VcpRequestPayload` 转换为标准化 HTTP 请求
- 在请求预处理阶段完成**多模态本地文件编码**（图片/视频/音频 → data URL）
- 根据用户设置执行**动态路由切换**与**上下文注入**（音乐状态、UI 规范）
- 支持**流式（SSE）**与**非流式**双模式响应处理
- 通过 `tokio::sync::oneshot` 实现全链路**请求中止机制**（含深层轮询捕获）
- 向前端推送标准化的 `StreamEvent` 事件序列

### 1.2 职责边界

| 职责领域 | 具体行为 | 对应源码位置 |
|---------|---------|------------|
| 请求参数序列化 | `VcpRequestPayload` 的 Rust 类型校验与 JSON 组装 | `VcpRequestPayload:30` |
| 多模态预处理 | 识别 `local_file` 类型，按扩展名分发到图片/视频/音频处理器 | `perform_vcp_request:241` |
| 动态路由切换 | 根据 `enableVcpToolInjection` 设置切换 `/v1/chat/completions` ↔ `/v1/chatvcp/completions` | `perform_vcp_request:393` |
| 上下文注入 | 读取 `music_state.json`、`songlist.json`，注入 System Message | `perform_vcp_request:403` |
| 流式 SSE 解析 | 使用 `LinesCodec` + `tokio::select!` 逐行解析 `data:` 事件 | `perform_vcp_request:536` |
| Aurora 语义沉淀驱动 | 每收到文本 chunk 追加到 `AuroraBuffer`，触发增量块解析与推测渲染 | `perform_vcp_request:591` |
| 请求中止 | `ActiveRequests` + attempt UUID + `oneshot::Sender` + RAII lease | `ActiveRequestEntry`, `ActiveRequestLease`, `interruptRequest` |
| 连接测试 | 对齐桌面端逻辑的 `/v1/models` 探测与模型计数 | `test_vcp_connection:762` |
| 活跃生成恢复 | 查询 `active_generations`、本地 `sse_cache` 及助手内存，恢复异常中断的流 | `get_active_generations:1611`, `recover_active_generation:1767` |
| 流式断点续传 | `recover_active_generation` 在已 claim 的 attempt 内按 `startIndex` 续接 Helper 流 | `resume_claimed_generation`, `handle_streaming_request` |
| 助手进程通信 | `LengthDelimitedCodec` 帧协议：4 字节大端长度 + JSON payload | `send_command_to_stream:705`, `connect_to_helper:612` |

### 1.3 调用入口

```text
Vue 3 前端（对话引擎）
    ↓ Tauri IPC invoke: sendToVCP
lib.rs（命令路由）
    ↓ 调用
vcp_client.rs
    ↓ perform_vcp_request
    ├─→ media_processor/ （多模态文件编码）
    ├─→ db_manager/settings_manager （读取设置）
    ├─→ aurora_pipeline.rs （流式语义沉淀）
    ↓ 返回 (Value, bool)
lib.rs
    ↓ Tauri IPC Channel<StreamEvent>
Vue 3 前端（消息渲染层）
```

内部 Rust 调用者（不经过前端）：
```text
agent_chat_application_service.rs ──→ perform_vcp_request ──→ 单聊消息处理
group_chat_application_service.rs ──→ perform_vcp_request ──→ 群聊接力赛编排
topic_summary_service.rs ──→ sendToVCP / perform_vcp_request ──→ 话题总结
```

---

## 2. 核心类型与数据结构

### 2.1 VcpRequestPayload

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VcpRequestPayload {
    pub vcp_url: String,        // VCP服务器URL
    pub vcp_api_key: String,    // API密钥
    pub messages: Vec<Value>,   // 消息数组（OpenAI 格式，含多模态 content 数组）
    pub model_config: Value,    // 模型配置（model, stream, temperature 等）
    pub message_id: String,     // 消息ID（UUID，用于跟踪和中止）
    pub context: Option<Value>, // 上下文信息（agentId, topicId, groupId 等）
}
```

- `messages` 中的 `content` 可以是字符串或数组。当为数组时，每个元素是一个 part 对象（如 `{type: "text", text: "..."}` 或 `{type: "local_file", path: "file://..."}`）。
- `model_config` 由前端组装，必须包含 `stream: bool` 字段以决定处理模式。
- `context` 原样透传，最终会出现在 `StreamEvent.context` 中，供前端路由到正确的消息气泡。

### 2.2 StreamEvent

```rust
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: String,                    // "data" | "aurora" | "thinking" | "end" | "error" | "reconnecting"
    pub chunk: Option<Value>,              // 原始 SSE chunk（仅 data）
    pub message_id: String,
    pub context: Option<Value>,
    pub finish_reason: Option<String>,     // "completed" | "cancelled_by_user" | "error"
    pub error: Option<String>,
    pub aurora: Option<AuroraUpdate>,      // 语义沉淀快照（仅 aurora）
    pub blocks: Option<Vec<ContentBlock>>, // 预渲染块（仅 end）
    pub timestamp: Option<u64>,            // 物理落笔时间戳（仅 end，由 finalize_stream_message 设置）
}
```

事件类型语义：

| `type` | 触发时机 | 前端行为 |
|--------|---------|---------|
| `data` | 每收到一个 SSE `data:` 行 | 兼容旧版渲染，直接追加原始文本 |
| `aurora` | AuroraBuffer 的 stable/tail 发生变化，或 33ms / 1024 字节双阈值节流到期 | 增量更新已闭合块列表 + 尾部推测渲染 + AST Diff 突变执行 |
| `thinking` | 后端在流式请求开始前主动发射 | 创建 thinking 占位消息骨架（is_thinking = true） |
| `end` | 流正常结束或被中止后，携带 timestamp 和最终 blocks | 隐藏"输入中"状态，显示最终 finish_reason |
| `error` | HTTP 错误、流读取异常、SSE 空闲超时 | 显示错误提示，终止渲染 |
| `reconnecting` | Android 流式代理重连期间 | 前端展示“重连中”状态 |

### 2.3 ActiveRequests

```rust
pub struct ActiveRequestEntry {
    attempt_id: Uuid,
    abort_tx: Mutex<Option<oneshot::Sender<()>>>,
}

pub struct ActiveRequests(pub Arc<DashMap<String, Arc<ActiveRequestEntry>>>);
```

- 键：`message_id`（String）
- 值：带唯一 `attempt_id` 的取消句柄；同一个 message ID 只允许一个 live attempt
- 使用 `DashMap` 而非 `Mutex<HashMap>`：支持高并发读写，无需锁竞争
- `Arc` 包装确保跨 `tokio::spawn` 克隆时的共享所有权

### 2.4 ActiveRequestLease（RAII 所有权）

```rust
pub struct ActiveRequestLease {
    requests: ActiveRequestMap,
    message_id: String,
    attempt_id: Uuid,
}

impl Drop for ActiveRequestLease {
    fn drop(&mut self) {
        // 仅当 map 当前仍是本 attempt 时移除
        remove_if_attempt_matches(self.message_id, self.attempt_id);
    }
}
```

- `try_acquire` 使用 DashMap entry API 拒绝重复 live ID，不再覆盖旧 sender；
- `interruptRequest` 只触发 cancel，不提前删除 entry；真正任务退出后才由 lease 释放；
- Drop 同时校验 `message_id + attempt_id`，旧任务不能删除后来同 ID 的新 attempt；
- Agent/Group 请求的 lease 覆盖网络请求和 `finalize_stream_message` 提交窗口。网络结束但 terminal 事务未提交时，该 generation 仍被识别为 live。

### 2.5 CancelledGroupTurns

```rust
pub struct CancelledGroupTurns(pub Arc<DashSet<String>>);
```

- 键：`topic_id`（String）
- 使用 `DashSet`：存在即代表该话题的群聊接力赛回合已被取消
- 由 `interruptGroupTurn` Command 写入，由 `group_chat_application_service.rs` 在编排循环中读取

---

## 3. 核心流程详解

### 3.1 整体请求生命周期

```
前端调用 sendToVCP(payload, channel)
           │
           ▼
    ┌──────────────┐
    │ 0. 数据验证   │ ← 过滤非对象消息，处理 content 数组
    │    与规范化   │   local_file → data URL（多模态预处理）
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │ 1. 读取设置   │ ← 加载 SQLite global 设置
    │    与路由决策 │   enableVcpToolInjection / agentMusicControl / enableAgentBubbleTheme
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │ 2. 上下文注入 │ ← music_state.json / songlist.json / UI 规范
    │    到 System  │   拼接为 top_parts + bottom_parts
    │    Message    │
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │ 3. 组装请求体 │ ← 注入 messages / messageId / stream
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │ 4. 申领请求   │ ← attempt UUID + oneshot::channel
    │    Lease      │   DashMap entry 拒绝重复 owner
    └──────┬───────┘
           │
    ┌──────┴───────┐
    ▼              ▼
┌────────┐    ┌────────┐
│ 流式模式 │    │非流式模式│
│(SSE)    │    │(JSON)   │
└───┬────┘    └───┬────┘
    │              │
    ▼              ▼
 tokio::select!   单次 await
 SSE lines loop   直接解析 JSON
    │              │
    ▼              ▼
 StreamEvent     Value
 (data/aurora/   返回
  end/error)
```

### 3.2 多模态预处理（阶段 0）

当 `content` 为数组且包含 `{"type": "local_file", "path": "file://..."}` 时，模块执行以下转换：

| 扩展名 | MIME | part_type | 处理方式 | 降级策略 |
|--------|------|-----------|---------|---------|
| png/jpg/jpeg/webp/gif/bmp/heic/heif/avif | image | `image_url` | ffmpeg 转 webp，长边缩放到 ≤1120px | 保留文本占位 `[附件文件: {path}]` |
| mp4/webm/3gp/3g2/mov | video | `image_url` | 场景检测 + 均匀采样抽帧 → JPEG base64 | 同上 |
| mp3/wav/ogg/flac/aac/m4a/opus/amr | audio | `input_audio` | ffmpeg 提取 MP3/AAC（32kbps）→ base64 | 同上 |
| 其他 | application | `file_url` | 不支持多模态，直接降级为文本占位 | — |

关键实现细节：
- 路径清洗：`"file://"` 前缀被移除
- 文件不存在或读取失败时，**静默降级为文本占位**，避免内容完全丢失（`if !converted` 分支）
- 图片/视频/音频处理均在 `tokio::task::spawn_blocking` 中执行，避免阻塞 async 运行时
- 视频抽帧有**硬上限 300 帧**，防止极端长视频导致 OOM 或 API 超时

### 3.3 动态路由（阶段 1–2）

**动态路由**：
- 若 `enableVcpToolInjection = true`，强制将路径替换为 `/v1/chatvcp/completions`（工具增强路由）
- 否则调用 `normalize_vcp_url()`，确保 URL 以 `/v1/chat/completions` 结尾

> **v1.1.3 变更说明**：历史版本中 `vcp_client.rs` 曾直接读取 `music_state.json` / `songlist.json` 并向 System Message 注入音乐状态与 UI 规范。当前代码中该逻辑已移除，所有上下文注入（System Prompt、Tavern 规则、历史消息压缩等）由 `context_assembler.rs` 在调用 `perform_vcp_request` 之前完成。`vcp_client.rs` 仅负责将已组装好的 `messages` 数组原样序列化并发送。

- 若消息列表中无 System Message，当前实现仍会自动在头部插入空内容的 System 角色，以保持与部分旧版模型的兼容性。

### 3.4 流式处理模式（阶段 6）

**HTTP 客户端配置**：
- **不设 `read_timeout`**：数小时自循环场景中，`read_timeout` 是定时炸弹
- `tcp_keepalive(Duration::from_secs(20))`：维持 TCP 层活性，防止 NAT/防火墙静默丢弃空闲连接

**SSE 解析流水线**：

```rust
let stream = resp.bytes_stream().map_err(IoError::other);
let reader = StreamReader::new(stream);
let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));
```

1. `bytes_stream()`：将 HTTP 响应体转为字节流
2. `StreamReader`：将字节流转为 `AsyncRead`
3. `FramedRead + LinesCodec`：按行解码，最大行长度 512 KB（防止恶意长行撑爆内存）

**双重 `tokio::select!` 中止架构**：

```
第一层 select!（请求发送阶段）
├─ abort_rx 触发 → 请求尚未建立，直接返回 aborted
└─ response_res 到达 → 进入第二层

第二层 select!（SSE 读取循环内，两路并发）
├─ abort_rx 触发 → 深层轮询捕获中止
│   ├─ aurora_buffer.finalize()
│   ├─ 发送最终 aurora 事件（含 cancelled_by_user）
│   └─ break 循环
└─ lines.next() 到达 → 解析单行
    ├─ "data: [DONE]" → 正常结束
    ├─ "data: {...}" → 提取 delta.content，追加到 AuroraBuffer
    └─ Err/None → 错误处理或容错结束
```

> **注意**：代码中不存在独立的 SSE 空闲超时（如 25s）分支。长连接由 TCP keepalive（20s）与 Android SSE Helper 的本地代理重连机制共同维护。若连接意外断开，非 Android 桌面调试场景会进入 `Retrying` 状态进行最多 3 次退避重连；Android 生产场景由 `recover_active_generation` 在其内部执行 Helper 接续。

**Aurora 驱动节流**（v1.1.0 更新）：
- 每收到非空文本 chunk，追加到 pending_chunk 缓冲区
- 通过 `flush_aurora_parse` 闭包执行**双阈值互备节流**：
  1. **时间阈值**：距离上次 flush ≥ **33ms**（`AURORA_PARSE_INTERVAL_MS`）
  2. **字节量阈值**：pending_chunk 累积 ≥ **1024 字节**（`AURORA_FORCE_PARSE_BYTES`）
  3. 任一条件满足即触发 `aurora_buffer.process_queue()` → `send_aurora_update()`
  4. `force=true` 时（流结束/中止/超时）绕过双阈值，立即 flush
- 双阈值互备确保高频小 chunk 和低频大 chunk 都不会超阈值延迟

### 3.4.1 Backend-Driven SSE Thinking 事件

新增 `StreamEvent` 类型：`thinking`（由 `StreamEvent::thinking(message_id, context)` 构造）。该事件是 Backend-Driven Streaming 架构的核心：由后端在流式请求开始前主动创建消息骨架，前端仅负责接收并 hydrate，无需在调用 `sendToVCP` 前预创建 thinking 消息。

**数据流**：

```
agent_chat_application_service.rs
    group_chat_application_service.rs
           │
           ▼ StreamEvent::thinking(message_id, context)
    前端 chatHistoryStore
           │
           ▼ 创建 thinking 占位消息（is_thinking = true）
vcp_client.rs SSE 循环
           │
           ▼ StreamEvent::data / StreamEvent::aurora
    前端
           │
           ▼ hydrate 增量内容到已有占位消息
    StreamEvent::end
           │
           ▼ 标记 is_thinking = false，显示最终内容
```

**触发位置**：

| 场景 | 发射时机 | 源码位置 |
|------|---------|---------|
| 单聊 | 发起 `perform_vcp_request` 前 | `agent_chat_application_service.rs` |
| 群聊接力赛 | 每个 Agent 轮次开始前 | `group_chat_application_service.rs` |

**设计收益**：
- 前端 `chatHistoryStore.ts` 不再预创建 thinking 消息，避免消息 ID 不一致与重复创建问题
- 后端完全掌控消息生命周期：从骨架创建 → 增量填充 → 结束标记
- 群聊接力赛中，每个 Agent 的 thinking 占位可独立渲染，用户体验与单聊一致

### 3.5 非流式处理模式（阶段 7）

- 直接 `await` 完整响应
- 检查 HTTP status，非 2xx 返回错误
- 解析 JSON 后返回 `{"response": vcp_response, "context": context}`
- 不经过 Aurora 流水线，不发送中间事件

### 3.6 请求中止机制

**三层防护**：

| 层级 | 机制 | 作用 |
|------|------|------|
| L1 | `interruptRequest` Command | 外部触发：按 `message_id` 取得当前 attempt，只发送取消信号且保留 map owner 到任务真实退出 |
| L2 | `tokio::select!` 第一层 | 在 HTTP 请求发送前捕获：未建立连接时直接短路返回 |
| L3 | `tokio::select!` 第二层（深层轮询） | 在 SSE 读取循环内捕获：即使正在等待下一行数据，也能立即响应中止 |

**关键修复 — 深层轮询**：
- 早期实现仅在请求发送前检查 `abort_rx`，导致流已建立后无法中止
- 当前实现将 `abort_rx` 与 `lines.next()` 放入同一 `select!` 分支，确保**即使在 I/O 等待间隙也能捕获信号**

**并发安全**：
- `ActiveRequestLease::drop` 只清理 token 匹配的条目；不存在 key-only remove；
- 重复生成/恢复会得到 `already_running` 或 duplicate rejection，不会替换现有取消句柄；
- finalizer 成功提交前 lease 不释放，因此恢复扫描不会在网络结束与 DB commit 之间误启动第二个 attempt。

---

## 4. 公共接口（Tauri Commands）

### 4.1 sendToVCP

```rust
pub async fn sendToVCP<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    payload: VcpRequestPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String>
```

- **前端调用方式**：`invoke('sendToVCP', { payload, streamChannel })`
- `stream_channel` 为 `Channel<StreamEvent>`，支持服务端向前端推送事件流
- 流式模式下，函数返回后仍会通过 `stream_channel` 持续发送事件，直到 `end` 或 `error`
- 返回的 `Value` 在流式模式下包含 `{ fullContent, streamingStarted, finishReason }`
- 面向正式 Agent/Group 对话时，generation 的 pending/terminal 生命周期由 application service 的 `begin_stream_message` / `finalize_stream_message` 管理；`sendToVCP` 本身不再以 key-only 删除恢复记录。

### 4.2 interruptRequest

```rust
pub fn interruptRequest(
    state: tauri::State<'_, ActiveRequests>,
    message_id: String,
) -> Result<Value, String>
```

- **同步函数**（非 `async`）：`oneshot::Sender::send` 是立即的
- 若找到对应 `message_id`，发送信号后返回 `success: true`
- 若未找到（可能已结束或从未开始），返回错误 `"Request {id} not found"`

### 4.3 interruptGroupTurn

```rust
pub fn interruptGroupTurn(
    state: tauri::State<'_, CancelledGroupTurns>,
    topic_id: String,
) -> Result<Value, String>
```

- 将 `topic_id` 插入 `DashSet`，标记该话题的群聊接力赛应被取消
- 实际取消检查由 `group_chat_application_service.rs` 在编排循环中执行

### 4.4 test_vcp_connection

```rust
pub async fn test_vcp_connection(
    vcp_url: String,
    vcp_api_key: String,
) -> Result<Value, String>
```

- **对齐桌面端逻辑**：解析 URL 提取 `protocol://host:port`，拼接 `/v1/models`
- 使用 10 秒超时（与生产请求的"无 read_timeout"不同）
- 返回 `{ success, status, modelCount, models }`

---

## 5. 工具函数

### 5.1 normalize_vcp_url

```rust
pub fn normalize_vcp_url(url_str: &str) -> String
```

- 若 URL 路径不以 `/chat/completions` 结尾，自动追加 `/v1/chat/completions`
- 处理有/无尾部斜杠两种情况
- 若解析失败，原样返回输入字符串（容错）

### 5.2 load_app_settings

```rust
async fn load_app_settings<R: Runtime>(app: &AppHandle<R>) -> Result<Settings, String>
```

- 从 SQLite `settings` 表读取 `key = 'global'` 的记录
- 若不存在，返回 `create_default_settings()`
- 仅在 `perform_vcp_request` 内部使用，非公共接口

### 5.3 get_app_data_path

```rust
async fn get_app_data_path<R: Runtime>(app: &AppHandle<R>) -> PathBuf
```

- 调用 `app.path().app_data_dir()`，失败时回退到 `"AppData"`
- 用于定位 `music_state.json` 和 `songlist.json`

---

## 6. 错误处理与边界情况

### 6.1 流式场景下的容错

| 场景 | 行为 | 源码位置 |
|------|------|---------|
| SSE 流无 `[DONE]` 但已有内容 | 视为正常结束，发送 `aurora` + `end` 事件 | `line_res = None` 分支，含内容判断 |
| SSE 流无 `[DONE]` 且无内容 | 视为异常断开，发送 `error` 事件 | `line_res = None` 分支，无内容判断 |
| 单条 SSE 行解析 JSON 失败 | 静默跳过（仅打印日志），继续读取下一行 | `serde_json::from_str` 未用 `?` |
| 空行 | `continue` 跳过 | `line.trim().is_empty()` |
| HTTP 非 2xx | 读取响应体文本，发送 `error` 事件，返回 `Err` | `resp.status().is_success()` 否定分支 |

### 6.2 多模态预处理容错

- 文件不存在：降级为文本占位 `[附件文件: {path}]`
- `spawn_blocking` panic：捕获并打印，降级为文本占位
- ffmpeg 返回错误：打印日志，降级为文本占位
- 未知扩展名：统一使用 `("application", "file_url")`，最终降级为文本占位

### 6.3 设置读取容错

- `load_app_settings` 失败（如数据库锁定）不会阻断请求，三个布尔设置均默认 `false`
- `music_state.json` / `songlist.json` 读取失败或解析失败：静默跳过，不注入对应上下文

---

## 7. 性能特征与安全约束

### 7.1 性能

| 指标 | 数值/策略 | 说明 |
|------|----------|------|
| SSE 行缓冲区 | 512 KB | 防止极端长行导致内存爆炸 |
| Aurora 节流间隔 | 自适应：33 ms / 100 ms / 200 ms，按尾部长度分段 | 避免长文本尾部频繁触发小粒度解析 |
| Aurora 字节阈值 | 自适应：1024 B / 4096 B / 8192 B，按尾部长度分段 | 长文本尾部累积更多内容再 flush，降低 CPU |
| TCP Keepalive | 20 s | 维持长连接活性，避免 NAT 超时 |
| Android 代理帧 | 4 字节大端长度前缀 + JSON payload | `LengthDelimitedCodec` 封装，助手与客户端统一帧格式 |
| 图片长边限制 | 1120 px | 控制多模态 payload 大小 |
| 视频最大帧数 | 300 帧 | 防止极端视频导致 OOM/API 超时 |
| 视频去重阈值 | 1.5 秒 | 时间戳差小于此值视为重复帧 |

### 7.2 安全

- **不设 read_timeout 的设计决策**：明确注释说明这是为了支持数小时级别的自循环场景。风险由 TCP keepalive 和上层应用逻辑共同控制。
- **路径遍历防护**：多模态预处理仅读取 `path` 字段指定的文件，不做 `app_data_dir` 限制（因为附件可能来自任意位置），但前端在传入前已通过文件选择器限制范围。
- **内存安全**：视频抽帧使用 `MAX_FRAMES` 硬上限；SSE 行使用 `LinesCodec::new_with_max_length`；图片处理在 `spawn_blocking` 中执行，不阻塞异步运行时。

---

## 8. Android 流式代理与断点续传（v1.1.3）

### 8.1 为什么需要本地 SSE 代理

Android 端在 v1.1.3 引入独立的 SSE Helper 进程（由 `tauri-plugin-vcp-mobile` 的 `stream` 模块管理）：
- 当 App 切后台或被系统限制网络时，`reqwest` 长连接可能被 OEM 策略中断。
- Helper 以独立前台服务形式维持与 VCP 服务器的 SSE 连接，即使主进程短暂被杀也能继续接收 Token。
- 主进程通过本地 TCP 套接字（`127.0.0.1:<sse_helper.port>`）与 Helper 通信，避免直接持有可能被系统回收的长连接。

### 8.2 LengthDelimitedCodec 帧协议

主进程 ↔ Helper 的所有命令与事件均采用统一的长度前缀帧：

```rust
// 发送端（vcp_client.rs:705-739）
let cmd_str = cmd.to_string();
let cmd_bytes = cmd_str.as_bytes();
let len = cmd_bytes.len() as u32;
stream.write_all(&len.to_be_bytes()).await?;
stream.write_all(cmd_bytes).await?;
stream.flush().await?;

// 接收端（vcp_client.rs:969, 1062）
FramedRead::new(stream, LengthDelimitedCodec::new())
```

| 字段 | 长度 | 说明 |
|------|------|------|
| 帧长度 | 4 字节（u32 big-endian） | 后续 JSON payload 的字节数 |
| payload | 变长 | JSON 对象，包含 `action`、`requestId`、`eventType`、`eventData`、`index` 等字段 |

### 8.3 命令协议

主进程向 Helper 发送的命令：

| action | 参数 | 用途 |
|--------|------|------|
| `start` | `url`, `headers`, `body`, `context` | 启动新的 SSE 会话 |
| `stop` | `requestId` | 通知 Helper 停止指定会话并释放资源 |
| `resume` | `startIndex` | 从指定事件索引续接已有会话 |
| `query` | `requestId` | 查询 Helper 内存中会话当前状态（`recover_active_generation` 使用） |

Helper 向主进程回传的事件帧：

| eventType | eventData | 含义 |
|-----------|-----------|------|
| `message` | SSE `data:` 内容（含 `[DONE]`） | 正常 Token 帧 |
| `closed` | — | 服务器关闭连接 |
| `error` | JSON `{ error }` | 代理层错误，立即失败 |

### 8.4 活跃生成注册表联动

`active_generations` 与空 pending 消息由 `message_service::begin_stream_message` 在一个事务内 insert-only 创建；事务提交后才向前端发 `thinking`。普通 `append_single_message` 不再根据 assistant/finish_reason 推断生成状态。

| 场景 | 行为 | 代码位置 |
|------|------|---------|
| 正常生成 | request lease 一直覆盖到 `finalize_stream_message` 原子提交 | Agent/Group application service |
| `recover_active_generation` | 先 claim attempt，再依次检查 DB pending、`sse_cache`、Helper，并在同一 lease 内完成续接/终态提交 | `vcp_client.rs` |
| terminal 已存在 | 返回已有 content/finishReason，迟到恢复不得反向改写 error | `recover_active_generation`, `mark_message_as_error` |

### 8.5 恢复流程

```text
recover_active_generation(msg_id)
    │
    ├─ try_acquire attempt 失败 → 返回 { status: "already_running" }
    ├─ DB 已无 pending active row → 幂等返回 terminal / not_found
    │
    ├─ 清理超过 24h 的本地 sse_cache 文件
    │
    ├─ 读取本地 sse_recovered_{hash}.json（24h 内有效）
    │   └─ 命中 → finalize_stream_message → 返回 completed
    │
    ├─ Android: 通过 TCP 向 Helper 查询会话状态
    │   ├─ completed → finalize → 返回 completed
    │   ├─ streaming → 同一 command 内 resume_claimed_generation
    │   └─ not_found → 继续下一步
    │
    └─ 在 pending 仍存在时原子标记 error；若其他 owner 已提交则幂等 no-op
       └─ 返回 { status: "failed" }
```

---

## 9. 与相关模块的关系

```
                    ┌─────────────────┐
                    │   vcp_client.rs  │
                    │   (本模块)        │
                    └────────┬────────┘
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
        ▼                    ▼                    ▼
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│ media_processor│   │aurora_pipeline│   │content_parser │
│   (多模态编码)  │   │ (语义沉淀管道) │   │ (块类型定义)  │
└───────────────┘   └───────────────┘   └───────────────┘
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
        ▼                    ▼                    ▼
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│ agent_chat_   │   │ group_chat_   │   │ topic_summary_│
│ application   │   │ application   │   │ service       │
│ _service      │   │ _service      │   │               │
└───────────────┘   └───────────────┘   └───────────────┘
```

- **→ media_processor/**：调用期依赖。`vcp_client.rs` 在请求预处理阶段调用 `convert_local_image_for_multimodal`、`process_video_for_multimodal`、`process_audio_for_multimodal`。
- **→ aurora_pipeline.rs**：调用期依赖。`vcp_client.rs` 在流式循环中驱动 `AuroraBuffer`，将 `AuroraUpdate` 包装为 `StreamEvent::aurora` 推送到前端。详见 [10_Aurora语义沉淀管道](10_Aurora语义沉淀管道.md)。
- **→ content_parser.rs**：类型依赖。`StreamEvent.blocks` 字段类型为 `Option<Vec<ContentBlock>>`。`ContentBlock` 的 9 种变体定义了前端渲染的原子单元。详见 [02_流式响应解析器](02_流式响应解析器.md) §1.2 中的对比表。
- **→ settings_manager.rs / db_manager.rs**：设置读取依赖。通过 `load_app_settings` 查询 SQLite。
- **← agent_chat_application_service.rs / group_chat_application_service.rs**：内部调用者。直接调用 `perform_vcp_request` 而非走 Tauri IPC，以实现 Rust 层编排逻辑。同时，这两个模块也是 `StreamEvent::thinking` 的发射源（详见 §3.4.1）。

---

## 10. 动态心跳与前后台状态（vcp_log_service）

`vcp_log_service.rs` 与 `vcp_client.rs` 同属 `infra/` 领域，负责 WebSocket 日志通道的生命周期管理。v1.1.3 将心跳自适应逻辑从**前端主动调用**迁移到**后端根据生命周期状态自动调整**，减少前后台切换时的竞态窗口。

### 10.1 运行时心跳配置

保留 Tauri Command 供手动/调试覆盖：

```rust
// src-tauri/src/vcp_modules/infra/vcp_log_service.rs:89
#[tauri::command]
pub async fn set_vcp_log_heartbeat(interval_ms: u64) -> Result<(), String>
```

- **默认值**：`HEARTBEAT_INTERVAL_MS = 15000`（AtomicU64，单位毫秒）
- **动态生效**：调用后立即通过 `HEARTBEAT_RESET_TX` mpsc 通道向 WebSocket 监听循环发送重置信号
- 监听循环内通过 `tokio::select!` 捕获 `reset_rx.recv()`，读取最新原子值后重新校准 `heartbeat_timer`（`tokio::time::Sleep::reset`），无需断开重连

> **注意**：v1.1.3 前端（`App.vue` / `useAppLifecycle`）不再在生命周期切换时调用此命令。日常运行的心跳调整由 Rust 侧的 `handle_foreground_state_change` 自动完成。

### 10.2 前后台自适应（后端自动）

Rust 生命周期控制器在收到 `set_app_foreground_state` 调用时，会同步调用 `vcp_log_service::handle_foreground_state_change`：

```rust
// src-tauri/src/vcp_modules/infra/vcp_log_service.rs:27
pub async fn handle_foreground_state_change(_app: &AppHandle, is_foreground: bool) {
    let heartbeat_ms = if is_foreground { 15000 } else { 120000 };
    HEARTBEAT_INTERVAL_MS.store(heartbeat_ms, Ordering::SeqCst);
    {
        let tx_lock = HEARTBEAT_RESET_TX.lock().await;
        if let Some(tx) = tx_lock.as_ref() {
            let _ = tx.send(()).await;
        }
    }
}
```

| 生命周期状态 | 心跳间隔 | 设计意图 |
|-------------|---------|---------|
| 前台 (`is_foreground = true`) | 15000 ms | 保持连接活性，确保日志与系统通知实时到达 |
| 后台 (`is_foreground = false`) | 120000 ms | 降低功耗与网络占用，减少 OEM 杀后台概率 |

触发链路：

```
Kotlin LifecycleBridge / App.vue visibilitychange
    │
    ▼
Rust lifecycle_controller::set_app_foreground_state(is_foreground)
    │
    ▼
vcp_log_service::handle_foreground_state_change(is_foreground)
    │
    ▼
HEARTBEAT_INTERVAL_MS 原子更新 → WebSocket 循环 reset 心跳定时器
```

### 10.3 后台日志缓存

为避免后台期间 WebView 积压消息，`vcp_log_service.rs` 在 `emit_log_event` 中增加了后台缓存逻辑：

```rust
// src-tauri/src/vcp_modules/infra/vcp_log_service.rs:59
fn emit_log_event<R: tauri::Runtime>(app: &AppHandle<R>, payload: serde_json::Value) {
    if !crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(app) {
        if let Ok(mut cache) = BACKGROUND_LOG_CACHE.lock() {
            cache.push(payload);
        }
        return;
    }
    let _ = app.emit("vcp-system-event", payload);
}
```

- 应用处于后台时，日志消息缓存在 Rust 侧 `BACKGROUND_LOG_CACHE`，不直接推送到 WebView
- 返回前台后通过 `flush_background_logs` 一次性补发，防止内存泄漏和前端消息积压

### 10.4 性能与稳定性收益

- **后台降频**：120s 心跳使 WebSocket 在后台维持最低限度的 NAT/防火墙保活，避免高频 Ping 唤醒 CPU 与射频模块
- **快速恢复**：前台切回时 15s 间隔立即恢复，日志通道无需重新握手
- **状态一致性**：心跳调整与生命周期状态在同一 Rust 调用链中完成，避免前端调用 `set_vcp_log_heartbeat` 与 `vcp-lifecycle-changed` 事件之间的时序窗口
- **与 StreamKeepaliveService 协同**：主进程前台保活服务维持进程存活，`:helper` 进程 SSE 代理维持流连接，自适应心跳降低网络层消耗

---

*最后更新：2026-07-04 | VCP Mobile v1.1.3*
*文档基于 `src-tauri/src/vcp_modules/infra/vcp_client.rs`（约 2100 行）及 `src-tauri/src/vcp_modules/chat/aurora_pipeline.rs`（~237行）的源码分析生成。*
