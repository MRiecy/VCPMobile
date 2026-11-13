---
title: DTO 兼容与默认值审计
scope: 手机同步核心与桌面同步路径只读对照
related_files:
  - src-tauri/src/vcp_modules/sync/sync_dto.rs
  - src-tauri/src/vcp_modules/sync/sync_types.rs
  - src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs
  - src-tauri/src/vcp_modules/sync/sync_hash.rs
  - src-tauri/src/vcp_modules/persistence/db_write_queue.rs
last_updated: 2026-11-13
---

# DTO 兼容与默认值审计

## 1. 结论与变更范围

反馈描述的直接失败机制成立：原手机 `GroupSyncDTO` 拒绝未知字段，桌面群组数据一旦携带 `enableContextMessageWindow`、`contextMessageWindowSize`，就无法反序列化；外层 `EntityPullData` 的 `untagged` 匹配又把具体字段错误折叠为 `data did not match any variant of untagged enum EntityPullData`。

本次已将手机实体接收规则调整为：**以手机 DTO 投影已知字段，忽略额外字段，可选字段缺失采用手机强类型默认，必需字段缺失或已知字段类型错误仍拒绝。** 群组缺失 `tagMatchMode` 时复用 `GroupConfig` 的 `default_tag_match_mode()`，补为 `Some("strict")`；其它业务 Option 的类型默认均为 `None`，不自行改成空串、空列表或邀请文案。显式 null 与已提供的合法值保留，不增加必填字段补值、不接入桌面新增窗口配置。实体类型改由外层 `entityType + ownerType` 决定，消除按字段形状试错的歧义。实体 Pull 响应外壳允许附加字段，失败信息包含具体实体与具体反序列化原因。

后续裁决明确了方向：**兼容字段保留，桌面到手机可缺省，手机到桌面仍构造完整兼容投影。** Group.mode/createdAt 缺失分别使用既有配置默认 sequential/0；消息上传的 groupId/topicId/isGroupMessage 从权威 TopicKey 生成，附件输出继续使用本机元数据。具体来源与 null 语义见 [兼容字段方向规则](18_DTO字段业务归属调查.md)。这不代表将所有缺失字段都视为 PATCH 保留旧值。

需要同时区分三个结论：

1. **解析阻断已修复。** 四类实体 DTO 均采用同一接收策略，避免下次 Agent 或 Topic 添加字段时重复发生本次故障。
2. **Legacy 群组哈希漂移仍然存在。** 本机桌面 Legacy 路径把 12 字段纳入哈希，手机使用 10 字段；忽略字段不能使两种投影的哈希相等。
3. **当前本机 CDS 投影仍为 10 字段。** 不能把“CDS 已经采用 12 字段”当作此次审计证实的事实。具体反馈环境可能使用 Legacy 或不同构建，需结合其 `backendMode` 和实际响应判断。

审计基于 Mobile `5fa2eb09` 加本次同步修改，包版本 `1.1.7`；只读对照 `E:/VCPChat` 的插件 `2.0.0` 与 CDS 源码。未修改桌面、数据库 schema、发布流程及其它功能。反馈所述条目序号和 column 4396 没有原始响应可供独立复核，不影响对失败机制的判断。

## 2. 失败链路与类型判别

### 2.1 一个 DTO 为什么使整批失败

实际路径位于 [pull_executor.rs](../../src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs) 的 `PullExecutor::pull_entities_batch`：

```text
POST /api/mobile-sync/entities/pull
  → 读取有大小上限的响应
  → 反序列化完整 EntityPullResponse
  → 遍历 results，验证请求身份和结果状态
  → 提交写队列
```

原实现中，某个群组 DTO 不匹配会使完整响应解析失败，此时尚未进入本批的写队列提交。因此“前面的 Agent 匹配成功”不能表述为“这些 Agent 已经落库”。另一方面，已完成的其它批次可能已经写入；整个同步会话并不是一个可统一回滚的大事务。后续身份或状态校验也发生在执行遍历中，不能由“整批预解析”推导所有语义错误都发生在写入前。

### 2.2 为什么不能只删拒绝未知字段的注解

原 `EntityPullData` 顺序为 `Agent → Group → GroupTopic → AgentTopic`。Group Topic 的必需字段是 Agent Topic 的子集；如果只允许未知字段，合法 Agent Topic 会先匹配 Group Topic，随后被执行器拒绝。即使调整顺序，也仍依赖字段形状猜测，缺少 `locked/unread` 的 Agent Topic 会尝试匹配另一种实体。

[sync_types.rs](../../src-tauri/src/vcp_modules/sync/sync_types.rs) 现在先读取外层身份，再选择唯一具体 DTO：

| entityType | ownerType | 唯一允许的 DTO |
|---|---|---|
| owner | agent | AgentSyncDTO |
| owner | group | GroupSyncDTO |
| topic | agent | AgentTopicSyncDTO |
| topic | group | GroupTopicSyncDTO |

内部 `EntityPullData` 不再实现独立反序列化。`data` 的缓冲值仅用于按已知身份解码；一个 DTO 失败仍使完整批次解析失败，没有新增“跳过坏条目并报告成功”的行为。当前 HTTP 响应仍受 10 MiB 上限约束。

实现上 `data` 先进入 `serde_json::Value`；如果 JSON 对象包含重复键，映射保留最后一个值，随后校验这个值。本次没有建立独立的重复键检测，不能将必填与类型检查表述为对所有畸形输入都维持旧版完全相同的拒收行为。

缺少字段时，错误可明确到例如 `Entity pull topic/agent/agent-1/agent-topic data: missing field locked`，实际 Serde 输出会给字段名加反引号。执行器也不再把整段响应正文拼入错误日志，避免较大的批次内容遮蔽关键错误。

### 2.3 产品版本与 Wire 版本不是同一个条件

[sync_version.rs](../../src-tauri/src/vcp_modules/sync/sync_version.rs) 的 `parse_version_ack` 比较的是 Wire 协议版本，产品包版本用于记录和展示。初始故障审查快照使用 Wire `1.5`；当前要求以该文件为准，不由本文历史版本号固定。`2.0.0` 与 `1.1.7` 没有大小兼容比较；两个产品版本不同本身不是此次拒收机制。相同 Wire 版本内部的 DTO 字段集合发生漂移，才解释了握手通过、批量实体下载失败的现象。

## 3. 默认值机制逐层对账

### 3.1 手机 Wire DTO 的必填与可选字段

下表来自 [sync_dto.rs](../../src-tauri/src/vcp_modules/sync/sync_dto.rs)。字段缺失、字段为 `null`、字段为空值不是同一个输入。

| DTO | 必需字段 | 当前缺省行为 |
|---|---|---|
| Agent | name、systemPrompt、model、temperature、contextTokenLimit、maxOutputTokens、streamOutput | 7 项全部必需，没有整对象默认值 |
| Group | name、members、useUnifiedModel | mode/createdAt 缺失分别为 sequential/0；tagMatchMode 缺失为 Some("strict")；其余四个 Option 缺失为 None；Option 显式 null 仍为 None |
| Agent Topic | id、name、createdAt、locked、unread、ownerId、configHash、updatedAt | 8 项全部必需 |
| Group Topic | id、name、createdAt、ownerId、configHash、updatedAt | 6 项全部必需 |
| Message | id、role、timestamp、updatedAt、contentHash | content 缺失变空字符串；name、归属提示、finishReason、attachments 等 Option 可缺失 |
| Attachment | type、name、size、hash | extractedText、imageFrames、createdAt 可缺失 |

`Option<T>` 即使没有显式 `#[serde(default)]` 也有缺失为 `None` 的默认语义；有字段级默认函数时则调用该函数。`tagMatchMode` 现在与 `GroupConfig` 共用同一个默认函数，不复制 `"strict"` 字面量。`memberTags` 因使用自定义反序列化函数而显式声明 `default`；这不是给群组全部字段补默认值。其非空值必须是字符串到字符串的映射，空 key 仍由 [group_types.rs](../../src-tauri/src/vcp_modules/group/group_types.rs) 拒绝。

本次不将群组 10 个字段全部升级为必填，也不把缺失的 `members` 补成空列表、缺失的 `streamOutput` 补成 false。仅对已裁决的兼容字段 mode/createdAt 采用既有配置默认；它们仍参与回推与哈希，不驱动手机本机群模式。

### 3.2 可选字段按类型默认进入快照写入

同步实体是配置快照，手机群组写入不是 PATCH。可选字段缺失不会阻止接收，接收后采用手机类型的默认状态；[db_write_queue.rs](../../src-tauri/src/vcp_modules/persistence/db_write_queue.rs) 的 `rusqlite_upsert_group` 使用该 DTO 覆盖对应共享字段：

| 群组字段 | 缺失时的 DTO 值 | 同步写库结果 | 配置哈希中的规范值 |
|---|---|---|---|
| memberTags | None | `{}` | `{}` |
| groupPrompt | None | NULL | 空字符串 |
| invitePrompt | None | NULL | 固定默认邀请词 |
| unifiedModel | None | NULL | 空字符串 |
| tagMatchMode | Some("strict") | `strict` | `strict` |

哈希规范值来自 [sync_hash.rs](../../src-tauri/src/vcp_modules/sync/sync_hash.rs) 的 `compute_group_config_hash`，不等于类型默认值。四项类型默认仍为 None 的字段继续按上表写 NULL 或空标签；没有获得新的具体业务默认值之前，不用哈希层默认值替换它们。采用类型默认不等于保留手机旧值；若要按未提供字段保留旧值，需要另行定义 PATCH 语义。

显式 `tagMatchMode: null` 仍解析为 None、写入 NULL，哈希层仍规范为 `strict`；缺字段才调用强类型默认。显式 `""`、`"natural"` 等字符串原样保留，类型错误仍拒绝。

显式空字符串仍是一个值。例如 `invitePrompt: ""` 不会使用默认邀请词；只有 `None` 会触发该哈希默认。`memberTags` 的空标签和历史成员 key 可以保留，校验也没有要求每个 key 都属于当前成员集合。

### 3.3 本地功能默认值不应混入 Wire DTO

手机已经支持群组上下文窗口，存放在 `GroupLocalSettings`，默认开关为 false、窗口大小为 100，正常范围为 1–10000。读取异常本地设置时按本地规则规范化，与网络 DTO 接收是两个边界。

共享群组同步 upsert 不写 `group_local_settings`。因此忽略桌面这两个额外字段的实际效果是：已有手机窗口设置保留，新同步群组使用手机本地默认；桌面窗口设置不导入手机，手机窗口设置也不回推桌面。这与 Rust 子树 AGENTS.md 中群组本地参数独立的约束一致。

### 3.4 消息正文默认值存在独立取舍

`MessageSyncDTO.content` 当前允许缺失并补 `""`，显式 `null` 则不合法。缺失正文可以在 DTO 层被接受，而 `contentHash` 必须存在且为小写 64 位 SHA-256 字符串。Pull 写入路径使用远端传来的 `contentHash`，没有在该路径重新计算正文哈希并比较。

这意味着“缺失正文”可能被转换为空正文后落库；哈希格式正确并不能证明正文完整。空正文也可能是附件消息的合法值，所以后续若收紧，应区分“明确空字符串”和“字段未发送”，并核对桌面消息生产者。本次保留既有行为，没有顺手收紧消息契约或新增正文猜测。

通用消息存储中的 `updated_at.unwrap_or(timestamp)` 是本地消息对象的规则，不代表 Wire 的 `updatedAt` 可省略；它在 `MessageSyncDTO` 中仍为必填。

### 3.5 控制字段的 Option 经常是条件必填

| 边界 | 结构层缺省 | 执行层要求 |
|---|---|---|
| Entity Pull data/error | Option，缺失为 None | 成功必须有 data 且无 error；失败必须有 error |
| MessageDiff 的操作字段 | pullMessageIds、pushTopic、deleteMessages 为 Option | 成功必须同时给出三项；失败不得携带操作字段 |
| Manifest deletedAt | Option | 删除动作必须携带合法时间；非删除动作不得携带 |
| Owner contentHashMismatch | 缺失为 false | 内容根差异相关的 SKIP 分支要求为 true，不能将缺失视为需要同步全部内容 |
| 完成 ACK 的 sessionId/attemptId/nonce | Option | 最终完成必须与当前会话、轮次、随机值全部匹配 |

对应实现为 `pull_entities_batch`、[batch_diff_handler.rs](../../src-tauri/src/vcp_modules/sync/sync_executor/batch_diff_handler.rs) 的 `parse_topic_decision`、[diff_handler.rs](../../src-tauri/src/vcp_modules/sync/sync_executor/diff_handler.rs) 和 [sync_service.rs](../../src-tauri/src/vcp_modules/sync/sync_service.rs)。不能因结构中出现 `default` 或 `Option`，就把这些缺失操作转成“无事发生”。

### 3.6 数据库与辅助状态的默认值

Manifest 构建和 `sync_hash.rs` 的数据库加载主要使用 `try_get` 并带上下文返回错误，没有用空配置掩盖列读取失败；墓碑另建 deleted 状态。空聚合集合的内容根使用空字符串，是已有协议表示，并非缺失元数据兜底。

日志等级无效时回到 Info、传输统计无会话时取空快照、统计锁中毒时恢复已有状态，属于辅助状态的降级。这些不会为共享实体补齐内容，不应与业务字段的默认值合并评价。

### 3.7 没有具体业务默认值的可选字段清单

本节只说明类型层默认，不能据此判断字段无业务或可以删除。51 字段业务矩阵及最新裁决见 [DTO 业务归属与兼容字段方向规则](18_DTO字段业务归属调查.md)：7 项兼容字段均保留，接收可缺省，上传仍构造桌面所需投影。

本次核对的同步业务 DTO 共 15 个 Option。严格按类型定义看，**没有完全未定义默认状态的字段**：群组 `tagMatchMode` 默认 `Some("strict")`，其余 14 项默认 `None`。下面完整列出“只有 None、没有具体业务值可补”的字段，供后续决定是否要在强类型定义中新增业务默认值。

| DTO | 字段 | 手机类型默认 | 为什么不能直接补空值或推测值 |
|---|---|---|---|
| Group | memberTags | GroupConfig 明确 None | 空映射是存储/哈希规范值，新建群组另有显式初始化 |
| Group | groupPrompt | GroupConfig 明确 None | None 不追加群设定段，空字符串的消费行为不完全相同 |
| Group | invitePrompt | GroupConfig 明确 None | None 不注入邀请消息；补哈希默认邀请词会改变实际上下文 |
| Group | unifiedModel | GroupConfig 明确 None | 未指定时使用发言 Agent 自身模型，没有全局默认型号 |
| Message | name | ChatMessage 派生 Default 为 None | 部分消息不需要发送者名称，None 与空串的 fingerprint 投影不同 |
| Message | agentId | 同上 | 真实发言者身份不能补成空 ID，None 与空串的 fingerprint 投影不同 |
| Message | groupId | 同上 | 不是每条消息都属于群组，不能制造归属 |
| Message | topicId | 同上 | 持久化归属由外层 TopicKey 提供，不是静态默认 ID |
| Message | isGroupMessage | 同上 | 存储层对 None 的 false 折叠不是 DTO 的 Some(false) 默认 |
| Message | finishReason | 同上 | 没有实际结束原因时不编造 stop 等原因 |
| Message | attachments | 同上 | None 是无附件数据的类型状态；本次不改成 Some(empty list) |
| Attachment | extractedText | Attachment 派生 Default 为 None | 没有提取结果，不能用空串冒充已提取数据 |
| Attachment | imageFrames | 同上 | 没有帧结果，不能用空数组冒充已提取数据 |
| Attachment | createdAt | 同上 | 没有具体默认时间，不补 0 或当前时刻 |

权威来源为 [GroupConfig](../../src-tauri/src/vcp_modules/group/group_types.rs) 的 `Default` 与字段级 Serde 默认，以及 [ChatMessage/Attachment](../../src-tauri/src/vcp_modules/chat/chat_manager.rs) 的派生 `Default`。消息 `content` 不是 Option，但已允许缺失并补空字符串，与 `ChatMessage::default()` 一致，无需另加默认。

手机新建群组流程会显式初始化空标签、空提示词和完整邀请文案，UI 编辑页也会把 None 投影为空输入框；这些均不等于 `GroupConfig::default()`。尤其 `invitePrompt=None` 在实际上下文组装中不会注入邀请消息，因此不把新建值或哈希值倒灌为同步缺省。

控制面 Option 另见第 3.5 节：成功数据、删除时间、差异动作和最终 ACK 身份都需要本轮真实信息，没有可人为填入的业务默认，继续保留条件必填校验。

## 4. 桌面 Legacy 与 CDS 的差异

### 4.1 同一个插件下存在两套投影

以下路径均相对只读桌面工程 `E:/VCPChat`：

| 环节 | Legacy | CDS |
|---|---|---|
| HTTP 路由 | `VCPDistributedServer/Plugin/VCPMobileSync/transport/routes.js` → `sync/entity.js` | 同一路由检测 centralSync 后进入 `sync/central.js` |
| 群组下载 DTO | `dto/group.dto.js` 的 GROUP_SYNC_FIELDS 与 extractGroupDTO，12 字段 | `rust_chat_data_service/src/sync.rs` 的 pull_entity → mobile_owner_sync_dto_from_value，10 字段 |
| 群组配置哈希 | `core/hash.js` 的 computeDtoHash，使用 12 字段白名单 | 同一 Rust canonical 投影生成配置哈希，10 字段 |

桌面 JS 的 `GROUP_SYNC_FIELDS` 明确加入两项窗口字段，而 CDS 的群组 canonical 函数仍只列手机的 10 项。CDS Pull 还会将生成的 DTO 哈希与已提交索引核对，然后返回该 DTO。因此仅根据 JS 白名单就推断中央数据库链路一定返回 12 字段，会跳过实际路由分流。

### 4.2 仅忽略字段不能解决 Legacy 的哈希差异

使用桌面当前真实 `extractGroupDTO` 和 `computeDtoHash`，对虚构输入 `{name: "Audit Group", createdAt: 0}` 做只读实验：

```text
桌面提取字段数：12
窗口默认值：false / 100
12 字段哈希：f9036fd3e2c149bf25b1ad5e4369668a4cabd720920d7681e452f351a990ffe4
10 字段投影：1cf7c42bb1bf4d9578362a78a280db3cdad3dea62e8671efceae418da8e92c79
```

第二个值使用相同 JS canonical 哈希函数，只从白名单中去掉两个窗口字段，以单独验证字段集合的影响。它不是对用户群组或真机运行数据的采样。即使新字段都是默认值，两种哈希仍不同。

Legacy 的 `sync/manifest.js` 在配置哈希不同时继续按 `updatedAt` 决定 PUSH/PULL。手机拉取后只计算自己的 10 字段哈希，所以下轮仍可能判为配置差异；方向取决于时间戳和当前状态。本次不会在手机保存一份额外远端原始 JSON、伪造哈希或扩大本地 DTO 来掩盖这个分歧。

后续要达到 Legacy 稳定收敛，需要在同步边界统一参与哈希的投影，或定义经过协商的投影版本。这个范围与改写 CDS 数据库 schema 不是同一件事。当前本机 CDS 已有 10 字段投影，值得先确认反馈设备实际使用哪条路径，再决定是否需要桌面同步插件修正。

### 4.3 手机回推不会自动清空桌面窗口设置

手机 `GroupSyncDTO::from` 仍只提取 10 字段。桌面 `applyGroupDTO` 只覆盖 `dto[field] !== undefined` 的字段，因此更新已有群组时，未收到的窗口字段保留原值；新建群组由 `createGroupConfig` 取 false/100。CDS 模式的实体 push 同样先走 JS 上传，再 reconcile 中央视图。

这只说明字段保留规则，不能替代前述哈希收敛判断。

### 4.4 邀请词默认值还有另一处分叉

桌面 JS `config/defaults.js` 的默认邀请词带 `[系统邀请指令:] ` 前缀；CDS canonical 投影与手机哈希默认邀请词不带这个前缀。显式传来的邀请词会被保留，因此并非所有群组都会因此发生差异；问题发生在缺失值经不同默认工厂补齐的路径。

后续应统一“共享字段的缺省规范值”，同时保持已存显式值不被默认值重写。本次没有更改邀请文案、桌面默认工厂或已有数据。

## 5. 过度严格门禁与必要校验

### 5.1 本次已优化的门禁

| 门禁 | 原行为 | 本次结果 |
|---|---|---|
| 四类 Entity DTO 的未知字段 | 任意新增字段拒绝整个 DTO | 忽略额外字段，保留必填与已知类型要求 |
| Entity Pull 响应及条目附加字段 | 外壳新增字段也拒绝 | 接受外壳附加字段；嵌套 WireSyncError 仍沿用自身契约 |
| data 类型选择 | untagged 多变体试错 | 按外层完整类型判别，错误指向具体实体 |
| 群组 tagMatchMode 缺省 | Wire 缺失为 None，与 GroupConfig 默认不一致 | 缺失复用强类型默认 strict；显式 null、空串和合法值保留 |
| 单向兼容字段 | 手机无独立消费者的字段仍被当作删除候选 | 保留字段；Group.mode/createdAt 允许缺省；消息上传按外层身份补齐，附件输出明确保留兼容键 |
| 解析错误输出 | 泛化 untagged 错误并拼接完整响应 | 具体 DTO 原因与身份；不再输出整批正文 |

### 5.2 仍存在的优化候选

这些是审计结论，不是已经实施的兼容扩展。

| 优先级 | 位置与当前行为 | 问题与合理方向 |
|---|---|---|
| 高 | `SyncLogEventFrame`、`DesktopPhaseEventFrame` 拒绝未知字段；部分只用于记录或未消费的 phase/ts 仍必填 | 辅助日志格式漂移会触发 FailAttempt，中止业务。应将日志可用性与同步数据正确性解耦，明确只对实际消费字段设要求 |
| 高 | `parse_ndjson_frame_inner` 要求 streamError 恰好 2 个字段；Topic 错误帧恰好 6 个字段；成功帧检查精确 key 集合 | 添加诊断信息也会拒收。可按已知字段解析，保留 kind、身份、ok/error 和消息数组语义；不能直接删除所有检查，因为精确 key 校验目前还间接禁止失败帧携带 messages |
| 中 | Entity Push、Avatar Push、Manifest、TopicDiff、MessageDiff、MessagePush 多个响应使用 deny_unknown_fields | 可逐个允许附加字段，同时保留动作枚举及成功/失败互斥。需要确认每个已有字段的实际消费方式 |
| 中 | VERSION_ACK 与 VersionClaim 禁止附加字段，versions 数量必须恰好两项 | 元数据扩展与协议不兼容被耦合；可单独审议额外描述字段。未知 component 或 Wire 版本变化涉及协商语义，不能自动猜测兼容 |
| 中 | WireSyncError 禁止附加字段，枚举值封闭 | 新诊断字段可能遮蔽原始业务错误。可容忍附加元数据；已知错误码的 kind/retry 冲突仍需约束，未知动作不能默认为自动重试 |
| 中 | Vue 同步启动前检测省电模式和电量，省电开启或已知电量低于 30% 时直接失败 | 这是应用策略，不是已观察到系统拒绝网络或数据库；低电量判断还未消费充电状态，充电中的 29% 同样被挡。可单独审议告警或用户继续方式 |
| 中 | 移动端配置直接拒绝 loopback 地址 | 对普通局域网误配置有帮助，但本机代理或 ADB reverse 等实际转发也使用 loopback。它是连接策略，不是 DTO 或 Wire 必要条件；如要支持此类链路，应明确产品边界后调整 |
| 中 | 实体按数量分批但另有 10 MiB 请求/响应上限 | 多个大实体可能整体超限。可以设计按字节预算分批，而不是取消上限或截断实体；单实体本身超限仍需明确处理 |

`OwnerManifestState`、`TopicManifestState`、`AvatarManifestState`、`MessageVersionState` 仍有 untagged 的 live/deleted 结构。当前主要在本机构造发送状态，不能机械删除其内部严格限制：同时带 live 和墓碑字段的对象可能因枚举顺序被错误分类。应先定义显式判别或互斥条件，再讨论扩展接收。

错误注册表也不是简单的“未知 code 一律拒绝”：当前 `validate_wire_error` 对语法合法的未知 code 可继续承载，对已知 code 则校验 kind/retry 一致性。应保留这种事实区别。

设备门禁的所有者是 [syncSession.ts](../../src/core/stores/syncSession.ts) 的启动流程：调用 `get_battery_status` 后在 Vue 决定是否阻断。Android 返回的省电状态还合并系统与部分厂商标记，iOS 返回系统低功耗模式观测。未知、越界电量或原生查询异常只警告后继续，没有把未知电量伪造成 100%；这条同步启动链路也没有 Wi-Fi-only 或蜂窝网络禁令。低电量、省电策略本次只审计，不修改原生行为或 UI。

### 5.3 应继续保留的校验

| 类别 | 保留原因 |
|---|---|
| 请求和结果完整对应，拒绝重复、越界和遗漏 | 防止漏同步、重复操作或把不属于本轮的数据写入 |
| ownerType/ownerId/topicId 与 Topic DTO 内部身份一致 | 同名话题可属于不同 Owner，必须防止串写 |
| 成功必须含数据、失败必须含错误，矛盾状态拒绝 | 完成结果必须可解释，不能将失败当空数据成功 |
| 已知字段类型、必需字段、Topic 哈希格式与更新时刻范围 | 防止缺失或类型错误数据进入持久化；不应做字符串到数字等隐式纠错 |
| 删除动作、墓碑、父实体及消息归属约束 | 避免已删除数据复活或跨层关系损坏 |
| Wire 版本与未知业务动作拒绝 | 没有对应操作语义时，字段容忍不等于业务可执行 |
| 写队列 drain、收尾 hash 冒泡、最终 ACK 绑定 | 确认已接受写入完成，避免旧 ACK 或未落盘数据产生成功终态 |
| 现有会话互斥、生成中检查 | 避免同步与未完成写入/消息生成争用。是否支持更细粒度并行属于独立状态机设计 |
| 响应、NDJSON 单行与并发预算 | 有界内存和背压，与排斥新增字段的过严校验性质不同 |

不要把“允许未知字段”推广为“忽略所有错误”。本次改动没有放松请求身份、删除动作、未知实体类别或成功终态。

生成中检查读取真实 `active_generations` 标记，但它是启动时检查，不是与消息生成入口共享的全程互斥锁，不能据此承诺同步过程中绝不产生新消息。iOS 在异步配置读取前后检查前台状态，后一次用于避免 await 期间进入后台；这属于实际生命周期条件。

### 5.4 已有的优化与合理降级

当前代码并非处处都过严，以下机制已降低慢网络、大数据量和可恢复辅助状态对同步的影响：

- HTTP 设置 10 秒连接超时，没有为整个数据请求设置统一墙钟总时限；NDJSON Pull 同样不按总传输时长中止。
- Pull worker 满额时等待槽位，不因话题总数多而直接拒绝；消息 Pull worker 并发 4，Entity/Avatar 操作并发 3。
- Entity Pull 批次预算 10 MiB、头像响应 20 MiB、NDJSON 单行 32 MiB；这些限制保护手机内存，应与分批策略共同维护。
- 自动 attempt 重启仅允许 `HTTP_TRANSPORT_FAILED` 与 `SYNC_SNAPSHOT_STALE`，最多 3 次重试，等待从 500ms 递增。必填字段缺失不会因重试变正确，因此没有给 DTO 错误自动重试是合理的。
- 中断路径会完成已接受写入的 flush/reconcile 并失效缓存。它保留已完成部分，不假装整次同步完全回滚，也不直接丢弃后台写入。
- 原生同步 `ForegroundLease` 申请失败仅警告继续；日志初始化失败同样不直接阻断数据同步，没有将辅助能力重复升级为必需权限门禁。

## 6. Magi 综合裁决与后续顺序

三方审查结论一致：

- **Melchior**：允许未知字段必须配合显式实体类型判别；保留整批预解析、身份和类型约束，不引入不受控缓冲或默认值补洞。
- **Balthasar**：手机窗口参数继续作为本地设置；错误应能指出具体实体。缺失可选字段覆盖旧值、消息正文缺省、成功提示的证据范围必须说清楚。
- **Casper**：本次在手机实体接收边界完成修复，桌面只读对照；优先报告 Legacy 哈希漂移，避免为消除表面报错而建立另一套远端状态缓存。

后续处理顺序建议为：先识别受影响环境的 `backendMode`，确认 Legacy 是否持续产生群组配置差异；再统一同步哈希投影与缺省规范值；随后分别处理辅助日志门禁、NDJSON/响应附加字段策略和消息正文缺省语义。每一步都应以实际生产者和消费方为依据，避免全局删除严格注解或全局添加 default。

当前 `SyncFinalizer` 的“一致性校验成功”主要表示本地写入和哈希冒泡完成，没有在这里再比较双端最终根哈希。因此它不能作为 Legacy 两种投影已收敛的证据。后续验证应观察同一数据不再修改时的第二轮 Manifest 是否仍持续产生配置 PUSH/PULL。

## 7. 验证结果与证据范围

本轮完成以下验证：

| 验证 | 结果与覆盖范围 |
|---|---|
| `pnpm check` | 通过；配置契约一致性、ESLint、Vue TypeScript、构建配置 TypeScript 与 cargo check 全部通过 |
| `cargo test --locked --lib vcp_modules::sync::` | 73 项通过；包含 sync_types 的 9 项测试及真实上传构造测试，覆盖接收缺省、完整兼容输出、执行器、差异处理、墓碑、哈希、收尾、会话、传输和日志 |
| `cargo test --locked --lib sync_group_upsert_preserves_local_mode_and_parameters` | 1 项通过；缺省 strict 经真实写入方法落库，手机群组本地参数不被覆盖 |
| DTO 与群组类型的 `rustfmt --check --edition 2021`，新增测试代码段格式校对 | 通过；同文件并行修改保持局部编辑，没有整体格式化其它任务的代码 |
| 同步代码与文档范围的 `git diff --check` | 通过 |
| 本机桌面真实模块的只读 Node 实验 | 默认窗口值下 12 字段与 10 字段投影哈希不相等，见第 4 节 |

新增测试使用虚构数据，覆盖：

- 9 个 Agent、带两项桌面窗口字段的 12 字段 Group，以及两类 Topic 同处一个批量响应时均可解析。
- 四类 DTO 和 Pull 外壳的附加字段可忽略；反序列化后再序列化仅保留手机已知字段。
- Agent Topic 不被 Group Topic 截获；Group Topic 即使带 `locked/unread` 仍按外层 group 身份解析。
- 四类实体 DTO 当前共 24 个必需字段逐项删除或设为 null，共 48 种反例均拒绝，缺字段错误包含实体与字段；Group.mode/createdAt 的缺失改为既有默认，显式 null 仍拒绝。
- 群组可选字段缺失采用强类型默认：tagMatchMode 为 strict，其余四项为 None；忽略窗口字段不改变手机群组哈希。
- tagMatchMode 显式 null、空串及 natural 保留，错误类型拒绝；缺省 strict 通过真实同步写入方法落库，本地群组参数保持不变。
- 桌面兼容字段缺失可接收；Group.mode/createdAt 仍在序列化输出中。消息上传使用真实 TopicKey，测试包含错误的历史 groupId/isGroupMessage 镜像，确认输出改由外层生成，agentId/contentHash 保持原值。
- 附件上传保留本机 imageFrames 数组或明确 null，createdAt 使用本机记录值；不把这份兼容投影声称为所有远端附件元数据的无损回传。
- 外层身份/ok 缺失、未知实体类别、已知字段类型错误、非法 memberTags、非法 Topic 哈希或更新时间仍拒绝。

这些结果支持手机代码层面的协议修复和现有同步回归保持通过。本轮没有连接反馈用户的实际 VChat 服务，也没有构建安装真机包；不据此宣称事故原始报文已重放、Android/iOS 设备同步已验证或 Legacy 最终哈希已收敛。
