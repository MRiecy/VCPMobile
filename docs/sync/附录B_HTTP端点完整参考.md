---
title: 附录B - HTTP端点完整参考
scope: 双端
version: 0.9.13
last_updated: 2026-05-13
---

# 附录B - HTTP 端点完整参考

> 本附录以纯参考表格式列出同步协议中全部 HTTP REST 端点。所有路径均挂载于桌面端插件的 `/api/mobile-sync` 前缀之下。移动端通过 `settings.sync_http_url` 拼接完整 URL。
> 
> **认证方式**：统一使用 `x-sync-token` Header 或 `Authorization: Bearer <token>`。桌面端中间件（`routes.js`）同时兼容 Query 参数 `?token=`，但移动端仅发送 Header。

---

## 表1：实体端点

| 路径 | 方法 | 认证 | 请求格式 | 响应格式 | Body限制 | 移动端调用函数 | 桌面端处理函数 | 对应代码文件 |
|-----|------|-----|---------|---------|---------|-------------|-------------|------------|
| `/download-entity` | `GET` | `x-sync-token` | Query: `?id=<uuid>&type=agent\|group\|agent_topic\|group_topic` | `JSON` — 对应 DTO 对象 | — | `PullExecutor::pull_agent`<br>`PullExecutor::pull_group`<br>`PullExecutor::pull_agent_topic`<br>`PullExecutor::pull_group_topic` | `downloadEntity` | `routes.js`<br>`pull_executor.rs` |
| `/download-entities` | `POST` | `x-sync-token` | `JSON` — `{ requests: [{id, type}, ...] }` | `JSON` — `[{id, type, data}, ...]` | 无显式限制<br>（Express json 默认） | `PullExecutor::pull_entities_batch` | `downloadEntities` | `routes.js`<br>`pull_executor.rs` |
| `/upload-entity` | `POST` | `x-sync-token`<br>`x-idempotency-key` | `JSON` — `{ id, type, data }` | `JSON` — `{ success, id, hash? }` | 5 MB | `PushExecutor::push_agent`<br>`PushExecutor::push_group` | `uploadEntity` | `routes.js`<br>`push_executor.rs` |
| `/upload-entities-batch` | `POST` | `x-sync-token` | `JSON` — `{ items: [{id, type, data}, ...] }` | `JSON` — `{ success: true, results: [...] }` | 10 MB | `PushExecutor::push_entities_batch` | `uploadEntitiesBatch` | `routes.js`<br>`push_executor.rs` |

### 1.1 GET /download-entity

**桌面端处理流程**
1. 从 Query 解析 `id` 与 `type`（`agent` / `group` / `agent_topic` / `group_topic`）。
2. 调用 `downloadEntity({ id, type })` 查询数据库并序列化为 DTO。
3. 若记录不存在，返回 `404`；成功返回对应 JSON DTO。

**DTO 类型映射**
| type | 移动端反序列化类型 | 关键字段 |
|------|------------------|---------|
| `agent` | `AgentSyncDTO` | `id`, `name`, `model`, `systemPrompt`, `temperature`, `tools`, `createdAt`, `updatedAt` |
| `group` | `GroupSyncDTO` | `id`, `name`, `description`, `agentIds`, `createdAt`, `updatedAt` |
| `agent_topic` | `AgentTopicSyncDTO` | `id`, `name`, `createdAt`, `locked`, `unread`, `ownerId` |
| `group_topic` | `GroupTopicSyncDTO` | `id`, `name`, `createdAt`, `ownerId` |

**移动端调用示例**
```rust
let url = format!("{}/api/mobile-sync/download-entity?id={}&type=agent", http_url, agent_id);
let res = client.get(&url)
    .header("x-sync-token", sync_token)
    .header("Authorization", format!("Bearer {}", sync_token))
    .send().await?;
let dto: AgentSyncDTO = res.json().await?;
```

### 1.2 POST /download-entities

**请求体结构**
```json
{
  "requests": [
    { "id": "uuid-1", "type": "agent" },
    { "id": "uuid-2", "type": "group_topic" },
    { "id": "uuid-3", "type": "agent_topic" }
  ]
}
```

**响应体结构**
```json
[
  { "id": "uuid-1", "type": "agent", "data": { /* AgentSyncDTO */ } },
  { "id": "uuid-2", "type": "group_topic", "data": { /* GroupTopicSyncDTO */ } }
]
```

**批量分块策略**
移动端在 `sync_service.rs` 中按实体类型分块发送：Agent/Group 每块 50 个，Topic 每块 1000 个。桌面端 `downloadEntities` 并行查询后按原顺序（或聚合顺序）返回结果数组。

### 1.3 POST /upload-entity

**请求体结构**
```json
{
  "id": "uuid",
  "type": "agent",
  "data": { /* 对应 SyncDTO 的 JSON 表示 */ }
}
```

**幂等性机制**
- 移动端在 Header 中附加 `x-idempotency-key`，桌面端通过 `checkIdempotency(opId)` 检查是否重复。
- 键值生成算法（`push_executor.rs`）：`SHA256(action + entity_type + id + minute_timestamp)`。
- 若重复，桌面端直接返回缓存结果，不做数据库写入。

**响应体结构**
```json
{ "success": true, "id": "uuid", "hash": "sha256-hex" }
```

### 1.4 POST /upload-entities-batch

**请求体结构**
```json
{
  "items": [
    { "id": "uuid-1", "type": "agent_topic", "data": { ... } },
    { "id": "uuid-2", "type": "group_topic", "data": { ... } }
  ]
}
```

**用途说明**
该端点主要用于 Phase 2 的 Topic 元数据批量推送（归口优化）。桌面端 `uploadEntitiesBatch` 接收数组后逐个写入数据库，返回聚合结果。若 `items` 非数组，返回 `400`。

---

## 表2：消息端点

| 路径 | 方法 | 认证 | 请求格式 | 响应格式 | Body限制 | 移动端调用函数 | 桌面端处理函数 | 对应代码文件 |
|-----|------|-----|---------|---------|---------|-------------|-------------|------------|
| `/download-messages-stream` | `POST` | `x-sync-token` | `JSON` — `{ requests: [{topicId, msgIds: []\|null}, ...] }` | `NDJSON` 流 — 逐 topic 分帧 | 5 MB<br>（请求体） | `PullExecutor::pull_messages_batch` | `downloadMessagesStreamRaw` | `routes.js`<br>`pull_executor.rs` |
| `/upload-messages-batch` | `POST` | `x-sync-token` | `NDJSON` 流 — 逐 topic 分帧 | `NDJSON` 流 — `{topicId, success, neededAttachmentHashes?, error?}` | 无显式限制 | `PushExecutor::push_messages_batch` | `uploadMessagesBatchRaw` | `routes.js`<br>`push_executor.rs` |

### 2.1 POST /download-messages-stream

**请求体结构**
```json
{
  "requests": [
    { "topicId": "topic-uuid-1", "msgIds": ["msg-1", "msg-2"] },
    { "topicId": "topic-uuid-2", "msgIds": [] }
  ]
}
```

- `msgIds` 为空数组时，桌面端返回该 topic 的全部消息。
- `msgIds` 为具体 ID 列表时，仅返回指定消息（增量拉取场景）。

**响应格式：NDJSON**
桌面端以换行符分隔的 JSON（NDJSON）逐 topic 返回，每行一个对象：

```ndjson
{"topicId":"topic-uuid-1","messages":[/* ChatMessage 数组 */]}
{"topicId":"topic-uuid-2","messages":[/* ... */]}
```

**移动端消费流程**
1. `pull_executor.rs` 建立 HTTP POST 连接后，通过 `res.bytes_stream()` 流式读取 chunk。
2. 使用缓冲区逐行解析 NDJSON，支持 chunk 边界跨越。
3. 每解析出一行 topic 数据，通过 `Semaphore(20)` 控制并发，spawn 异步任务调用 `process_topic_messages()`。
4. 单 topic 处理失败不中断流，错误通过 `_error` 字段返回。

**字段规范化**
在 `process_topic_messages` 中，桌面端原始消息会经过以下规范化：
- `isThinking`: `0/1` → `bool`
- `isGroupMessage`: `0/1` → `bool`
- `timestamp`: 字符串数字 → `u64`
- 附件 `size`: `i64` → `u64`

### 2.2 POST /upload-messages-batch

**请求格式：NDJSON**
移动端直接上传换行分隔的 JSON，无需先序列化为 JSON 数组：

```ndjson
{"topicId":"topic-uuid-1","messages":[{"id":"msg-1","role":"user",...},{...}]}
{"topicId":"topic-uuid-2","messages":[...]}
```

Content-Type 设置为 `application/x-ndjson`。

**响应格式：NDJSON**
```ndjson
{"topicId":"topic-uuid-1","success":true,"neededAttachmentHashes":["hash1","hash2"]}
{"topicId":"topic-uuid-2","success":false,"error":"Topic not found on desktop"}
```

**附件后置上传策略**
1. 移动端解析响应中所有 `neededAttachmentHashes`。
2. 与本地 `uploaded_hashes`（`Arc<RwLock<HashSet<String>>>`）去重。
3. 按每批 3 个并发调用 `upload_attachment()`（POST `/upload-attachment`）。
4. 上传成功后写入 `uploaded_hashes`，避免同一会话重复上传。

**DTO 构建逻辑**
移动端根据 `owner_type` 将 `ChatMessage` 转为不同 DTO：
- `role == "user"` → `UserMessageSyncDTO`
- `owner_type == "group"` → `GroupMessageSyncDTO`（需查询 `avatar_color`）
- 否则 → `AgentMessageSyncDTO`（需查询 `avatar_color`）

---

## 表3：附件与头像端点

| 路径 | 方法 | 认证 | 请求格式 | 响应格式 | Body限制 | 移动端调用函数 | 桌面端处理函数 | 对应代码文件 |
|-----|------|-----|---------|---------|---------|-------------|-------------|------------|
| `/upload-attachment` | `POST` | `x-sync-token` | `Raw Binary` — 文件字节流<br>Query: `?hash=<sha256>&type=<mime>&name=<filename>` | `JSON` — `{ success, hash }` | 100 MB | `upload_attachment`<br>（`push_executor.rs` 内部） | `uploadAttachment` | `routes.js`<br>`push_executor.rs` |
| `/download-attachment` | `GET` | `x-sync-token` | Query: `?hash=<sha256>` | `Binary` — `sendFile` | — | `message_service.rs`<br>`ensure_attachments_locally` | `downloadAttachment` | `routes.js`<br>`message_service.rs` |
| `/upload-avatar` | `POST` | `x-sync-token` | `Raw Binary` — 图片字节流<br>Query: `?id=<owner_id>&type=agent\|group` | `JSON` — `{ success, id }` | 10 MB | `PushExecutor::push_avatar` | `uploadAvatar` | `routes.js`<br>`push_executor.rs` |
| `/download-avatar` | `GET` | `x-sync-token` | Query: `?id=<owner_id>&type=agent\|group` | `Binary` — `sendFile` | — | `PullExecutor::pull_avatar` | `downloadAvatar` | `routes.js`<br>`pull_executor.rs` |

### 3.1 POST /upload-attachment

**请求结构**
- Header: `Content-Type: application/octet-stream`
- Query 参数:
  - `hash` — 附件的 SHA-256 哈希（唯一标识）
  - `type` — MIME 类型，需 URL 编码（如 `image%2Fpng`）
  - `name` — 原始文件名，需 URL 编码
- Body: 文件原始二进制字节

**移动端上传流程**
```rust
let file_data = tokio::fs::read(file_path).await?;
let url = format!(
    "{}/api/mobile-sync/upload-attachment?hash={}&type={}&name={}",
    http_url, hash, urlencoding::encode(&mime_type), urlencoding::encode(&display_name)
);
client.post(&url)
    .header("Content-Type", "application/octet-stream")
    .body(file_data)
    .send().await?;
```

**桌面端处理**
桌面端 `uploadAttachment` 将原始字节写入 `appDataPath/attachments/<hash>`，并在数据库建立映射记录。

### 3.2 GET /download-attachment

**移动端调用场景**
附件下载不在 `PullExecutor` 中直接触发，而是在消息渲染时由 `message_service.rs` 的 `ensure_attachments_locally` 按需懒加载：

```rust
let url = format!("{}/api/mobile-sync/download-attachment?hash={}", settings.sync_http_url, hash);
match client.get(&url).header("x-sync-token", &settings.sync_token).send().await {
    Ok(resp) if resp.status().is_success() => {
        if let Ok(bytes) = resp.bytes().await {
            let _ = fs::write(&local_path, bytes).await;
        }
    }
    _ => {} // 下载失败则跳过，UI 显示裂图
}
```

**桌面端处理**
桌面端通过 `downloadAttachment(hash)` 查询本地附件路径，使用 Express 的 `res.sendFile()` 直接传输文件。

### 3.3 POST /upload-avatar

**请求结构**
- Header: `Content-Type: <mime_type>`（如 `image/png`）
- Query: `?id=<owner_id>&type=agent|group`
- Body: 图片原始二进制字节

**移动端流程**
1. 从数据库 `avatars` 表读取 `image_data` 与 `mime_type`。
2. 若记录存在，直接以原始字节作为 Body POST 到桌面端。

### 3.4 GET /download-avatar

**请求结构**
- Query: `?id=<owner_id>&type=agent|group`
- `id` 为空时，桌面端可能返回默认头像（取决于具体实现）。

**移动端重试机制**
`PullExecutor::pull_avatar` 实现了指数退避重试：
- 最大重试次数：3 次
- 初始延迟：200ms，每次翻倍（200ms → 400ms → 800ms）
- 重试触发条件：网络请求失败 或 响应体解码失败

---

## 表4：删除端点

| 路径 | 方法 | 认证 | 请求格式 | 响应格式 | Body限制 | 移动端调用函数 | 桌面端处理函数 | 对应代码文件 |
|-----|------|-----|---------|---------|---------|-------------|-------------|------------|
| `/delete-entity` | `POST` | `x-sync-token` | `JSON` — `{ id, type, deletedAt }` | `JSON` — `{ success }` | 无 | *通过 WebSocket 触发*<br>`SYNC_ENTITY_DELETE` | `deleteEntity` | `routes.js`<br>`sync_service.rs` |
| `/delete-message` | `POST` | `x-sync-token` | `JSON` — `{ msgId, deletedAt }` | `JSON` — `{ success }` | 无 | *通过 WebSocket 触发*<br>`SYNC_DELETE_NOTIFY` | `deleteMessage` | `routes.js`<br>`sync_service.rs` |

### 4.1 POST /delete-entity

**请求体结构**
```json
{
  "id": "entity-uuid",
  "type": "agent",
  "deletedAt": 1715000000000
}
```

**字段约束**
- `id`: 必填，实体 UUID
- `type`: 必填，枚举值 `agent` / `group` / `topic` / `avatar`
- `deletedAt`: 必填，Unix 时间戳（毫秒），用于软删除标记

**移动端触发方式**
移动端不直接通过 HTTP 调用此端点。删除操作通过 WebSocket 通知桌面端（`SYNC_ENTITY_DELETE` 消息），由桌面端自行处理本地软删除。若桌面端需要向移动端反向通知删除，则通过 `SYNC_DELETE_NOTIFY` WebSocket 消息触发移动端 `DeleteExecutor` 执行本地软删除。

**桌面端处理**
`deleteEntity` 根据 `type` 在数据库对应表中设置 `deleted_at = deletedAt` 时间戳，实现软删除。实际物理清理由桌面端后台任务处理。

### 4.2 POST /delete-message

**请求体结构**
```json
{
  "msgId": "message-uuid",
  "deletedAt": 1715000000000
}
```

**字段约束**
- `msgId`: 必填，消息 UUID
- `deletedAt`: 必填，Unix 时间戳（毫秒）

**移动端触发方式**
与 `/delete-entity` 相同，移动端通过 WebSocket 发送 `SYNC_DELETE_NOTIFY`（`dataType = Message`），由桌面端 `deleteMessage` 执行消息软删除。桌面端在 `messages` 表中设置 `deleted_at` 字段。

---

## 附录：HTTP 状态码与错误处理

| 状态码 | 场景 | 移动端行为 |
|-------|------|----------|
| `200 OK` | 请求成功 | 正常解析响应体 |
| `400 Bad Request` | 参数缺失或格式错误（如 `items` 非数组、`requests` 为空） | 记录日志，通常视为协议错误 |
| `401 Unauthorized` | `x-sync-token` 或 `Authorization` 不匹配 | 触发连接重置，建议用户检查同步令牌 |
| `404 Not Found` | 实体/附件/头像不存在 | `pull_agent_topic` / `pull_group_topic` 对 404 静默跳过；附件 404 则 UI 显示裂图 |
| `500 Internal Server Error` | 桌面端处理异常 | 打印错误日志，当前任务失败，不影响其他并发任务 |

### 流式端点特殊错误帧

对于 `/download-messages-stream` 和 `/upload-messages-batch` 这两个 NDJSON 流式端点：

- **流级错误**（桌面端在开始传输后发生异常）：桌面端写入 `{"_stream_error": "error message"}\n` 后结束响应。
- **Topic 级错误**：单 topic 处理失败时，桌面端写入 `{"topicId":"tid","_error":"reason"}\n`，移动端跳过该 topic 继续消费后续行。

### 并发与限流

| 层级 | 并发控制 |
|------|---------|
| 移动端总并发 | `NetworkAwareSemaphore`，默认上限 `clamp(cores * 1.5, 6, 12)` |
| 消息 Pull 并发 | `Semaphore(20)`，限制同时处理的 topic 数 |
| 附件上传并发 | 硬编码 `MAX_CONCURRENT_UPLOADS = 3` |
| 实体分块大小 | Agent/Group: 50/批；Topic: 1000/批 |
| 消息分块大小 | `MAX_MESSAGES_PER_BATCH = 10000`（控制 WS payload，非 HTTP） |

---

## 附录：端到端调用链速查

```
Phase 1 (Owner Metadata)
  PULL Agent/Group  →  GET  /download-entity
  PULL Avatar       →  GET  /download-avatar
  PUSH Agent/Group  →  POST /upload-entity
  PUSH Avatar       →  POST /upload-avatar

Phase 2 (Topic Metadata)
  PULL Topics       →  POST /download-entities
  PUSH Topics       →  POST /upload-entities-batch

Phase 3 (Messages)
  PULL Messages     →  POST /download-messages-stream  (NDJSON)
  PUSH Messages     →  POST /upload-messages-batch     (NDJSON)
  PUSH Attachments  →  POST /upload-attachment         (Raw Binary)

Runtime (Lazy Load)
  Download Attachment → GET /download-attachment
```

---

*本文档由源码自动生成基准，覆盖 `routes.js`、`sync_service.rs`、`pull_executor.rs`、`push_executor.rs` 及 `message_service.rs` 中的全部 HTTP 交互路径。*
