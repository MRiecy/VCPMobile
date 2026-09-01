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
| 1 | `VERSION_CHECK` | M→D | WS 连接建立后的第一条业务消息 | `versions[mobile_app,wire]` | 发送并启动握手 deadline | 严格校验后构造 `VERSION_ACK` | `sync_service.rs`, `protocol.js` |
| 2 | `VERSION_ACK` | D→M | 收到 `VERSION_CHECK` 后 | `versions[desktop_plugin,wire]`, `backendMode` | 兼容性只由 Wire 决定；记录当前桌面诊断投影 | 回显插件包版本、固定 Wire 与 `legacy/cds` 模式 | `sync_service.rs`, `protocol.js` |
| 3 | `PHASE_START` | M→D | 各同步阶段（Phase）开始时由移动端发送，通知桌面端进入新阶段 | `phase: string`，取值：`owner_metadata`、`topic_metadata`、`messages` | `run_sync_session` 中在每个 Phase 入口通过 `ws_stream.send` 发送；同时更新前端 `vcp-sync-progress` 事件 | `index.js` 中记录日志 `logger.logInfo`，返回 `PHASE_ACK` 确认帧 | `sync_service.rs`, `index.js` |
| 4 | `PHASE_COMPLETED` | M→D | 各阶段完成后由移动端发送；最终 `messages` 帧是完成态提交边界 | 普通帧：`phase`；最终帧：`phase`, `sessionId`, `attemptId`, `nonce` | Finalize 落盘与哈希事务成功后发送，安装当前 pending key 和 30 秒 watchdog | `index.js` 记录并对最终帧原样回显身份字段 | `sync_service.rs`, `index.js` |
| 5 | `PHASE_ACK` | D→M | 桌面端确认收到 `PHASE_START` 或 `PHASE_COMPLETED` | 普通 ACK：`phase`；最终 ACK：`phase`, `sessionId`, `attemptId`, `nonce` | 普通 ACK 仅记录；最终 ACK 必须精确匹配当前 pending key 并原子消费一次 | `index.js` 对最终 `messages` 帧必须原样回显四个身份字段 | `sync_service.rs`, `index.js` |
| 6 | `SYNC_LOG_EVENT` | D→M | 桌面端主动上报日志事件，通过 WS 广播给所有已连接客户端 | `level: string`，`message: string`，`phase: string`（可选） | 写入 Mobile 控制台/持久诊断文件；不将桌面端原始文本转发到 WebView | 桌面端内部 `SyncLogger` 触发 WS 广播，三个输出通道（控制台、文件、WS）同时写入 | `sync_service.rs`, `core/logger.js` |
| 7 | `DESKTOP_PHASE_START` | D→M | 桌面端报告自身阶段开始，与移动端的 `PHASE_START` 对应 | `phase: string` | 以 `[Desktop] Phase X started` 写入诊断日志，不直接展示原始阶段字符串 | 桌面端 `logger.startPhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |
| 9 | `DESKTOP_PHASE_COMPLETE` | D→M | 桌面端报告自身阶段完成 | `phase: string` | 日志输出：`[Desktop] Phase X completed` | 桌面端 `logger.completePhase` 方法触发 WS 广播 | `sync_service.rs`, `core/logger.js` |

---

## 表2：清单与差异比对消息（Manifest & Diff Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 10 | `SYNC_MANIFEST_REQUEST` | M→D | Phase 1 的 Owner/Avatar 波次或 Phase 2 Topic 波次 | `manifestType`, `items`; Topic 另含 `targetedOwners` | 构造强类型 Manifest | Legacy/CDS 使用同一动作矩阵 | `sync_types.rs`, `sync/manifest.js` |
| 11 | `SYNC_MANIFEST_RESULT` | D→M | Manifest 仲裁完成 | `manifestType`, `results[{完整身份,action,...}]` | 分类 Pull/Push/Delete | 返回强类型决策 | `diff_handler.rs`, `sync/manifest.js` |
| 12 | `SYNC_TOPIC_DIFF_REQUEST` | M→D | Phase 2.5 | `topics[{ownerType,ownerId,topicId,configHash,contentHash}]` | 发送完整 TopicKey | 比对双 Hash | `sync_service.rs`, `sync/diff.js` |
| 13 | `SYNC_TOPIC_DIFF_RESULT` | D→M | Topic 双 Hash 比对完成 | `changedTopics: TopicKey[]` | 进入 Phase 3 或直接跳过 | 返回变更 Topic 完整身份 | `sync_service.rs`, `sync/diff.js` |
| 14 | `SYNC_MESSAGE_DIFF_REQUEST` | M→D | Phase 3 分批比对 | `topics[{完整 TopicKey,contentHash,messages}]` | 显式 live/deleted 消息状态 | Fast Path 后执行 LWW | `sync_service.rs`, `sync/diff.js` |
| 15 | `SYNC_MESSAGE_DIFF_RESULT` | D→M | 消息仲裁完成 | `results[{完整 TopicKey,ok,pullMessageIds,pushTopic,deleteMessages}]` | 删除→Pull→flush→整 Topic Push | 每个请求 Topic 精确返回一项 | `batch_diff_handler.rs`, `sync/diff.js` |

---

## 表3：实时变更通知消息（Real-time Notification Messages）

| 序号 | 消息名称 | 方向 | 触发时机 | Payload 关键字段 | 移动端处理函数/位置 | 桌面端处理函数/位置 | 对应代码文件 |
|-----|---------|------|---------|-----------------|-------------------|-------------------|------------|
| 17 | `SYNC_ENTITY_DELETE` | M→D | Mobile 本地删除或处理 `PUSH_DELETE` 后通知桌面端 | `targetType`, 完整对象身份, `deletedAt` | 作为在线低延迟通知发送 | 桌面按原时间幂等提交；离线事实由 Manifest/Message Push 重放 | `sync_types.rs`, `index.js` |
| 18 | `SYNC_ERROR` | 双向 | 任一端遇到不可恢复错误 | `error: {code, origin, stage, kind, retry, message, failedTopicIds}` | 严格解析完整对象并保留根因；诊断 message 不进入 WebView 主文案 | `error-contract.js` 构造统一外壳 | `sync_error.rs`, `transport/websocket.js` |

---

## 表4：Payload 字段详细说明

| 字段名 | 数据类型 | 出现位置 | 必填 | 默认值 | 说明 |
|--------|---------|---------|------|--------|------|
| `type` | `string` | 所有消息 | 是 | — | 消息类型标识符，区分大小写，必须为首层字段 |
| `versions` | `VersionClaim[2]` | `VERSION_CHECK`, `VERSION_ACK` | 是 | — | 顺序无关、组件唯一；CHECK 固定 `mobile_app+wire`，ACK 固定 `desktop_plugin+wire` |
| `component` | `mobile_app \| desktop_plugin \| wire` | `versions[]` | 是 | — | 当前帧允许的版本轴；未知、缺失或重复均拒绝 |
| `version` | `string` | `versions[]` | 是 | — | 1–64 字节安全 ASCII token；只有 `wire` 参与兼容仲裁 |
| `backendMode` | `legacy \| cds` | `VERSION_ACK` | 是 | — | 当前 session 的桌面后端；仅展示和归因 |
| `phase` | `string` | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 是 | — | 阶段名称，取值：`owner_metadata`、`topic_metadata`、`messages` |
| `sessionId` | `u64` / `number` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 移动端同步会话 owner generation；桌面端必须原样回显 |
| `attemptId` | `u64` / `number` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 当前会话内的 reconnect attempt；桌面端必须原样回显 |
| `nonce` | `string` | 最终 `PHASE_COMPLETED`, `PHASE_ACK` | 最终帧必填 | — | 每次 Finalize 新生成的 UUID v4；用于拒绝过期和重放 ACK |
| `manifestType` | `owner/topic/avatar` | Manifest 请求/结果 | 是 | — | 决定清单的唯一状态形态 |
| `items` | 强类型状态数组 | `SYNC_MANIFEST_REQUEST` | 是 | `[]` | live 状态携带专用 Hash+`updatedAt`；墓碑只携带 `deletedAt` |
| `targetedOwners` | `OwnerKey[]` | Topic Manifest | 是 | `[]` | 即使 Owner 当前为零 Topic，也明确进入比较范围 |
| `topics` | `TopicKey` 扩展数组 | Topic/Message Diff | 是 | — | 每项都携带 `ownerType+ownerId+topicId` |
| `changedTopics` | `TopicKey[]` | `SYNC_TOPIC_DIFF_RESULT` | 是 | `[]` | 双 Hash 不一致的话题 |
| `results` | 强类型结果数组 | 三类 Result | 是 | `[]` | 结果必须唯一并覆盖当前请求集合 |
| `ok` | `boolean` | Message Diff 逐 Topic 结果 | 是 | — | `true` 使用决策字段；`false` 只使用 `error` |
| `pullMessageIds` / `pushTopic` / `deleteMessages` | 数组 / 布尔 / 数组 | Message Diff 成功结果 | 是 | — | `pushTopic` 表示回写整 Topic 最终视图 |
| `error` | `WireSyncError` | `SYNC_ERROR`, `ok:false` | 是 | — | 完整 Wire 1.5 对象 |
| `level` | `string` | `SYNC_LOG_EVENT` | 是 | — | 日志级别：`info`（白色）、`success`（绿色）、`warning`（黄色）、`error`（红色） |
| `message` | `string` | `SYNC_LOG_EVENT`, `SyncError` | 是 | — | 诊断文本；Mobile 持久化时脱敏，前端不直接展示 |
| `targetType` | `owner/topic/avatar/message` | `SYNC_ENTITY_DELETE` | 是 | — | 决定随后的完整身份字段 |
| `deletedAt` | `number` | Diff 删除动作、`SYNC_ENTITY_DELETE` | 是 | — | 非负毫秒时间戳 |
| `code` | `string` | `SyncError` | 是 | — | 稳定大写机器码；已登记码映射固定中文文案，未知合法码保留但不展示原始 message |
| `origin` | 闭合集合字符串 | `SyncError` | 是 | — | `mobile_ui/mobile_native/mobile_sync/desktop_plugin/desktop_cds` |
| `stage` | 闭合集合字符串 | `SyncError` | 是 | — | 失败被确认时的精确阶段，不能统一降级为 `connect` |
| `kind` | 闭合集合字符串 | `SyncError` | 是 | — | `device/configuration/connection/compatibility/protocol/data/storage/internal` |
| `retry` | 闭合集合字符串 | `SyncError` | 是 | — | `automatic/after_user_action/manual/never`，直接控制 UI 行为 |
| `failedTopicIds` | `string[]` | `SyncError` | 是 | `[]` | 去重且最多 8 项；仅用于诊断定位 |
| `ownerType/ownerId/topicId/msgId` | `string` | 对应对象身份 | 条件 | — | 不使用裸 `id` 或 Avatar 拼接 ID |
| `contentHashMismatch` | `boolean` | Owner Manifest 结果 | 否 | `false` | Owner 下级内容根不一致，触发靶向 Topic 比对 |
| `action` | `string` | Manifest Result | 是 | — | `PULL/PUSH/PULL_DELETE/PUSH_DELETE/SKIP` |

---

## 表5：Manifest 状态字段

| 字段名 | Rust 类型 | JSON 序列化键 | Option | 必填 | 默认值 | 说明 |
|--------|----------|--------------|--------|------|--------|------|
| `owner_type` | `OwnerType` | `ownerType` | 否 | 是 | — | Agent/Group 命名空间；Avatar 另允许 User |
| `owner_id` | `String` | `ownerId` | 否 | 是 | — | Owner/Avatar 身份；Topic 在此基础上增加 `topicId` |
| `topic_id` | `String` | `topicId` | 否 | Topic | — | Topic 身份字段 |
| `config_hash` | `String` | `configHash` | 否 | live Owner/Topic | — | 同步 DTO Hash |
| `content_hash` | `String` | `contentHash` | 否 | live Owner/Topic | — | 下级内容聚合 Hash |
| `binary_hash` | `String` | `binaryHash` | 否 | live Avatar | — | 原始头像 bytes Hash |
| `updated_at` | `i64` | `updatedAt` | 否 | live | — | LWW 时间 |
| `deleted_at` | `i64` | `deletedAt` | 否 | deleted | — | 墓碑时间；删除状态不伪造 live Hash |

---

## 表6：Manifest Result 字段

| 字段名 | 类型 | 出现条件 | 必填 | 说明 |
|--------|------|---------|------|------|
| 完整身份 | `OwnerKey/TopicKey/AvatarKey` | 始终 | 是 | 不使用裸 `id` |
| `action` | `string` | 始终 | 是 | `PULL`、`PUSH`、`PULL_DELETE`、`PUSH_DELETE`、`SKIP` |
| `deletedAt` | `number` | `PULL_DELETE/PUSH_DELETE` | 条件 | 非负安全整数 |
| `contentHashMismatch` | `boolean` | Owner 结果 | 否 | 配置相同但下级内容根不同 |

---

## 表7：差异动作（Diff Action）语义详解

| 动作 | 全称 | 语义 | 移动端行为 | 桌面端行为 | 触发条件 |
|------|------|------|-----------|-----------|---------|
| `SKIP` | Skip | 数据一致，无需操作 | 无操作 | 无操作 | 双端 `config_hash` 与 `content_hash` 均一致，且均未删除 |
| `PULL` | Pull | 桌面端数据较新，移动端需拉取 | 调用 `PullExecutor` 通过 HTTP GET/POST 拉取实体或消息 | 返回实体 DTO 或消息流 | 自身 Hash 不同且 Desktop `updatedAt` 不早于 Mobile，或 Mobile 无此记录 |
| `PUSH` | Push | 移动端数据较新，需推送到桌面端 | 调用 `PushExecutor` 通过 HTTP POST 推送实体或消息 | 接收 DTO，执行 `applyAgentDTO` / `handleTopicUpload` 等合并逻辑 | 自身 Hash 不同且 Mobile `updatedAt` 更晚，或 Desktop 无此记录 |
| `PULL_DELETE` | Pull Delete | Desktop 墓碑获胜 | Mobile 幂等删除 | 已删除 | Desktop 墓碑存在且 Mobile 未删除 |
| `PUSH_DELETE` | Push Delete | Mobile 墓碑获胜 | 发送 `SYNC_ENTITY_DELETE`；后续清单继续重放 | 按原 `deletedAt` 删除 | Mobile 墓碑存在且 Desktop 未删除 |

---

## 表8：双端消息处理矩阵汇总

| 消息类型 | 移动端发送时机 | 桌面端处理函数 | 桌面端响应 | 所属协议阶段 | 关键常量/阈值 |
|---------|-------------|-------------|-----------|------------|-------------|
| `VERSION_CHECK` | WS 连接建立后 0ms | `index.js` switch-case | `VERSION_ACK` | 握手 | `VERSION_CHECK_TIMEOUT = 5s` |
| `VERSION_ACK` | —（接收） | `run_sync_session` 版本校验 | 无 | 握手 | Wire 必须为 `1.5`；插件版本仅诊断；模式仅属于当前 session |
| `PHASE_START` | 各 Phase 开始时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | `PHASE_RESPONSE_TIMEOUT = 60s` |
| `PHASE_COMPLETED` | 各 Phase 完成时 | `index.js` 记录日志 | `PHASE_ACK` | 全阶段 | `phase_gate` 去重；最终 ACK 严格匹配四元身份且只消费一次 |
| `SYNC_MANIFEST_REQUEST` | Phase 1 Owner/Avatar；Phase 2 Topic | `handleSyncManifest` | `SYNC_MANIFEST_RESULT` | Phase 1/2 | 每个内部波次只能消费一次结果 |
| `SYNC_MANIFEST_RESULT` | —（接收） | 强类型任务派发 | 无 | Phase 1/2 | `pending_tasks` + `total_tasks` 计数 |
| `SYNC_TOPIC_DIFF_REQUEST` | Phase 2.5 | `handleTopicDiff` | `SYNC_TOPIC_DIFF_RESULT` | Phase 2.5 | 最多 10000 Topic |
| `SYNC_TOPIC_DIFF_RESULT` | —（接收） | 设置 `changed_topics` | 无 | Phase 2.5 | 完整 TopicKey 的无重复子集 |
| `SYNC_MESSAGE_DIFF_REQUEST` | Phase 3 分批发送 | `handleMessageDiff` | `SYNC_MESSAGE_DIFF_RESULT` | Phase 3 | 单 Topic/单批 10000，attempt 100000 |
| `SYNC_MESSAGE_DIFF_RESULT` | —（接收） | 删除→Pull→flush→Push | 无 | Phase 3 | `Phase3Tracker` 按完整 TopicKey 去重 |
| `SYNC_ENTITY_DELETE` | 本地软删除提交后实时发送 | `index.js` 删除处理 | 无 | 实时通知 | 离线墓碑由 Manifest/Message Push 重放 |
| `SYNC_LOG_EVENT` | —（接收） | `emit_sync_log` 转发前端 | 无 | 全阶段 | WS 广播给所有已连接客户端 |

---

## 表9：消息时序关系（单次同步会话中的发送顺序）

| 序号 | 发送方 | 消息类型 | 说明 |
|-----|--------|---------|------|
| 1 | 移动端 | `VERSION_CHECK` | 连接后第一条消息 |
| 2 | 桌面端 | `VERSION_ACK` | 立即响应 |
| 3 | 移动端 | `PHASE_START` (owner_metadata) | Phase 1 开始 |
| 4 | 移动端 | `SYNC_MANIFEST_REQUEST` (owner) | Owner 清单 |
| 5 | 桌面端 | `SYNC_MANIFEST_RESULT` (owner) | Owner 决策 |
| 6 | 移动端 | `SYNC_MANIFEST_REQUEST` (avatar) | Avatar 清单 |
| 7 | 桌面端 | `SYNC_MANIFEST_RESULT` (avatar) | Avatar 决策 |
| 10 | 移动端 | `PHASE_COMPLETED` (owner_metadata) | Phase 1 结束 |
| 11 | 移动端 | `PHASE_START` (topic_metadata) | Phase 2 开始 |
| 12 | 移动端 | `SYNC_MANIFEST_REQUEST` (topic) | 靶向 Topic 清单 |
| 13 | 桌面端 | `SYNC_MANIFEST_RESULT` (topic) | Topic 决策 |
| 14 | 移动端 | `PHASE_COMPLETED` (topic_metadata) | Phase 2 结束（逻辑上包含 Phase 2.5） |
| 15 | 移动端 | `SYNC_TOPIC_DIFF_REQUEST` | Phase 2.5 开始 |
| 16 | 桌面端 | `SYNC_TOPIC_DIFF_RESULT` | 变更话题列表 |
| 17 | 移动端 | `PHASE_START` (messages) | Phase 3 开始 |
| 18 | 移动端 | `SYNC_MESSAGE_DIFF_REQUEST` (batch 1) | 第 1 批消息状态 |
| 19 | 桌面端 | `SYNC_MESSAGE_DIFF_RESULT` | 第 1 批决策 |
| 20 | 移动端 | `SYNC_MESSAGE_DIFF_REQUEST` (batch N) | 后续批次 |
| 21 | 桌面端 | `SYNC_MESSAGE_DIFF_RESULT` | 后续批次决策 |
| 22 | 移动端 | `PHASE_COMPLETED` (messages, `sessionId/attemptId/nonce`) | Finalize 本地提交后启动最终 ACK 等待 |
| 23 | 桌面端 | `PHASE_ACK` (exact echo) | 原样回显 `phase/sessionId/attemptId/nonce` |
| 24 | 移动端 | WS Close | 精确 ACK 消费且最终 drain 成功后主动断开 |

---

## 表10：WebSocket 连接管理与错误码

| 错误码 | 名称 | 触发场景 | 发送方 | 处理方式 |
|--------|------|---------|--------|---------|
| `4002` | Unsupported Path | 连接路径不是 `/` 或 `/ws-sync` | 桌面端 | Mobile 以 `WS_PATH_INVALID` 终止，不做无效重连 |
| `4001` | Unauthorized | Query Param 中的 `token` 与 `syncToken` 不匹配 | 桌面端 | 桌面端主动关闭连接，移动端需检查同步令牌配置 |
| `1000` | Normal Closure | 同步正常完成，移动端主动关闭 | 移动端 | 会话结束，无异常 |
| `1006` | Abnormal Closure | 网络中断、进程崩溃等非正常关闭 | 双方 | 移动端触发指数退避重试机制 |

---

## 表11：消息大小与性能约束

| 消息类型 | 典型大小 | 最大建议大小 | 约束来源 | 超限后果 |
|---------|---------|-------------|---------|---------|
| `VERSION_CHECK` / `VERSION_ACK` | < 100 B | 1 KB | 无 | — |
| `PHASE_START` / `PHASE_COMPLETED` / `PHASE_ACK` | < 200 B | 1 KB | 无 | — |
| `SYNC_MANIFEST_REQUEST/RESULT` | 与实体数线性相关 | 每类固定状态形态 | 强类型解析 | Topic 使用靶向 Owner 范围 |
| `SYNC_TOPIC_DIFF_REQUEST/RESULT` | 与 Topic 数线性相关 | 最多 10000 Topic | 完整 TopicKey | Hash 固定 64 字节或空根 |
| `SYNC_MESSAGE_DIFF_REQUEST` | 最多 8 MiB | 10000 状态/批 | 序列化前预算 | 单 Topic 超限直接失败 |
| `SYNC_MESSAGE_DIFF_RESULT` | 与请求批次绑定 | 一批在途 | 精确覆盖请求 Topic | HTTP/DB 收尾前不发下一批 |
| `SYNC_LOG_EVENT` | < 1 KB | 无硬性限制 | 无 | 高频日志可能占用带宽 |

---

## 表12：移动端内部触发点与 WS 消息映射

| 内部触发点 | 触发 WS 消息 | 触发时机 | 发送目标 |
|-----------|------------|---------|---------|
| 版本握手 / attempt 建立 | `VERSION_CHECK`, `PHASE_START` | 每次连接成功后 | 桌面端 |
| attempt-local Owner kickoff | `SYNC_MANIFEST_REQUEST` | 每个 attempt 进入主循环时 | 桌面端 |
| `StartTopicMetadata` | `PHASE_START`, Topic Manifest | Phase 1 完成且存在变更 Owner | 桌面端 |
| `StartTopicValidation` | `SYNC_TOPIC_DIFF_REQUEST` | Phase 2 完成 | 桌面端 |
| `StartMessages` | `PHASE_START`, `SYNC_MESSAGE_DIFF_REQUEST` | Phase 2.5 有变更 Topic | 桌面端 |
| `Finalize` | `PHASE_COMPLETED` (messages + 四元身份) | Phase 3 所有 Topic 完成 | 桌面端 |
| `NotifyDelete` | `SYNC_ENTITY_DELETE` | 本地实体软删除提交后 | 桌面端 |

---

## 表13：桌面端 `onMessage` 消息分发逻辑

| 接收消息类型 | 处理函数 | 文件位置 | 返回值类型 |
|-------------|---------|---------|-----------|
| `VERSION_CHECK` | 直接构造 `VERSION_ACK` | `index.js` | `VERSION_ACK` |
| `SYNC_MANIFEST_REQUEST` | `handleSyncManifest(payload)` | `sync/manifest.js` | `SYNC_MANIFEST_RESULT` |
| `SYNC_TOPIC_DIFF_REQUEST` | `handleSyncTopicDiff(payload)` | `sync/diff.js` | `SYNC_TOPIC_DIFF_RESULT` |
| `SYNC_MESSAGE_DIFF_REQUEST` | `handleSyncMessageDiff(payload)` | `sync/diff.js` | `SYNC_MESSAGE_DIFF_RESULT` |
| `PHASE_START` | 记录日志，返回 `PHASE_ACK` | `index.js` | `PHASE_ACK` |
| `PHASE_COMPLETED` | 记录日志；最终帧原样回显四元身份 | `index.js` | `PHASE_ACK` |
| `SYNC_ENTITY_DELETE` | `deleteEntity` / `deleteMessage` | `index.js` | 无业务 ACK |
| `VERSION_ACK` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
| `PHASE_ACK` | —（桌面端发送） | 普通阶段仅记录；最终阶段精确匹配 pending key | — |
| `SYNC_LOG_EVENT` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
| `SYNC_ERROR` | —（桌面端仅发送，不作为桌面端入站帧） | — | — |
## 表14：消息与前端事件映射

| WS 消息 | 前端事件 | Payload 字段映射 | 触发 UI 更新 |
|---------|---------|-----------------|-------------|
| `PHASE_START` / `PHASE_COMPLETED` | `vcp-sync-progress` | `phase`, `total`, `completed` | 进度条更新 |
| `SYNC_LOG_EVENT` | 无直接 WebView 事件 | `level`, `message` | 脱敏后写入持久诊断日志 |
| `SYNC_ERROR` | `vcp-sync-status` | `status:"error"`, `error:{code,category,message,guidance,...}` | 错误卡只展示固定 `message + guidance` |
| `VERSION_ACK`（校验通过） | `vcp-sync-status` | `sessionId`, `status:"open"`, `desktop:{packageVersion,backendMode}` | 同步面板进入进行中状态并显示桌面后端 |
| `Finalize` 完成 | `vcp-sync-completed` | `status`, `summary` | 关闭面板时统一刷新数据 |
| `DESKTOP_PHASE_*` | 无直接 WebView 事件 | `phase` | 写入诊断日志；用户阶段由 Mobile 结构化进度事件展示 |

---

## 表15：消息命名规范与命名空间约定

| 命名模式 | 使用场景 | 示例 | 说明 |
|---------|---------|------|------|
| `SYNC_*` | 同步核心业务消息 | `SYNC_MANIFEST_REQUEST/RESULT` | 请求/结果成对命名 |
| `PHASE_*` | 阶段控制与确认 | `PHASE_START`, `PHASE_COMPLETED`, `PHASE_ACK` | 小写阶段名作为参数 |
| `DESKTOP_*` | 桌面端主动上报的进度消息 | `DESKTOP_PHASE_START` | 前缀标识来源端，避免命名冲突 |
| `VERSION_*` | 握手协议 | `VERSION_CHECK`, `VERSION_ACK` | 仅握手阶段使用 |
| 协议版本 | 只在握手 `versions` 表达 | `{component:"wire",version:"1.5"}` | 业务帧名不带 `V2/BATCH` 历史后缀 |
| `*_DELETE` | Mobile 在线删除通知 | `SYNC_ENTITY_DELETE` | 属于当前 session/attempt；离线删除仍由 Manifest/Phase 3 墓碑重放 |

---

## 表16：WebSocket 消息快速排查索引

| 现象 / 问题 | 检查消息类型 | 排查方向 | 关键代码位置 |
|------------|------------|---------|------------|
| 同步卡住，进度条不动 | `PHASE_START` / `PHASE_COMPLETED` / `PHASE_ACK` | 检查 `phase_gate`、差异任务错误；Finalize 时确认 peer 原样回显 `sessionId/attemptId/phase/nonce` | `sync_service.rs` phase_gate / final ACK 逻辑 |
| Phase 3 未传消息 | `SYNC_TOPIC_DIFF_REQUEST/RESULT` | `changedTopics` 为空时正确跳过 | `sync/diff.js` |
| 消息重复同步 | `SYNC_MESSAGE_DIFF_RESULT` | 检查完整 TopicKey 覆盖与 `pullMessageIds` | `Phase3Tracker` |
| 删除后另一端仍有数据 | Diff 删除动作 / `SYNC_ENTITY_DELETE` | 检查完整身份与 `deletedAt`，再检查下一次 Manifest 是否仍携带墓碑 | `sync_service.rs`, `DeleteExecutor` |
| 版本不匹配导致连接断开 | `VERSION_CHECK` / `VERSION_ACK` | 核对双方 Wire 是否为 1.5；插件包版本只用于诊断 | `sync_service.rs` |
| WS 连接频繁断开 | `SYNC_LOG_EVENT` / `SYNC_ERROR` | 检查网络稳定性、服务端状态与连接错误日志 | `sync_service.rs` 连接管理逻辑 |
| Phase 2 数据传输量过大 | Topic Manifest | 检查 `targetedOwners` 与 `changed_owners` | `phase1_metadata.rs` |
| 消息级差异比对过慢 | `SYNC_MESSAGE_DIFF_REQUEST` | 检查 `contentHash` Fast Path 与分片 | `sync/diff.js` |
| 日志终端无桌面端输出 | `DESKTOP_PHASE_*` / `SYNC_LOG_EVENT` | 检查桌面端 `SyncLogger` 是否启用 WS 通道；检查 WebSocket 连接是否建立 | `core/logger.js` WS 广播逻辑 |
| 附件元数据存在但文件无法打开 | `SYNC_MESSAGE_DIFF_RESULT` | 检查本机 CAS；同步不传附件二进制 | 本机附件存储 |

---

*当前硬切基线：VCPMobileSync 包 `1.5.0`、Wire `1.5`；不保留旧字段或旧帧别名。*
