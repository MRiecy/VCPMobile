---
title: 附录A - WebSocket消息类型完整参考
scope: 双端
version: 0.9.13
last_updated: 2026-05-13
---

# 附录A - WebSocket 消息类型完整参考

> 本附录以纯参考表格式列出同步会话中全部 WebSocket（WS）消息类型。方向列中 **M→D** 表示 Mobile（移动端）发往 Desktop（桌面端），**D→M** 表示 Desktop 发往 Mobile。

---

## 表1：控制面消息（Control Plane Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 1 | `VERSION_CHECK` | M→D | WS 连接建立后，移动端主动发送版本校验请求，作为同步会话第一条业务消息 | `mobileVersion: string`（移动端应用版本号） | `run_sync_session` 中直接构造 JSON 并通过 `ws_stream.send` 发送；发送后启动 `VERSION_CHECK_TIMEOUT` 定时器 | `index.js` 中 switch-case 匹配 `"VERSION_CHECK"`，读取 `plugin-manifest.json` 的 `version` 字段构造 `VERSION_ACK` 返回 | `sync_service.rs:318-325`, `index.js` |
| 2 | `VERSION_ACK` | D→M | 桌面端收到 `VERSION_CHECK` 后立即回复，携带自身插件版本 | `version: string`（桌面端插件版本号） | `run_sync_session` 中接收并校验版本字符串精确匹配；不匹配则断开连接并提示用户更新插件 | `index.js` 中读取 `plugin-manifest.json` 的 `version` 字段返回 | `sync_service.rs:328-382`, `index.js` |
| 3 | `PHASE_START` | M→D | 各同步阶段（Phase）开始时由移动端发送，通知桌面端进入新阶段 | `phase: string`，取值：`owner_metadata`、`topic_metadata`、`messages` | `run_sync_session` 中在每个 Phase 入口通过 `ws_stream.send` 发送；同时更新前端 `vcp-sync-progress` 事件 | `index.js` 中记录日志 `logger.logInfo`，返回 `PHASE_ACK` 确认帧 | `sync_service.rs`, `index.js` |
| 4 | `PHASE_COMPLETED` | M→D | 各阶段完成后由移动端发送，通知桌面端阶段结束；Finalize 阶段也使用此消息 | `phase: string`（可选，Finalize 阶段可省略） | `SyncCommand::Finalize` 或阶段自然结束时触发发送；发送后可能立即关闭 WS | `index.js` 中记录日志，返回 `PHASE_ACK` | `sync_service.rs`, `index.js` |
| 5 | `PHASE_ACK` | D→M | 桌面端确认收到 `PHASE_START` 或 `PHASE_COMPLETED` | `phase: string` | 移动端仅接收并输出日志，无显式状态机处理；作为冗余确认防止消息丢失 | `index.js` 中统一返回确认帧，结构为 `{ type: "PHASE_ACK", phase }` | `sync_service.rs`, `index.js` |
| 6 | `SYNC_LOG_EVENT` | D→M | 桌面端主动上报日志事件，通过 WS 广播给所有已连接客户端；用于前端 Mini Log Terminal 实时展示 | `level: string`（`info`/`success`/`warning`/`error`），`message: string`，`phase: string`（可选） | 通过 `emit_sync_log` 函数转发到前端 `vcp-log` 事件；`level` 映射到 UI 颜色 | 桌面端内部 `SyncLogger` 触发 WS 广播，三个输出通道（控制台、文件、WS）同时写入 | `sync_service.rs`, `core/logger.js` |
| 7 | `DESKTOP_PHASE_START` | D→M | 桌面端报告自身阶段开始，与移动端的 `PHASE_START` 对应 | `phase: string` | 日志输出格式：`[Desktop] Phase X started`；前端以灰色前缀展示 | 桌面端 `logger.startPhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |
| 8 | `DESKTOP_PHASE_PROGRESS` | D→M | 桌面端报告阶段进度，每处理 100 条记录自动触发 | `phase: string`，`processed: number`，`success: number`，`errors: number` | 日志输出：`[Desktop] Phase X in progress (OK:N ERR:M)` | 桌面端 `logOperation` 中 `processed % 100 === 0` 时自动触发 | `sync_service.rs`, `core/logger.js` |
| 9 | `DESKTOP_PHASE_COMPLETE` | D→M | 桌面端报告自身阶段完成 | `phase: string` | 日志输出：`[Desktop] Phase X completed` | 桌面端 `logger.completePhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |

---

## 表2：清单与差异比对消息（Manifest & Diff Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 10 | `SYNC_MANIFEST` | M→D | Phase 1 发送 Agent/Group/Avatar 清单；Phase 2 发送 Topic 清单（附带 `targetedOwners`） | `data: EntityState[]`（实体状态数组），`dataType: string`（`agent`/`group`/`avatar`/`topic`），`phase: number`（1 或 2），`targetedOwners: string[]`（V2 Phase 2 优化） | `SyncCommand::StartManualSync` 触发 Phase 1 的三个 Manifest；`PipelineCommand::StartTopicMetadata` 触发 Phase 2 的靶向 Topic Manifest | `handleSyncManifest`（`sync/manifest.js`）：加载本地清单、两轮遍历比对、输出 Action 列表 | `sync_service.rs`, `manifest_builder.rs`, `sync/manifest.js` |
| 11 | `SYNC_DIFF_RESULTS` | D→M | 桌面端完成 `SYNC_MANIFEST` 比对后返回差异动作列表 | `data: DiffResult[]`（差异结果数组），`dataType: string`，`phase: number` | `run_sync_session` WS 处理器中解析 JSON，按 `action` 字段分类为 `batch_pull_requests`、`push_topics_to_fetch`、`other_items` 三类并行执行 | `handleSyncManifest` 返回：`getLocalManifest` → 两轮遍历算法 → 组装 `SYNC_DIFF_RESULTS` | `sync_service.rs`, `sync/manifest.js` |
| 12 | `SYNC_TOPIC_HASH_BATCH` | M→D | **已废弃**：V1 协议中 Phase 2 发送 Topic 单哈希批量比对请求 | `hashes: Record<topicId, contentHash>`（Key 为话题 ID，Value 为单哈希字符串） | 旧版代码保留兼容路径；V2 中不再主动发送 | `handleSyncTopicHashBatch`（`sync/diff.js`）：逐 Topic 查询 `aggregated_hash` 比对 | `sync_service.rs`（旧代码）, `sync/diff.js` |
| 13 | `SYNC_TOPIC_HASH_BATCH_V2` | M→D | Phase 2.5 发送 Topic 双哈希（Dual-Hash）批量比对请求；仅针对 Phase 1 筛选出的 `changed_owners` 下的话题 | `hashes: Record<topicId, {configHash: string, contentHash: string}>` | `PipelineCommand::StartTopicValidation` 触发；调用 `Phase3Message::get_targeted_topic_hashes` 批量查询 SQLite，组装为 JSON Map | `handleSyncTopicHashBatchV2`（`sync/diff.js`）：逐 Topic 查询 `hash`（对应 `config_hash`）与 `aggregated_hash`（对应 `content_hash`），双字段均一致才判定为未变更 | `sync_service.rs`, `phase3_message.rs`, `sync/diff.js` |
| 14 | `SYNC_TOPIC_HASH_RESULTS` | D→M | 桌面端完成双哈希比对后返回变更话题列表 | `changedTopics: string[]`（变更话题 ID 数组） | 接收后写入 `changed_topics` 共享状态（`Arc<Mutex<Vec<String>>>`），触发 `SyncCommand::StartMessages` 进入 Phase 3 | `handleSyncTopicHashBatchV2` 返回：遍历比对结果，收集不一致或不存在的话题 ID | `sync_service.rs`, `sync/diff.js` |
| 15 | `SYNC_MESSAGE_DIFF_BATCH` | M→D | Phase 3 分批发送消息级哈希映射；按 `MAX_MESSAGES_PER_BATCH`（10000 条）拆分为多个批次 | `topics: Record<topicId, {topicHash: string, messages: Record<msgId, hash>}>` | `PipelineCommand::StartMessages` 触发；调用 `Phase3Message::get_topic_message_hashes` 批量查询话题哈希与消息哈希；`build_diff_batches` 按消息数分片 | `handleSyncMessageDiffBatch`（`sync/diff.js`）：Fast Path（话题级哈希直接匹配则跳过）→ Detailed Path（逐消息哈希比对） | `sync_service.rs`, `phase3_message.rs`, `sync/diff.js` |
| 16 | `SYNC_DIFF_RESULTS_BATCH` | D→M | 桌面端完成消息级差异计算后返回逐 Topic 的 Pull/Push 指令 | `results: Record<topicId, {toPull: string[], toPush: boolean}>` | 解析结果后：先执行 `PushExecutor::push_messages_batch`（推送移动端消息），再执行 `PullExecutor::pull_messages_batch`（拉取桌面端消息）；确保 Push 先于 Pull | `handleSyncMessageDiffBatch` 返回：`toPull` 为桌面端有而移动端缺失的消息 ID 列表；`toPush` 为布尔值表示移动端是否有桌面端缺失的消息 | `sync_service.rs`, `sync/diff.js` |

---

## 表3：实时变更通知消息（Real-time Notification Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 17 | `SYNC_ENTITY_UPDATE` | M→D | 移动端检测到本地实体变更时实时通知（如用户修改 Agent 配置、新建 Topic） | `id: string`（实体 ID），`dataType: string`（实体类型），`hash: string`（新哈希），`ts: i64`（更新时间戳） | `SyncCommand::NotifyLocalChange` 触发发送；由前端业务逻辑或数据库触发器调用 | `index.js` 中调用 `upsertEntityIndex` 更新桌面端索引数据库；若实体不存在则插入新记录 | `sync_service.rs`, `index.js` |
| 18 | `SYNC_DELETE_NOTIFY` | M→D | 移动端实体被软删除后通知桌面端同步删除状态 | `id: string`，`dataType: string`，`deletedAt: i64`（软删除时间戳） | `SyncCommand::NotifyDelete` 触发发送；在执行 `DeleteExecutor::soft_delete_*` 后发出 | `index.js` 中根据 `dataType` 调用 `deleteEntity`（Agent/Group/Topic）或 `deleteMessage`（消息） | `sync_service.rs`, `index.js` |
| 19 | `SYNC_ENTITY_DELETE` | M→D | `PUSH_DELETE` 动作执行后，移动端二次通知桌面端确认删除；与 `SYNC_DELETE_NOTIFY` 语义相同但触发路径不同 | `id: string`，`dataType: string` | `SyncCommand::NotifyDelete` 在处理 `PUSH_DELETE` Action 时触发；通过 `tx_internal.send` 异步发送 | 桌面端执行软删除索引更新；对 Agent/Group 类型可能同时执行物理目录删除（`fs.rm`） | `sync_service.rs`, `index.js` |
| 20 | `SYNC_ERROR` | D→M | 桌面端遇到不可恢复错误（如数据库损坏、配置解析失败） | `code: number`（错误码），`message: string`（错误描述） | 移动端记录错误日志（`emit_sync_log`），更新同步状态为 `error`；可能断开连接 | 桌面端内部错误处理触发，如 `handleSyncManifest` 中 `data` 非数组时返回 | `sync_service.rs`, `index.js` |
| 21 | `SYNC_ACK` | D→M | 桌面端确认收到 `SYNC_ENTITY_UPDATE` 或 `SYNC_DELETE_NOTIFY` | `id: string`（对应实体 ID） | 移动端不处理，可选输出调试日志；设计为异步 fire-and-forget | `index.js` 中统一返回确认帧，结构简单 | `sync_service.rs`, `index.js` |

---

## 表4：废弃与兼容性消息（Deprecated & Compatibility Messages）

| 序号 | 消息名称 | 方向 | 状态 | 替代方案 | 保留原因 | 对应代码文件 |
|-----|---------|------|------|---------|---------|------------|
| 22 | `GET_MESSAGE_MANIFEST` | M→D | 已废弃 | `SYNC_MESSAGE_DIFF_BATCH` | 兼容旧客户端 | `sync_service.rs`（旧代码保留，不再主动发送） |
| 23 | `MESSAGE_MANIFEST_RESULTS` | D→M | 已废弃 | `SYNC_DIFF_RESULTS_BATCH` | 兼容旧客户端 | `sync/diff.js`（旧代码保留） |
| 24 | `PHASE_MANIFESTS` | D→M | 已废弃/显式忽略 | `SYNC_MANIFEST`（移动端主动发送） | 桌面端旧版协议中用于下发阶段清单；V2 中移动端主动发送 `SYNC_MANIFEST`，桌面端不再下发。移动端 WS 处理器中显式忽略此消息类型，防止旧插件干扰状态机 | `sync_service.rs`（接收端显式忽略） |
| 25 | `SYNC_TOPIC_HASH_BATCH` | M→D | 已废弃 | `SYNC_TOPIC_HASH_BATCH_V2` | 兼容旧客户端；桌面端仍保留处理函数 | `sync_service.rs`（旧代码）, `sync/diff.js` |

---

## 表5：Payload 字段详细说明

| 字段名 | 数据类型 | 出现位置 | 必填 | 默认值 | 说明 |
|--------|---------|---------|------|--------|------|
| `type` | `string` | 所有消息 | 是 | — | 消息类型标识符，区分大小写，必须为首层字段 |
| `mobileVersion` | `string` | `VERSION_CHECK` | 是 | — | 移动端应用版本号，编译期通过 `env!("CARGO_PKG_VERSION")` 嵌入 |
| `version` | `string` | `VERSION_ACK` | 是 | — | 桌面端插件版本号，运行时读取 `plugin-manifest.json` 的 `version` 字段 |
| `phase` | `string` | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 是 | — | 阶段名称，取值：`owner_metadata`、`topic_metadata`、`messages` |
| `data` | `EntityState[]` | `SYNC_MANIFEST` | 是 | `[]` | 实体状态向量数组，每个元素为一条实体的指纹与元数据 |
| `dataType` | `string` | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 是 | — | 实体类型枚举值：`agent`、`group`、`avatar`、`topic`；序列化为小写 |
| `phase` (number) | `number` | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 是 | — | 阶段编号：`1`=Owner Metadata, `2`=Topic Metadata；用于桌面端日志分类 |
| `targetedOwners` | `string[]` | `SYNC_MANIFEST` (phase=2) | 否 | `[]` | V2 优化字段：仅针对特定 Owner ID 列表的话题构建清单；为空数组时视为全量 |
| `hashes` | `object` | `SYNC_TOPIC_HASH_BATCH_V2` | 是 | — | Key 为 `topicId`，Value 为 `{configHash: string, contentHash: string}` 对象 |
| `changedTopics` | `string[]` | `SYNC_TOPIC_HASH_RESULTS` | 是 | `[]` | 双哈希比对后判定为变更的话题 ID 列表；空数组表示所有话题一致，可跳过 Phase 3 |
| `topics` | `object` | `SYNC_MESSAGE_DIFF_BATCH` | 是 | — | Key 为 `topicId`，Value 含 `topicHash`（话题聚合哈希）与 `messages`（消息哈希映射） |
| `results` | `object` | `SYNC_DIFF_RESULTS_BATCH` | 是 | — | Key 为 `topicId`，Value 为 `{toPull: string[], toPush: boolean}` |
| `toPull` | `string[]` | `SYNC_DIFF_RESULTS_BATCH` | 是 | `[]` | 需从桌面端拉取的消息 ID 列表；桌面端有而移动端无（或哈希不同）的消息 |
| `toPush` | `boolean` | `SYNC_DIFF_RESULTS_BATCH` | 是 | `false` | 移动端是否需要向桌面端推送该话题的消息；`true` 表示移动端有桌面端缺失的消息 |
| `level` | `string` | `SYNC_LOG_EVENT` | 是 | — | 日志级别：`info`（白色）、`success`（绿色）、`warning`（黄色）、`error`（红色） |
| `message` | `string` | `SYNC_LOG_EVENT`, `SYNC_ERROR` | 是 | — | 日志文本或错误描述；前端直接展示 |
| `id` | `string` | `SYNC_ENTITY_UPDATE`, `SYNC_DELETE_NOTIFY`, `SYNC_ACK` | 是 | — | 实体唯一标识符；对 Avatar 类型格式为 `owner_type:owner_id` |
| `deletedAt` | `number` | `SYNC_DELETE_NOTIFY` | 是 | — | 软删除时间戳，毫秒级 Unix Epoch；非空即视为已删除 |
| `hash` | `string` | `SYNC_ENTITY_UPDATE` | 是 | — | 实体当前内容指纹，64 字符十六进制 SHA-256；用于快速判断内容是否变更 |
| `ts` | `i64` / `number` | `SYNC_ENTITY_UPDATE` | 是 | — | 实体最后更新时间戳，毫秒级 Unix Epoch；LWW 仲裁依据 |
| `code` | `number` | `SYNC_ERROR` | 是 | — | 错误码；当前未定义标准化错误码体系，通常为 `500` 或自定义值 |
| `ownerType` | `string` | `SYNC_DIFF_RESULTS` 中 DiffResult | 否 | — | 仅 Topic 类型使用，区分 `agent` 与 `group`，指导路由到正确的 Pull/Push Executor |
| `mismatchedContent` | `boolean` | `SYNC_DIFF_RESULTS` 中 DiffResult | 否 | `false` | V2 标记；`true` 表示 `content_hash` 不一致，用于填充 `changed_owners` 触发 targeted topic sync |
| `action` | `string` | `SYNC_DIFF_RESULTS` 中 DiffResult | 是 | — | 差异动作：`PULL`（移动端拉取）、`PUSH`（移动端推送）、`DELETE`（移动端软删除）、`PUSH_DELETE`（移动端删除并通知桌面端）、`SKIP`（无需操作） |

---

## 表6：`EntityState` 结构完整字段

| 字段名 | Rust 类型 | JSON 序列化键 | Option | 必填 | 默认值 | 说明 |
|--------|----------|--------------|--------|------|--------|------|
| `id` | `String` | `id` | 否 | 是 | — | 实体唯一标识；Agent/Group 为自身 ID；Topic 为 topic ID；Avatar 为 `owner_type:owner_id` |
| `hash` | `String` | `hash` | 否 | 是 | — | 向后兼容的单一哈希；V2 中对于 Agent/Group/Topic 等价于 `config_hash` |
| `config_hash` | `Option<String>` | `configHash` | 是 | 否 | `None` | 配置内容指纹（V2 引入）；代表实体静态配置的 SHA-256，如名称、模型参数等 |
| `content_hash` | `Option<String>` | `contentHash` | 是 | 否 | `None` | 内容聚合指纹（V2 引入）；代表子实体集合的 Merkle Root，如 Topic 下消息的聚合哈希 |
| `ts` | `i64` | `ts` | 否 | 是 | — | 绝对时间戳 / 逻辑时钟，毫秒级 Unix Epoch；LWW（Last-Write-Wins，最后写入胜出）裁决标准 |
| `deleted_at` | `Option<i64>` | `deletedAt` | 是 | 否 | `None` | 软删除时间戳；非空表示该实体已被逻辑删除，用于双向删除同步 |
| `owner_type` | `Option<String>` | `ownerType` | 是 | 否 | `None` | 仅用于 `topic` 类型，区分 `"agent"` 和 `"group"`，指导路由到 `AgentTopicSyncDTO` 或 `GroupTopicSyncDTO` |

---

## 表7：`DiffResult` 结构完整字段

| 字段名 | 类型 | 出现条件 | 必填 | 说明 |
|--------|------|---------|------|------|
| `id` | `string` | 始终 | 是 | 实体唯一标识符 |
| `action` | `string` | 始终 | 是 | 操作类型：`PULL`、`PUSH`、`DELETE`、`PUSH_DELETE`、`SKIP` |
| `ownerType` | `string` | Topic / Agent / Group 类型 Diff 结果 | 否 | 所有者类型，用于路由到正确的 Pull/Push Executor；`agent_topic` 或 `group_topic` |
| `deletedAt` | `number` | `action` 为 `DELETE` 或 `PUSH_DELETE` 时 | 条件 | 软删除时间戳，毫秒级 Unix Epoch |
| `mismatchedContent` | `boolean` | 仅 Agent/Group 类型的 Diff 结果 | 否 | V2 标记；`true` 表示 `content_hash` 不匹配，用于引导后续 targeted topic sync |

---

## 表8：差异动作（Diff Action）语义详解

| 动作 | 全称 | 语义 | 移动端行为 | 桌面端行为 | 触发条件 |
|------|------|------|-----------|-----------|---------|
| `SKIP` | Skip | 数据一致，无需操作 | 无操作 | 无操作 | 双端 `config_hash` 与 `content_hash` 均一致，且均未删除 |
| `PULL` | Pull | 桌面端数据较新，移动端需拉取 | 调用 `PullExecutor` 通过 HTTP GET/POST 下载实体或消息 | 返回实体 DTO 或消息流 | 桌面端 `updated_at` 更新，或移动端无此记录，或 `remote.ts ≤ local.ts` 时默认走 PULL |
| `PUSH` | Push | 移动端数据较新，需推送到桌面端 | 调用 `PushExecutor` 通过 HTTP POST 上传实体或消息 | 接收 DTO，执行 `applyAgentDTO` / `handleTopicUpload` 等合并逻辑 | 移动端 `updated_at` 更新（`remote.ts > local.ts`），或桌面端无此记录 |
| `DELETE` | Delete | 移动端已标记删除，桌面端需同步删除 | 执行本地软删除（幂等，若已删除则跳过） | 执行软删除索引更新；Agent/Group 类型同时删除物理目录 | 移动端 `deletedAt` 存在，桌面端未删除 |
| `PUSH_DELETE` | Push Delete | 桌面端已删除，需通知移动端同步删除 | 执行本地软删除，并发送 `SYNC_ENTITY_DELETE` WS 通知桌面端 | 无额外操作（已删除） | 桌面端 `deletedAt` 存在，移动端未删除 |

---

## 表9：双端消息处理矩阵汇总

| 消息类型 | 移动端发送时机 | 桌面端处理函数 | 桌面端响应 | 所属协议阶段 | 关键常量/阈值 |
|---------|-------------|-------------|-----------|------------|-------------|
| `VERSION_CHECK` | WS 连接建立后 0ms | `index.js` switch-case | `VERSION_ACK` | 握手 | `VERSION_CHECK_TIMEOUT = 5s` |
| `VERSION_ACK` | —（接收） | `run_sync_session` 版本校验 | 无 | 握手 | `EXPECTED_PLUGIN_VERSION = "0.9.13"` |
| `PHASE_START` | 各 Phase 开始时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | 看门狗检查周期 `10s × 6 = 60s` |
| `PHASE_COMPLETED` | 各 Phase 完成时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | `phase_gate` 去重，每阶段仅发送一次 |
| `SYNC_MANIFEST` | Phase 1（3条：agent/group/avatar）Phase 2（1条：topic） | `handleSyncManifest` | `SYNC_DIFF_RESULTS` | Phase 1/2 | Agent/Group 批量 chunk=50；Topic chunk=1000 |
| `SYNC_DIFF_RESULTS` | —（接收） | `run_sync_session` 差异任务派发 | 无 | Phase 1/2 | `pending_tasks` + `total_tasks` 原子计数 |
| `SYNC_TOPIC_HASH_BATCH_V2` | Phase 2.5 开始时 | `handleSyncTopicHashBatchV2` | `SYNC_TOPIC_HASH_RESULTS` | Phase 2.5 | 无显式批次限制 |
| `SYNC_TOPIC_HASH_RESULTS` | —（接收） | `run_sync_session` 设置 `changed_topics` | 无 | Phase 2.5 | 空数组时跳过 Phase 3 |
| `SYNC_MESSAGE_DIFF_BATCH` | Phase 3 分批发送 | `handleSyncMessageDiffBatch` | `SYNC_DIFF_RESULTS_BATCH` | Phase 3 | `MAX_MESSAGES_PER_BATCH = 10000` |
| `SYNC_DIFF_RESULTS_BATCH` | —（接收） | `run_sync_session` Push 先于 Pull 执行 | 无 | Phase 3 | `Phase3Tracker` HashSet 去重防下溢 |
| `SYNC_ENTITY_UPDATE` | 本地实体变更时实时发送 | `index.js` `upsertEntityIndex` | `SYNC_ACK` | 实时通知 | 无批次限制 |
| `SYNC_DELETE_NOTIFY` | 本地软删除后实时发送 | `index.js` `deleteEntity`/`deleteMessage` | `SYNC_ACK` | 实时通知 | 无批次限制 |
| `SYNC_LOG_EVENT` | —（接收） | `emit_sync_log` 转发前端 | 无 | 全阶段 | WS 广播给所有已连接客户端 |

---

## 表10：消息时序关系（单次同步会话中的发送顺序）

| 序号 | 发送方 | 消息类型 | 说明 |
|-----|--------|---------|------|
| 1 | 移动端 | `VERSION_CHECK` | 连接后第一条消息 |
| 2 | 桌面端 | `VERSION_ACK` | 立即响应 |
| 3 | 移动端 | `PHASE_START` (owner_metadata) | Phase 1 开始 |
| 4 | 移动端 | `SYNC_MANIFEST` (agent) | 第 1 条清单 |
| 5 | 移动端 | `SYNC_MANIFEST` (group) | 第 2 条清单 |
| 6 | 移动端 | `SYNC_MANIFEST` (avatar) | 第 3 条清单 |
| 7 | 桌面端 | `SYNC_DIFF_RESULTS` (agent) | 第 1 条差异结果 |
| 8 | 桌面端 | `SYNC_DIFF_RESULTS` (group) | 第 2 条差异结果 |
| 9 | 桌面端 | `SYNC_DIFF_RESULTS` (avatar) | 第 3 条差异结果 |
| 10 | 移动端 | `PHASE_COMPLETED` (owner_metadata) | Phase 1 结束 |
| 11 | 移动端 | `PHASE_START` (topic_metadata) | Phase 2 开始 |
| 12 | 移动端 | `SYNC_MANIFEST` (topic, phase=2) | 靶向 Topic 清单 |
| 13 | 桌面端 | `SYNC_DIFF_RESULTS` (topic) | Topic 差异结果 |
| 14 | 移动端 | `PHASE_COMPLETED` (topic_metadata) | Phase 2 结束（逻辑上包含 Phase 2.5） |
| 15 | 移动端 | `SYNC_TOPIC_HASH_BATCH_V2` | Phase 2.5 开始 |
| 16 | 桌面端 | `SYNC_TOPIC_HASH_RESULTS` | 变更话题列表 |
| 17 | 移动端 | `PHASE_START` (messages) | Phase 3 开始 |
| 18 | 移动端 | `SYNC_MESSAGE_DIFF_BATCH` (batch 1) | 第 1 批消息哈希 |
| 19 | 桌面端 | `SYNC_DIFF_RESULTS_BATCH` | 第 1 批差异结果 |
| 20 | 移动端 | `SYNC_MESSAGE_DIFF_BATCH` (batch N, 如有) | 后续批次 |
| 21 | 桌面端 | `SYNC_DIFF_RESULTS_BATCH` | 后续批次结果 |
| 22 | 移动端 | `PHASE_COMPLETED` (messages) | Phase 3 结束 |
| 23 | 移动端 | `PHASE_COMPLETED` (final) | Finalize 结束 |
| 24 | 移动端 | WS Close | 移动端主动断开 |

---

## 表11：WebSocket 连接管理与错误码

| 错误码 | 名称 | 触发场景 | 发送方 | 处理方式 |
|--------|------|---------|--------|---------|
| `1008` | Policy Violation | 连接路径不是 `/` 或 `/ws-sync` | 桌面端 | 桌面端主动关闭连接 |
| `4001` | Unauthorized | Query Param 中的 `token` 与 `syncToken` 不匹配 | 桌面端 | 桌面端主动关闭连接，移动端需检查同步令牌配置 |
| `1000` | Normal Closure | 同步正常完成，移动端主动关闭 | 移动端 | 会话结束，无异常 |
| `1006` | Abnormal Closure | 网络中断、进程崩溃等非正常关闭 | 双方 | 移动端触发指数退避重试机制 |

---

## 表12：消息大小与性能约束

| 消息类型 | 典型大小 | 最大建议大小 | 约束来源 | 超限后果 |
|---------|---------|-------------|---------|---------|
| `VERSION_CHECK` / `VERSION_ACK` | < 100 B | 1 KB | 无 | — |
| `PHASE_START` / `PHASE_COMPLETED` / `PHASE_ACK` | < 200 B | 1 KB | 无 | — |
| `SYNC_MANIFEST` (agent/group) | 1-50 KB | 无硬性限制 | Express JSON 解析 | 过大时内存峰值上升 |
| `SYNC_MANIFEST` (topic) | 10-500 KB | 无硬性限制 | Express JSON 解析 | 靶向同步（targetedOwners）已大幅降低体积 |
| `SYNC_DIFF_RESULTS` | 1-100 KB | 无硬性限制 | Express JSON 解析 | — |
| `SYNC_TOPIC_HASH_BATCH_V2` | 10-200 KB | 无硬性限制 | WS 帧大小 | 哈希字符串固定 64 字节，体积与 Topic 数量线性相关 |
| `SYNC_TOPIC_HASH_RESULTS` | < 10 KB | 无硬性限制 | WS 帧大小 | — |
| `SYNC_MESSAGE_DIFF_BATCH` | 500 KB - 2 MB | ~2 MB | WS 网关/代理帧大小限制 | 超过 2MB 可能导致部分网关截断；已按 10000 条消息分片 |
| `SYNC_DIFF_RESULTS_BATCH` | < 500 KB | 无硬性限制 | WS 帧大小 | — |
| `SYNC_LOG_EVENT` | < 1 KB | 无硬性限制 | 无 | 高频日志可能占用带宽 |

---

## 表13：移动端 `SyncCommand` 枚举与 WS 消息映射

| `SyncCommand` 变体 | 触发 WS 消息 | 触发时机 | 发送目标 |
|-------------------|------------|---------|---------|
| `StartManualSync` | `VERSION_CHECK`, `PHASE_START`, `SYNC_MANIFEST` | 用户点击"同步"按钮 | 桌面端 |
| `StartTopicMetadata` | `PHASE_START` (topic_metadata), `SYNC_MANIFEST` (topic) | Phase 1 完成且 `changed_owners` 非空 | 桌面端 |
| `StartTopicValidation` | `SYNC_TOPIC_HASH_BATCH_V2` | Phase 2 完成 | 桌面端 |
| `StartMessages` | `PHASE_START` (messages), `SYNC_MESSAGE_DIFF_BATCH` | Phase 2.5 完成且 `changedTopics` 非空 | 桌面端 |
| `Finalize` | `PHASE_COMPLETED` (messages, final) | Phase 3 所有 Topic 完成 | 桌面端 |
| `NotifyLocalChange` | `SYNC_ENTITY_UPDATE` | 本地实体变更监听器触发 | 桌面端 |
| `NotifyDelete` | `SYNC_DELETE_NOTIFY`, `SYNC_ENTITY_DELETE` | 本地软删除执行后 | 桌面端 |

---

## 表14：桌面端 `onMessage` 消息分发逻辑

| 接收消息类型 | 处理函数 | 文件位置 | 返回值类型 |
|-------------|---------|---------|-----------|
| `VERSION_CHECK` | 直接构造 `VERSION_ACK` | `index.js` | `VERSION_ACK` |
| `SYNC_MANIFEST` | `handleSyncManifest(payload)` | `sync/manifest.js` | `SYNC_DIFF_RESULTS` |
| `SYNC_TOPIC_HASH_BATCH` | `handleSyncTopicHashBatch(payload)` | `sync/diff.js` | `SYNC_TOPIC_HASH_RESULTS` |
| `SYNC_TOPIC_HASH_BATCH_V2` | `handleSyncTopicHashBatchV2(payload)` | `sync/diff.js` | `SYNC_TOPIC_HASH_RESULTS` |
| `SYNC_MESSAGE_DIFF_BATCH` | `handleSyncMessageDiffBatch(payload)` | `sync/diff.js` | `SYNC_DIFF_RESULTS_BATCH` |
| `PHASE_START` | 记录日志，返回 `PHASE_ACK` | `index.js` | `PHASE_ACK` |
| `PHASE_COMPLETED` | 记录日志，返回 `PHASE_ACK` | `index.js` | `PHASE_ACK` |
| `SYNC_ENTITY_UPDATE` | `upsertEntityIndex(...)` | `index.js` | `SYNC_ACK` |
| `SYNC_DELETE_NOTIFY` | `deleteEntity` / `deleteMessage` | `index.js` | `SYNC_ACK` |
| `VERSION_ACK` | —（移动端发送，桌面端不接收） | — | — |
| `PHASE_ACK` | —（桌面端发送，移动端不接收） | — | — |
| `SYNC_LOG_EVENT` | —（桌面端发送，移动端不接收） | — | — |
| `SYNC_ERROR` | —（桌面端发送，移动端不接收） | — | — |
| `SYNC_ACK` | —（桌面端发送，移动端不接收） | — | — |
| `GET_MESSAGE_MANIFEST` | 旧代码兼容处理 | `sync/diff.js` | 差异结果 |
| `PHASE_MANIFESTS` | 显式忽略 | `index.js` | 无 |

---

## 表15：V1 与 V2 协议消息差异对照

| 维度 | V1 协议 | V2 协议 | 差异说明 |
|------|--------|--------|---------|
| Topic 哈希比对 | `SYNC_TOPIC_HASH_BATCH`（单哈希） | `SYNC_TOPIC_HASH_BATCH_V2`（双哈希） | V2 区分 `configHash` 与 `contentHash`，精准筛选需同步消息的 Topic |
| Topic Manifest 范围 | 全量 Topic（所有 Owner） | 靶向 Topic（仅 `changed_owners`） | V2 通过 `targetedOwners` 字段缩小 Phase 2 数据传输量 |
| Owner 差异标记 | 仅 `PUSH`/`PULL` | 新增 `mismatchedContent` | V2 在 `SYNC_DIFF_RESULTS` 中标记 `content_hash` 不一致的 Owner，用于填充 `changed_owners` |
| 消息分批策略 | 单批次发送所有消息哈希 | 按 `MAX_MESSAGES_PER_BATCH=10000` 分片 | V2 避免 WS payload 过大导致网关截断 |
| 版本校验 | 无 | `VERSION_CHECK` / `VERSION_ACK` | V2 新增严格版本匹配，防止协议不兼容 |
| Phase 2.5 | 不存在 | 逻辑独立子阶段 | V2 在 Phase 2 与 Phase 3 之间插入 Topic Validation，不改变 `PipelinePhase` 枚举 |
| 空集合哈希 | 桌面端可能为 `null` | 统一为 `""` | V2 修复空 Topic 哈希值不一致导致的虚假差异 |

---

## 表16：消息与前端事件映射

| WS 消息 | 前端事件 | Payload 字段映射 | 触发 UI 更新 |
|---------|---------|-----------------|-------------|
| `PHASE_START` / `PHASE_COMPLETED` | `vcp-sync-progress` | `phase`, `total`, `completed` | 进度条更新 |
| `SYNC_LOG_EVENT` | `vcp-log` | `level`, `message` | Mini Log Terminal 追加 |
| `SYNC_ERROR` | `vcp-sync-status` | `status: "error"`, `message` | 顶部状态栏变红 |
| `VERSION_ACK`（校验通过） | `vcp-system-event` | `type: "vcp-log-message"`, `status: "success"` | 显示"已连接桌面端" |
| `Finalize` 完成 | `vcp-sync-completed` | `agentsChanged`, `groupsChanged`, `topicsChanged`, `messagesChanged` | 触发 Pinia Store 刷新 |
| `DESKTOP_PHASE_*` | `vcp-log` | `[Desktop] ...` 前缀日志 | 日志终端展示桌面端进度 |

---

## 表17：消息命名规范与命名空间约定

| 命名模式 | 使用场景 | 示例 | 说明 |
|---------|---------|------|------|
| `SYNC_*` | 同步核心业务消息 | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 大驼峰式，描述同步操作语义 |
| `PHASE_*` | 阶段控制与确认 | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 小写阶段名作为参数 |
| `DESKTOP_*` | 桌面端主动上报的进度消息 | `DESKTOP_PHASE_START` | 前缀标识来源端，避免命名冲突 |
| `VERSION_*` | 握手协议 | `VERSION_CHECK`, `VERSION_ACK` | 仅握手阶段使用 |
| `*_BATCH` / `*_BATCH_V2` | 批量请求 | `SYNC_TOPIC_HASH_BATCH_V2` | V2 后缀表示协议升级版本 |
| `*_NOTIFY` / `*_UPDATE` | 实时通知 | `SYNC_ENTITY_UPDATE`, `SYNC_DELETE_NOTIFY` | 无会话阶段限制，随时发送 |

---

## 表18：WebSocket 消息快速排查索引

| 现象 / 问题 | 检查消息类型 | 排查方向 | 关键代码位置 |
|------------|------------|---------|------------|
| 同步卡住，进度条不动 | `PHASE_START` / `PHASE_COMPLETED` | 检查 `phase_gate` 是否重复发送同一阶段；确认 `SYNC_DIFF_RESULTS` 是否遗漏 | `sync_service.rs` phase_gate 逻辑 |
| Phase 3 执行但无消息传输 | `SYNC_TOPIC_HASH_BATCH_V2` → `SYNC_TOPIC_HASH_RESULTS` | 检查 `changedTopics` 是否为空数组（所有 Topic 双哈希一致，正确跳过 Phase 3） | `sync/diff.js` `handleSyncTopicHashBatchV2` |
| 消息重复同步 | `SYNC_DIFF_RESULTS_BATCH` | 检查 `Phase3Tracker` HashSet 去重是否失效；检查 `toPull` 列表是否包含已存在消息 | `sync_service.rs` `Phase3Tracker` |
| 实体变更未实时同步 | `SYNC_ENTITY_UPDATE` | 检查前端是否调用 `notifyEntityUpdate`；检查桌面端 `upsertEntityIndex` 是否成功 | `index.js` `upsertEntityIndex` |
| 删除后另一端仍有数据 | `SYNC_DELETE_NOTIFY` / `SYNC_ENTITY_DELETE` | 检查 `deletedAt` 是否正确设置；检查桌面端 `deleteEntity` 是否执行物理删除 | `sync_service.rs` `DeleteExecutor` |
| 版本不匹配导致连接断开 | `VERSION_CHECK` / `VERSION_ACK` | 核对移动端 `env!("CARGO_PKG_VERSION")` 与桌面端 `plugin-manifest.json` 的 `version` 字段 | `sync_service.rs` 版本校验逻辑 |
| WS 连接频繁断开 | `SYNC_LOG_EVENT` / `SYNC_ERROR` | 检查看门狗超时（60s 无 Phase 进展）；检查网络稳定性 | `sync_service.rs` 看门狗逻辑 |
| Phase 2 数据传输量过大 | `SYNC_MANIFEST` (topic, phase=2) | 检查 `targetedOwners` 是否正确填充；确认 `changed_owners` 是否包含未变更 Owner | `manifest_builder.rs` `build_targeted_topic_manifest` |
| 消息级差异比对过慢 | `SYNC_MESSAGE_DIFF_BATCH` | 检查是否已启用 Fast Path（话题级哈希匹配直接跳过）；检查分片策略 | `sync/diff.js` `handleSyncMessageDiffBatch` |
| 日志终端无桌面端输出 | `DESKTOP_PHASE_*` / `SYNC_LOG_EVENT` | 检查桌面端 `SyncLogger` 是否启用 WS 通道；检查 WebSocket 连接是否建立 | `core/logger.js` WS 广播逻辑 |
| 附件未随消息同步 | `SYNC_DIFF_RESULTS_BATCH` (toPull) | 检查 `neededAttachmentHashes` 是否计算正确；检查附件文件是否在桌面端物理存在 | `sync/message.js` `neededAttachmentHashes` |

---

*本附录基于 VCPMobile v0.9.13 双端源代码整理。消息类型、字段结构与处理逻辑以实际代码实现为准。*
