# 群组升级：同步 DTO 与演进风险审计

审计日期：2026-10-19。范围为 VCPMobile 与 `E:\VCPChat` 当前生产源码。本文交付审计与实施建议，未修改生产代码、数据库、用户配置或运行中的服务。

## 1. 结论

已有设计考虑了“可同步字段白名单”和“保留桌面专有字段”，有可以继续利用的边界；但没有形成能在一处声明、覆盖三条实现路径的群组契约。`config/defaults.js` 文件头所称“新增字段：只需在此文件添加，无需改动其他模块”与实际代码不符。

本轮必须同时处理以下问题：

1. `modeSettings`、顺序列表和全部 JEV 群配置尚未进入同步 DTO；仅改变这些字段不会改变现有群配置 hash。
2. 既存桌面群的 `modeSettings` 通常会被旧插件保留，但不会传到 Mobile。新建或从 Mobile 恢复桌面群时，新配置会缺失；不能把“原地保存没有删除”说成“支持同步”。
3. Mobile 回推平铺自然随机标签后，桌面运行时优先读取的嵌套字段仍保留旧值，出现写回成功、业务行为没有改变的情况。
4. legacy 与 CDS 目前已经存在群 DTO 差异：JS 多了群历史窗口两字段，Mobile 的严格 DTO 不接受；CDS 则不投影这两字段。邀请提示词默认值也存在差异。
5. 新契约上线不只是增加字段和提升 wire 版本。需要正确迁移持久 hash、保持业务更新时间，并防止旧 Mobile 数据库生成的“新字段默认值”覆盖桌面已有的新配置。

## 2. 实际生产链路

### 2.1 桌面配置的当前权威结构

桌面业务读取和保存群配置均调用 `normalizeGroupModeSettings`：

- [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js#L467)：归一 `modeSettings`、旧字段兼容投影、成员与顺序。
- [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js#L661)：`getAgentGroupConfig` 从物理 `config.json` 读取后归一。
- [groupchat.js](E:/VCPChat/Groupmodules/groupchat.js#L701)：保存时合并每个模式子树，再归一、写盘。
- [grouprenderer.js](E:/VCPChat/Groupmodules/grouprenderer.js#L941)：UI 保存 `modeSettings`，同时写入旧平铺兼容字段；L955 明确注释权威数据位于 `modeSettings`。

同步插件不经过群业务保存函数。它直接读取物理配置，按自己的 DTO 规则提取或覆盖，再原子写回；因此不能假定桌面业务的归一、校验和兼容镜像会自动发生。

### 2.2 桌面 → Mobile：legacy 后端

```text
config.json
  → index.js scanEntities / extractGroupDTO
  → GROUP_SYNC_FIELDS + computeDtoHash
  → legacy owners.config_hash / updated_at
  → WebSocket owner manifest 判定 PULL
  → HTTP /entities/pull
  → entity.js downloadEntities / extractGroupDTO / snapshot hash 核对
  → Mobile EntityPullResponse / GroupSyncDTO 反序列化
  → DbWriteTask::Group
  → rusqlite_upsert_group
  → groups + group_members
```

关键证据：

- [DTO 白名单与提取](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/dto/group.dto.js#L8)。
- [索引扫描中的 DTO/hash](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/index.js#L891)。
- [manifest 读取持久 hash](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js#L143)。
- [HTTP 双后端分流](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js#L307)。
- [下载时验证物理 DTO 与已提交索引一致](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js#L309)。
- [Mobile 严格反序列化](../../src-tauri/src/vcp_modules/sync/sync_dto.rs#L66)、[拉取与入写队列](../../src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs#L554)、[落库](../../src-tauri/src/vcp_modules/persistence/db_write_queue.rs#L1542)。

### 2.3 桌面 → Mobile：CDS 后端

```text
config.json
  → CDS ingest.rs 读取完整 JSON
  → sync.rs mobile_owner_sync_dto_from_value 的独立 Rust 投影
  → 同一投影计算 hash → CDS owners 表
  → WebSocket 经 central.js /v3/sync/manifest
  → HTTP 经 central.js /v3/sync/entities/pull
  → CDS 重新读取 config.json / 同一 Rust 投影 / 已提交 hash 核对
  → Mobile 同一 GroupSyncDTO 与落库链
```

关键证据：

- [CDS OwnerRecord 构造](E:/VCPChat/rust_chat_data_service/src/ingest.rs#L1028)：`config_hash` 来自同步投影，`source_config_hash` 是物理文件 hash，两者职责不同。
- [CDS 群投影](E:/VCPChat/rust_chat_data_service/src/sync.rs#L2335)：独立列举字段，未引用 JS 白名单。
- [CDS 拉取](E:/VCPChat/rust_chat_data_service/src/sync.rs#L1033)：读取物理 JSON 后再投影，并验证提交 hash。
- [插件 CDS 拉取转发](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/central.js#L384)。
- [CDS OwnerRecord](E:/VCPChat/rust_chat_data_service/src/domain.rs#L156) 只承载身份、配置路径、hash 和话题；[storage.rs](E:/VCPChat/rust_chat_data_service/src/storage.rs#L540) 的 owners 表没有逐个群模式字段列。因此新增群模式配置通常不需要给 CDS owners 表增加业务列，实际变更重点在投影、归一和 hash 演进。

### 2.4 Mobile → 桌面：两个后端共用 JS 写回

```text
Mobile groups / group_members
  → group_service::read_group_config
  → GroupSyncDTO::from / hash / 持久版本核对
  → HTTP /entities/push
  → routes.js uploadEntity
  → 已存在：applyGroupDTO，仅覆盖白名单中出现的字段
  → 新实体：createGroupConfig，手写完整新建配置
  → config.json 原子写入
  → legacy 更新本地索引
  → CDS 模式额外 reconcileOwners，形成 CDS 提交视图后才返回 HTTP
```

关键证据：

- [Mobile push_group](../../src-tauri/src/vcp_modules/sync/sync_executor/push_executor.rs#L829)。
- [HTTP owner 写回及 CDS reconcile](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js#L373)。
- [实际物理写入与索引更新](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js#L807)。
- [已有/新建群分支](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js#L898)。
- [applyGroupDTO](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/dto/group.dto.js#L49) 与 [createGroupConfig](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/config/defaults.js#L121)。

“CDS 负责同步”不意味着可以只修改 CDS：CDS 模式的写回仍依赖 JS 群 DTO；反过来只改 JS 也不会改变 CDS 下行的 Rust 投影。

## 3. 字段矩阵

“缺失”指当前 DTO/hash/落库链没有表达对应字段；不是说物理桌面文件没有保存。

| 桌面配置字段 | 当前 JS legacy DTO/hash | 当前 CDS DTO/hash | Mobile DTO/持久化 | 实际风险 |
| --- | --- | --- | --- | --- |
| `name`、`members`、`mode`、`createdAt` | 有 | 有 | 有 | `mode` 是普通字符串，`jev` 会被带入，但其余 JEV 配置缺失；模式执行能力另见群组审计 |
| `groupPrompt`、`invitePrompt`、`useUnifiedModel`、`unifiedModel` | 有 | 有 | 有 | 邀请提示词缺省规则不一致，见 §4.2 |
| `memberTags`、`tagMatchMode` 平铺字段 | 有 | 有 | 有 | 与桌面嵌套权威值形成双源，Mobile 写回可能被遮蔽 |
| `enableContextMessageWindow`、`contextMessageWindowSize` | 有 | 缺失 | 缺失，DTO 拒绝未知字段 | legacy 群拉取可能直接反序列化失败；CDS 下行不传，窗口变更不触发其群 hash |
| `modeSettings.sequential.speakerOrder` | 缺失 | 缺失 | 缺失 | 自定义顺序只在桌面存在，不触发同步 |
| `sequentialSpeakerOrder` 旧平铺兼容字段 | 缺失 | 缺失 | 缺失 | 不能用旧别名绕过当前缺口 |
| `modeSettings.naturerandom.memberTags` | 缺失 | 缺失 | 缺失 | 桌面同键优先，回推 flat 标签不能修改它 |
| `modeSettings.naturerandom.tagMatchMode` | 缺失 | 缺失 | 缺失 | nested 为 natural 时，flat 改 strict 无效 |
| `modeSettings.invite_only` | 缺失 | 缺失 | 缺失 | 当前业务归一为空模式配置，但应由新契约明确其所有权 |
| `modeSettings.jev.navigatorPrompt` | 缺失 | 缺失 | 缺失 | 导航提示词不会同步 |
| `modeSettings.jev.speakerThreshold` | 缺失 | 缺失 | 缺失 | 发言候选截断阈值不会同步 |
| `modeSettings.jev.continueThreshold` | 缺失 | 缺失 | 缺失 | 继续裁决阈值不会同步 |
| `modeSettings.jev.stopThreshold` | 缺失 | 缺失 | 缺失 | 停止裁决阈值不会同步 |
| `modeSettings.jev.maxSpeakersPerRound` | 缺失 | 缺失 | 缺失 | 单轮 K 不会同步 |
| `modeSettings.jev.maxAutonomousRounds` | 缺失 | 缺失 | 缺失 | 自治轮数上限不会同步 |
| `modeSettings.jev.historyWindow` | 缺失 | 缺失 | 缺失 | JEV 裁决历史窗口不会同步；它与群成员模型历史窗口是两个字段 |
| `modeSettings.jev.continueDebounceMs` | 缺失 | 缺失 | 缺失 | 继续操作防抖设置不会同步 |
| `modeSettings.jev.fallbackPolicy` | 缺失 | 缺失 | 缺失 | `stop` / `top_speaker` 的回退策略不会同步 |
| `modeSettings.jev.memberStyles` | 缺失 | 缺失 | 缺失 | 成员触发风格不会同步 |
| `modeSettings.game` 及未来桌面扩展分支 | 缺失 | 缺失 | 缺失 | 本轮不据此扩大为游戏移植；写回受支持分支时须保留目的端专有分支 |

JEV 字段、缺省值和范围来自 [jevDecisionMode.js L11–60](E:/VCPChat/Groupmodules/modes/jevDecisionMode.js#L11)。当前 JEV 归一只为现有成员保留 `memberStyles`，与历史自然标签“移除成员后仍保留”不是同一种规则，实施时不能套用同一清理逻辑。

全局 `jevEnabled`、provider、URL、API key、模型、超时、重试、代理与 OpenRouter 元信息属于 [globalJevService.js](E:/VCPChat/modules/services/globalJevService.js#L39) 消费的设备服务配置；当前群 DTO 没有这些字段。建议 JEV 适配继续通过 Mobile 全局配置 owner 管理连接与凭证，不顺带把 API key 塞入群同步 DTO。

## 4. 已存在的具体缺陷与兼容风险

### 4.1 嵌套自然随机配置遮蔽旧 Mobile 的写回

[groupchat.js L488–500](E:/VCPChat/Groupmodules/groupchat.js#L488) 的实际规则为：

```javascript
memberTags = { ...legacyMemberTags, ...naturalSettings.memberTags };
tagMatchMode = naturalSettings.tagMatchMode === 'natural'
  || config.tagMatchMode === 'natural' ? 'natural' : 'strict';
```

例如桌面文件同时保存 `memberTags.a = "旧值"` 和 `modeSettings.naturerandom.memberTags.a = "旧值"`。Mobile 回推后 JS 只把平铺值改成“新值”；桌面下次读取仍得到嵌套“旧值”。下一次桌面业务保存归一配置，还会将旧值写回平铺字段。`natural → strict` 同样可能失效。

这是当前源代码可确定的调用链结果；不依赖猜测未来升级。不应通过“再同步一次”处理，应收口字段权威和桌面边界适配。

### 4.2 legacy / CDS / Mobile 已发生契约漂移

JS [GROUP_SYNC_FIELDS](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/dto/group.dto.js#L17) 含历史窗口两字段，并且 [默认值](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/config/defaults.js#L30) 会无条件填入缺失值。因此即使用户从未打开窗口功能，legacy 输出也带这两个 key。

Mobile [GroupSyncDTO](../../src-tauri/src/vcp_modules/sync/sync_dto.rs#L66) 声明 `deny_unknown_fields`，不存在这两个字段；[EntityPullData](../../src-tauri/src/vcp_modules/sync/sync_types.rs#L251) 使用严格 DTO 的 untagged 枚举；[pull_executor](../../src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs#L554) 对整批结果反序列化。由此可判断当前 legacy 群响应与 Mobile 契约不匹配。本文没有连接用户服务执行真实同步，不将源码推导写成设备实测结果。

CDS [群投影](E:/VCPChat/rust_chat_data_service/src/sync.rs#L2335) 与 Mobile 的字段集合更接近，直接遗漏窗口设置。于是切换后端不只是性能变化，还改变群配置的同步语义。

另一个漂移是 `invitePrompt` 缺省值：

- JS [defaults.js L26](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/config/defaults.js#L26) 以 `[系统邀请指令:] ` 开头。
- CDS [sync.rs L2363](E:/VCPChat/rust_chat_data_service/src/sync.rs#L2363) 和 Mobile [sync_hash.rs L222](../../src-tauri/src/vcp_modules/sync/sync_hash.rs#L222) 的缺省不含该前缀。

差异只在缺失/null需要回退时显现；显式保存的相同字符串可保持一致。不能把这个默认漂移解释为每个现存群的提示词都不一致。

### 4.3 “保留未知字段”只保护既存目的端文件

`applyGroupDTO` 对未进入白名单的配置字段不做删除，所以旧 Mobile 上传普通群配置通常不会直接删除既存桌面 `modeSettings`。但这不等于未知字段能往返：

- 下行根本不传，新建 Mobile 群不保存；
- `createGroupConfig` 手写旧字段，桌面新建/恢复时没有 `modeSettings`；
- Mobile `update_group_config` 虽然先合并 JSON，随后 [反序列化回 GroupConfig](../../src-tauri/src/vcp_modules/group/group_service.rs#L323)，未知字段没有持久保留容器；
- Mobile SQL 明确逐列读写，没有原始 JSON 透传槽。

不能只移除 `deny_unknown_fields` 作为修复：这会把显式报错改成静默丢字段，仍没有落库和回推能力。

### 4.4 群 hash 是整实体 LWW，不是字段合并

当 config hash 不同，legacy [manifest.js L430–443](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js#L430) 和 CDS [sync.rs L884–896](E:/VCPChat/rust_chat_data_service/src/sync.rs#L884) 均比较整个 owner 的 `updatedAt`，较新端胜出；同时间偏桌面 PULL。没有对 `modeSettings` 子字段进行三方合并。

因此必须区分三种情况：

1. **协议重投影**：同一业务数据因新增字段进入 hash 而产生新 hash。
2. **迁移补齐**：旧 Mobile 从未取得 JEV/顺序设置，正在第一次获得这些数据。
3. **真实编辑**：用户主动设置、清空或恢复某个模式配置。

三者不能都写成“现在发生一次群编辑”。

## 5. 升级与旧端兼容

### 5.1 可复用的严格版本握手

JS [protocol.js L3、286–320](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/protocol.js#L286) 与 Mobile [sync_version.rs L5、116](../../src-tauri/src/vcp_modules/sync/sync_version.rs#L116) 目前均为 wire `1.5`，只接受相等版本。插件 [websocket.js L216](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/websocket.js#L216) 要求首个业务帧是 VERSION_CHECK。

新群契约应提升 wire 版本，并在标准客户端进入元数据同步前明确拒绝旧协议；本轮无需为了猜测中的旧客户端维持两套群业务规则。包版本与 wire 版本分别表达发行身份与协议兼容，不能用包版本提升替代 wire 更新。

边界：HTTP [routes.js L222–273](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js#L222) 当前只有 token 认证；[parseEntityItem L45–70](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js#L45) 校验外层身份和 data 为对象，不验证内层完整群 DTO，也不绑定 WebSocket 已协商版本。因此“握手挡住旧端”针对按当前生产流程运行的客户端；新 DTO 的服务端形状/字段验证仍须在写回边界完成，不能宣称 wire bump 能约束任意独立 HTTP 请求。

CDS 的内部 `/v3` 与数据库 `schema_version` 也不是群 DTO 版本。插件与 CDS 必须配套更新，并在验收中覆盖新插件配旧 CDS 的实际错误表现，避免只看外部 wire 版本认为链路已一致。

### 5.2 已有重投影门禁可以复用

插件 [index.js L282–305](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/index.js#L282) 在 `owner_metadata` PHASE_START 中：

- CDS：等待 `centralSync.reconcile()`；[central.js L185–207](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/central.js#L185) 处理 SERVICE_BUSY 重试；CDS [ingest.rs L99–127](E:/VCPChat/rust_chat_data_service/src/ingest.rs#L99) 扫描群配置并重新生成 owner 记录。
- legacy：等待 `refreshLegacyCommitView()`，`scanEntities` 重新提取 DTO 与 hash。

后续 manifest 通过 WebSocket 的顺序消息链等待这一步完成。因此在完整更新投影实现后，不需要为了群字段升级删除整个 CDS 数据库或重建全部消息；可以复用该提交视图门禁。

但是 CDS [storage.rs L548–552](E:/VCPChat/rust_chat_data_service/src/storage.rs#L548) 与 legacy [core/db.js L224–231](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/core/db.js#L224) 在 hash 改变时会把 `updated_at` 改成当前时间。纯契约重 hash 可能导致桌面被错误判为最新，覆盖 Mobile 尚未同步的真实编辑。必须设计幂等的契约升级标识/重投影步骤，并保持已有业务版本；不能只是触发一次普通全量 reconcile。

Mobile [phase1_metadata.rs L16–26](../../src-tauri/src/vcp_modules/sync/sync_pipeline/phase1_metadata.rs#L16) 只读取 `groups.config_hash`，不实时重建。现有 [recompute_group_config_hash](../../src-tauri/src/vcp_modules/sync/sync_hash.rs#L424) 不是全局升级流程，当前生产调用用于 Agent 删除后的群成员调整。新增数据库迁移之后，还须在允许新 manifest 前，用新契约重算已有群的 hash；这一步须可重试、可中断恢复，并保持非业务变更的 `updated_at`。

### 5.3 旧 Mobile 数据首次获得新字段

即使双方重 hash 不更新时间，仍有一个不能忽略的场景：桌面已有自定义顺序/JEV 设置；旧 Mobile 从未收到它们，但最近改过群名。升级 Mobile 后若为新列填入默认 `modeSettings` 并让它按旧的“群名更新时间”赢得整实体 LWW，就会把迁移默认值上传覆盖桌面真实配置。

建议明确一次性迁移行为：

- 区分“旧数据库尚未取得新字段”与“用户显式选择默认/清空”。缺失状态是本地迁移事实，不应伪装成显式群配置。
- 对两端同 ID 的既有群，在完整新 DTO 首次允许回推前，补入桌面已保存的新字段；同时保留 Mobile 已有旧字段的编辑事实，不以一次整群覆盖掩盖冲突。
- Mobile 独有群可由当前成员和明确默认值初始化；桌面独有群正常拉取完整新 DTO。
- 完成迁移后回到单一新契约，避免长期维护双字段业务状态。具体迁移合并提交顺序应作为同步阶段的实施设计和回归场景，不提前声称已解决。

### 5.4 条件写与桌面并发编辑的真实边界

交叉复核发现，首次迁移的“读取版本—用户确认—条件提交”还需扩展现有接口：owner manifest 的动作结果及 owner pull 不携带完整对端 `configHash/updatedAt` 快照，owner push 也没有 expected-version 条件。见 [CDS manifest response](E:/VCPChat/rust_chat_data_service/src/sync.rs#L181)、[Mobile owner pull response](../../src-tauri/src/vcp_modules/sync/sync_types.rs#L335)、[插件写入入口](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js#L373)。新契约不能仅增加 modeSettings 而默认已经具备跨版本合并输入。

更重要的是，**现有锁并不串行化全部桌面群配置 writer**：

- [utils/lock.js](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/utils/lock.js#L6) 是插件内部内存 Map；[entity.js](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js#L785) 的 acquireLock 只约束参与同锁的插件调用。
- 桌面 [groupChatHandlers.js](E:/VCPChat/modules/ipc/groupChatHandlers.js#L107) 直接调用业务保存；[groupchat.js](E:/VCPChat/Groupmodules/groupchat.js#L691) 先读文件，L735 直接 `fs.writeJson`，不获取插件锁。头像和话题配置另有同文件写入口。
- CDS [protocol.rs](E:/VCPChat/rust_chat_data_service/src/protocol.rs#L652) 的 reconcile lock 只串行化 CDS reconcile；watcher 写入抑制标记也不是全部 writer 共用互斥。
- 同步临时文件 rename 和 [writeJsonAtomic](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js#L1052) 的重复读取检查不能变成跨 writer 的 compare-and-swap。

因此 expected baseline 在插件锁内检查，可以约束同锁同步写入并拒绝已观察到的桌面变化；桌面仍可能在检查与 rename 之间写入，或拿着早先读取的旧配置在同步结束后保存。提交后重读与 CDS 确认可发现一部分竞争，但不保证发现所有被覆盖的中间写入。

`configHash` 还只覆盖共享投影，不覆盖 topics、头像或桌面专有设置；同步却可能整文件写回。实施至少应读取最新完整配置后合并，分开检查共享契约版本与完整物理源快照，提交后再次核验，并在已发现冲突时停止/重试，而不是仅比较共享 hash 就声称保护全部字段。

若目标是严格保证桌面正常编辑与同步任意交错均无丢失，必须让所有相关群配置 writer 进入同一个持久化 owner/协调机制。这将涉及当前不允许修改的 VCPChat 非同步模块，**不能在当前授权范围内宣称已完成该保证**。本轮只记录边界；是否扩大范围是后续产品/工程决定。现有允许范围内的改进与严格跨 writer 保证应分别验收。

## 6. 建议的契约收口

### 6.1 保持一份可移植语义

建议以桌面当前 `modeSettings` 为模式配置语义来源：顺序、自然随机、邀请、JEV 各有明确结构。群公共配置、成员模型上下文窗口与 JEV 裁决窗口分别表达。

平铺 `memberTags`、`tagMatchMode`、`sequentialSpeakerOrder` 只作为桌面旧物理配置的读取/写回适配，不应成为第二份可独立编辑的同步权威。归一应在读取旧配置、应用新 DTO和新建配置三条路径中一致。写入受支持模式子树时保留桌面 `modeSettings.game` 等目的端专有分支，也保留已支持模式对象内不属于共享契约的专有 key；不能用整个 `modeSettings = incoming` 或整块 JEV 替换删除它们。

明确 `members` 是成员关系/展示顺序，`speakerOrder` 是顺序发言配置，两者不能借用同一数组。成员删除、重新加入、未知成员、重复 ID、已删除 Agent 墓碑的处理须与真实群业务一致。

### 6.2 收口重复映射，不先引入大框架

- **JS**：用统一字段描述、默认值和模式归一服务驱动 extract/apply/create/hash 输入。新建构造不再手写另一份同步字段清单；桌面专有字段的初始化仍由新建适配器负责。
- **CDS**：保留 Rust 边界，由单一群投影函数同时服务 ingest hash 和 entity pull。无需给 owners 表复制每个 JEV 字段，也无需修改消息传输路径。
- **Mobile**：由类型化群模式配置承担业务与持久语义，DTO 和 hash 共用 canonical 投影；避免 hash 再手写一份字段子集。SQLx 业务写入与 rusqlite 同步写入两个实际通路必须都覆盖，不能只改常用编辑路径。
- **跨语言一致性**：用仓库内相对路径的行为案例验证三实现对真实业务输入的相同投影、canonical JSON 和 hash。案例是检查手段，不反向定义产品；实现以桌面当前规则和本轮批准目标为准。暂不需要为少量群字段建设通用 schema/codegen 平台。

新增 JEV 浮点阈值必须明确数字 canonical 规则。JS [core/hash.js L23–29](E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/core/hash.js#L23) 仅对 `temperature` 的整数使用 `.0`；CDS [sync.rs L2485](E:/VCPChat/rust_chat_data_service/src/sync.rs#L2485) 也只特殊处理 temperature；Mobile [sync_types.rs L103](../../src-tauri/src/vcp_modules/sync/sync_types.rs#L103) 对 Number 使用 serde 数字文本。直接添加 Rust `f64` 字段可能出现 `1.0` 与 JS `1` 的 hash 差异，需覆盖阈值 0、1、普通小数与精度边界；不能随意照搬 temperature 两位小数规则改变阈值语义。

## 7. 最小完整修改清单

这是实施范围建议，当前均未修改。

| 子系统 | 必须覆盖的生产路径 | 目的 |
| --- | --- | --- |
| VCPMobile 群领域 | `group_types.rs`；`group_service.rs` 的单读、批读、新建、全保存、patch 保存；群执行消费方 | 模式配置有唯一类型和实际行为；字段不会在业务保存中丢失 |
| VCPMobile 持久化 | 新增 migrations；`db_manager.rs` 升级接入；`db_write_queue.rs::rusqlite_upsert_group` | 老库升级、同步落库、重启读取完整，迁移中断可恢复 |
| VCPMobile 同步 | `sync_dto.rs`；`sync_hash.rs` 的 canonical 与 `load_group_dto`；`sync_version.rs` | DTO/hash/DB装配一致，已存群先完成升级，再发新 manifest |
| VCPMobile 同步调用 | `push_executor.rs`、`pull_executor.rs`、`phase1_metadata.rs` 的实际调用验证；仅按迁移需要修改 | 确保类型新增被完整消费，不能假定泛型传输即代表落库完成 |
| 插件群契约 | `dto/group.dto.js`、`config/defaults.js` | extract/apply/create 同源，兼容物理旧字段，修复默认差异 |
| 插件 hash/index | `core/hash.js`、`core/db.js`、`index.js` 的投影升级接入 | canonical 数值一致，契约重 hash 不变成业务新编辑 |
| 插件写回 | `sync/entity.js`、`transport/routes.js` | 验证完整新群 DTO、正确应用模式子树、保留专有扩展，CDS成功提交后返回 |
| 插件握手 | `protocol.js` 及已有握手契约案例 | 明确拒绝不支持新契约的标准旧端 |
| CDS | `sync.rs` 群投影/hash；`ingest.rs`/`storage.rs` 的升级重投影处理 | 下行与插件一致，持久视图升级保持正确业务版本 |
| CDS 边界 | `domain.rs`、`sync_wire.rs`、`protocol.rs` 逐调用核验 | 目前 owner 群内容投影不在 sync_wire；不要为“字段清单看起来完整”而无意义改这些文件；配套版本识别按真实接入需要决定 |

VChat 的修改只能落在用户允许的 `VCPDistributedServer/Plugin/VCPMobileSync/**` 与 `rust_chat_data_service/**`。群业务文件仅作只读语义参照；不能通过改 `Groupmodules` 消除适配差异。

## 8. 验证矩阵

| 要推翻的不确定性 | 具体场景与应观察证据 |
| --- | --- |
| 三条投影不一致 | 同一真实群配置经 JS legacy、CDS、Mobile 得到相同 canonical JSON/hash；包括缺省、显式默认、空值、非 ASCII key、对象 key 顺序变化 |
| 数组误排序 | 只调换 `speakerOrder` 时 hash 变化，成员集合不变；只调换 JSON 对象 key 时 hash 不变 |
| 新字段漏 hash | 逐一改变 10 个 JEV 字段、顺序配置、两种历史窗口，只有相应群 config hash 改变；不要求消息 hash 改变 |
| 旧字段遮蔽 | 桌面 nested/flat 故意冲突，按当前权威读出；Mobile 修改自然标签和 natural→strict 后，桌面实际 getAgentGroupConfig 使用新值，再保存不会回滚 |
| 新建与已有写回行为不同 | 双向新建、更新、删除后恢复允许的业务路径分别验证；已存在桌面 game/未知专有分支、topics、头像保持其归属字段，不被模式配置应用误删 |
| DTO落库后丢字段 | HTTP pull → DbWriteTask::Group → 数据库 → 清缓存/重启 → group_service读取 → HTTP push → 桌面读取，最终字段值保持一致 |
| 成员生命周期破坏顺序或风格 | 重复/缺失成员、墓碑 Agent、加入/移除/重新加入；speakerOrder、成员Tags、JEV memberStyles分别按真实规则处理，不复用错误过滤策略 |
| 后端切换改变语义 | 同一数据用 legacy 与 CDS 分别完整同步；窗口字段、提示词默认、完整新模式配置一致，第二轮无无意义往返 |
| 新旧端混跑 | 新/旧 Mobile × 新/旧插件；新插件 × 旧CDS。观察握手、拒绝时点、数据库和配置文件无意外部分写入；不只检查一条错误日志 |
| 契约升级改变LWW输赢 | 两端业务值不变、只升级hash契约；业务updated_at保持。另加入Mobile更新群名未同步、桌面已有JEV设置，验证两类数据不会被迁移默认覆盖 |
| 升级重投影未完成 | 旧DB、首次安装、迁移一半中断重启、CDS clean fast-start、PHASE_START reconcile busy/failure；成功manifest必须使用新提交视图，失败不能宣称迁移完成 |
| JEV浮点编码漂移 | 0、1、0.16、高精度有限小数、范围外或非法值；先遵守业务归一规则，再对照跨语言字节与hash |

实施后跨层/核心变更须运行 `pnpm check`；同步和迁移案例分别运行相关 Rust/Node 测试。实际设备界面、JEV真实网络请求和双端真实同步各需自己的证据，不可互相替代。

## 9. 本轮证据与限制

已沿桌面群配置 → JS/CDS 两种投影 → manifest/hash → HTTP → Mobile 写队列/SQL → Mobile push → JS 写回追踪生产路径，并只读检查了当前 Git 工作区。

额外运行了一次纯内存 Node 探针，直接调用生产 `extractGroupDTO`、`applyGroupDTO`、`createGroupConfig` 与 `computeDtoHash`，使用虚构群 ID/成员，没有读取用户群数据或写文件。结果为：

- 顺序/JEV设置变化没有改变 hash。
- createGroupConfig 没有保存 modeSettings。
- applyGroupDTO 改了 flat 标签，但保留旧 nested 标签和 natural 模式。
- legacy 提取始终含两个群成员模型上下文窗口字段，默认邀请词确含 `[系统邀请指令:] ` 前缀。

没有执行真实双端同步、设备操作、数据库迁移或 JEV 请求；没有为这份只读审计运行完整编译。上述运行结果只验证被调用纯函数，链路其余结论来自明确的当前生产源码。
