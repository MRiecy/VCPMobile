---
title: 同步 DTO 业务归属与兼容字段方向规则
scope: 手机业务消费与同步边界
status: 已裁决保留兼容字段，接收可缺省，上传构造完整桌面投影
related_files:
  - src-tauri/src/vcp_modules/sync/sync_dto.rs
  - src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs
  - src-tauri/src/vcp_modules/sync/sync_executor/push_executor.rs
  - src-tauri/src/vcp_modules/persistence/db_write_queue.rs
  - src-tauri/src/vcp_modules/persistence/message_repository.rs
last_updated: 2026-11-13
---

# 同步 DTO 业务归属与兼容字段方向规则

## 1. 结论

手机业务消费和桌面兼容输出是两个方向的要求。已按用户裁决保留全部兼容字段：**桌面到手机允许这些字段缺失，手机到桌面仍按已保存值、手机既有默认或权威会话身份构造桌面需要的投影。** 不因某字段没有独立手机业务消费者就从 DTO 删除。

前次调查指出的 7 项继续保留，删除建议已撤销。完整调查仍覆盖 6 类 DTO、51 个字段；原先 14 个 Option 中有 9 项真实手机业务字段、5 项兼容投影字段，这一消费事实不变，但不再据此得出删字段的结论。

| DTO | 字段 | 手机端现状 | 已落实的方向规则 |
|---|---|---|---|
| Group | mode | 手机执行使用独立本机 chatMode；共享 mode 参与哈希和回推 | 接收缺失用现有默认 sequential，有值保留；回推继续携带 |
| Group | createdAt | 主要是共享存储、哈希和回推 | 接收缺失用配置默认 0，有值保留；回推继续携带，0 是兼容占位 |
| Message | groupId | 外层 ownerType/ownerId 是归属权威 | 接收可缺；群消息上传取外层 ownerId，单聊明确输出 null |
| Message | topicId | 外层已有必需 topicId | 接收可缺，提供时保留既有一致性检查；上传始终取外层 topicId |
| Message | isGroupMessage | 手机群聊语境由 ownerType 决定 | 接收可缺；上传由 ownerType 生成 true 或 false，不依赖旧镜像列 |
| Attachment | imageFrames | 手机视频抽帧不读此字段，但桌面仍需要兼容形状 | 接收可缺；上传已有本机数组，无值明确输出 null，不伪造抽帧结果 |
| Attachment | createdAt | 接收值未直接用于落库，上传来自本机附件表 | 接收可缺；继续从本机表构造上传值，不在出口生成当前时间 |

**必须保留的典型字段**包括群组成员标签、群组提示词、邀请提示词、统一模型、消息发言助手 ID、消息终态、附件列表和附件提取文本。这些字段会改变谁发言、用什么模型、模型看到什么内容、消息如何显示或朗读。

其中 `Message.agentId` 应当按业务条件要求：**群聊 assistant 消息必须能确定真实发言助手**。外层群组 ID 不能替代它；user/system 消息又不能机械地全局强制一个助手 ID。

初次调查快照为 Mobile `070bd18e` 加本会话已有改动；此后按上述裁决调整了 Group 缺省、消息上传投影、兼容字段序列化、注释和验证。桌面 `E:/VCPChat` 仍仅作只读对照，没有修改数据库 schema，也没有改变群组共享哈希字段集合。

## 2. 按业务解释忽略 可选 必需

当前以方向为边界解释三类规则；尤其不能把接收宽容扩大成上传删字段：

| 类别 | 判断依据 | 不足以作为依据的现象 |
|---|---|---|
| 忽略 | 对端新增且不在手机 DTO 中的字段忽略；已有兼容字段允许接收缺省，不要求驱动手机业务 | 手机无独立消费者不等于可删除桌面兼容输出 |
| 可选 | 手机有业务，且字段缺失时有清楚、可执行的业务默认 | Rust 能构造 None、空串或 0，不自动证明该值能替代真实业务数据 |
| 必需 | 手机需要真实身份、版本、内容或条件成立时的必要信息，不能补造 | 消费层为避免崩溃提供显示回退，不代表已经恢复真实身份或历史事实 |

“必需”可以由 role、ownerType 或功能条件决定，不必把整个字段无条件设成必填。例如群聊 assistant 的发言人必需，user 消息不需要假助手身份。

上一轮将 `Option::None` 作为类型层默认进行了对账，但那不足以完成业务分类。这次将其分开：`memberTags=None` 有“没有标签”的正常业务解释；群聊 assistant 的 `agentId=None` 却会使音色选择失败，二者不能因为 Rust 类型一样就采用同一政策。

同步自身也属于业务：消息主键、版本时间、哈希以及墓碑虽不一定显示在 UI，仍承担正确寻址和变更判断。兼容输出也是保留字段的明确消费方；本次保留群组共享哈希投影，不因手机本地业务分离而重写桌面契约。

本机专属设置另有既定边界。手机实际上支持群聊上下文窗口，但窗口开关/大小归 `GroupLocalSettings`，不属于共享同步；来自桌面的同名额外字段继续忽略，是因为设置归属不同，不是手机没有该功能。

## 3. 为什么这些字段曾经成为 Option

Git 历史支持三种不同来源，不能统一解释为“功能后来废弃”：

| 提交 | 可核实的变化 | 对本次判断的意义 |
|---|---|---|
| `056a4d36`，2026-08-25 | User、Agent、Group 三类消息 DTO 合并成 MessageSyncDTO；去掉缺失 ID 补空串的旧构造方式，直接保留 Option | 不同角色共用一个类型，导致 agentId、finishReason 等只在部分消息上适用；这不是废弃证明 |
| `66513be6`、`6211ff74`，2026-08-23 | 贯通 TopicKey/MessageKey 复合身份 | 外层身份逐渐成为权威后，内层 groupId/topicId/isGroupMessage 成为重复投影 |
| `5d7e8b4c`，2026-10-20 | 删除共享 mode 的执行回退，改用独立 GroupLocalSettings.chat_mode | Group.mode 确有“业务执行已分离、字段仍留在同步链路”的证据 |
| `fbb960ad`，2026-09-01 | Topic 增加 configHash/updatedAt，同时删除 AgentTopic Wire 上 locked/unread 的默认 | 某些严格必填是后来主动收紧的版本契约，不是这些字段没有业务默认 |
| `c728733e`，2026-08-25 | 清理部分附件同步路径，删除 status，但保留 imageFrames/createdAt | 两个字段留在搬运链；尚不能据此断言手机曾经消费它们后又删除了对应功能 |
| `3242e4a9`，2026-08-28 | memberTags 从任意 JSON 强化为 MemberTags 映射并增加校验 | 该字段仍持续维护，与当前自然发言业务相符 |

因此更准确的表述是：**有些是不同角色共用 DTO 的条件字段，有些是权威身份迁移后的重复投影，有些是历史透传遗留，还有些仍是活跃配置。**

## 4. 全部 51 个字段的业务矩阵

“当前”列描述业务字段接收契约；7 项兼容字段的方向规则已落实。其余标为“候选”的默认值放宽或条件必需仍是独立审议项，本次没有据此扩大修改范围。

### 4.1 AgentSyncDTO 共 7 项

7 项都仍有真实业务；当前 Wire 全部必填，但手机完整配置已有具体默认。按新标准，均可列为“可选并补手机默认”的候选，不能因为目前不是 Option 就永远维持必填。

| 字段 | 当前 | 手机实际用途 | 建议及缺省值 |
|---|---|---|---|
| name | 必填 | 助手列表、设置、会话名称、上下文及通知 | 可选，`Unnamed Agent` |
| systemPrompt | 必填 | 单聊/群聊使用的系统提示词；手机专用提示词不为空时才覆盖它 | 可选，空字符串 |
| model | 必填 | 实际请求模型路由 | 可选，`gemini-2.5-flash`；缺失后会明确使用该型号 |
| temperature | 必填 | 本机 useTemperature 开启后发送采样参数 | 可选，`1.0`；开关关闭不代表字段无业务 |
| contextTokenLimit | 必填 | 实际请求上下文 Token 限制 | 可选，`1_000_000` |
| maxOutputTokens | 必填 | 实际请求 max_tokens | 可选，`64_000` |
| streamOutput | 必填 | 流式/非流式执行与对应生命周期 | 可选，`true` |

依据：[AgentConfig 默认定义][A1]、[单聊实际请求组装][A2]、[群聊系统提示词选择][G1]。`agent_service` 对 Vue 编辑投影隐藏共享 systemPrompt，不代表 Rust 内部已停止消费它。

这些是当前手机默认，不能保证缺失 model 后使用的默认型号适合每个服务端；这是接受该可选策略时会发生的实际行为。

### 4.2 GroupSyncDTO 共 10 项

| 字段 | 当前 | 手机实际用途 | 建议及缺省语义 |
|---|---|---|---|
| name | 必填 | 群列表、设置、会话名称和上下文中的群名 | 可选，`Unnamed Group` |
| members | 必填 | 成员管理、选择可发言 Agent、群调度 | 可选候选，空列表；列表内提供的每个 ID 必须是真实成员身份 |
| mode | 缺失 sequential | 不再驱动手机执行，主要存储/哈希/回推 | 保留兼容字段，有值不改，上传继续携带 |
| memberTags | Option | 自然随机发言中的标签命中、概率、顺序及编辑界面 | 可选，业务默认“没有标签”；无需伪造标签 |
| groupPrompt | Option | 给每个成员系统提示词追加群设定；JEV 判断也使用 | 可选，缺失不追加群设定段 |
| invitePrompt | Option | 替换发言助手名后，作为邀请消息加入本次模型上下文 | 可选，缺失不注入邀请消息；不自动补哈希层长文案 |
| useUnifiedModel | 必填 | 群组模型路由开关 | 可选，`false` |
| unifiedModel | Option | 开关启用且模型非空时采用统一模型 | 可选，缺失继承各发言 Agent 的模型；这是现有实际回退 |
| tagMatchMode | Option，缺失 strict | 自然随机模式的 strict/natural 标签匹配策略 | 可选，`strict` |
| createdAt | 缺失 0 | 主要存储/哈希/回推 | 保留兼容字段；0 仅为既有配置默认占位，上传继续携带 |

依据：[群组类型与本机模式][G2]、[群组上下文][G1]、[群组模型/邀请与调度][G3]、[标签与发言策略][G4]、[群设置界面][G5]、[群组配置哈希][H1]。

两个重要细节：

- `AgentList` 显示的 `group.mode` 来自本地有效模式投影，不是读取同步 mode 后执行。不能根据同名 UI 属性误判同步字段仍有业务。
- 缺 members 补空列表会使同步快照成为空群；写队列会重建成员关系。这不是“缺失则保留旧成员”。如果用户不接受这个结果，就应把 members 列为必需集合，而不是将空列表默认藏在实现里。

### 4.3 AgentTopicSyncDTO 共 8 项

| 字段 | 当前 | 手机实际用途 | 建议 |
|---|---|---|---|
| id | 必填 | 话题定位、消息父身份、导航、元数据哈希 | 必需，必须对应外层 topicId |
| name | 必填 | 话题标题、重命名；没有统一的实际远端标题默认 | 必需 |
| createdAt | 必填 | 话题分组/排序、活动时间回退、元数据哈希 | 必需，不能拿当前时间或 0 补造历史时间 |
| locked | 必填 | 话题菜单切换、解锁标记、共享状态与哈希 | 可选候选，但先统一 true/false 默认冲突 |
| unread | 必填 | 未读红点、聚合未读、标记已读与计数清零 | 可选候选，默认 `false` |
| ownerId | 必填 | 完整父身份与 live 父对象验证 | 必需，必须对应外层 ownerId |
| configHash | 必填 | 远端话题配置版本、差异判断和 Owner 内容根 | 必需，不补造 |
| updatedAt | 必填 | 配置版本时间、LWW、后续清单 | 必需，不补当前时间 |

依据：[Topic 字段定义][T1]、[话题业务与创建入口][T2]、[话题列表交互][T3]、[同步话题落库][D1]。

`locked` 目前有 UI/状态消费，不能删除；但未发现手机聊天或删除逻辑以该字段作阻断条件，因此不将它描述为“手机删除保护”。其默认有真实冲突：Topic 的字段级 Serde 默认是 false，创建入口与数据库默认是 true。**当前 Topic 没有实现 Default；旧文档中的 Topic::default() 说法不能作为依据。**

locked/unread 加入可选策略时，还需确保补值后的元数据与远端 configHash 对应。当前落库直接保存远端 hash，仅为字段补值并不能证明版本一致。

### 4.4 GroupTopicSyncDTO 共 6 项

| 字段 | 当前 | 手机实际用途 | 建议 |
|---|---|---|---|
| id | 必填 | 群话题定位、消息父身份、导航、元数据哈希 | 必需，对应外层 topicId |
| name | 必填 | 群话题标题与重命名 | 必需 |
| createdAt | 必填 | 话题时间分组/排序、活动时间回退 | 必需 |
| ownerId | 必填 | 群归属及 live 父群验证 | 必需，对应外层 ownerId |
| configHash | 必填 | 配置版本与差异判断 | 必需 |
| updatedAt | 必填 | 配置版本时钟 | 必需 |

GroupTopic 没有 locked/unread 两字段；同步新建时本地表给固定值，UI 也没有 Agent 话题那套切换入口。对端额外传来的同名字段可继续忽略，不需要为“类型一致”加入群话题 DTO。依据同上及 [Topic 哈希投影][H1]。

“身份必需”指完整实体必须有一份可信身份，不是要求每层都复制它。当前 Topic DTO 与外壳各有 ID，必须一致；若以后也统一到外壳，可在完整实体进入领域前归一。但 Topic.id 还参与元数据哈希与写入，这不同于目前已经完全由外壳替代的 Message.topicId。

### 4.5 MessageSyncDTO 共 13 项

| 字段 | 当前 | 手机实际用途 | 建议 |
|---|---|---|---|
| id | 必填 | MessageKey、去重、编辑、删除、附件关系、缓存与哈希叶子 | 全局必需 |
| role | 必填 | 消息布局、上下文角色、群聊统计与朗读作者判定 | 全局必需；不用默认 user/assistant 猜测 |
| name | Option | 历史署名、群聊发言前缀、JEV speakerName、消息指纹 | 保留；是否允许“无历史署名”需明确，不能把显示回退名写回作为默认 |
| content | 缺失空串 | 正文、搜索、渲染、上下文和指纹 | 可选候选，现行默认空串；附件消息允许空正文，但缺失正文的完整性取舍需知晓 |
| timestamp | 必填 | 历史排序/分页、UI 时间、群聊距离统计与消息指纹 | 全局必需 |
| updatedAt | 必填 | 消息版本、冲突、重试与 Diff | 全局必需 |
| agentId | Option | 群聊真实发言者、头像、音色、自然发言策略、JEV 统计、消息指纹 | 至少群聊 assistant 条件必需；单聊作者可由外层 Agent 身份确定 |
| groupId | Option | 外层 owner 身份是权威，内层是桌面兼容投影 | 保留可缺省接收；群上传 ownerId，单聊上传 null |
| topicId | Option | 提供时核对外层 topicId；落库/加载不以它为权威 | 保留可缺省接收；上传取外层 topicId |
| isGroupMessage | Option，写库缺失 false | 真实群聊语境由 ownerType 决定 | 保留可缺省接收；上传取 ownerType==group，不依赖库内旧值 |
| finishReason | Option | assistant 终态、已停止提示、流投影退休、终态提交幂等 | 保留；已完成 assistant 同步消息可列条件必需候选，不默认 completed |
| attachments | Option | 附件显示、CAS、文档文本、多模态上下文、重生成与消息指纹 | 可选，业务默认无附件；缺失代表空集合，不是保留旧关系 |
| contentHash | 必填 | 同步变更检测、消息聚合根、渲染缓存有效性 | 全局必需；格式正确不等于内容已验证 |

依据：[消息 Pull 解析与外层身份][P1]、[消息快照写入][D1]、[消息加载与名称投影][M1]、[实际上下文消费][M2]、[群聊朗读作者判定][M3]、[流投影终态判定][M4]、[消息展示][M5]、[消息指纹][H1]。

name、agentId、finishReason 的角色条件见第 5 节。它们有实际业务，不能因缺省为 None 就全部忽略，也不能为所有 role 统一补值。

### 4.6 AttachmentSyncDTO 共 7 项

| 字段 | 当前 | 手机实际用途 | 建议 |
|---|---|---|---|
| type | 必填 | 附件分类、预览、图片/音频/视频进入模型的判断 | 保留；当前宜维持必需，空 MIME 的 UI 回退与后端媒体行为尚不一致 |
| name | 必填 | 文件名显示、扩展名分类、上下文文件标签及打开/分享名称 | 可选候选，需统一“未命名”语义；现有空串与上下文“附件”回退并非全端一致 |
| size | 必填 | 大小显示、CAS 文件长度校验、多模态文件元数据 | 必需；0 是合法空文件长度，不能直接兼作未知大小 |
| hash | 必填 | 附件内容身份、CAS 定位、主键、消息附件关系与消息指纹 | 必需，不能补空 hash |
| extractedText | Option | 原件不在手机时仍可把桌面提取文本送入模型；本地有文件时也用于文档上下文 | 可选，默认无提取结果；必须保留该字段 |
| imageFrames | Option | 桌面兼容元数据，不驱动手机抽帧 | 保留；接收可缺，上传已有数组或明确 null |
| createdAt | Option | 同步接收值未用于落库，回推来自本机表 | 保留；接收可缺，上传继续按本机元数据构造 |

依据：[附件转换与加载][M1]、[文档/多模态上下文][M2]、[同步附件写入][D1]、[CAS 实体长度校验][F1]、[前端附件分类][F2]、[Push 附件重建][P2]。

这里不能机械照搬 `Attachment::default()` 的空串/0：

- UI 的 `AttachmentClassifier` 能在 MIME 为空时根据文件扩展名分类，但上下文层先用 MIME 的 image/audio/video 前缀判断是否产生 local_file；空 MIME 可能让一个仍可预览的媒体不进入模型上下文。
- CAS 的 `validate_attachment_cas_path` 比较实际长度是否等于 expected_size，0 不表示通配未知。非空本地文件与缺省 0 不匹配时会被判为不可用。
- 上下文中的空文件名回退为“附件”，其它 UI/打开文件路径仍消费实际 name。若把 name 设成可选，应明确统一显示默认，不只是加一个 Serde 注解。

这些字段都不是无业务字段。是否放宽 type/name 的缺省，应先统一真正的业务默认；本报告不新增 MIME 猜测、自动补长度或文件名纠错。

## 5. 消息身份与终态应按条件要求

### 5.1 群聊发言助手与所属群不是一个 ID

| 消息场景 | 权威会话身份 | 发言者 agentId |
|---|---|---|
| 单聊 assistant | 外层 `(agent, ownerId, topicId)` | 作者可由 ownerId 确定；如果输入另有 agentId，应定义一致性要求 |
| 群聊 assistant | 外层 `(group, groupId, topicId)` | 必须有真实发言助手 ID，不能用 groupId 替代 |
| user/system 等 | 外层完整 TopicKey | 不要求凭空提供一个助手 ID；应按该角色真实语义处理 |

[read_narration_author][M3] 已提供直接证据：单聊 assistant 用外层 ownerId；群聊 assistant 缺少或空 agentId 会返回“群聊消息缺少发言助手，无法确定朗读音色”。该 ID 还影响自然发言概率和 JEV 最近发言统计。

当前 Pull 对 agentId 没有对应的条件必需检查，会把缺失值写入数据库；因此这里是值得收紧的真实业务缺口，不是为防未知风险新增门禁。

历史成员退出群组不改变其旧消息的作者。校验应要求真实作者身份信息，而不是额外要求该作者仍在当前 members 列表。单聊虽可派生作者，也不能偷偷把 None 改成 Some(ownerId) 后沿用旧 contentHash，因为 agentId 是否存在参与消息指纹。

### 5.2 name 有显示回退 但没有统一的历史署名默认

当前有多种回退：数据库加载可能采用当前 Agent 名称，群聊上下文用 User/AI，JEV 用“用户”/AI，用户气泡又可用当前用户设置。它们能让界面或上下文继续工作，但不会恢复“发送这条消息当时叫什么”。

因此需要在下面两种政策中明确一种，不能称字段已经无用：

- 若共享历史署名是必须保真的数据，则在需要署名的消息角色中要求原值，不从当前名称补造。
- 若产品允许“未记录历史署名”，则将这个状态明确为可选默认，展示时派生名称，保留 canonical 中的缺席，不把回退名称写回消息。

推荐保留该字段，并在实施前明确上述语义。它参与消息指纹，None、空字符串、AI 和具体姓名不是同一份 canonical 数据。

### 5.3 finishReason 不是无用的展示附属字段

非空 finishReason 会使 `chatStreamStore.acceptPersistedMessage` 退休该消息的活跃流投影；`cancelled_by_user` 会显示已停止；Rust 终态提交也用它识别 AlreadyFinalized。

建议将同步中的已完成 assistant 消息列为“需要明确终态”的场景；user 消息没有生成终态，允许缺失。实施前要核对桌面生产者和既有历史数据能否给出真实值。统一补 completed 会把 error、cancelled 和未知状态混成正常完成，不能当作已有默认。

### 5.4 需要保持的共同身份检查

外层 ownerType/ownerId/topicId、消息 id、实体类型与请求结果对应关系继续必需。内层归属字段保留为兼容投影，外层仍是权威；上传依据外层重建，不将缺失字段转成错误归属。条件校验的独立建议不在本次实施，无需再用 untagged 猜测消息类别。

## 6. 兼容字段的实际来源与业务边界

### 6.1 群组 mode 和 createdAt

`GroupConfig.effective_mode()` 只读取 `local_settings.chat_mode`。共享 mode 与 createdAt 仍被 `GroupSyncDTO::from` 回推，并进入群组 configHash。这说明它们当前有协议耦合，却没有相应手机运行消费。

按最新裁决，保留现有共享投影：Group.mode 缺失复用 default_group_mode() 的 sequential，createdAt 缺失用配置默认 0；提供值原样保留。此缺省不会更改独立的本机 chatMode，也不会新增隐藏原始 JSON 或第二份状态。

回推与哈希仍携带这两项，不修改桌面配置或 CDS schema。当前 Legacy 的 12 字段与手机 10 字段哈希差异仍是独立问题；接收缺省和完整输出不等于已经消除那项投影差异。

### 6.2 imageFrames 与手机视频能力无关

生产路径是 `context_assembler` 生成 local_file → `vcp_client` 按 hash 解析本机 CAS → `process_video_for_multimodal` → `video_extractor` 调原生 processVideo 并缓存抽取的 JPEG 帧。它不读取 DTO 或数据库中的 imageFrames。

当前检索中的 imageFrames/image_frames 命中主要为定义、SQL、转换和回推，没有生产展示或上下文消费者。保留它是为了桌面兼容输出；有本机元数据则发数组，无值发 null，不要求手机为同步额外执行抽帧。另一个 `useAttachmentPreview` 中的文本预览代码也不能当作活跃消费者证据：当前仅有 re-export，未找到生产调用方；实际附件文本预览走 CAS 原始文件读取。

### 6.3 附件 createdAt 的值已经不保真

接收时虽然复制进 `Attachment.created_at`，但同步调用 `rusqlite_upsert_attachment_core` 传的是消息 timestamp；核心写入也用这个时间，未消费 att.created_at。普通消息仓储有相同处理。

首次建立本地附件时，远端附件时间 9、消息时间 100 会得到本地附件时间 100；之后 Push 从本地附件表重建 DTO。若同 hash 早有本地附件记录，则还可能采用已有本机时间。该字段没有完成“远端时间入库再原样回推”的业务。

本次保留这个兼容字段，继续从附件表构造输出，不删除本机 CAS 注册时间，也不在上传时另补当前时钟。允许输入缺失与输出能构造兼容时间，不等于远端创建时间原样往返；文档与日志应保留这个区别。

### 6.4 extractedText 是保留项 但当前并非完整版本化元数据

桌面专用附件没有在手机下载原件时，模型上下文仍可用同步来的 extractedText；缺失则只能提示“内容未同步至移动端”。这是确实的手机功能。

不过当前消息指纹只纳入附件 hash，不纳入提取文本；同一消息 contentHash 不变时，附件更新可能跳过，即使更新执行也以已有非空提取文本优先。故它当前主要承担初次同步/补缺用途，不应宣称衍生附件元数据每一轮都完整双向保真。

## 7. 保留兼容字段的实验依据

以下为裁决前针对“上传省略兼容字段”的实验，保留其真实结果作为不删除字段的依据，不表示新输出实现仍省略这些字段。实验调用本机桌面生产模块 `sync/projection.js` 的 `projectMobileMessage`、`mergeMobileMessage`，使用虚构消息和附件，数据库查询以返回无记录的桩替代，没有读取或修改用户数据。

| 输入或结果 | 实验值 |
|---|---|
| 桌面已有消息 | groupId、topicId、isGroupMessage 均存在 |
| 桌面已有附件 | imageFrames 为非空数组，createdAt 为 9 |
| 手机上传消息 | 省略上述字段，消息 timestamp 为 100，附件 hash/type/name/size 保留 |
| 合并后消息 | groupId、topicId、isGroupMessage 全部被删除 |
| 合并后附件 | imageFrames 变为 null，createdAt 变为 100 |
| 合并前后消息 fingerprint | 相等 |

原因可直接定位：

- `VCPMobileSync/sync/projection.js` 的 MOBILE_MESSAGE_PATCH_FIELDS 包含三个消息标记；`mergeMobileMessage` 对缺席字段执行 delete，投影不会用外层身份自动补回它们。
- `config/defaults.js` 的 `createDesktopAttachment` 把缺失 imageFrames 补 null、createdAt 补消息时间；合并附件对象时这些值覆盖已有 `_fileManagerData`。
- CDS 消息 Push 也先通过同一 JS 投影，再由 Rust 同步合并处理，不会自然避开这些覆盖。
- 三个消息标记、附件帧和附件创建时间都不参与消息内容指纹，所以 hash 不变无法证明桌面 JSON 没有变化。

外层 TopicKey 仍确定真实文件/数据库归属，所以旧实验的省略不会直接把消息搬到另一会话，但桌面消息标记确会丢失。现行实现因此保留字段：手机上传从 TopicKey 明确生成 topicId、isGroupMessage 和适用的 groupId，不依赖可能为空或陈旧的消息镜像列。

附件输出保留本机已存 imageFrames/createdAt；没有帧元数据时明确输出 null。这仍不保证保护桌面已有而手机未知的帧信息，显式 null 也不是“保留桌面原值”指令。本次采用既有桌面兼容形状，没有将所有省略/null 统一改成 PATCH，也没有修改桌面合并逻辑。

## 8. 本次裁决与独立待审议项

| 项目 | 事实 | 建议审议结果 |
|---|---|---|
| 7 项兼容字段 | 不驱动独立手机业务，但服务桌面输出 | 已裁决保留：接收允许缺省，上传按权威来源构造；不删字段或共享哈希键 |
| Agent 7 项、Group name/useUnifiedModel 等 | 都有具体业务默认，现 Wire 却必填 | 可按新标准允许缺省，明确实际采用的值 |
| Group members | 有合法空群默认，但缺省会重置成员集合 | 明确接受空群快照还是要求完整 members；不引入未定义 PATCH |
| 群聊 assistant agentId | 缺失会破坏朗读和发言统计，无法从群组 ID 推导 | 条件必需，提供真实发言助手，不要求仍在当前成员列表 |
| Message name | 有实际消费，但仅有多种显示回退，没有统一历史署名默认 | 明确允许“无历史署名”，或对需要保真署名的角色要求原值 |
| assistant finishReason | 有终态消费，缺省无法判定真实结束原因 | 条件必需候选；先核对桌面生成与存量数据，不补 completed |
| AgentTopic locked/unread | 有实际 UI 状态；locked 默认冲突，且元数据已带远端 hash | 先统一默认及版本投影，再放宽缺省；unread=false 还会清本地未读计数 |
| Attachment type/name/size | 有业务，类型层空串/0 不一定是完整业务默认 | 不以 derive(Default) 自动放宽；type/name 先统一缺省，size 继续携带真实长度 |

表中除 7 项兼容字段以外，其余是尚未在本任务中实施的独立建议。默认只处理缺键，不将 Group.mode/createdAt 的显式 null 或已知字段类型错误自动纠正。控制帧的 data/error、删除时间、差异动作和最终 ACK 身份仍按其协议要求处理，不据此补造业务信息。

## 9. 验证范围

本报告完成了 51 个字段的源码消费追踪，覆盖 DTO → Pull → 写库 → 加载 → Vue/模型上下文/群调度/音色 → Push/哈希，并核对了相关历史提交。对声明/搬运命中与真正业务消费进行了区分；负向结论表述为“当前生产路径未发现”，不由文件名、类型注释或旧文档单独推断。

桌面省略字段实验有实际断言，验证了删除/覆盖行为与 fingerprint 不变。裁决后的回归另外检查 Group 缺省后仍输出兼容字段、消息及附件缺字段可接收、上传按 TopicKey 重建归属并保留本机附件元数据。没有进行真机 UI、实际账号数据或双端联网同步验证，不把旧省略实验当作新输出投影的运行证据。

裁决实现已通过 `pnpm check` 与 `cargo test --locked --lib vcp_modules::sync::`（73 项）。上传投影测试直接调用生产 `load_outbound_message_page`，同时覆盖单聊/群聊、陈旧归属镜像、明确 null、已有帧数组和附件表创建时间。检查基于当时工作树快照，不替代其它并行改动的验证。

## 10. 代码定位

以下链接指向本仓库生产代码；行号和函数名以本轮读取的工作树为准。

- [A1 AgentConfig 与默认函数][A1]：`Default`、default_model 等。
- [A2 单聊实际请求组装][A2]：194–219 行附近。
- [G1 群组系统提示词组装][G1]：11–38 行。
- [G2 群组配置与本机模式][G2]：GroupLocalSettings、GroupConfig、effective_mode。
- [G3 群组模型与邀请消息][G3]：151–180 行；成员加载与模式执行约 462–492 行。
- [G4 群组标签与发言策略][G4]：359–543 行；JEV 最近发言投影约 131–224 行。
- [G5 群组设置 UI][G5]：提示词、统一模型、标签编辑与本机 chatMode。
- [T1 Topic 字段定义][T1]：当前没有 Default impl，locked/unread 有字段级 Serde 默认。
- [T2 Topic 创建与状态更新][T2]：创建 locked=true，切换 locked/unread 及版本更新。
- [T3 Topic 菜单与未读展示][T3]：137–159、309–349 行附近。
- [P1 Pull 解析与身份核验][P1]：195–368 行消息解析，418–483 行提交外层 TopicKey。
- [P2 Push 消息与附件重建][P2]：417–628 行。
- [D1 同步写入队列][D1]：话题 1850–1967、消息约 1974–2144、附件约 2300–2581 行。
- [M1 历史加载与附件投影][M1]：ROW_SELECT、project_history_attachment、load_topic_history_for_context。
- [M2 模型上下文消费][M2]：群聊署名、提取文本与 local_file。
- [M3 消息朗读作者][M3]：read_narration_author，17–54 行。
- [M4 已持久化消息的流终态处理][M4]：acceptPersistedMessage，1094–1103 行。
- [M5 消息展示][M5]：停止原因、头像与附件。
- [H1 配置与消息哈希][H1]：消息指纹 98–154，Topic 元数据 176–197，群组配置 214–228 行。
- [F1 CAS 与提取文本][F1]：validate_attachment_cas_path、ensure_extracted_text。
- [F2 附件 UI 分类][F2]：空 MIME 的扩展名回退。
- [F3 手机视频抽帧][F3]：process_video_for_multimodal，从实体视频生成本机帧。

桌面只读定位：`E:/VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/projection.js` 的 projectMobileMessage/mergeMobileMessage，`config/defaults.js` 的 createDesktopAttachment，`sync/central.js` 的消息 Push；CDS 对照为 `E:/VCPChat/rust_chat_data_service/src/sync.rs` 与 `sync_wire.rs`。

[A1]: ../../src-tauri/src/vcp_modules/agent/agent_types.rs
[A2]: ../../src-tauri/src/vcp_modules/agent/agent_chat_application_service.rs
[G1]: ../../src-tauri/src/vcp_modules/group/group_context_assembler.rs
[G2]: ../../src-tauri/src/vcp_modules/group/group_types.rs
[G3]: ../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs
[G4]: ../../src-tauri/src/vcp_modules/group/group_speaking_policy.rs
[G5]: ../../src/features/agent/GroupSettingsView.vue
[T1]: ../../src-tauri/src/vcp_modules/chat/topic_types.rs
[T2]: ../../src-tauri/src/vcp_modules/chat/topic_service.rs
[T3]: ../../src/features/topic/TopicList.vue
[P1]: ../../src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs
[P2]: ../../src-tauri/src/vcp_modules/sync/sync_executor/push_executor.rs
[D1]: ../../src-tauri/src/vcp_modules/persistence/db_write_queue.rs
[M1]: ../../src-tauri/src/vcp_modules/chat/message_service.rs
[M2]: ../../src-tauri/src/vcp_modules/chat/context_assembler.rs
[M3]: ../../src-tauri/src/vcp_modules/persistence/message_repository.rs
[M4]: ../../src/core/stores/chatStreamStore.ts
[M5]: ../../src/features/chat/MessageRenderer.vue
[H1]: ../../src-tauri/src/vcp_modules/sync/sync_hash.rs
[F1]: ../../src-tauri/src/vcp_modules/infra/file_manager.rs
[F2]: ../../src/features/chat/attachment/utils/AttachmentClassifier.ts
[F3]: ../../src-tauri/src/vcp_modules/infra/media_processor/video_extractor.rs
