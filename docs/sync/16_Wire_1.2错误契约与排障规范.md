---
title: Wire 1.2 错误契约与排障规范
scope: 双端
related_files:
  - src-tauri/src/vcp_modules/sync/sync_error.rs
  - src-tauri/src/vcp_modules/sync/sync_service.rs
  - src/core/stores/syncSession.ts
  - src/features/sync/SyncSessionView.vue
  - VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/error-contract.js
version: 1.2.0
last_updated: 2026-08-13
---

# Wire 1.2 错误契约与排障规范

## 1. 目标与兼容边界

Wire 1.2 将同步错误从不可判定的字符串升级为双端共用的结构化对象，解决三个问题：

1. 设备预检、版本兼容、连接、数据、存储和生命周期错误不再依赖关键词猜测；
2. WebSocket、HTTP、NDJSON 与 Phase 3 逐 Topic 结果使用同一字段集合；
3. VCPChat 返回的稳定根因码可穿过 Mobile Rust 层，到达前端安全错误卡，同时原始诊断文本只进入脱敏日志。

当前唯一兼容组合是 VCPMobile `1.1.4`、VCPMobileSync `1.2.0`、wire protocol `1.2`。这是硬切协议：1.1 及字符串错误格式均拒绝，不做双格式兼容。

## 2. 唯一错误对象

```json
{
  "code": "SYNC_OWNER_CONFLICT",
  "origin": "desktop_cds",
  "stage": "messages",
  "kind": "data",
  "retry": "manual",
  "message": "owner identity conflict",
  "failedTopicIds": ["topic-a"]
}
```

字段要求：

| 字段 | 类型与边界 | 语义 |
|---|---|---|
| `code` | `^[A-Z][A-Z0-9_]{0,63}$` | 稳定机器码；不得使用 `ENOENT`、`EAI_*`、`ERR_*`、`SQLITE_*` 等平台原生码 |
| `origin` | 闭合集合 | 最初确认失败事实的组件 |
| `stage` | 闭合集合 | 失败被确认时所在的同步阶段 |
| `kind` | 闭合集合 | 用户排障维度，不等同于阶段 |
| `retry` | 闭合集合 | UI 唯一重试策略来源 |
| `message` | 非空，最多 1024 字符 | 仅用于诊断日志，不作为用户主文案 |
| `failedTopicIds` | 去重字符串数组，最多 8 项，每项最多 512 字符 | 有界定位信息；无失败 Topic 时也必须显式为 `[]` |

对象拒绝未知字段。缺字段、错类型、重复 Topic ID、越界值或旧字符串格式都属于协议错误，必须 fail closed。

## 3. 四个正交维度

### 3.1 来源 `origin`

| 值 | 所有者 |
|---|---|
| `mobile_ui` | Vue/Pinia 会话展示与监听边界 |
| `mobile_native` | Android 原生设备状态，如电池和省电模式 |
| `mobile_sync` | Mobile Rust 同步核心、执行器与本地数据库 |
| `desktop_plugin` | VCPChat MobileSync 插件和传输层 |
| `desktop_cds` | VCP-CDS 数据服务 |

捕获边界可以补充尚未确定的来源，但不得把已有 `desktop_cds` 根因改写为 `desktop_plugin`。

### 3.2 阶段 `stage`

| 值 | 阶段 |
|---|---|
| `preflight` | 电量、省电模式、入口条件 |
| `startup` | 本地状态、服务启动、并发 owner 获取 |
| `connect` | HTTP/WS 建连与认证 |
| `handshake` | `VERSION_CHECK` / `VERSION_ACK` |
| `owner_metadata` | Agent、Group、Avatar 清单和实体传输 |
| `topic_metadata` | Topic 清单与实体传输 |
| `topic_validation` | Topic 双哈希校验 |
| `messages` | Phase 3 消息、附件、墓碑和实时变更 |
| `finalize` | 写队列 drain、哈希修复与最终 ACK |
| `shutdown` | 取消、join、旧会话退出 |
| `history` | Change Feed 与诊断日志历史 |

超时码必须保留发生阶段。例如 `VERSION_CHECK_TIMEOUT` 是 `handshake`，`PHASE3_RESPONSE_TIMEOUT` 是 `messages`，`FINAL_ACK_TIMEOUT` 是 `finalize`；不得统一归为 `connect`。

### 3.3 类别 `kind`

| 值 | 判定问题 | 典型错误 |
|---|---|---|
| `device` | 手机当前物理/系统状态是否允许同步 | `POWER_SAVE_MODE`, `BATTERY_TOO_LOW` |
| `configuration` | 地址、路径、令牌或能力开关是否正确 | `TOKEN_MISMATCH`, `SYNC_CONFIG_INVALID` |
| `connection` | 已配置的通道是否可达或按时响应 | `NETWORK_TIMEOUT`, `FINAL_ACK_TIMEOUT` |
| `compatibility` | 双端声明的包/wire 版本是否兼容 | `PLUGIN_VERSION_MISMATCH` |
| `protocol` | 帧结构、字段类型、集合完整性是否满足契约 | `VERSION_ACK_INVALID`, `PHASE3_FRAME_INVALID` |
| `data` | 业务身份、归属、存在性或预算是否有效 | `SYNC_OWNER_CONFLICT`, `TOPIC_NOT_FOUND` |
| `storage` | DB、文件或写队列是否完成读写 | `SYNC_DB_QUERY_FAILED`, `SYNC_DB_DRAIN_FAILED` |
| `internal` | owner/epoch、任务生命周期或组件状态是否正常 | `SYNC_PHASE_STALLED`, `SYNC_PREVIOUS_SESSION_EXIT_FAILED` |

类别与阶段必须独立。省电错误始终是 `device/preflight`；版本字段合法但版本不匹配是 `compatibility/handshake`；版本帧畸形是 `protocol/handshake`；旧同步任务无法退出是 `internal/shutdown`。任何实现都不得因文本中同时出现“version”“power”或“timeout”而改变类别。

设备预检与版本握手不存在并行所有权：前端在调用 `start_manual_sync` 前完成电量和省电检查，并在每个 `await` 后校验当前 view generation；只有预检通过后 Rust 才创建 session、建立 WebSocket 并发送 `VERSION_CHECK`。因此预检失败时尚不存在版本握手，晚到的设备结果也不能提交到新 attempt。

### 3.4 重试 `retry`

| 值 | UI 行为 |
|---|---|
| `automatic` | 当前组件自行退避重试；错误卡不提供手动按钮 |
| `after_user_action` | 用户完成明确修复后显示“处理后重试” |
| `manual` | 可立即显示“重新同步” |
| `never` | 当前状态下重试无意义，不显示按钮 |

前端不得依据中文文案或 `kind` 自行推断重试行为。

`SERVICE_BUSY` 在 VCPChat 内部会先进行有界自动退避；只有退避耗尽才跨端成为终态，因此 wire 上标记为 `manual`，避免 Mobile 错误地隐藏“重新同步”按钮。

## 4. 传输映射

同一对象在不同通道只允许增加固定外壳：

| 边界 | 格式 |
|---|---|
| WebSocket 终态 | `{ "type": "SYNC_ERROR", "error": <SyncError> }` |
| HTTP 非 2xx | `{ "error": <SyncError> }` |
| 普通 JSON `success:false` | `{ "success": false, "error": <SyncError> }` |
| NDJSON 流级 | `{ "_stream_error": <SyncError> }` |
| NDJSON Topic 级 | `{ "topicId": "...", "_error": <SyncError> }` 或 `{ "success": false, "error": <SyncError> }` |
| Phase 3 失败决策 | `{ "ok": false, "error": <SyncError> }` |

不得在任一边界发送 `error: "database failed"`、`_error: "reason"`、顶层 `code/message` 或只含 `{code,message}` 的缩减对象。

## 5. 根因传播与展示所有权

错误传播遵守以下优先级：

1. 已存在且合法的稳定 `code`、`kind` 与 `retry` 是根因事实，外层 catch 不得覆盖；
2. 捕获边界可在掌握更准确信息时收窄 `origin`、`stage` 和 `failedTopicIds`；
3. 无结构错误时，边界必须指定自己的稳定 fallback code；原生 errno 不得冒充 wire code；
4. 未登记但格式合法的上游 code 原样保留，其 `kind/origin/stage/retry` 仍按 wire 对象传递；
5. Mobile Rust 内部 `Result<_, String>` 通过私有 `SYNC_WIRE_ERROR:<json>` 标记暂存完整对象；该标记不是公开 wire 格式。

VCPChat 拥有诊断事实，Mobile 拥有用户文案：

- VCPChat `message` 保留可排障细节，但不得包含令牌、认证头或未脱敏绝对路径；
- Mobile 将诊断 detail 写入会话日志，再按精确 code 注册表生成固定中文 `message + guidance`；
- 未登记 code 使用 `kind` 对应的固定兜底文案，绝不把上游 `message` 直接渲染到 WebView；
- 错误卡只显示固定原因、一个下一步，以及低显著度的 `阶段 · 来源 · code`；失败 Topic ID 仅进入复制诊断和日志。

### 5.1 VCP-CDS 上游适配

VCP-CDS internal protocol 2 不是 Mobile Wire 1.2。它当前有四种较窄的失败形态：HTTP `ErrorDetail {code,message,retryable}`、Phase 3 `SyncDecisionError {code,message}`、流式 Pull Topic 帧中的诊断字符串 `_error`，以及逐 Topic Push 结果中的字符串 `error`。`VCPMobileSync/sync/central.js` 是唯一翻译边界：

- HTTP 异常保留 CDS code，补 `origin=desktop_cds`、当前 `stage`、精确 `kind/retry` 与失败 Topic；
- Phase 3 二字段错误在返回 Mobile 前扩展为七字段 `SyncError`；
- Pull 字符串错误不做文案分类，统一映射为 `SYNC_MESSAGE_READ_FAILED / desktop_cds / messages / storage / manual`，原字符串仅保留为诊断 `message`；
- Push 字符串错误同样不做文案分类，统一映射为 `SYNC_MESSAGE_WRITE_FAILED / desktop_cds / messages / storage / manual`；
- Manifest、消息 Manifest、Topic hash 与 Phase 3 的返回形状和请求集合覆盖均在该边界校验；畸形 CDS 成功帧归为 `SYNC_PROTOCOL_INVALID / desktop_cds`，不得等到 Mobile 后再误报为本地协议错误；
- 已登记的 CDS code（如 `INVALID_REQUEST`、`NOT_FOUND`、`AMBIGUOUS_IDENTITY`、`SERVICE_BUSY`、`INTERNAL_ERROR`）按精确表分类；
- CDS internal protocol 的 `PROTOCOL_MISMATCH` 在适配边界重命名为 `CDS_PROTOCOL_MISMATCH`，不得与 Mobile wire 的 `PROTOCOL_MISMATCH` 共用用户文案；
- ChatDataService 客户端产生的 `TIMEOUT`、`UNAVAILABLE`、`INVALID_RESPONSE`、`RESPONSE_TOO_LARGE` 等本地 code 也按精确表分类；
- 新出现且格式合法的 CDS code 保留原码，以 `internal/manual` 安全兜底；平台 errno 回落到当前边界 code。

不得要求 VCP-CDS 直接复用 Wire 对象，也不得把 internal protocol 的 `retryable` 布尔值未经翻译暴露给 Mobile。

## 6. 新增错误码规则

新增错误时按以下顺序执行：

1. 在最接近根因的所有者处创建具体 code，禁止 `UNKNOWN_ERROR` 或按字符串包含关系分类；
2. 明确填写四个维度，检查它们是否与已有 code 正交；
3. 若 code 会到达用户，在 Mobile `sync_error.rs` 注册固定中文原因和唯一下一步；
4. 确保 WS、HTTP、NDJSON 与批结果均使用完整对象；
5. 补充至少一个根因穿透测试和一个旧字符串拒绝测试；
6. 若字段、枚举或兼容语义变化，升级 wire protocol 和插件版本，不做隐式兼容。

代码命名建议使用“对象 + 失败事实”，例如 `TOPIC_PUSH_OWNER_CONFLICT`、`SYNC_DB_DRAIN_FAILED`。不要把阶段、平台异常文本和用户文案拼入 code。

## 7. 共享验收证据

双端各自保存字节完全相同的 fixture：

- `error_contract_1_2_golden.json`，SHA-256 `434279b33a86a2206c1e4f47caccb4e72f05b2f9d48e093af95d5ebae6947adb`；
- `protocol_1_2_golden.json`，SHA-256 `62d4eecb639feb1a6e46302dc4046c622a5477d6a53463320c891757be629a9b`。

错误 fixture 的 `registeredSemantics` 固定所有跨端 code 的 `kind/retry` 二元组；任一端遗漏 code 或改变其类别、重试策略，契约测试必须立即失败。`origin/stage` 仍由实际捕获边界收窄，不作为 code 的静态属性。

最低验收覆盖：

- 严格字段、枚举、长度、重复 ID 和未知字段拒绝；
- WebSocket、HTTP、NDJSON、Phase 3 使用同一对象；
- CDS 根因穿过插件和 Mobile，不退化为外层通用码；
- 未登记稳定 code 保留，平台 errno 回落到边界 code；
- 设备、兼容、协议、数据、存储和内部生命周期类别不会互相混淆；
- `automatic` / `never` 不显示手动重试，`after_user_action` 明示先处理再重试。

## 8. 排障顺序

用户提供诊断信息后，按 `stage → origin → code → failedTopicIds → logFile` 定位：

1. `preflight/mobile_native`：先检查电量和省电策略，不检查桌面版本；
2. `handshake/desktop_plugin`：先核对 `1.2.0 / 1.2`，不归因于电源状态；
3. `topic_metadata|messages/desktop_cds`：检查 CDS 返回对象和对应 Topic；
4. `finalize/mobile_sync`：检查写队列 drain 与最终 ACK，不将其误报为普通网络建连失败；
5. `shutdown/mobile_sync`：检查 session generation、owner、cancel 和 join，禁止重启一个仍在退出的旧 attempt。

只有日志中的原始 `message` 用于工程诊断；用户操作建议以 Mobile 固定 `guidance` 为准。
