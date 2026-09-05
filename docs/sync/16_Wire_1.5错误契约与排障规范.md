---
title: Wire 1.5 错误契约与排障规范
scope: 双端
related_files:
  - src-tauri/src/vcp_modules/sync/sync_error.rs
  - src-tauri/src/vcp_modules/sync/sync_service.rs
  - src/core/stores/syncSession.ts
  - src/features/sync/SyncSessionView.vue
  - VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/error-contract.js
---

# Wire 1.5 错误契约与排障规范

## 1. 目标与兼容边界

Wire 1.5 保持双端共用的结构化错误对象，并统一逐项结果与 NDJSON 流帧：

1. 设备预检、版本兼容、连接、数据、存储和生命周期错误不再依赖关键词猜测；
2. WebSocket、HTTP、NDJSON 与 Phase 3 逐 Topic 结果使用同一字段集合；
3. VCPChat 返回的稳定根因码与完整诊断原文穿过 Mobile Rust 层，同时进入前端错误卡和持久日志。

当前桌面插件为 `1.5.0`，Wire protocol 为 `1.5`。兼容性只由 Wire 版本判断；旧字段、字符串错误和旧帧名均不兼容。

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
| `message` | 非空字符串 | 完整诊断原文，直接进入日志与错误卡 |
| `failedTopicIds` | 去重字符串数组，每项最多 512 字符 | 全部失败 Topic；无失败 Topic 时也必须显式为 `[]` |

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

超时码必须保留发生阶段。例如 `WS_CONNECT_TIMEOUT` 是 `connect`，`VERSION_CHECK_TIMEOUT` 是 `handshake`，`TOPIC_HASH_RESPONSE_TIMEOUT` 是 `topic_validation`，`FINAL_ACK_TIMEOUT` 是 `finalize`。

### 3.3 类别 `kind`

| 值 | 判定问题 | 典型错误 |
|---|---|---|
| `device` | 手机当前物理/系统状态是否允许同步 | `POWER_SAVE_MODE` |
| `configuration` | 地址、路径、令牌或能力开关是否正确 | `SYNC_TOKEN_MISSING`, `SYNC_CONFIG_INVALID` |
| `connection` | 已配置的通道是否可达或按时响应 | `CONNECTION_REFUSED`, `WS_CONNECT_TIMEOUT` |
| `compatibility` | 双端声明的 Wire 是否兼容 | `WIRE_VERSION_MISMATCH` |
| `protocol` | 帧结构、字段类型、集合完整性是否满足契约 | `VERSION_ACK_INVALID`, `PHASE3_FRAME_INVALID` |
| `data` | 业务身份、归属、存在性或预算是否有效 | `SYNC_OWNER_CONFLICT`, `TOPIC_NOT_FOUND` |
| `storage` | DB、文件或写队列是否完成读写 | `SYNC_DB_QUERY_FAILED`, `SYNC_DB_DRAIN_FAILED` |
| `internal` | owner/epoch、任务生命周期或组件状态是否正常 | `SYNC_PREVIOUS_SESSION_EXIT_FAILED` |

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

`HTTP_TRANSPORT_FAILED` 与 `SYNC_SNAPSHOT_STALE` 同样在 Mobile 内部先消耗既有整 attempt 重试槽；重试耗尽后才成为 `manual` 终态。前者表示尚未得到可验证 HTTP 结果或响应流中断，后者表示 Manifest/Diff 之后物理来源或墓碑状态已经变化。二者都不引入阶段续传。

## 4. 传输映射

同一对象在不同通道只允许增加固定外壳：

| 边界 | 格式 |
|---|---|
| WebSocket 终态 | `{ "type": "SYNC_ERROR", "error": <SyncError> }` |
| HTTP 非 2xx | `{ "error": <SyncError> }` |
| 普通 JSON 逐项失败 | `{ "ok": false, "error": <SyncError> }` |
| NDJSON 流级 | `{ "kind":"streamError", "error": <SyncError> }` |
| NDJSON Topic 级 | `{ "kind":"topic", 完整TopicKey, "ok":false, "error":<SyncError> }` |
| Phase 3 失败决策 | `{ "ok": false, "error": <SyncError> }` |

不得在公共边界发送字符串错误、下划线哨兵或只含 `{code,message}` 的缩减对象。

## 5. 根因传播与展示所有权

错误传播遵守以下优先级：

1. 已存在且合法的稳定 `code`、`kind` 与 `retry` 是根因事实，外层 catch 不得覆盖；
2. 捕获边界可在掌握更准确信息时收窄 `origin`、`stage` 和 `failedTopicIds`；
3. 无结构错误时，边界必须指定自己的稳定 fallback code；原生 errno 不得冒充 wire code；
4. 未登记但格式合法的上游 code 原样保留，其 `kind/origin/stage/retry` 仍按 wire 对象传递；
5. Mobile Rust 内部 `Result<_, String>` 通过私有 `SYNC_WIRE_ERROR:<json>` 标记暂存完整对象；该标记不是公开 wire 格式。

VCPChat 与 Mobile 共同保真诊断事实：

- VCPChat `message` 原样保留令牌、认证头、URL、绝对路径与根因链；
- Mobile 将同一原文写入会话日志并直接显示，code 注册表只补充分类、重试动作和 guidance；
- 未登记 code 仍保留完整上游 `message`；
- 错误卡显示原始原因、guidance、`阶段 · 来源 · code`、全部失败 Topic ID 与日志绝对路径。

### 5.1 VCP-CDS 上游适配

VCP-CDS internal protocol 3 不是 Mobile Wire 1.5。CDS 的 HTTP 与逐项失败统一为 `{code,message,retryable}`；Central Adapter 是唯一翻译边界：

- HTTP 异常保留 CDS code，补 `origin=desktop_cds`、当前 `stage`、精确 `kind/retry` 与失败 Topic；
- CDS 逐项错误在返回 Mobile 前扩展为七字段 `SyncError`；
- 消息 Pull/Push 的 CDS 私有 code 分别映射为 `SYNC_MESSAGE_READ_FAILED` 与 `SYNC_MESSAGE_WRITE_FAILED`，原始 `message` 同时进入错误卡与日志；
- Entity Pull 将 `ENTITY_NOT_FOUND` 映射为 `SYNC_ENTITY_NOT_FOUND`，其他读取错误映射为 `SYNC_ENTITY_READ_FAILED`；
- Central 校验 CDS 响应外壳、完整身份和 Message Diff 请求集合覆盖；Mobile 的强类型解析继续作为公共 Wire 最终门禁；
- 已登记的 CDS code（如 `INVALID_REQUEST`、`NOT_FOUND`、`AMBIGUOUS_IDENTITY`、`SERVICE_BUSY`、`INTERNAL_ERROR`）按精确表分类；
- CDS internal protocol 的 `PROTOCOL_MISMATCH` 在适配边界重命名为 `CDS_PROTOCOL_MISMATCH`，不得与公开 Wire 的 `WIRE_VERSION_MISMATCH` 共用用户文案；
- 中央模式启动失败时，插件仍开放认证 WebSocket 控制面并在 `VERSION_ACK` 前发送结构化错误：二进制缺失、internal protocol 不匹配、schema 不匹配分别映射为 `CDS_BINARY_NOT_FOUND`、`CDS_PROTOCOL_MISMATCH`、`CDS_SCHEMA_MISMATCH`，其他启动错误映射为 `CDS_STARTUP_FAILED`；
- 上述失败不得挂载同步 HTTP 数据面，严禁自动回退 Legacy（引导用户在电脑端运行 `node rust_chat_data_service/build-runtime.js` 重新编译 CDS，以真正使用最新的 CDS 模式）；完全无法连接 WebSocket 时 Mobile 只能报告连接故障，不能臆测插件或 CDS 根因；
- ChatDataService 客户端产生的 `TIMEOUT`、`UNAVAILABLE`、`INVALID_RESPONSE`、`RESPONSE_TOO_LARGE` 等本地 code 也按精确表分类；
- 新出现且格式合法的 CDS code 保留原码，以 `internal/manual` 安全兜底；平台 errno 回落到当前边界 code；
- 移动端在遇到此类 CDS/Wire 兼容与启动异常时，错误卡除展示固定 guidance 外，还会显式渲染重新编译命令 `node rust_chat_data_service/build-runtime.js` 并支持一键复制到剪贴板。

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

双端各自保存同名、版本无关的契约样本：

- `wire_error_contract.json`：覆盖完整错误外壳与代表性非法输入；
- `message_canonical_contract.json`：覆盖消息逻辑投影、指纹，以及动态 key 的 JSON 转义与 UTF-8 排序。可选字段省略与 `null` 视为同一逻辑状态。

fixture 只记录真正跨端的结构与语义，不复制各端本地错误码注册表，也不绑定某次 Wire 版本或文件 SHA。仅当对应契约本身变化时才修改样本。

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
2. `handshake/desktop_plugin`：先核对 Wire `1.5`，插件包版本只用于定位构建；
3. `topic_metadata|messages/desktop_cds`：检查 CDS 返回对象和对应 Topic；
4. `finalize/mobile_sync`：检查写队列 drain 与最终 ACK，不将其误报为普通网络建连失败；
5. `shutdown/mobile_sync`：检查 session generation、owner、cancel 和 join，禁止重启一个仍在退出的旧 attempt。

只有日志中的原始 `message` 用于工程诊断；用户操作建议以 Mobile 固定 `guidance` 为准。
