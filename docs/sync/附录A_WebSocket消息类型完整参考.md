---
title: 附录A - WebSocket消息类型完整参考
scope: 双端
---

# 附录A - WebSocket 消息类型完整参考

> 本附录以纯参考表格式列出同步会话中全部 WebSocket（WS）消息类型。方向列中 **M→D** 表示 Mobile（移动端）发往 Desktop（桌面端），**D→M** 表示 Desktop 发往 Mobile。

---

## 表1：控制面消息（Control Plane Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 1 | `VERSION_CHECK` | M→D | WS 连接建立后的第一条业务消息 | `mobileVersion: string`, `protocolVersion: "1.2"` | `run_sync_session` 发送并启动 `VERSION_CHECK_TIMEOUT` | 严格校验后构造 `VERSION_ACK` | `sync_service.rs`, `index.js` |
| 2 | `VERSION_ACK` | D→M | 桌面端收到 `VERSION_CHECK` 后立即回复 | `pluginVersion: "1.2.0"`, `protocolVersion: "1.2"` | 两字段精确匹配；缺失、错类型、旧 `version` 字段或不匹配均终止 attempt | 返回插件包版本与固定 wire 版本 | `sync_service.rs`, `index.js` |
| 3 | `PHASE_START` | M→D | 各同步阶段（Phase）开始时由移动端发送，通知桌面端进入新阶段 | `phase: string`，取值：`owner_metadata`、`topic_metadata`、`messages` | `run_sync_session` 中在每个 Phase 入口通过 `ws_stream.send` 发送；同时更新前端 `vcp-sync-progress` 事件 | `index.js` 中记录日志 `logger.logInfo`，返回 `PHASE_ACK` 确认帧 | `sync_service.rs`, `index.js` |
| 4 | `PHASE_COMPLETED` | M→D | 各阶段完成后由移动端发送；最终 `messages` 帧是完成态提交边界 | 普通帧：`phase`；最终帧：`phase`, `sessionId`, `attemptId`, `nonce` | Finalize 落盘与哈希事务成功后发送，安装当前 pending key 和 30 秒 watchdog | `index.js` 记录并对最终帧原样回显身份字段 | `sync_service.rs`, `index.js` |
| 5 | `PHASE_ACK` | D→M | 桌面端确认收到 `PHASE_START` 或 `PHASE_COMPLETED` | 普通 ACK：`phase`；最终 ACK：`phase`, `sessionId`, `attemptId`, `nonce` | 普通 ACK 仅记录；最终 ACK 必须精确匹配当前 pending key 并原子消费一次 | `index.js` 对最终 `messages` 帧必须原样回显四个身份字段 | `sync_service.rs`, `index.js` |
| 6 | `SYNC_LOG_EVENT` | D→M | 桌面端主动上报日志事件，通过 WS 广播给所有已连接客户端 | `level: string`，`message: string`，`phase: string`（可选） | 写入 Mobile 控制台/持久诊断文件；不将桌面端原始文本转发到 WebView | 桌面端内部 `SyncLogger` 触发 WS 广播，三个输出通道（控制台、文件、WS）同时写入 | `sync_service.rs`, `core/logger.js` |
| 7 | `DESKTOP_PHASE_START` | D→M | 桌面端报告自身阶段开始，与移动端的 `PHASE_START` 对应 | `phase: string` | 以 `[Desktop] Phase X started` 写入诊断日志，不直接展示原始阶段字符串 | 桌面端 `logger.startPhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |
| 8 | `DESKTOP_PHASE_PROGRESS` | D→M | 桌面端报告阶段进度，每处理 100 条记录自动触发 | `phase: string`，`processed: number`，`success: number`，`errors: number` | 日志输出：`[Desktop] Phase X in progress (OK:N ERR:M)` | 桌面端 `logOperation` 中 `processed % 100 === 0` 时自动触发 | `sync_service.rs`, `core/logger.js` |
| 9 | `DESKTOP_PHASE_COMPLETE` | D→M | 桌面端报告自身阶段完成 | `phase: string` | 日志输出：`[Desktop] Phase X completed` | 桌面端 `logger.completePhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |

---

## 表2：清单与差异比对消息（Manifest & Diff Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 10 | `SYNC_MANIFEST` | M→D | Phase 1 先发 Agent/Group，drain 后再发 Avatar；Phase 2 发送 Topic 清单（附带 `targetedOwners`） | `data: EntityState[]`（实体状态数组），`dataType: string`（`agent`/`group`/`avatar`/`topic`），`phase: number`（1 或 2），`targetedOwners: string[]`（V2 Phase 2 优化） | `SyncCommand::StartManualSync` 触发 Owner 波次；Owner 完成门触发 Avatar 波次；`PipelineCommand::StartTopicMetadata` 触发 Phase 2 靶向 Topic Manifest | `handleSyncManifest`（`sync/manifest.js`）：加载本地清单、两轮遍历比对、输出 Action 列表 | `sync_service.rs`, `phase1_metadata.rs`, `sync/manifest.js` |
| 11 | `SYNC_DIFF_RESULTS` | D→M | 桌面端完成 `SYNC_MANIFEST` 比对后返回差异动作列表 | `data: DiffResult[]`（差异结果数组），`dataType: string`，`phase: number` | `run_sync_session` WS 处理器中解析 JSON，按 `action` 字段分类为 `batch_pull_requests`、`push_topics_to_fetch`、`other_items` 三类并行执行 | `handleSyncManifest` 返回：`getLocalManifest` → 两轮遍历算法 → 组装 `SYNC_DIFF_RESULTS` | `sync_service.rs`, `sync/manifest.js` |
| 12 | `SYNC_TOPIC_HASH_BATCH_V2` | M→D | Phase 2.5 发送 Topic 双哈希批量比对请求；仅针对 Phase 1 筛选出的 `changed_owners` 下的话题 | `hashes: Record<topicId, {configHash: string, contentHash: string}>` | `PipelineCommand::StartTopicValidation` 触发；调用 `Phase3Message::get_targeted_topic_hashes` 批量查询 SQLite，组装为 JSON Map | `handleSyncTopicHashBatchV2`（`sync/diff.js`）：逐 Topic 查询 `hash`（对应 `config_hash`）与 `aggregated_hash`（对应 `content_hash`），双字段均一致才判定为未变更 | `sync_service.rs`, `phase3_message.rs`, `sync/diff.js` |
| 13 | `SYNC_TOPIC_HASH_RESULTS` | D→M | 桌面端完成双哈希比对后返回变更话题列表 | `changedTopics: string[]`（变更话题 ID 数组） | 接收后写入 `changed_topics` 共享状态（`Arc<Mutex<Vec<String>>>`），触发 `SyncCommand::StartMessages` 进入 Phase 3 | `handleSyncTopicHashBatchV2` 返回：遍历比对结果，收集不一致或不存在的话题 ID | `sync_service.rs`, `sync/diff.js` |
| 14 | `SYNC_MESSAGE_DIFF_BATCH` | M→D | Phase 3 分批发送消息级版本状态；按 `MAX_MESSAGES_PER_BATCH`（10000 条）拆分为多个批次 | `topics: Record<topicId, {topicHash: string, messages: Record<msgId, {hash,updatedAt}>}>` | `PipelineCommand::StartMessages` 触发；调用 `Phase3Message::get_topic_message_hashes` 批量查询话题哈希、消息哈希与最终更新时间；`build_diff_batches` 按消息数分片 | `handleSyncMessageDiffBatch`（`sync/diff.js`）：Fast Path → 时间优胜、同时间 Hash 仲裁的 Detailed Path | `sync_service.rs`, `phase3_message.rs`, `sync/diff.js` |
| 15 | `SYNC_DIFF_RESULTS_BATCH` | D→M | 桌面端完成消息级差异计算后返回逐 Topic 决策 | `results: Record<topicId, Phase3Decision>`，值为 `{ok:true,toPull,toPush,toDelete}` 或 `{ok:false,error}` | 严格校验后按桌面墓碑 → Pull → 落库 → 整 Topic Push 执行；当前批完全收尾前不发送下一批 | 每个请求 Topic 必须精确返回一项，查询失败必须返回 `ok:false` | `sync_service.rs`, `sync/diff.js` |

---

## 表3：实时变更通知消息（Real-time Notification Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 16 | `SYNC_DELETE_NOTIFY` | D→M | 桌面端通知 Mobile 执行远端墓碑 | `id: string`，`dataType: string`，`deletedAt: non-negative i64`；Message 另需 `topicId` | WS owner 在 60 秒可取消边界内调用 `DeleteExecutor::soft_delete_*`；缺字段/错型立即终止 attempt | 桌面端实体或消息删除后发送 | `sync_service.rs`, `index.js` |
| 17 | `SYNC_ENTITY_DELETE` | M→D | Mobile 本地删除或处理 `PUSH_DELETE` 后通知桌面端 | `id: string`，`dataType: string`，`deletedAt: non-negative i64`；Message 另需 `topicId` | `SyncCommand::NotifyDelete` / `NotifyMessageDelete` 发送；传输失败进入共享重试预算 | 桌面端按原时间戳幂等软删除；Message 离线遗漏另由 HTTP 墓碑重放补齐 | `sync_service.rs`, `index.js` |
| 18 | `SYNC_ERROR` | 双向 | 任一端遇到不可恢复错误 | `error: {code, origin, stage, kind, retry, message, failedTopicIds}` | 严格解析完整对象并保留根因；诊断 message 不进入 WebView 主文案 | `error-contract.js` 构造统一外壳 | `sync_error.rs`, `transport/websocket.js` |
| 19 | `SYNC_ACK` | D→M | 桌面端确认收到 `SYNC_ENTITY_DELETE` | `id: string`（对应实体 ID） | 移动端不处理 | `index.js` 返回确认帧 | `sync_service.rs`, `index.js` |

---

## 表4：Payload 字段详细说明

| 字段名 | 数据类型 | 出现位置 | 必填 | 默认值 | 说明 |
|--------|---------|---------|------|--------|------|
| `type` | `string` | 所有消息 | 是 | — | 消息类型标识符，区分大小写，必须为首层字段 |
| `mobileVersion` | `string` | `VERSION_CHECK` | 是 | — | 移动端应用版本号，编译期通过 `env!("CARGO_PKG_VERSION")` 嵌入 |
| `pluginVersion` | `string` | `VERSION_ACK` | 是 | — | 当前必须精确为 `1.2.0` |
| `protocolVersion` | `string` | `VERSION_CHECK`, `VERSION_ACK` | 是 | — | 当前必须精确为 `1.2` |
| `phase` | `string` | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 是 | — | 阶段名称，取值：`owner_metadata`、`topic_metadata`、`messages` |
| `sessionId` | `u64` / `number` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 移动端同步会话 owner generation；桌面端必须原样回显 |
| `attemptId` | `u64` / `number` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 当前会话内的 reconnect attempt；桌面端必须原样回显 |
| `nonce` | `string` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 每次 Finalize 新生成的 UUID v4；用于拒绝过期和重放 ACK |
| `data` | `EntityState[]` | `SYNC_MANIFEST` | 是 | `[]` | 实体状态向量数组，每个元素为一条实体的指纹与元数据 |
| `dataType` | `string` | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 是 | — | 实体类型枚举值：`agent`、`group`、`avatar`、`topic`；序列化为小写 |
| `phase` (number) | `number` | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 是 | — | 阶段编号：`1`=Owner Metadata, `2`=Topic Metadata；用于桌面端日志分类 |
| `targetedOwners` | `string[]` | `SYNC_MANIFEST` (phase=2) | 否 | `[]` | V2 优化字段：仅针对特定 Owner ID 列表的话题构建清单；为空数组时视为全量 |
| `hashes` | `object` | `SYNC_TOPIC_HASH_BATCH_V2` | 是 | — | 兼容哈希 Map；中央路径收到严格 `topics` 列表时不得单独依赖该 Map 推断 Owner |
| `topics` (array) | `object[]` | `SYNC_TOPIC_HASH_BATCH_V2` | 是 | — | 每项为 `{topicId, ownerType, ownerId, configHash, contentHash}`，提供无歧义 Topic 身份 |
| `changedTopics` | `string[]` | `SYNC_TOPIC_HASH_RESULTS` | 是 | `[]` | 双哈希比对后判定为变更的话题 ID 列表；空数组表示所有话题一致，可跳过 Phase 3 |
| `topics` (map) | `object` | `SYNC_MESSAGE_DIFF_BATCH` | 是 | — | Key 为 `topicId`，Value 含必填 `ownerType + ownerId + topicHash + messages` |
| `results` | `object` | `SYNC_DIFF_RESULTS_BATCH` | 是 | — | Key 为 `topicId`，Value 为严格判别联合；必须精确覆盖当前请求 topic |
| `ok` | `boolean` | `SYNC_DIFF_RESULTS_BATCH` | 是 | — | `true` 使用 `toPull/toPush`；`false` 使用 `error` 并立即终止 attempt |
| `toPull` / `toPush` | `string[]` / `boolean` | `ok:true` | 是 | — | 成功分支严禁携带 `error` |
| `error` | `SyncError` | `SYNC_ERROR`, `ok:false` | 是 | — | 完整 Wire 1.2 对象；失败分支严禁携带 `toPull/toPush` |
| `level` | `string` | `SYNC_LOG_EVENT` | 是 | — | 日志级别：`info`（白色）、`success`（绿色）、`warning`（黄色）、`error`（红色） |
| `message` | `string` | `SYNC_LOG_EVENT`, `SyncError` | 是 | — | 诊断文本；Mobile 持久化时脱敏，前端不直接展示 |
| `id` | `string` | `SYNC_DELETE_NOTIFY`, `SYNC_ENTITY_DELETE`, `SYNC_ACK` | 是 | — | 实体唯一标识符；对 Avatar 类型格式为 `owner_type:owner_id` |
| `deletedAt` | `number` | `SYNC_DELETE_NOTIFY` | 是 | — | 软删除时间戳，毫秒级 Unix Epoch；非空即视为已删除 |
| `code` | `string` | `SyncError` | 是 | — | 稳定大写机器码；已登记码映射固定中文文案，未知合法码保留但不展示原始 message |
| `origin` | 闭合集合字符串 | `SyncError` | 是 | — | `mobile_ui/mobile_native/mobile_sync/desktop_plugin/desktop_cds` |
| `stage` | 闭合集合字符串 | `SyncError` | 是 | — | 失败被确认时的精确阶段，不能统一降级为 `connect` |
| `kind` | 闭合集合字符串 | `SyncError` | 是 | — | `device/configuration/connection/compatibility/protocol/data/storage/internal` |
| `retry` | 闭合集合字符串 | `SyncError` | 是 | — | `automatic/after_user_action/manual/never`，直接控制 UI 行为 |
| `failedTopicIds` | `string[]` | `SyncError` | 是 | `[]` | 去重且最多 8 项；仅用于诊断定位 |
| `ownerType` | `string` | `SYNC_DIFF_RESULTS` 中 DiffResult | 否 | — | 仅 Topic 类型使用，区分 `agent` 与 `group`，指导路由到正确的 Pull/Push Executor |
| `mismatchedContent` | `boolean` | `SYNC_DIFF_RESULTS` 中 DiffResult | 否 | `false` | V2 标记；`true` 表示 `content_hash` 不一致，用于填充 `changed_owners` 触发 targeted topic sync |
| `action` | `string` | `SYNC_DIFF_RESULTS` 中 DiffResult | 是 | — | 差异动作：`PULL`（移动端拉取）、`PUSH`（移动端推送）、`DELETE`（移动端软删除）、`PUSH_DELETE`（移动端删除并通知桌面端）、`SKIP`（无需操作） |

---

## 表5：`EntityState` 结构完整字段

| 字段名 | Rust 类型 | JSON 序列化键 | Option | 必填 | 默认值 | 说明 |
|--------|----------|--------------|--------|------|--------|------|
| `id` | `String` | `id` | 否 | 是 | — | 实体唯一标识；Agent/Group 为自身 ID；Topic 为 topic ID；Avatar 为 `owner_type:owner_id` |
| `hash` | `String` | `hash` | 否 | 是 | — | 向后兼容的单一哈希；V2 中对于 Agent/Group/Topic 等价于 `config_hash` |
| `config_hash` | `Option<String>` | `configHash` | 是 | 否 | `None` | 配置内容指纹（V2 引入）；代表实体静态配置的 SHA-256，如名称、模型参数等 |
| `content_hash` | `Option<String>` | `contentHash` | 是 | 否 | `None` | 内容聚合指纹（V2 引入）；代表子实体集合的 Merkle Root，如 Topic 下消息的聚合哈希 |
| `ts` | `i64` | `ts` | 否 | 是 | — | 绝对时间戳 / 逻辑时钟，毫秒级 Unix Epoch；LWW（Last-Write-Wins，最后写入胜出）裁决标准 |
| `deleted_at` | `Option<i64>` | `deletedAt` | 是 | 否 | `None` | 软删除时间戳；非空表示该实体已被逻辑删除，用于双向删除同步 |
| `owner_type` | `Option<String>` | `ownerType` | 是 | 否 | `None` | 仅用于 `topic` 类型，区分 `"agent"` 和 `"group"`，指导路由到 `AgentTopicSyncDTO` 或 `GroupTopicSyncDTO` |
| `owner_id` | `Option<String>` | `ownerId` | 是 | 否 | `None` | 仅用于 `topic` 类型；与 `ownerType`、Topic ID 共同构成复合身份，协议 1.2 的 Topic manifest 中必须出现 |

---

## 表6：`DiffResult` 结构完整字段

| 字段名 | 类型 | 出现条件 | 必填 | 说明 |
|--------|------|---------|------|------|
| `id` | `string` | 始终 | 是 | 实体唯一标识符 |
| `action` | `string` | 始终 | 是 | 操作类型：`PULL`、`PUSH`、`DELETE`、`PUSH_DELETE`、`SKIP` |
| `ownerType` | `string` | Topic / Agent / Group 类型 Diff 结果 | 否 | 所有者类型，用于路由到正确的 Pull/Push Executor；`agent_topic` 或 `group_topic` |
| `ownerId` | `string` | Topic 的 `PUSH`/`PULL` 结果 | 条件 | 精确父实体 ID；不得由 Topic ID 或路径模糊推导 |
| `deletedAt` | `number` | `action` 为 `DELETE` 或 `PUSH_DELETE` 时 | 条件 | 软删除时间戳，毫秒级 Unix Epoch |
| `mismatchedContent` | `boolean` | 仅 Agent/Group 类型的 Diff 结果 | 否 | V2 标记；`true` 表示 `content_hash` 不匹配，用于引导后续 targeted topic sync |

---

## 表7：差异动作（Diff Action）语义详解

| 动作 | 全称 | 语义 | 移动端行为 | 桌面端行为 | 触发条件 |
|------|------|------|-----------|-----------|---------|
| `SKIP` | Skip | 数据一致，无需操作 | 无操作 | 无操作 | 双端 `config_hash` 与 `content_hash` 均一致，且均未删除 |
| `PULL` | Pull | 桌面端数据较新，移动端需拉取 | 调用 `PullExecutor` 通过 HTTP GET/POST 下载实体或消息 | 返回实体 DTO 或消息流 | 桌面端 `updated_at` 更新，或移动端无此记录，或 `remote.ts ≤ local.ts` 时默认走 PULL |
| `PUSH` | Push | 移动端数据较新，需推送到桌面端 | 调用 `PushExecutor` 通过 HTTP POST 上传实体或消息 | 接收 DTO，执行 `applyAgentDTO` / `handleTopicUpload` 等合并逻辑 | 移动端 `updated_at` 更新（`remote.ts > local.ts`），或桌面端无此记录 |
| `DELETE` | Delete | 移动端已标记删除，桌面端需同步删除 | 执行本地软删除（幂等，若已删除则跳过） | 执行软删除索引更新；Agent/Group 类型同时删除物理目录 | 移动端 `deletedAt` 存在，桌面端未删除 |
| `PUSH_DELETE` | Push Delete | 桌面端已删除，需通知移动端同步删除 | 执行本地软删除，并发送 `SYNC_ENTITY_DELETE` WS 通知桌面端 | 无额外操作（已删除） | 桌面端 `deletedAt` 存在，移动端未删除 |

---

## 表8：双端消息处理矩阵汇总

| 消息类型 | 移动端发送时机 | 桌面端处理函数 | 桌面端响应 | 所属协议阶段 | 关键常量/阈值 |
|---------|-------------|-------------|-----------|------------|-------------|
| `VERSION_CHECK` | WS 连接建立后 0ms | `index.js` switch-case | `VERSION_ACK` | 握手 | `VERSION_CHECK_TIMEOUT = 5s` |
| `VERSION_ACK` | —（接收） | `run_sync_session` 版本校验 | 无 | 握手 | `EXPECTED_PLUGIN_VERSION = "1.2.0"`；旧 1.1.0 拒绝、不降级 |
| `PHASE_START` | 各 Phase 开始时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | `PHASE_RESPONSE_TIMEOUT = 60s` |
| `PHASE_COMPLETED` | 各 Phase 完成时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | `phase_gate` 去重；最终 ACK 严格匹配四元身份且只消费一次 |
| `SYNC_MANIFEST` | Phase 1（Owner 两条，drain 后 Avatar 一条）；Phase 2（Topic 一条） | `handleSyncManifest` | `SYNC_DIFF_RESULTS` | Phase 1/2 | 波次缺任一整帧响应 60 秒后失败 |
| `SYNC_DIFF_RESULTS` | —（接收） | `run_sync_session` 差异任务派发 | 无 | Phase 1/2 | `pending_tasks` + `total_tasks` 原子计数 |
| `SYNC_TOPIC_HASH_BATCH_V2` | Phase 2.5 开始时 | `handleSyncTopicHashBatchV2` | `SYNC_TOPIC_HASH_RESULTS` | Phase 2.5 | 当前 attempt 最多 10,000 Topic，超限在网络前失败 |
| `SYNC_TOPIC_HASH_RESULTS` | —（接收） | `run_sync_session` 设置 `changed_topics` | 无 | Phase 2.5 | 必须为已发 Topic 的无重复子集，最多 10,000；空数组时跳过 Phase 3 |
| `SYNC_MESSAGE_DIFF_BATCH` | Phase 3 分批发送 | `handleSyncMessageDiffBatch` | `SYNC_DIFF_RESULTS_BATCH` | Phase 3 | 单批/单 Topic 最多 10,000 消息，attempt 最多 100,000 |
| `SYNC_DIFF_RESULTS_BATCH` | —（接收） | `run_sync_session` 按墓碑、Pull、落库、整 Topic Push 执行 | 无 | Phase 3 | `Phase3Tracker` 按 Topic 去重完成 |
| `SYNC_ENTITY_DELETE` | 本地软删除提交后实时发送 | `index.js` `deleteEntity`/`deleteMessage` | `SYNC_ACK` | 实时通知 | Message 离线遗漏由 Phase 3 HTTP 重放 |
| `SYNC_DELETE_NOTIFY` | —（接收） | `DeleteExecutor::soft_delete_*` | 无 | 实时通知 | 严格要求 `deletedAt`；Message 另需 `topicId` |
| `SYNC_LOG_EVENT` | —（接收） | `emit_sync_log` 转发前端 | 无 | 全阶段 | WS 广播给所有已连接客户端 |

---

## 表9：消息时序关系（单次同步会话中的发送顺序）

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
| 22 | 移动端 | `PHASE_COMPLETED` (messages, `sessionId/attemptId/nonce`) | Finalize 本地提交后启动最终 ACK 等待 |
| 23 | 桌面端 | `PHASE_ACK` (exact echo) | 原样回显 `phase/sessionId/attemptId/nonce` |
| 24 | 移动端 | WS Close | 精确 ACK 消费且最终 drain 成功后主动断开 |

---

## 表10：WebSocket 连接管理与错误码

| 错误码 | 名称 | 触发场景 | 发送方 | 处理方式 |
|--------|------|---------|--------|---------|
| `1008` | Policy Violation | 连接路径不是 `/` 或 `/ws-sync` | 桌面端 | 桌面端主动关闭连接 |
| `4001` | Unauthorized | Query Param 中的 `token` 与 `syncToken` 不匹配 | 桌面端 | 桌面端主动关闭连接，移动端需检查同步令牌配置 |
| `1000` | Normal Closure | 同步正常完成，移动端主动关闭 | 移动端 | 会话结束，无异常 |
| `1006` | Abnormal Closure | 网络中断、进程崩溃等非正常关闭 | 双方 | 移动端触发指数退避重试机制 |

---

## 表11：消息大小与性能约束

| 消息类型 | 典型大小 | 最大建议大小 | 约束来源 | 超限后果 |
|---------|---------|-------------|---------|---------|
| `VERSION_CHECK` / `VERSION_ACK` | < 100 B | 1 KB | 无 | — |
| `PHASE_START` / `PHASE_COMPLETED` / `PHASE_ACK` | < 200 B | 1 KB | 无 | — |
| `SYNC_MANIFEST` (agent/group) | 1-50 KB | 无硬性限制 | Express JSON 解析 | 过大时内存峰值上升 |
| `SYNC_MANIFEST` (topic) | 10-500 KB | 无硬性限制 | Express JSON 解析 | 靶向同步（targetedOwners）已大幅降低体积 |
| `SYNC_DIFF_RESULTS` | 1-100 KB | 无硬性限制 | Express JSON 解析 | — |
| `SYNC_TOPIC_HASH_BATCH_V2` | 10-200 KB | 无硬性限制 | WS 帧大小 | 哈希字符串固定 64 字节，体积与 Topic 数量线性相关 |
| `SYNC_TOPIC_HASH_RESULTS` | < 10 KB | 无硬性限制 | WS 帧大小 | — |
| `SYNC_MESSAGE_DIFF_BATCH` | 500 KB - 8 MiB | 8 MiB | 实际 JSON 序列化字节预算 + 10000 条消息上限 | 单 topic 超限直接失败，不发送超大帧 |
| `SYNC_DIFF_RESULTS_BATCH` | < 500 KB | 与当前请求批次绑定 | 一批在途门禁 | 当前批 HTTP/DB 收尾前不发送下一批 |
| `SYNC_LOG_EVENT` | < 1 KB | 无硬性限制 | 无 | 高频日志可能占用带宽 |

---

## 表12：移动端 `SyncCommand` 枚举与 WS 消息映射

| `SyncCommand` 变体 | 触发 WS 消息 | 触发时机 | 发送目标 |
|-------------------|------------|---------|---------|
| `StartManualSync` | `VERSION_CHECK`, `PHASE_START`, `SYNC_MANIFEST` | 用户点击"同步"按钮 | 桌面端 |
| `StartTopicMetadata` | `PHASE_START` (topic_metadata), `SYNC_MANIFEST` (topic) | Phase 1 完成且 `changed_owners` 非空 | 桌面端 |
| `StartTopicValidation` | `SYNC_TOPIC_HASH_BATCH_V2` | Phase 2 完成 | 桌面端 |
| `StartMessages` | `PHASE_START` (messages), `SYNC_MESSAGE_DIFF_BATCH` | Phase 2.5 完成且 `changedTopics` 非空 | 桌面端 |
| `Finalize` | `PHASE_COMPLETED` (messages + 四元身份) | Phase 3 所有 Topic 完成 | 桌面端 |
| `NotifyDelete` | `SYNC_ENTITY_DELETE` | 本地实体软删除提交后 | 桌面端 |
| `NotifyMessageDelete` | `SYNC_ENTITY_DELETE` | 本地消息软删除提交后 | 桌面端 |

---

## 表13：桌面端 `onMessage` 消息分发逻辑

| 接收消息类型 | 处理函数 | 文件位置 | 返回值类型 |
|-------------|---------|---------|-----------|
| `VERSION_CHECK` | 直接构造 `VERSION_ACK` | `index.js` | `VERSION_ACK` |
| `SYNC_MANIFEST` | `handleSyncManifest(payload)` | `sync/manifest.js` | `SYNC_DIFF_RESULTS` |
| `SYNC_TOPIC_HASH_BATCH_V2` | `handleSyncTopicHashBatchV2(payload)` | `sync/diff.js` | `SYNC_TOPIC_HASH_RESULTS` |
| `SYNC_MESSAGE_DIFF_BATCH` | `handleSyncMessageDiffBatch(payload)` | `sync/diff.js` | `SYNC_DIFF_RESULTS_BATCH` |
| `PHASE_START` | 记录日志，返回 `PHASE_ACK` | `index.js` | `PHASE_ACK` |
| `PHASE_COMPLETED` | 记录日志；最终帧原样回显四元身份 | `index.js` | `PHASE_ACK` |
| `SYNC_ENTITY_DELETE` | `deleteEntity` / `deleteMessage` | `index.js` | `SYNC_ACK` |
| `VERSION_ACK` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
| `PHASE_ACK` | —（桌面端发送） | 普通阶段仅记录；最终阶段精确匹配 pending key | — |
| `SYNC_LOG_EVENT` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
| `SYNC_ERROR` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
| `SYNC_ACK` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
## 表14：消息与前端事件映射

| WS 消息 | 前端事件 | Payload 字段映射 | 触发 UI 更新 |
|---------|---------|-----------------|-------------|
| `PHASE_START` / `PHASE_COMPLETED` | `vcp-sync-progress` | `phase`, `total`, `completed` | 进度条更新 |
| `SYNC_LOG_EVENT` | 无直接 WebView 事件 | `level`, `message` | 脱敏后写入持久诊断日志 |
| `SYNC_ERROR` | `vcp-sync-status` | `status:"error"`, `error:{code,category,message,guidance,...}` | 错误卡只展示固定 `message + guidance` |
| `VERSION_ACK`（校验通过） | `vcp-sync-status` | `sessionId`, `status:"connected"` | 同步面板进入进行中状态 |
| `Finalize` 完成 | `vcp-sync-completed` | `agentsChanged`, `groupsChanged`, `topicsChanged`, `messagesChanged` | 触发 Pinia Store 刷新 |
| `DESKTOP_PHASE_*` | 无直接 WebView 事件 | `phase` | 写入诊断日志；用户阶段由 Mobile 结构化进度事件展示 |

---

## 表15：消息命名规范与命名空间约定

| 命名模式 | 使用场景 | 示例 | 说明 |
|---------|---------|------|------|
| `SYNC_*` | 同步核心业务消息 | `SYNC_MANIFEST`, `SYNC_DIFF_RESULTS` | 大驼峰式，描述同步操作语义 |
| `PHASE_*` | 阶段控制与确认 | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 小写阶段名作为参数 |
| `DESKTOP_*` | 桌面端主动上报的进度消息 | `DESKTOP_PHASE_START` | 前缀标识来源端，避免命名冲突 |
| `VERSION_*` | 握手协议 | `VERSION_CHECK`, `VERSION_ACK` | 仅握手阶段使用 |
| `*_BATCH` / `*_BATCH_V2` | 批量请求 | `SYNC_TOPIC_HASH_BATCH_V2` | V2 后缀表示协议升级版本 |
| `*_NOTIFY` / `*_DELETE` | 实时通知 | `SYNC_ENTITY_DELETE`, `SYNC_DELETE_NOTIFY` | 无会话阶段限制，但仍属于当前 session/attempt |

---

## 表16：WebSocket 消息快速排查索引

| 现象 / 问题 | 检查消息类型 | 排查方向 | 关键代码位置 |
|------------|------------|---------|------------|
| 同步卡住，进度条不动 | `PHASE_START` / `PHASE_COMPLETED` / `PHASE_ACK` | 检查 `phase_gate`、差异任务错误；Finalize 时确认 peer 原样回显 `sessionId/attemptId/phase/nonce` | `sync_service.rs` phase_gate / final ACK 逻辑 |
| Phase 3 执行但无消息传输 | `SYNC_TOPIC_HASH_BATCH_V2` → `SYNC_TOPIC_HASH_RESULTS` | 检查 `changedTopics` 是否为空数组（所有 Topic 双哈希一致，正确跳过 Phase 3） | `sync/diff.js` `handleSyncTopicHashBatchV2` |
| 消息重复同步 | `SYNC_DIFF_RESULTS_BATCH` | 检查 `Phase3Tracker` HashSet 去重是否失效；检查 `toPull` 列表是否包含已存在消息 | `sync_service.rs` `Phase3Tracker` |
| 删除后另一端仍有数据 | `SYNC_DELETE_NOTIFY` / `SYNC_ENTITY_DELETE` | 检查 `deletedAt` 是否正确设置；检查桌面端 `deleteEntity` 是否执行物理删除 | `sync_service.rs` `DeleteExecutor` |
| 版本不匹配导致连接断开 | `VERSION_CHECK` / `VERSION_ACK` | 核对桌面端 `plugin-manifest.json.version` 是否精确等于 `EXPECTED_PLUGIN_VERSION` 1.2.0；1.1.0 不兼容 | `sync_service.rs` 版本校验逻辑 |
| WS 连接频繁断开 | `SYNC_LOG_EVENT` / `SYNC_ERROR` | 检查网络稳定性、服务端状态与连接错误日志 | `sync_service.rs` 连接管理逻辑 |
| Phase 2 数据传输量过大 | `SYNC_MANIFEST` (topic, phase=2) | 检查 `targetedOwners` 是否正确填充；确认 `changed_owners` 是否包含未变更 Owner | `manifest_builder.rs` `build_targeted_topic_manifest` |
| 消息级差异比对过慢 | `SYNC_MESSAGE_DIFF_BATCH` | 检查是否已启用 Fast Path（话题级哈希匹配直接跳过）；检查分片策略 | `sync/diff.js` `handleSyncMessageDiffBatch` |
| 日志终端无桌面端输出 | `DESKTOP_PHASE_*` / `SYNC_LOG_EVENT` | 检查桌面端 `SyncLogger` 是否启用 WS 通道；检查 WebSocket 连接是否建立 | `core/logger.js` WS 广播逻辑 |
| 附件元数据存在但文件无法打开 | `SYNC_DIFF_RESULTS_BATCH` (toPull) | 检查本机是否已有相同 Hash 的 CAS；同步不传输附件二进制 | 本机附件存储 |

---

*当前硬切兼容基线：VCPMobile `1.1.4` + VCPMobileSync 包 `1.2.0` + wire protocol `1.2`；旧字段、字符串错误与 1.1 peer 不兼容。*
