---
id: MOD-VCP-CLI-009
version: "1.1.5"
date: 2026-08-28
module: vcp_client.rs
scope: src-tauri/src/vcp_modules/
related: [aurora_pipeline.rs, media_processor/, content_parser.rs, agent_chat_application_service.rs, group_chat_application_service.rs, message_service.rs]
---

# 09_VCP 请求客户端（VCP Client）

## 1. 概述

### 1.1 模块定位

`vcp_client.rs` 是 VCP Mobile 核心层（Rust 后端）的**统一 VCP 请求处理模块**，位于 `src-tauri/src/vcp_modules/infra/vcp_client.rs`（约 2100 行）。该模块对应原桌面端项目的 `modules/vcpClient.js`，负责处理所有与 VCP 服务器的通信，是前端对话引擎与后端网络层之间的唯一 HTTP 出入口。

> **2026-10-17 生命周期收口**：前端统一调用 `reconnect_chat_streams`，Rust 协调活动 request、群聊换棒、helper 与离线结果。原 `get_active_generations`、`recover_active_generation` 及无消费者的 `sendToVCP` 不再是公开 IPC；内部恢复仍从 claim 到持久提交持有同一 attempt lease，不重发上游请求。正文只有生产者 buffer 一份权威流状态，消费者更换在该生产者锁下要求先 snapshot 后 delta。本文其余旧行号、事件索引及 `sse_cache` 描述属于历史实现；本轮具体契约和验证见 [阶段五记录](../../plan/vcpmobile-ios-compatibility-research/phase-5/01-SSE统一接管与操作边界重构.md)。

> 2026-10-17 资源收尾：网络层以 `VcpResponse` 移交正文、结束原因和取消状态，单聊/群聊/恢复直接把正文移动到 finalizer；内部恢复返回 `RecoveryStatus`，权威正文仍只经原终态 Channel 交付。渲染缓存采用 JSON 直接写入 zstd 编码器，避免完整未压缩 JSON 中间缓冲；worker 输入与终态事件的共享优化未实施。范围及验证见 [资源收尾记录](../../plan/vcpmobile-ios-compatibility-research/phase-5/02-SSE资源收尾重构.md)。

已经取得成功、取消或错误终态、但 SQLite 提交失败时，没有 helper 交接副本的路径由现有 `RequestOutput.pending_terminal` 保留共享的 `PendingStreamTerminal`（正文、结束原因、Agent 身份）。错误入口由调用方传入已有的发言者身份，在首次数据库访问前保留含原错误原因的正文；是否仍待提交、已经结束或被删除，统一由终态事务裁定。生成 lease 退出后只保留这一待提交 owner；同进程恢复先重新认领并重试原终态提交，不重新 POST，也不把已有正文替换为空的“后台进程销毁”错误。提交成功、已有持久终态、消息移除或显式删除后释放该 owner；晚到错误不计为新的群成员发言完成。Android 已有 helper handoff 或完成文件继续由其原有交接路径持有，不额外保留第二份。此机制只保留已取得的终态，不增加生成期间正文落盘，也不改变进程死亡后未持久化正文可能丢失的边界。

Android helper 的 HTTP 失败诊断保留状态码，异常和 reason phrase 只使用非空文本；HTTP/2 没有 reason phrase 时仍能区分 401、429、500 等失败。错误正文不另行读取，终态与资源交接流程不变。

其核心职责包括：
- 使用 Rust 设置快照冻结本次 turn 的最终 Chat 端点并组装 HTTP 请求
- 在请求预处理阶段完成**多模态本地文件编码**（图片/视频/音频 → data URL）
- 根据用户设置执行**动态路由切换**与**上下文注入**（音乐状态、UI 规范）
- 支持**流式（SSE）**与**非流式**双模式响应处理
- 通过 `tokio::sync::oneshot` 实现全链路**请求中止机制**（含深层轮询捕获）
- 向前端推送标准化的 `StreamEvent` 事件序列

请求准备由一个可取消工作槽覆盖多模态处理与最终序列化，完成后释放，不占用整轮网络时长。音频 24 MiB、视觉 16 MiB 均按编码后的 data URL 字节计数；完整 HTTP JSON 通过有界 writer 写入私有文件，最终上限为 128 MiB。Android 控制帧只发送正文 token 与长度，Helper 原子认领文件并分块上传；Rust 直连传输以只读文件映射提供可重放正文，保留 307/308 重定向语义，不另建完整字符串或字节数组副本。正文文件随实际上传 owner 结束清理，与响应帧/离线结果的生命周期分开。

### 1.2 职责边界

| 职责领域 | 具体行为 | 对应源码位置 |
|---------|---------|------------|
| 请求参数序列化 | `VcpRequestPayload` 的 Rust 类型校验与 JSON 组装 | `VcpRequestPayload:30` |
| 多模态预处理 | 识别 `local_file` 类型，按扩展名分发到图片/视频/音频处理器 | `perform_vcp_request:241` |
| Chat 端点解析 | `ChatEndpointMode` + 请求用途经唯一 resolver 派生标准、ChatVCP 或 Raw 端点 | `resolve_chat_endpoint` |
| 上下文注入 | 读取 `music_state.json`、`songlist.json`，注入 System Message | `perform_vcp_request:403` |
| 流式 SSE 解析 | 使用 `LinesCodec` + `tokio::select!` 逐行解析 `data:` 事件 | `perform_vcp_request:536` |
| Aurora 语义沉淀驱动 | 每收到文本 chunk 追加到 `AuroraBuffer`，触发增量块解析与推测渲染 | `perform_vcp_request:591` |
| 请求中止 | `ActiveRequests` + attempt UUID + `CancellationToken` + RAII lease；pending 表保留明确停止意图 | `ActiveRequestEntry`, `ActiveRequestLease`, `interruptRequest` |
| 连接测试 | 对齐桌面端逻辑的 `/v1/models` 探测与模型计数 | `test_vcp_connection:762` |
| 活跃生成恢复 | 统一绑定当前界面输出，按 pending、live owner 与离线 mailbox 安排对账 | `reconnect_chat_streams` |
| 流式接管 | 内部恢复在同一 attempt 内接收 helper 的完整前缀快照，再消费后续增量 | `resume_claimed_generation`, `handle_streaming_request` |
| 助手进程通信 | `LengthDelimitedCodec` 帧协议：4 字节大端长度 + JSON payload | `send_command_to_stream:705`, `connect_to_helper:612` |

### 1.3 调用入口

```text
Vue 3 前端（对话引擎）
    ↓ Tauri IPC: handle_agent_chat_message / handle_group_chat_message
Rust application service
    ↓ 读取一次 Settings，freeze_chat_connection
vcp_client.rs（接收已冻结最终端点）
    ↓ perform_vcp_request
    ├─→ media_processor/ （多模态文件编码）
    ├─→ Android SSE Helper（复用同一最终 URL）
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
topic_summary_service.rs ──→ resolve_chat_endpoint(Auxiliary) ──→ 标准/Raw 辅助请求
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

### 2.5 ActiveGroupTurns

```rust
pub struct ActiveGroupTurns(Arc<DashMap<TopicKey, CancellationToken>>);
```

- 键：完整 `TopicKey(owner_type, owner_id, topic_id)`
- 回合入口通过 lease 原子取得所有权；同一 Topic 的重复回合会被拒绝
- `interruptGroupTurn` 只取消当前所有者，lease 退出时释放条目

---

## 3. 核心流程详解

### 3.1 整体请求生命周期

```
前端调用 handle_agent_chat_message / handle_group_chat_message
           │
           ▼
    ┌──────────────┐
    │ 0. 数据验证   │ ← 过滤非对象消息，处理 content 数组
    │    与规范化   │   local_file → data URL（多模态预处理）
    └──────┬───────┘
           │
           ▼
    ┌──────────────┐
    │ 1. 冻结设置   │ ← ChatEndpointMode + 原始 URL + API Key
    │    与最终端点 │   同一群聊 turn 的所有接力成员复用该快照
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

当 `content` 数组包含 `{"type": "local_file", "hash": "..."}` 时，Rust 解析本机 CAS 记录，按实际 MIME 选择处理器；主机路径不进入模型消息。

| MIME 类别 | 输出 | 处理方式 | 失败反馈 |
|---|---|---|---|
| image | `image_url`，真实 JPEG/PNG Data URI | 既有双端原生用途派生，图片实现保持不变 | 图片处理错误返回现有失败链 |
| video | 多个 `image_url`，JPEG Data URI | Android 系统缩放抽帧；iOS AVAssetImageGenerator，最多 300 帧 | 抽帧、读取或预算失败阻止本次请求，不用文件名代替内容 |
| audio | `image_url`，`data:audio/mp4;base64,...` | Android Media3 / iOS 原生转换为 AAC-LC、16 kHz、单声道、32 kbps，封装 M4A | 无原件直读回退；明确指出失败附件 |

音频使用 VCP 桌面端及服务器多模态处理器已有的媒体 Data URI 契约；不是标准 OpenAI `input_audio`，不据此承诺所有直连模型均接收音频。编码、容器和 MIME 由原生结果与文件核对，不再把所有非 AAC 标成 MP3。

音视频资源 worker 持有原生调用的接收端，执行登记、处理、取消、最终回调及清理。请求 CancellationToken 控制停止，准备结束后再次检查取消，未取消才发送网络请求。已保存的用户消息和附件保留。群聊成员的附件准备或模型请求失败时，Rust 将错误写入该成员的消息，再继续调度下一位成员。预算、缓存版本及临时资源细节见[双端实施记录](../../plan/vcpmobile-ios-compatibility-research/phase-5/05-双端音视频附件处理.md)。

### 3.3 Chat 端点路由（阶段 1–2）

路由决策只由 `resolve_chat_endpoint` 持有：

| 模式 | 交互请求（Agent / 群聊 / 助手 / 模型测试） | 辅助请求（话题总结） |
|------|------------------------------------------------|----------------------|
| `standard` | 补全或替换为 `/v1/chat/completions` | 标准 Chat |
| `vcpTools` | 补全或替换为 `/v1/chatvcp/completions` | 标准 Chat |
| `raw` | 完整 URL 原样使用 | 完整 URL 原样使用 |

resolver 位于 `infra/vcp_http_endpoint.rs`，仅接受带 host 的 HTTP/HTTPS，拒绝用户信息、控制字符与 fragment；保留端口、query 和反向代理路径前缀。`/v1` 被视为 API 基址，已知 Chat 末尾只替换末尾段。正式请求、重试与 Android SSE Helper 均复用 turn 起点冻结的同一最终 URL。

该纯模块也拥有 `resolve_service_base`、`resolve_http_origin` 和 `append_url_segments`。管理 API 与日记从同一基址规则剥离 `/v1`、标准或增强聊天后缀，保留部署前缀，但不继承 Chat 配置中的 query；各操作继续自行添加查询参数。Raw 根地址、`/v1` 或已知聊天端点可推导管理基址，未知 Raw 路径明确返回不可推导，主聊天仍按原地址请求。Raw 模型发现继续遵守原有更窄的规则，只从已知完整聊天端点派生。

表情服务显式使用 `resolve_http_origin`，列表与图片均位于服务器根路径，不扩展子路径部署。路径段 API 接收原始值并编码一次，拒绝会被静默跳过的 `.` / `..` 段。HTTP 鉴权与请求执行留在各消费者：管理 Basic、Human Tool Bearer、图片 `pw=` 路径凭证；WebSocket 独立使用 `vcp_ws_endpoint.rs` 的协议规则。

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

### 4.1 reconnect_chat_streams

首次 `invoke('reconnect_chat_streams', { connectionId, channel })` 登记当前 WebView 的输出；后续同界面只传 `connectionId`，复用 Rust 持有的 Channel 实例。可选 Channel 由 `JavaScriptChannelId` 延迟创建，避免 Tauri 在重复反序列化时重置同一 JS 回调的消息序号。

命令只返回活动消息/群轮的对账收据，不返回正文，也不等待生成结束。各条生成的接管状态、快照、增量和持久终态经 Channel 投递；前端不按收据再调用单消息恢复。新界面更换消费者，原 request owner 保留；旧连接的迟到工作不能重新取得输出所有权。

正式生成继续通过 Agent/Group application service 建立 pending 消息并调用 finalizer。`sendToVCP` 无实际消费者，其专用包装及命令注册已移除。

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
    state: tauri::State<'_, ActiveGroupTurns>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<Value, String>
```

- 取消完整 Topic 身份下的当前群聊回合；没有活动回合时返回 `not_running`
- 编排循环在每位成员开始前检查该回合自己的取消令牌

### 4.4 test_vcp_connection

```rust
pub async fn test_vcp_connection(
    vcp_url: String,
    vcp_api_key: String,
    chat_endpoint_mode: ChatEndpointMode,
) -> Result<Value, String>
```

- 通过中央 resolver 派生 `/v1/models`，保留反向代理前缀、端口与 query
- `raw` 仅在完整 URL 以已知 Chat 端点结尾时安全派生；否则返回 `modelDiscoveryAvailable=false`，不把它误报为 Chat 连接失败
- 使用 10 秒超时（与生产请求的"无 read_timeout"不同）
- 返回 `{ success, status, modelCount, models, modelDiscoveryAvailable }`

---

## 5. 工具函数

### 5.1 resolve_chat_endpoint

```rust
pub fn resolve_chat_endpoint(
    raw_url: &str,
    mode: ChatEndpointMode,
    purpose: ChatRequestPurpose,
) -> Result<String, String>
```

- 是业务 Chat 路径改写的唯一 owner；`utils.rs` 与 `model_manager.rs` 不再各自拼路由
- Raw 模式在通过安全校验后逐字返回原 URL
- 标准模式和 VCP 增强模式保留代理前缀，并识别 `/v1` 与两个已知 Chat 末尾

### 5.2 freeze_chat_connection

```rust
pub fn freeze_chat_connection(
    settings: &Settings,
    purpose: ChatRequestPurpose,
) -> Result<ChatConnectionSnapshot, String>
```

- 将设置中的原始 URL、枚举模式与 API Key 固化为请求快照
- Agent、群聊、邀约、悬浮助手和模型测试均在 Rust 命令起点调用；网络重试不再重读设置

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
| 请求明确失败 | 写入简短错误原因并提交 `finish_reason=error`，同时删除 active row | Agent/Group application service, `mark_message_as_error` |
| 统一接管 | live owner 只更换消费者；缺 owner 的 pending/离线结果由内部恢复认领 lease 后收尾 | `reconnect_chat_streams`、内部 `recover_active_generation` |
| terminal 已存在 | 向当前消费者发送持久 content/finishReason/timestamp，迟到结果不覆盖数据库 | `emit_persisted_terminal`、`finalize_stream_message` |

### 8.5 恢复流程

```text
reconnect_chat_streams(connectionId, 首次 channel)
    │
    ├─ 活动请求/群轮 → 绑定当前消费者，后续发言继承
    ├─ pending / 离线文件 → 安排由 request lease 持有的内部恢复
    │   ├─ 离线结果 → 原 finalizer 持久提交
    │   ├─ helper 存活 → 前缀 snapshot → 后续 delta → terminal
    │   ├─ 已请求停止 → 对账并取消，不恢复继续生成
    │   ├─ 明确 not_found → 再查 mailbox 后按真实中断/停止收尾
    │   └─ 暂时不可达 → 保留 pending，Channel 告知待重试/停止待确认
    └─ 返回活动收据；各消息持久 end 仍经同一 Channel 投递
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

## 10. VCPLog 与 VCPInfo 的连接策略

两条 WebSocket 共用 Rust 的 `vcp_ws_activity`。单聊从请求准入到释放持续持有聊天计数，群聊从取得整轮租约到结束持有一次，覆盖非流式回复、JEV 决策和发言间隙。语音提交使用同一组请求和群轮。Android helper 的断开接管沿用其活动观察。

| 状态 | 两条 WS 的行为 |
|------|---------------|
| 前台，或后台仍有活跃聊天 | 定期发送带序号的 Ping 并核对 Pong。失活后立即尝试替换一次；持续握手失败或接通即断时按退避策略重试。 |
| 后台且最后一个聊天已结束 | 保留已连接的 socket，继续接收服务端消息，暂停主动 Ping 和重连。 |
| 回到前台，或原生网络报告恢复可用且仍需连接 | 唤醒退避，复用正在进行的握手。已有连接立即探活。Ping 写出后等待匹配 Pong 5 秒，调度暂停时先处理已经到达的帧。 |

Android 和 iOS 使用同一连接策略。Android 聊天租约与 SSE helper、iOS 聊天音频租约继续由相应任务管理。日志连接本身不申请原生保活资源。

Android 系统若限制应用 UID 的后台网络，前台服务运行期间也可能同时失去 WebSocket 和聊天 HTTP 连接。两条 WebSocket 恢复后可以继续接收日志和召回信息。若聊天 HTTP 在首个响应块前关闭且服务端已经取消生成，客户端保留该次请求的真实错误。

### 10.1 后台日志缓存

后台收到的日志和审批进入 Rust 有界缓存，回前台后交给 WebView。连接状态根据当前快照刷新，以免缓存中的旧状态覆盖已恢复的连接。

---

*最后更新：2026-07-04 | VCP Mobile v1.1.3*
*文档基于 `src-tauri/src/vcp_modules/infra/vcp_client.rs`（约 2100 行）及 `src-tauri/src/vcp_modules/chat/aurora_pipeline.rs`（~237行）的源码分析生成。*
