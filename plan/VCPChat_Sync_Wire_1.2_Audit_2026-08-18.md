# VCPChat 同步协议 Wire 1.2 基础逻辑缺陷深度审计

> **审计日期**：2026-08-18
> **审计对象**：`/home/dudu/VCPChat`（本地 checkout：`origin/main @ 5b4bfdd`，同步子系统最后改动 `1a80189`，2026-08-13）
> **子系统范围**：`VCPDistributedServer/Plugin/VCPMobileSync/`（JS 桌面插件，下称"插件"）+ `rust_chat_data_service/`（Rust 中央数据服务，下称"CDS"）
> **对照基准**：`/home/dudu/VCPMobile`（移动端，v1.1.4，`docs/sync/` + `src-tauri/src/vcp_modules/sync/` 实现的 fail-closed 契约）
> **触发背景**：两次修复 PR 后，群友实测仍连续暴露运行时错误：先是 `SYNC_DIFF_RESULTS.phase must be an integer`（Phase 1），修复后又出现 Phase 2（Topic Metadata）`INTERNAL_ERROR / internal service error`（`origin=desktop_plugin`）。本文回答的核心问题：**1.2 相比 1.0，基础逻辑到底有哪些缺陷，为什么修一处漏一处。**
>
> **免责边界**：本文所有 文件:行号 均基于上述本地 checkout。群友已合入的 phase 修复不在此 checkout 中（本 checkout 的中央路径仍是缺 phase 状态，见 S1）；若 upstream 已有更新，相关条目需重新核对。
>
> **修订记录（2026-08-19，第二轮）**：
> 1. 插件侧 P0 已落地三项（工作区改动，未 commit）：S1 中央路径 phase 校验+回填（`sync/central.js handleSyncManifest`）、S7 WS 边界 origin 保留（`transport/websocket.js`）、P0-5 CDS 错误排障通道（`withCdsErrorContext` 先写插件日志再翻译）。
> 2. **线上 Phase 2 `INTERNAL_ERROR` 的真根因经群友复测日志实锤：不是 tombstone（S3-α 仍是活跃的潜在触发器），而是话题分支遗留的 `topicId` 冲突**——CDS 抛出 `Message msg_group_invited___... topicId group_topic_1768827528240 conflicts with frame topic group_topic_1772805119874`。这正是 P0-5 日志通道的设计目的：把 S2 黑洞里的真根因捞出来。详见 §4 补遗与新增 S11。
> 3. 据此完成第三轮修复（P0-6）：**topicId 一致性校验在三端从硬失败降级为 frame 权威归一化**（CDS `sync_wire.rs` / 插件 `canonical.js` / 移动端 `pull_executor.rs`），golden fixture 用例迁移并同步全部 SHA 钉值。本轮刻意给防护**减重**而非加码，理由见 S11。
>
> **修订记录（2026-08-19，第四轮——S3 家族修复，工作区未 commit）**：
> 4. **S3-α/β 墓碑短路**：CDS `topic_manifest`/`owner_manifest` 对 `deleted_at.is_some()` 的条目立即返回（hash 三字段空串），跳过 metadata 解析、健康检查、content hash 与磁盘读——依据是墓碑条目的 hash 在全链路无任何消费者（CDS manifest diff 只产出 action、`message_diff` 过滤 deleted 行、移动端只校验 deletedAt）。
> 5. **S3-γ 对齐既有先例**：`topic_hash_diff` 中单 topic manifest 失败从整批冒泡改为 `tracing::warn!` + 记 changed（保守重拉），语义与 `resolve_topic` 失败的既有容错、`message_diff` 的 `TOPIC_HASH_FAILED` 一致。
> 6. **S3-δ 弃用移除**：v1 `/v1/sync/messages/pull` 批量端点、CDS `pull_messages` 包装、插件 `client.js syncMessagesPull` 一并删除（冗余验证：活调用方为零，v2 流式直调 `pull_topic_messages`，后者保留）。
> 7. **S3-ε 分层处理**：`message_manifest` 对 deleted 行跳过哈希（占位符 `TOMBSTONE_CONTENT_HASH` = sha256("")，64-hex 兼容性格式）；存活行单条失败降级为**哨兵哈希** `sha256("vcp-invalid-message:"+raw)`（确定性、永不匹配诚实哈希 → 保守重拉暴露问题；实际传输层 fail-closed 不变），`topic_content_hash` 同步接入——S4 其余毒点不再能经聚合哈希炸掉 Phase 1/2 整表。
> 8. 本轮全部改动限于 CDS sync 层 + 插件 CDS client 清理；移动端零改动；无 wire 契约/fixture/SHA 变更。测试：CDS 28/28（新增 5 项回归）、插件 56 pass/1 skip、VCPMobile `pnpm check` 无污染。

> **修订记录（2026-08-19，第五轮——墓碑全景核查 + 推拉短路 + 版本校验治理，工作区未 commit）**：
> 9. **Q1 墓碑面完整核查**：owners/topics/messages/avatar/tombstones 五个消费面逐点过查，第四轮的墓碑短路**无遗漏**（核查表见 S3 节末）。同族残留的是 3 个"活条目不健康"缺口（非墓碑形态）：缺口 A（活 topic 但 source 不健康 → Phase 2 整表 500）、缺口 B（活 owner 但 config.json 不可读 → Phase 1 整表 500，即"读不存在文件夹"场景的真实炸点）、缺口 C（`owner_content_hash` 被单个不健康 topic 炸掉）。
> 10. **Q2 推拉短路核查**：推/拉路径本身的缺文件处理基本完备（upload-entity mkdir 递归、ENOENT 正常新建、per-topic `_error` 帧隔离）；新发现缺口 D——批量上传父 config 缺失报 `SYNC_ENTITY_BATCH_FAILED` 而单条路径报 `SYNC_ENTITY_NOT_FOUND`，两路错误码不一致。
> 11. **Q3 版本校验结论**：三方精确版本门禁**保留**（CDS exe 随仓提交，版本漂移=打包事故，门禁是对的打包自检）；但修三处失衡——`health()` 与 READY 握手重复校验（去重）、`retryable:false` 被重启循环无视（熔断）、CDS 缺席导致插件整体不注册（降级注册）。
> 12. **F1-F3（CDS sync.rs，需重建 exe 生效）**：F1 `topic_manifests` 单条失败 → warn + 哨兵条目（`sha256("vcp-unhealthy-topic:"+owner+":"+topic)`，ts 保 PULL 偏向）；F2 `owner_content_hash` 聚合循环 per-topic 哨兵；F3 `ManifestItem` 加 `#[serde(skip)] degraded` 内部标记，`owner_manifest` config 失败 → degraded 条目，`manifest()` 新增两个 SKIP 分支（remote 存活 → SKIP+mismatchedContent=true；尾部循环 → SKIP）。降级原则：**topic 下游全程 per-topic 隔离 → 哨兵推进隔离管线求"每轮可见"；owner 下游是 attempt-fatal 的实体下载 → SKIP 求"有界跳过"**。
> 13. **F4-F7（插件/chatDataService，重启即生效）**：F4 entity.js 批量文件级 catch 识别 ENOENT → `SYNC_ENTITY_NOT_FOUND`（对齐单条路径）；F5 client.js `health()` 删版本比较、改校验 `status==='ready'`（唯一版本门禁收敛到 `validateHandshake`）；F6 lifecycle.js 记录 `lastStartError` + `_blockNonRetryableRestart`（retryable=false → circuitOpen + 可操作日志，消除杀-起 5 次循环）；F7 index.js `registerRoutes` 降级注册（`cdsClient`/`centralDegraded` 替代 throw）——WS/HTTP 常开、VERSION_CHECK 可过，中央同步请求收结构化 `CDS_UNAVAILABLE`（origin=desktop_cds）而非 TCP 拒绝。
> 14. 测试：CDS 33/33（新增 5 项）、插件 60 pass/1 skip（新增 4 项于 `tests/mobile-sync-degraded-mode.test.js`；websocket.js 新增 `stopWsServer` 测试导出）、VCPMobile `pnpm check` 绿（移动端零改动）。

---

## 1. TL;DR — 四个层面的系统性失衡

1.2 的问题不是单点 bug，而是一组系统性失衡。**每一次"修好一个报错"都只是让同步流程推进到下一个缺陷引爆点**——这正是"两次 PR 后仍不断冒新错误"的结构性原因：

1. **同一 wire 帧存在两条生成路径（legacy 插件 / CDS 中央），必填字段只在其中一条上存在**（S1）。`phase` 缺陷即此机理：legacy 路径回填 `phase`，CDS 的 `ManifestResponse` 从无此字段，中央适配器原样透传不补。修掉 `phase` 只是堵了第一个缺口，同类字段漂移没有任何机制性防护。
2. **CDS 错误层把一切失败折叠成无根因的 `INTERNAL_ERROR`**（S2）。参数校验失败、数据状态异常、SQLite 错误共享同一个 HTTP 500 + 固定文案 `"internal service error"` + `retryable: true`。不同根因共享同一对外症状，排障必须翻 CDS 进程的 stderr tracing 日志——这就是"错误修了还冒"的直接原因：**它们根本不是同一个错误**。
3. **fail-fast 无隔离 × 严格 canonicalizer × 历史脏数据 = 单点毒化整阶段**（S3/S4/S5）。1.1 硬切引入的严格校验让任何一条 1.0 时代的脏数据（已删除的 topic、缺 id 的旧消息、invalid 的 history source、0 字节 history.json）都能把整阶段 manifest 炸成 500，且**没有条目级容错、没有自动恢复路径**。
4. **三方（Mobile ↔ 插件 ↔ CDS）精确版本硬绑定，无协商、无降级通道**（S8）。任何一环版本不齐，呈现的都不是结构化"请升级"，而是断连或插件整体缺席（TCP 拒绝）。

**当前线上症状（Phase 2 `INTERNAL_ERROR`）的根因链已逐行坐实**，见 §4。它属于第 2+3 层的复合缺陷：tombstone topic 击穿 `topic_manifest`（S3-α），错误被 CDS 泛化（S2），origin 又被 WS 边界覆盖（S7），最终移动端拿到一个零诊断信息的 `desktop_plugin / topic_metadata / INTERNAL_ERROR`。

> **2026-08-19 修订**：S3-α 的推导仍然成立且该缺陷仍在代码中，但群友复测日志实锤**本次线上案例的实际触发器是 S11（话题分支遗留的 topicId 冲突）**——两者经 S2 折叠后对外症状完全一致，唯有 P0-5 日志通道能区分。这恰好演示了 S2 不修、排障就永远靠猜。

---

## 2. 症状时间线

| 时间 | 事件 | 暴露的缺陷层 |
|---|---|---|
| 1.0 时代（06-20 ~ 07-29 前） | legacy 单路径，哈希不可失败、错误静默吞掉，"能跑"但**错误被伪装成无变化** | —（宽容但不可信） |
| 中央化（07-29，d00c10b） | CDS 接管数据面，`ManifestResponse` 无 `phase` 字段（缺陷由此诞生，但当时移动端未硬校验） | S1 潜伏 |
| 1.1 硬切（08-11，8853517） | 严格 canonicalizer、健康检查、版本门禁、final ACK 全部上线；旧脏数据从"静默放过"变成"致命" | S3/S4/S5 变为致命 |
| 1.2（08-13，6ea2a4a） | 错误契约标准化（七字段对象、71 项注册表）；**但未触碰 CDS 错误黑洞与双路径漂移** | S2 保留 |
| 群友实测 ① | Phase 1 报 `SYNC_DIFF_RESULTS.phase must be an integer` → 第一次 PR 修复 | S1 第一缺口 |
| 群友实测 ② | Phase 1 通过，Phase 2（Topic Metadata）报 `INTERNAL_ERROR / internal service error`（origin=desktop_plugin） | **S3-α + S2 + S7 复合**（本文 §4） |
| 08-18 第三轮（插件侧） | S1 中央路径 phase、S7 origin 保留、P0-5 CDS 错误排障通道落地（工作区，未 commit） | S1/S7 修复，S2 获得旁路观测 |
| 群友实测 ③ | 凭 P0-5 通道拿到真根因：`Message ... topicId group_topic_1768827528240 conflicts with frame topic group_topic_1772805119874`——**话题分支遗留数据**，非 tombstone | **S11**（S4 家族新成员） |
| 08-19 第三轮（三端） | topicId 校验降级为 frame 权威归一化；golden fixture 迁移 `owner_topic_conflict` 用例并同步全部 SHA 钉值 | **S11 修复（P0-6）** |
| 08-19 第四轮（CDS 同步层） | S3 家族整体修复：α/β 墓碑短路、γ 对齐 per-topic 容错先例、δ 弃用移除 v1 批量 pull、ε 分层（墓碑行跳过 + 存活行哨兵哈希） | **S3 全部关闭** |
| 08-19 第五轮（CDS+插件） | 墓碑面全景核查确认无遗漏；同族"活条目不健康"缺口 A/B/C 以哨兵/降级关闭（F1-F3）；缺口 D 批量上传错误码对齐（F4）；S8 版本校验治理：health 去重（F5）、非 retryable 熔断（F6）、降级注册（F7） | **缺口 A-D 关闭，S8 部分落地** |

关键认知：②不是"①没修干净"，而是**推进到下一阶段后撞上的另一类缺陷**。只要 S2（错误黑洞）不堵，后续任何数据状态异常都会以同一面目反复出现。

---

## 3. 协议考古：1.0 → 1.1 → 1.2 演进与新不变量

### 3.1 Commit 时间线（每个 commit 引入的协议语义）

| Commit | 日期 | 语义变化 / 新引入的不变量 |
|---|---|---|
| `ffcedcd` Add files via upload | 06-06 | `Groupmodules/groupchat.js` 创建，**group 消息从诞生起 JSON 内就带 `topicId` 字段**；此时任何层面都不校验它与所属话题是否一致——为日后话题分支的 topicId 漂移埋下数据形态基础（见 S11）。 |
| `527b2b1` 新增pc/手机消息同步插件 | 06-20 | **1.0 诞生**。私有 `sync_state.db` 索引（entity/message/attachment/avatar 四表 + 软删墓碑）；WS 消息族：`SYNC_MANIFEST`/`GET_MESSAGE_MANIFEST`/`SYNC_TOPIC_HASH_BATCH(+_V2)`/`SYNC_MESSAGE_DIFF_BATCH`/`PHASE_START`/`PHASE_COMPLETED`/`SYNC_ENTITY_UPDATE`/`VERSION_CHECK`/`SYNC_DELETE_NOTIFY`；HTTP NDJSON 流式 pull/push。`VERSION_CHECK` **仅信息性**（回 `{type:"VERSION_ACK", version}`，不校验、不要求首帧、不拒旧端）。`SYNC_DELETE_NOTIFY` 的 `deletedAt` 用**桌面端时钟** `Date.now()`。错误处理：非法 dataType 静默 `return null`（手机端干等超时）；HTTP 错误为 `{error:"字符串"}`；**diff 查询出错时"保守视为有变化"或返回空 `changedTopics`——错误被伪装成无变化**。 |
| `c9c0ad1` 一期工程推动 | 07-29 | **CDS 诞生**（internal protocol 1）。`error.rs` 确立 `ServiceError` 模型：**一切 `anyhow`/`rusqlite` 错误 → `INTERNAL_ERROR` + 固定文案**（真实根因只进桌面 tracing 日志）。此设计从 CDS 第一天就在，是后来所有"泛化 INTERNAL_ERROR"的根源。 |
| `d00c10b` 完成底层数据库重构 | 07-29 | **中央索引模式上线**：CDS `sync.rs`（+1222 行）提供 `/v1/sync/*`；插件新增 `sync/central.js` 适配器；`MobileSyncUseCentralIndex` 默认开启。此版 CDS 哈希是**宽容**的（`mobile_message_hash_from_json` 不可失败，解析失败回退空串）；**无 history source 健康检查**；**`ManifestResponse` 无 phase 字段**（S1 缺陷由此诞生）。 |
| `34c1e4b` fix | 07-29 | `Plugin.js` 改为 await `registerRoutes`（修启动窗口 404）；中央适配器 reconcile 遇 `SERVICE_BUSY` 退避（30×500ms）；**avatar manifest 不得转给 CDS**（否则 CDS 以空集生成全量 PUSH）。 |
| `b2c0706` align 1.1 final ACK | 08-11 | 新增 `protocol.js`：`createPhaseAck`——`PHASE_COMPLETED` 的 `PHASE_ACK` 必须原样回显 `sessionId/attemptId/nonce`，缺字段不伪造。插件升 1.1.0。 |
| `8853517` hard-cut wire 1.1 | 08-11 | **最大的一次硬切**。新不变量：①`VERSION_CHECK` 必须首帧且每连接仅一次，protocolVersion 精确匹配；②手写 JSON 解析器拒绝重复键；③任何错误 → `SYNC_ERROR` 帧 + `close(1002)` 断连；④JS `canonical.js`/`projection.js` 与 Rust `sync_wire.rs` 严格 canonicalizer（消息必须有非空 id/role、合法整数 timestamp、字符串 content、**墓碑消息不得出现在活帧**、附件 hash 只接受 64 位小写 hex、**消息 `topicId` 必须等于 frame topic——后实锤为过度防护，见 S11**）；⑤**`ensure_topic_sync_source_healthy` 诞生**（history source 非 ready 即失败）；⑥消息哈希从不可失败变为可失败；⑦`PHASE_COMPLETED`(owner/topic) 在中央模式**先等 CDS 全量 reconcile 才 ACK**；⑧`SYNC_DELETE_NOTIFY` → `SYNC_ENTITY_DELETE`，`deletedAt` 改由移动端提供；⑨全套预算（单帧 32MiB/总量 256MiB/1 万 Topic/10 万 Message）+ 消息串行链；⑩CDS 升 internal protocol 2，新增 `/v2/sync/*` 流式端点；⑪双端字节级 golden fixture。 |
| `16f28ed` harden CDS delivery & owner contracts | 08-12 | **Owner 复合身份契约**：topic manifest/diff/pull/push 必须带 `ownerType+ownerId+topicId`；跨 owner 同名 topic → 硬失败；废弃 `file_path LIKE` 模糊匹配。CDS 侧 `validate_manifest_request` 全量校验（hash 必须小写 SHA-256、ts 必须非负安全整数、topic 必须有 owner 且在 targetedOwners 内）。新增 CI `mobile_sync.yml`。 |
| `6ea2a4a` standardize Wire 1.2 errors | 08-13 | **Wire 1.2 = 错误契约标准化**（相对 1.1 的唯一协议变化）：`error-contract.js`，错误统一为七字段对象 `{code,origin,stage,kind,retry,message,failedTopicIds}`；71 项 code 注册表冻结 `kind/retry`；message 脱敏；CDS 错误在 `central.js` 唯一边界翻译；`PROTOCOL_MISMATCH` 改名 `CDS_PROTOCOL_MISMATCH`；对 CDS"成功"响应做形状再校验。版本常量 1.1→1.2。 |
| `1a80189` self-contained gates | 08-13 | 错误契约测试改用 fake express/ws（**不再起真实服务器**，见 §7 测试盲区 #2）；golden 改名 `protocol_1_2_golden.json`。 |

### 3.2 1.0 vs 1.2 消息流对比

**1.0（`527b2b1`，仅 legacy 路径）**：

```
WS connect (?token=)                    ← 唯一鉴权；无版本门槛
VERSION_CHECK → VERSION_ACK {version}   ← 纯信息交换，任何时刻可发、可不发
PHASE_START {phase} → PHASE_ACK {phase}
SYNC_MANIFEST {dataType, data[], targetedOwners?}
  → SYNC_DIFF_RESULTS {data:[{id, action:PUSH|PULL|DELETE|PUSH_DELETE|SKIP}], dataType, phase}
SYNC_TOPIC_HASH_BATCH{_V2} → SYNC_TOPIC_HASH_RESULTS {changedTopics}
SYNC_MESSAGE_DIFF_BATCH → SYNC_DIFF_RESULTS_BATCH {results:{topicId:{toPull,toPush}}}  ← 无 ok/error 判别联合
SYNC_ENTITY_UPDATE / SYNC_DELETE_NOTIFY → SYNC_ACK   ← 删除时间戳用桌面时钟
HTTP: download/upload-entity(-batch)、download/upload-messages(NDJSON)、附件/头像、delete-entity/message
错误：WS 静默吞掉（只记日志）；HTTP {error:"字符串"}
Topic 身份：裸 topicId + file_path LIKE 模糊猜 owner
```

**1.2（当前，中央模式默认）的硬性增量**：

- `VERSION_CHECK` 必须是**首个业务帧、每连接一次、protocolVersion 精确等于 "1.2"**；违反即 `SYNC_ERROR` + 断连（1002）。
- 全链路 JSON 重复键拒绝；WS 单帧 ≤32MiB；消息处理串行化。
- `SYNC_MANIFEST` 必须带正确 `phase`（owner=1/topic=2，**仅 legacy 路径强校验**，见 S1）；topic 类必须带 `targetedOwners` 和逐项 `ownerType/ownerId`。
- Phase 3 decision 必须是严格判别联合 `{ok:true,toPull,toPush}` / `{ok:false,error}`。
- `PHASE_COMPLETED` 的 ACK 回显 `sessionId/attemptId/nonce`；中央模式 owner/topic 元数据阶段 ACK 前**先等 CDS reconcile 落盘**。
- 删除必须带移动端 `deletedAt`；CDS 为本机从未见过的消息也写墓碑，重放保留最早删除时间。
- 错误全通道统一七字段对象；字符串错误/未知字段拒绝。
- 中央数据面走 CDS internal protocol 2；插件对 CDS 每个响应做形状校验。

### 3.3 考古结论：1.2 的"硬"与 1.0 的"软"错配

1.0 的设计哲学是**宽容到不可信**（错误伪装成无变化、哈希永不失败、版本不校验）；1.1/1.2 的硬切把它一次性翻转到**严格到无容错**（一切异常 fail-closed、attempt 级中止），但只完成了"严格化"，没有完成配套的**三件事**：

- **错误分层**（哪些错误该中止 attempt、哪些该跳过条目、哪些该让用户修复数据）——S2/S3；
- **旧数据迁移/豁免路径**（1.0 时代积累的脏数据在 1.1 规则下全部致命）——S4/S5；
- **双路径一致性机制**（legacy 与中央两条生成路径的字段对齐靠人肉）——S1。

这就是"1.2 基础逻辑缺陷"的本质：**门禁立起来了，但门后的房间没有打扫，且着火了看不出是哪个房间。**

---

## 4. 当前线上根因链（已逐行坐实）：Phase 2 `INTERNAL_ERROR`

**症状**（移动端日志）：

```
[Desktop] message:SYNC_MANIFEST -> info (dataType=topic)
[ERROR] [sync] [Desktop] message_handler:INTERNAL_ERROR -> error
  (origin=desktop_plugin stage=topic_metadata internal service error)
```

**完整因果链**（每一环均已核对源码）：

### 环 1：CDS 清单查询把 tombstone topic 纳入候选（设计意图）

`rust_chat_data_service/src/sync.rs:1162-1164`（`topic_manifests`）：

```rust
"SELECT owner_type, owner_id, topic_id
 FROM topics ORDER BY owner_type, owner_id, topic_ordinal"
```

**不过滤 `deleted_at` 本身是有意的**——diff 算法（sync.rs:491-500、535-537）需要 tombstone 条目产出 `PUSH_DELETE` 通知移动端删除。缺陷不在"查出来"，而在下一步"急切求全"。

### 环 2：对 tombstone topic 急切计算 content hash

sync.rs:1183-1186：

```rust
keys.into_iter()
    .filter(|key| targeted_owners.is_none_or(|owners| owners.contains(&key.owner_id)))
    .map(|key| topic_manifest(database, &key))
    .collect()   // ← 任一 topic 失败即整批 Err
```

`topic_manifest`（sync.rs:1189-1213）能查到 tombstone 行（无 deleted 过滤），然后**无条件**调用 `topic_content_hash`（sync.rs:1246），其第一行就是 `ensure_topic_sync_source_healthy(database, key)?`。

### 环 3：健康检查 SQL 与环 1 不对称，无行即报错

sync.rs:1271-1277（`ensure_topic_sync_source_healthy`）：

```rust
"SELECT t.source_path, hs.status, hs.last_error
 FROM topics t
 LEFT JOIN history_sources hs ON hs.source_path=t.source_path
 WHERE t.owner_type=?1 AND t.owner_id=?2 AND t.topic_id=?3
   AND t.deleted_at IS NULL"   // ← tombstone topic 查不到行
```

对已删除 topic，`query_row` 返回 `QueryReturnedNoRows` → anyhow 错误冒泡。**"清单查询不滤 tombstone、健康检查滤 tombstone"的不对称即缺陷本体。**

### 环 4：CDS 错误层把一切折叠成泛化 500

handler 一律 `.map_err(ServiceError::internal)`（protocol.rs:457）；`error.rs:85-90`：

```rust
Self::Internal(_) => (
    StatusCode::INTERNAL_SERVER_ERROR,
    "INTERNAL_ERROR",
    true,                            // ← retryable:true，误导重试
    "internal service error".to_string(),  // ← 固定文案，根因不透出
),
```

真实的 `QueryReturnedNoRows` 只经 `tracing::error!(error = ?error, ...)`（error.rs:97-99）写进 CDS 进程 stderr，**永不上 wire**。

### 环 5：插件边界保留 code、标注 origin=desktop_cds

CDS client（`modules/services/chatDataService/client.js:167-177`）抛出 `ChatDataServiceError{code:"INTERNAL_ERROR", status:500}`；`sync/central.js:157-163` 的 `withCdsErrorContext` 保留 code，收窄 `origin:"desktop_cds", stage:"topic_metadata"`。**到这一环为止，溯源信息还是对的。**

### 环 6：WS 边界无条件覆盖 origin（溯源信息被抹掉）

`transport/websocket.js:218-225`：

```js
} catch (e) {
  terminated = true;
  const error = withSyncErrorContext(e, {
    code: "SYNC_ATTEMPT_FAILED",
    origin: "desktop_plugin",       // ← 无条件覆盖上游的 desktop_cds
    stage: errorStageForPayload(payload, versionAccepted, currentStage),
  });
```

`withSyncErrorContext`（error-contract.js:361-374）的"边界收窄"语义让 fallback 里的 `desktop_plugin` 总是赢。移动端最终收到 `{type:"SYNC_ERROR", error:{code:"INTERNAL_ERROR", origin:"desktop_plugin", stage:"topic_metadata", kind:"internal", retry:"manual", message:"internal service error"}}`，随后 `ws.close(1002)`，整个 attempt 中止。

### 触发条件与定性

- **触发**：任一 targeted owner 下存在任一 `topics.deleted_at IS NOT NULL` 的行。来源：用户删过话题（storage.rs:1235 `mark_topic_deleted`）、删过整个 Agent/Group（storage.rs:394 `reconcile_missing_owners` 级联标记）。
- **永久性问题**：CDS 未发现 tombstone 物理清理代码（`tombstones.expires_at` 只写不读，storage.rs:1336-1338），**topics 表中的 tombstone 行永久存在——此缺陷一旦可触发就永远可触发，必须修代码而非清数据**。
- **注意备选根因**：同一症状也可能是"history source 不健康"（status=invalid/missing 或文件存在未 ingest，见 S5）。两者共享同一对外症状（S2 所致），区分只能看 CDS stderr 日志。**这正是必须修 S2 的理由。**

### 实锤补遗（2026-08-19）：本次线上案例的真凶是 topicId 冲突，不是 tombstone

插件侧 P0-5 排障通道落地后，群友复测拿到了 CDS 侧真根因：

```
Message msg_group_invited_____1765271785553_group_topic_1768827528240__Agent_... 
  topicId group_topic_1768827528240 conflicts with frame topic group_topic_1772805119874
```

**机理**：用户在桌面端使用过"话题分支"功能。`modules/chatManager.js:1550` 创建分支时执行 `currentChatHistory.slice(0, messageIndex + 1)` **原样复制**消息数组到新话题，消息 JSON 内的 `topicId` 仍是旧话题 ID。CDS 计算 group content hash 时遍历该 group 全部 topic 的消息（`sync.rs:1114 → 1234 → 1246 → 1448 mobile_message_hash_from_json → canonicalize_message`），在 `sync_wire.rs` 的 topicId 一致性检查处 bail → 经 S2 折叠为 `INTERNAL_ERROR`。触发链与 §4 环 1-6 完全同构，只是"环 2/3"的炸点从 tombstone 健康检查换成了 canonicalizer。

**关键推论**：
1. 这不是"历史脏数据"，而是**现行合法功能（话题分支）确定性地产出协议 1.1 视为非法的数据形态**——只要分支过含 topicId 字段的消息（group 消息自 `ffcedcd` 起全部自带），同步必炸。影响面远大于 tombstone。
2. 曾有人记得"插件私有 SQL 索引解决过此问题"——**考古证明这是错误记忆**：`message_index (topic_id, msg_id)` 复合主键确实天然支持同一消息存在于多话题，但 1.0 能同步的真正原因是 **`d00c10b` 时代的 `mobile_message_hash_from_json` 只取 content+附件 hash，根本不校验 topicId**；且切换 `MobileSyncUseCentralIndex=false` 并不能绕过，legacy 路径 `canonicalizeHistory`（`message.js`）有同样的检查。校验是 1.1 硬切（`8853517`）在**三端同时**新增的。
3. 修复必须三端同语义降级（详见 S11 与 §9 P0-6），已于本轮完成。

---

## 5. 结构性缺陷清单（S1–S10，按严重度排序）

### S1【严重】同一响应帧双生成路径，必填字段只存在于 legacy 路径

- **证据**：legacy `sync/manifest.js:417-422` 返回 `{type, data, dataType, phase: payload.phase}`，且 287-292 先强校验 `phase===1/2`；CDS `sync.rs:79-86` 的 `ManifestResponse` **自 d00c10b 起从未有过 phase 字段**；中央适配器 `sync/central.js:136-164` 原样透传 CDS 响应，既不转发 `payload.phase` 也不回填——**本 checkout 中 central.js 全文 grep 不到一个 "phase"**（群友的修复在 upstream/分发版中，此处未含）。
- **机理**：移动端 `diff_handler.rs:26-40` 对 `SYNC_DIFF_RESULTS.phase` 是硬门禁（缺失/非整数/不匹配即 FailAttempt）。中央模式下每个 manifest 响应必然触发移动端 fail-closed。
- **同类风险（机制性）**：插件对 CDS 响应的字段校验清单（type/dataType/data/changedTopics/results…）全部是**手工对齐** CDS Rust struct 的 serde 输出，无 schema 共享、无生成代码。任何一端加/漏字段都只能靠人肉发现——`phase` 是倒下的第一块骨牌，不是最后一块。已确认的形状不对称还有：`MESSAGE_MANIFEST_RESULTS` 中央版（central.js:211-222）含 `ownerType/ownerId`，本地版（manifest.js:485-489）**不含**。
- **测试层面的放行**：`tests/mobile-sync-central-adapter.test.js:13` 的 mock `syncManifest` 用 `...request` 展开恰好不含 phase，断言也不查 phase——**mock 与断言双双缺失，结构性放行**。
- **修复方向**：①`central.js:156` 改为 `return { ...response, phase: payload.phase }`，并像本地路径一样先校验 phase 与 dataType 的对应（topic=2，其余=1）；②同审 `handleMessageManifest/handleTopicHashBatch/handleMessageDiffBatch` 的响应字段完整性；③**机制层**：为 CDS 响应 struct 生成/共享 JSON Schema，插件侧校验从"白名单几个字段"改为"schema 全量校验"，把字段漂移变成 CI 可见错误。

### S2【严重】CDS 错误黑洞：一切失败折叠成无根因 `INTERNAL_ERROR`

- **证据**：`error.rs:52-92` 的映射表（Internal → 500 / 固定文案 / `retryable:true`）；`error.rs:116-126` 的 `From<anyhow::Error>`/`From<rusqlite::Error>` 全部归并入 Internal；`protocol.rs:451-507` 全部 `/v1/sync/*` handler 一律 `.map_err(ServiceError::internal)`。
- **机理与后果**：
  - **真根因被吞**："history source exists but has not been ingested"（sync.rs:1288）、"message X cannot cross sync wire"（sync.rs:587）、"topic owner conflicts with the CDS index"（sync.rs:469）等可行动的错误，过 HTTP 后只剩 `INTERNAL_ERROR`。排障只能翻 CDS stderr tracing（error.rs:97-99）。
  - **错误分类也错**：连**客户端请求校验失败**（`validate_manifest_request` 的全部 `anyhow::ensure!`，sync.rs:341-433——如 "topic manifest requires targetedOwners"、"unsupported manifest dataType"、"manifest item configHash is required"）也返回 500 而非 400；`resolve_topic` 的 "topic was not found"（sync.rs:1307/1338）变 500 而非 404。
  - **`retryable:true` 误导**：对永久性校验错误/数据错误标 retryable，诱导无效重试；移动端注册语义中 `INTERNAL_ERROR` 是 `retry=manual`（"没救，请人工"）。
  - **插件侧二次折损**：`client.js:167-177` 只取 body 的 `code/message`，HTTP status 与 `retryable` 不进入 wire 对象；`cleanMessage`（error-contract.js:175-198）再做脱敏截断。
- **修复方向**：①CDS sync 层引入结构化错误（如 `SyncError` 枚举：Validation/NotFound/Conflict/Internal），handler 分别映射 `InvalidRequest(400)`/`NotFound(404)`/`Ambiguous(409)`/`Internal(500)`；至少把 `validate_*` 失败从 Internal 拆出。②500 响应 body 或插件日志中携带 anyhow 根因摘要（可对移动端脱敏，但必须进 `desktop_cds` 侧可操作日志）。③`retryable` 按错误性质赋值，而非对 Internal 一律 true。

### S3【严重】tombstone 查询不对称家族：5 处"全有或全无"（✅ 2026-08-19 第四轮全部关闭）

§4 根因（记为 S3-α）不是一个孤立笔误，而是一族同构缺陷——**"清单/批处理查询故意包含 tombstone（为了产出删除信号），但明细/哈希/健康检查查询过滤 tombstone 或要求数据健康"**：

| # | 位置 | 缺陷 | 触发条件 | 后果 |
|---|---|---|---|---|
| S3-α | sync.rs:1162→1183→1271 | topic manifest 对 tombstone 急切求值 | 任一已删 topic | **整个 topic manifest 500**（当前线上症状） |
| S3-β | sync.rs:1122→1141→1464 | `owner_manifest` 对已删 owner 仍 `fs::read` config.json（`mobile_owner_config_hash`） | 用户删过 Agent/Group（目录已物理删除） | **整个 agent/group manifest 500**——注意：这意味着 **Phase 1 同样可被 tombstone 击穿**，只是当前群友数据里 owner 无 tombstone 所以先炸了 Phase 2 |
| S3-γ | sync.rs:695-704 | `topic_hash_diff`：`resolve_topic` 失败有容错（记 changed），但紧接着的 `topic_manifest` 失败却整体冒泡 | topic 存活但 source 不健康/含毒消息 | 整个 `/v1/sync/topic-diff` 批 500；对照 `message_diff`（sync.rs:778-787）对同一函数用 `TOPIC_HASH_FAILED` 做了 per-topic 隔离——**两处行为不一致** |
| S3-δ | sync.rs:836-840 | v1 `/v1/sync/messages/pull`：`pull_topic_messages` 任一失败炸整批；且 `seen == wanted` 校验（894-900）在客户端请求的 msg_id 已被桌面 tombstone 时直接失败 | 任一 topic pull 失败 | 整批 500；**v2 流式版已改为 per-topic `_error` 帧（protocol.rs:586-595），v1 未跟进** |
| S3-ε | sync.rs:583-595 | `message_manifest`：SQL 故意不滤 deleted（要输出墓碑条目），但对**所有行**（含已删）算 `mobile_message_hash_from_json`，单条失败整体 Err | 任一无法 wire 化的消息（见 S4 毒点清单） | 该 topic 的 message manifest 500 |

- **修复方向**：统一原则——**tombstone 条目短路**（`deleted_at.is_some()` 时跳过 content hash/健康检查/磁盘读，config_hash 可用空串或读 DB 已存列；tombstone 条目只需要 id/ts/deletedAt/owner 身份即可产出 `PUSH_DELETE`）+ **条目级容错**（单条目失败降级为该条目保守重拉或带错误标记，不炸整批，对齐 `message_diff` 与 v2 pull 已确立的先例）。
- **✅ 修复落地（08-19 第四轮，工作区未 commit）**：
  - **α/β**：`topic_manifest`/`owner_manifest` 对墓碑条目 early-return（hash 三字段空串，跳过 metadata 解析/健康检查/磁盘读）。安全性依据（已逐消费者核实）：CDS manifest diff 对墓碑只产出 action 不读 hash；`message_diff` 的 active 表过滤 deleted 行；移动端 `diff_handler` 对 DELETE 条目仅校验 id+deletedAt；唯一全字段格式门禁在插件休眠路径 `handleMessageManifest`（移动端从不发 `GET_MESSAGE_MANIFEST`），占位符保持 64-hex 兼容。
  - **γ**：`topic_hash_diff` 单 topic 失败 → `tracing::warn!` + 记 changed，对齐 `message_diff` `TOPIC_HASH_FAILED` 先例。
  - **δ**：v1 批量 pull 端点/`pull_messages`/插件 `syncMessagesPull` 一并删除（冗余验证：活调用方为零；v2 流式直调 `pull_topic_messages`，保留）。
  - **ε**：deleted 行跳过哈希（占位符 `TOMBSTONE_CONTENT_HASH` = sha256("")）；存活行单条失败降级为**哨兵哈希** `sha256("vcp-invalid-message:"+raw)`——确定性（同 row 每次同步同值，不抖动）、永不匹配诚实哈希（移动端必判 changed → diff/pull 暴露）、64-hex 合规；`topic_content_hash` 同步接入。**实际传输层（pull/push）fail-closed 不变**，毒数据永远到不了手机，只是毒化半径从"整阶段 500"收敛为"该 topic 每轮重试一次并日志报警"。
  - 回归测试 5 项（墓碑 topic/owner 短路 + PUSH_DELETE 端到端、γ 降级、ε 墓碑占位符、ε 哨兵确定性与聚合一致性），CDS 28/28 绿。
- **✅ 第五轮补查与关闭（08-19，工作区未 commit）——墓碑面经全景核查无遗漏；同族 3 个"活条目不健康"缺口（非墓碑形态）关闭**：
  - 墓碑消费面核查表（全部 ✅）：

    | 墓碑面 | 消费点 | 状态 |
    |---|---|---|
    | owners.deleted_at | `owner_manifest` | ✅ S3-β 短路（第四轮） |
    | topics.deleted_at | `topic_manifest(s)` / `owner_content_hash` | ✅ S3-α 短路 + SQL 过滤 |
    | topics/messages.deleted_at | `resolve_topic` / `pull_topic_messages` / `topic_content_hash` | ✅ 过滤；墓碑视为 not-found，条目级隔离既有 |
    | messages.deleted_at | `message_manifest` | ✅ S3-ε 占位符 |
    | messages 墓碑重放 | `apply_explicit_message_tombstones` | ✅ MIN(deleted_at) 幂等 |
    | avatar_index.deleted_at | 插件 `manifest.js` | ✅ hash 存索引列，manifest 不读磁盘 |
    | tombstones.expires_at | 只写不读 | 已知设计（§10 已录） |
  - **缺口 A（Phase 2 整表 500）**：`topic_manifests` 的 `collect`——活 topic 但 history source 不健康（0 字节 history.json 永久 invalid / exists-but-never-ingested，见 S5）→ 健康检查 bail → 整批 500。S3-γ 只修了下游 `topic_hash_diff`，manifest 本体未修。→ **F1**：单条失败 → warn + 哨兵条目（config/content hash 均填 `sha256("vcp-unhealthy-topic:"+owner+":"+topic)`，ts 保 PULL 偏向）——由既有比对逻辑自动产出 PULL，把毒 topic 推进 Phase 2.5/3 的 per-topic 隔离管线，**每轮双端日志可见**。
  - **缺口 B（Phase 1 整表 500）**：`owner_manifest` 的 `collect`——活 owner 但 config.json 在两次 reconcile 之间被物理删除/写到一半 → `fs::read` 失败 → 整批 500。→ **F3**：`ManifestItem` 加 `#[serde(skip)] degraded` 标记，失败 owner 降级为 degraded 条目，`manifest()` 新增 SKIP 分支（remote 存活 → SKIP+mismatchedContent=true，其下健康 topic 在 Phase 2 照常同步；手机还没有 → 尾部 SKIP；remote 已删 → DELETE 优先）。不能用哨兵-PULL：实体下载失败是 attempt-fatal，只会平移爆炸半径。
  - **缺口 C（Phase 1 整表 500）**：`owner_content_hash` 聚合循环被单个不健康 topic 炸掉。→ **F2**：循环内 per-topic 哨兵（ε 在 topic 级的同构镜像），毒化经 mismatchedContent 转译为 topic 级可见降级。
  - 降级总原则：**形态跟着下游隔离能力走**——topic 下游全程 per-topic 隔离，用哨兵求"每轮可见"；owner 下游是 attempt-fatal 的实体下载，用 SKIP 求"有界跳过"。F1-F3 只动 Err 分支，健康条目输出逐字节不变（测试锁定）。
  - 回归测试 5 项（F1 降级+PULL、F1 边界 DELETE/尾部 PULL、F2 聚合存活、F3 SKIP 两分支、F3 边界 DELETE），CDS 33/33 绿。

### S4【严重】严格 canonicalizer × 历史脏数据 = 单点毒化整阶段

- **机制**：1.1 硬切把消息哈希从"不可失败（解析失败回退空串）"改为"可失败 bail"（`sync_wire.rs:184-190` 等），同时 manifest 各层都是 `collect::<Result>` 急切求值（S3）。于是**任何一条 1.0 时代入库的脏数据都能让整阶段 manifest 500**。
- **已确认的毒点清单**（全部是 1.1 前合法入库的数据形态）：
  1. **无 `id` 的历史消息**——ingest 给 DB 行分配 `synthetic_`/`#duplicate_N` 键但 `metadata_json` 存原始对象（ingest.rs:376-398、422），sync 时 canonicalizer 因缺 id bail；
  2. **一 topic 内重复 id**——pull 时 sync.rs:888-891 "duplicate stored message id" bail；
  3. **`content` 非字符串**（多模态/工具消息的结构化 content）、浮点/ISO 字符串 `timestamp`、缺 `role`；
  4. **曾被软删又留在文件里的 `status:"removed"`/带 `deletedAt` 的消息**——canonicalize_message 直接 bail；
  5. **history source 状态非 ready**（见 S5）；
  6. **消息 `topicId` 与 frame topic 不一致**——话题分支合法产出的形态，已单独立项为 S11 并于本轮修复。
- **后果放大**：毒点经 S3 的 collect 放大为整个 owner/topic manifest 失败 → 经 S2 折叠为 INTERNAL_ERROR → 移动端整个 attempt 中止，且**无法从错误得知是哪个 topic/哪条消息**。
- **隔离设计不一致**：Phase 3 的 `message_diff` 有逐 topic `ok:false` 隔离，Phase 1/2 的 manifest 没有——**阶段间容错标准不统一**。
- **修复方向**：①manifest 层引入与 Phase 3 同级的条目级隔离；②canonicalizer 失败时产出"该条目需保守重拉/跳过 + 诊断 id"，而非炸批；③提供一次性数据修复工具或 ingest 时规范化（补 id 等），把毒点消灭在入库侧。

### S5【高】history_sources 状态机的死锁区：没有自动恢复路径

- **状态机**（ingest.rs / storage.rs）：`ready`（ingest 成功，storage.rs:960-987）/ `invalid`（ingest 失败，storage.rs:744-765——已有消息保留但 sync 拒绝服务，fail-closed 是明文契约）/ `missing`（曾 ready 的源消失，级联 tombstone，storage.rs:508-602）/ **无行**（topic 已配置但 history.json 从未存在 = 合法空 topic）。
- **四个死锁/竞态区**：
  1. **"文件存在但从未 ingest"毒态**：`upsert_topic_source`（storage.rs:360-392）只写 `topics` 不写 `history_sources`，reconcile 在 upsert 提交后才 ingest（ingest.rs:71-107）——进程在两者之间崩溃即留下"topics 有行、sources 无行、文件存在"，此后 `ensure_topic_sync_source_healthy` 永久 bail "exists but has not been ingested"（sync.rs:1288）。
  2. **0 字节 history.json 永久 invalid**：写入中断的 0 字节文件 → ingest.rs:251-253 判 empty → `mark_source_invalid` → 之后每次 reconcile 重试仍 empty → **永不自愈**。
  3. **watcher 失败不置恢复标记**：watcher.rs:249-265 notify ingest 失败只记日志、**不置 `reconcile_required`**（仅索引更新失败才置位）——该 source 长期处于"文件存在未 ingest"直到下一次 reconcile，窗口可能持续很久。
  4. **READY 先于 reconcile 的启动竞态**：main.rs:197-256 READY handshake 先于后台 reconcile 发布（后者 ~100ms 后才启动且持 `reconcile_lock`）——窗口内 manifest 请求可能命中毒态 → 500；而插件 `central.js:107-134` 只对 `SERVICE_BUSY` 重试，**对 500 不重试**。
- **修复方向**：①毒态自检自愈（reconcile 启动时扫描"topics 有行、sources 无行、文件存在"并补 ingest）；②0 字节/截断文件的明确恢复语义（视为空历史或等待重写，而非永久 invalid）；③watcher 失败置 `reconcile_required`；④sync 层对"未 ingest"错误标记 retryable 且插件侧对该具体错误退避重试（这依赖 S2 的错误分层先行）。

### S6【高】PHASE_COMPLETED → CDS 全量 reconcile 串行门（含误删风险）

- **证据**：`index.js:171-179`（8853517 引入）：中央模式下 owner/topic 元数据阶段的 `PHASE_COMPLETED` 在 ACK 前 `await centralSync.reconcile()`。CDS `/v1/reconcile` 与自身后台 reconcile 抢 `reconcile_lock`（protocol.rs:327-330 → `ServiceError::Busy`），插件侧 30×500ms 退避（central.js:107-134），耗尽则阶段失败。
- **误删风险**：`reconcile_missing_owners`（storage.rs:394-487）——**本轮扫描中 config.json 解析失败的 owner 被 skip（ingest.rs:204-214 只 warn），随后被当作"已消失"打墓碑**。同步中途触发全量重扫，恰在桌面端正在写 config.json 的瞬间读到半截文件 → CDS 把活 agent/group 判死 → owner 及其全部 topic/message 被级联 tombstone → 下一轮 manifest diff 向手机推 `PUSH_DELETE` → **同步流程本身成为数据删除的触发器**。
- **修复方向**：①解析失败与文件不存在区分处理（前者 skip 本轮、绝不 tombstone；后者需 N 轮连续缺席才 tombstone）；②reconcile 读 config 失败时重试/延迟判定；③评估把 reconcile 门从 PHASE_COMPLETED 关键路径上移走（异步化，用 change feed 收敛）。

### S7【高】WS 边界 origin 覆盖：溯源信息系统性丢失

- **证据**：websocket.js:218-225（§4 环 6）。`withSyncErrorContext` 的 fallback 语义让 `desktop_plugin` 总是赢：
  - CDS 错误的 `desktop_cds` 被改标 `desktop_plugin`（本次症状）；
  - **手机上报的 `SYNC_ERROR` 也被改标**：websocket.js:184-190 先以 `origin:"mobile_sync"` 抛出，外层 catch 又改回 `desktop_plugin`——`MOBILE_SYNC_ERROR` 定义的 origin（error-contract.js:132）**永远到不了 wire**。
- **契约违背**：移动端文档明确规定"捕获边界可收窄 origin，但**不得把已确认的 `desktop_cds` 改写为 `desktop_plugin`**"（VCPMobile `docs/sync/16_Wire_1.2错误契约与排障规范.md:66`）。当前实现直接违反 wire 1.2 自家契约。
- **修复方向**：websocket.js catch 中，当 `e.origin` 已是合法枚举值（尤其 `desktop_cds`/`mobile_sync`）时保留，fallback 仅在缺失时补 `desktop_plugin`；`stage` 同理可保留上游更精确的值。改 `withSyncErrorContext` 调用方式即可，无需动 error-contract 语义。

### S8【高】三方精确版本硬绑定，无协商、无降级通道

- **三个接口全是精确匹配硬门禁**：
  - Mobile↔插件（WS）：protocol.js:130-160 精确匹配 "1.2"，否则 `PROTOCOL_MISMATCH` → SYNC_ERROR + close(1002)；`EXPECTED_PLUGIN_VERSION="1.2.0"` 是对**自己 manifest** 的自检。
  - 插件↔CDS（HTTP）：READY 握手与 health 均精确要求 internal protocol 2；不匹配 → facade 吞错返 null → `index.js:82-88` 抛 "VCP-CDS is unavailable" → **插件整体不注册，WS 端口根本不开**。
- **后果**：版本不匹配时手机端得到的不是结构化"请升级"，而是三种完全不同的症状之一（结构化 PROTOCOL_MISMATCH / WS 1002 断连 / **TCP 连接拒绝**——唯一连错误帧都到不了手机的失败点），极易误判为网络故障。
- **加重情节**：CDS 以**编译好的 exe 提交在 git 里**（`modules/services/chatDataService/bin/`，仅 win32-x64，每个相关 commit 换二进制）——忘换二进制即触发上述黑屏式失败；`canonical.js:6` 还重复定义了一份 `WIRE_PROTOCOL_VERSION`（双源漂移隐患）。
- **修复方向**：①版本不匹配统一走结构化 SYNC_ERROR（插件缺席场景至少做到 WS 端口常开、握手期报错）；②README 已自认"插件 1.2.0 + CDS protocol 2 + VCPMobile 1.1.4 同批次发布或回滚"——短期保留硬切可接受，但需在 RELEASE checklist 中机制化二进制同步；③消除版本常量双源。
- **✅ 部分落地（08-19 第五轮，工作区未 commit）**：①插件↔CDS 的**重复版本校验去重**——READY 握手 `validateHandshake`（protocol+schema 双校验）保留为唯一门禁，`client.js health()` 删除毫秒级重复比较、改校验 `status==='ready'`；②**非 retryable 熔断**——`retryable:false` 的确定性失败（PROTOCOL_MISMATCH/SCHEMA_MISMATCH 等）原本被 exit → `_scheduleRestart` 无视、杀-起 5 次才熔断；现 lifecycle.js 记录 `lastStartError`，exit 处理器与重启定时器两处经 `_blockNonRetryableRestart` 直接 circuitOpen + 可操作日志（指明重建/替换 exe）；③**降级注册**——CDS 缺席时 `registerRoutes` 不再 throw：WS/HTTP 照常注册、VERSION_CHECK 可过（不依赖 CDS），中央同步请求经 `requireClient()` 抛结构化 `CDS_UNAVAILABLE`（origin=desktop_cds）→ WS 边界转 SYNC_ERROR 上 wire，取代"TCP 拒绝"这一唯一无错误帧的失败点；`MobileSyncUseCentralIndex=false` 显式回退不变。**版本门禁本身保留**——CDS exe 随仓提交，版本漂移=打包事故，启动一次性精确门禁是对的打包自检。**剩余**：`WIRE_PROTOCOL_VERSION` 双源（canonical.js:6）与 CDS exe 的 release checklist 机制化未做。

### S9【中】NDJSON 错误帧语义分裂

- **证据**：CDS pull 的逐 topic 失败发**字符串** `_error` 帧（protocol.rs:586-594），编码失败发 `_stream_error`（610-626）；插件边界对 `_error` **翻译后放行**（central.js:48-62），对 `_stream_error` **throw 中止整条流**（42-46）；push 侧 catch 里按 **code 前缀字符串分类**（`startsWith("PROTOCOL_")`，central.js:601-622）决定逐 topic 失败帧还是整流中止。
- **后果**：同类"单 topic 失败"因发生位置不同而时而是可跳过帧、时而中止全部 pull/push；按错误码命名约定做控制流极脆——新增 code 若不以 `PROTOCOL_` 开头就被静默降级为逐 topic 失败（反之误中止）。
- **修复方向**：以结构化 `kind`/severity 字段而非 code 前缀做分流决策；`_error` 与 `_stream_error` 的语义在协议文档中钉死。

### S10【中】次级裂缝集（单项影响小， collectively 侵蚀正确性）

1. **ts 平票偏向桌面**：manifest.js:355-367 与 CDS sync.rs:506-516 同为 `remote.ts > local.ts ? PUSH : PULL`——ts 相等且 hash 不同 → 永远 PULL。若双端时钟/仲裁方向不镜像，每次同步重复传输同一实体（不丢数据但震荡）。
2. **消息内容冲突无 ts 仲裁**：diff.js:365-375 凡 hash 不一致一律 `toPull`（桌面赢），与实体层 ts 仲裁语义不一致；手机端刚编辑的同一条消息会被桌面旧版覆盖。
3. **墓碑复活竞态**：diff.js:355-383 详细路径只查 `deleted_at IS NULL` 的消息；桌面已软删、手机仍存活的消息会让 `toPush=true` 被手机推回。删除通知与 diff 之间无顺序保证，窗口内删除会复活。
4. **`createPhaseAck` 静默回退**：protocol.js:172 对缺 phase 的 PHASE_COMPLETED 默认伪造 `owner_metadata`——与"字段缺失不伪造"的自身注释精神相悖，可能 ACK 错阶段；index.js:181 对**所有** PHASE_COMPLETED 都传 `echoFinalIdentity:true`，不区分是否 final。
5. **幂等表纯内存**：core/idempotency.js 5 分钟内存窗、check-then-record 非原子、重启即失；仅 `/upload-entity` 有幂等键，其余写端点（batch 上传、删除、NDJSON push）依赖底层语义幂等兜底。
6. **中央 push 静默丢弃 `deletedMessageIds`**：central.js:500-535 只解构 `frame.messages`——当前配对手机经 `SYNC_ENTITY_DELETE` 删消息所以不发作，但协议层面是静默发散点。
7. **SYNC_ACK id 不对称**：DELETE 分支回显 `rawId`、其他分支回显 `safeId`（index.js:358 vs 213），avatar 删除场景两者不同（`agent:xyz` vs `xyz`）。
8. **裸 Error 穿透边界**：db.js:116-128 的 "Topic id ... is ambiguous across desktop owners" 无 code，到 WS 边界被兜底为 `SYNC_ATTEMPT_FAILED` 而非更准确的 `SYNC_INDEX_INVALID`；message.js:691-698 对 `Content-Length: abc` 报 500 而非 400。
9. **写序抖动**：message.js:358-360 写 history.json 仅按 timestamp 排序无 id tiebreak，同 timestamp 消息展示顺序不稳定（指纹因 ingest 侧 timestamp+id 排序而稳定，仅影响展示）。
10. **`writeIntentLock` 启发式**：1s `setTimeout` 延迟释放（message.js:537-541 等），watcher 事件晚于 1s 到达时失去防回环保护。
11. **批量/单条上传错误码不一致（缺口 D）** ✅ **第五轮修复**：单条上传路径对父 config 缺失有预检并报 `SYNC_ENTITY_NOT_FOUND`，批量路径无预检——ENOENT 落入文件级 catch，全组报 `SYNC_ENTITY_BATCH_FAILED`；手机端无法区分"先补建父 Agent"与真正的写入失败。entity.js 批量文件级 catch 已识别 ENOENT 对齐为 `SYNC_ENTITY_NOT_FOUND`（其余错误保持 `SYNC_ENTITY_BATCH_FAILED`），回归测试锁定两路分歧（`tests/mobile-sync-degraded-mode.test.js`）。

### S11【严重·本轮已修复】topicId 一致性校验：守卫元数据的过度防护

- **缺陷定性**：Wire 1.1 硬切（`8853517`）在**三端**同时引入"消息 `topicId` 必须等于 frame topic，否则整帧/整阶段失败"：CDS `sync_wire.rs canonicalize_message`、插件 `sync/canonical.js canonicalizeMessage`、移动端 `pull_executor.rs parse_topic_ndjson_frame`。这条校验守卫的是一个**业务从未维护过的不变量**，属于过度设计：
  1. **topicId 是来源元数据，不是消息身份**。三端的消息指纹均只含 `content + attachmentHashes`（`core/hash.js computeMessageFingerprint`、`sync_wire.rs message_fingerprint`、移动端 `HashAggregator`），topicId 不参与任何哈希——重写它不引起任何重同步。
  2. **frame topic 才是双端存储权威**：移动端按 frame `topic_id` 落盘（`pull_executor.rs DbWriteTask::TopicMessages`）、CDS 按行级 `topic_id`、插件 legacy `message_index` 按 `(topic_id, msg_id)` 复合主键。消息 JSON 内的 topicId 在哪一层都不决定归属。
  3. **话题分支合法地制造冲突**：`chatManager.js:1550` 的 `slice()` 原样复制消息到分支话题；group 消息自 `ffcedcd`（2025-06-06）起自带 topicId。这不是脏数据，是现行功能的标准输出。
  4. **1.0 从未校验**——"1.0 时代私有 SQL 索引解决过此问题"是错误记忆（详见 §4 补遗推论 2）。
- **修复（P0-6，已落地）**：三端统一降级为 **frame 权威归一化**——`topicId` 缺失/null 不变；与 frame 不一致（或非字符串）时重写为 frame topic 并记有界日志（CDS `tracing::warn!`、插件 `topicIdRewrites` 独立计数+调用方日志、移动端 `log::warn!`），永不因此失败。其余校验（id/role/timestamp/content/墓碑/附件）全部保留。
- **关键实现约束**：重写计数**不得混入附件 warnings**——插件 `projection.js:52` 与 CDS `sync.rs` push 校验把 `warnings.count > 0` 当作"附件非法"硬失败门禁，混入会导致上行 push 误拒。
- **契约同步**：golden fixture `owner_topic_conflict` 用例从 `invalidFrames` 迁移为 `validFrames` 的 `topic_id_rewrite_to_frame`（锁定新语义的字节级跨端一致），两份 fixture 副本保持字节一致，全部 SHA 钉值（CDS 测试常量、JS 契约测试、移动端测试常量、插件 README、VCPMobile `docs/sync/04` 与 `16`）同步更新。
- **边界**：CDS 改动严格限制在 `sync_wire.rs` canonicalizer（同步层），未触碰任何非同步核心业务。`chatManager.js:1550` 源头重写（`.map(msg => ({...msg, topicId: newTopicId}))`）仍作为可选补充建议——可防止未来数据漂移，但属 VCPChat 主体业务代码，需单独批准。
- **遗留**:CDS 错误分层（S2）仍是独立待修项；tombstone 家族（S3）已于第四轮关闭——topicId 归一化与 S3 修复共同消除了清单/拉取方向的全部已知触发器类别。

---

## 6. 移动端契约基准 vs 桌面端实现差距表

移动端（VCPMobile v1.1.4）的总体姿态是 **fail closed**：几乎所有契约违背都不是警告而是 attempt/session 级致命错误，且协议类错误不自动重试。以下逐项对照桌面端实现（✗ = 已确认差距，⚠ = 风险点）：

| 移动端硬校验（出处） | 桌面端现状 | 判定 |
|---|---|---|
| `SYNC_DIFF_RESULTS.phase` 必须为整数且与当前阶段匹配（diff_handler.rs:26-40） | legacy 路径回填并校验（manifest.js:287-292、417-422）；**中央路径不回填**（central.js:136-164，本 checkout） | ✗ S1 |
| 错误对象七字段 `{code,origin,stage,kind,retry,message,failedTopicIds}`，origin 不得把 `desktop_cds` 改写为 `desktop_plugin`（docs/sync/16:50、66） | WS 边界无条件覆盖 origin 为 `desktop_plugin`（websocket.js:218-225） | ✗ S7 |
| `INTERNAL_ERROR` 注册语义：`kind=internal, origin=desktop_cds, retry=manual`（sync_error.rs:559-566）——移动端**中止会话、不降级、不自动重试** | CDS 把校验错误/数据错误也映射成 INTERNAL_ERROR（S2），移动端无从区分"可行动的数据问题"与"真内部故障" | ✗ S2 |
| `DELETE`/`PUSH_DELETE` 必须携带非负整数 `deletedAt`（diff_handler.rs:62-75）；清单应**显式包含墓碑条目**（phase1_metadata.rs:13-53 移动端自身如此） | CDS diff 算法支持 PUSH_DELETE（sync.rs:491-500、535-537）✔；但 tombstone 条目在生成途中炸掉（S3-α/β），删除信号根本发不出去 | ✗ S3 |
| Topic 的 PULL/PUSH 必须携带 `ownerType∈{agent,group}`，PUSH 另需非空 `ownerId`（diff_handler.rs:100-151） | CDS topic action 携带 owner_type/owner_id（16f28ed 加固）✔ | ✔ |
| `SYNC_TOPIC_HASH_RESULTS.changedTopics`：去重非空字符串数组、请求集合子集、整帧仅到达一次（sync_service.rs:2990-3031） | CDS `topic_hash_diff` 对 resolve 失败记 changed 有容错，但 `topic_manifest` 失败炸整批（S3-γ） | ⚠ S3-γ |
| Phase 3 `results` topic 集合与请求**精确一致**；`ok` 判别联合互斥字段（batch_diff_handler.rs:120-249、310-327） | CDS `message_diff` per-topic `ok:false` 隔离（sync.rs:778-787）✔ | ✔ |
| 最终 PHASE_ACK 原样回显 `phase/sessionId/attemptId/nonce` 四元组，缺失不伪造（sync_service.rs:221-245） | `createPhaseAck` 回显且 fail-closed（protocol.js:168-184）✔；但缺 phase 时静默伪造 `owner_metadata`（protocol.js:172，S10-4） | ⚠ |
| VERSION_ACK 双字段精确匹配 `"1.2.0"`/`"1.2"`，不匹配即断连不降级（sync_service.rs:1287-1327） | 插件侧同样精确匹配（protocol.js:130-160）✔；但插件↔CDS 不匹配时插件整体缺席，手机端只见 TCP 拒绝（S8） | ⚠ S8 |
| 断线续传 = 整 attempt 重来，**要求桌面端所有阶段幂等可重入**（sync_service.rs:3147-3190） | 实体/话题上传 DTO 合并幂等、消息 push 内容哈希 upsert、墓碑 MIN(deleted_at) 幂等 ✔；HTTP 幂等键仅覆盖 `/upload-entity`（S10-5） | ⚠ |
| 收到 `SYNC_ERROR` → 关 WS、终止会话、不自动重试（sync_service.rs:2852-2885） | 任何单帧失败 → SYNC_ERROR + close(1002)（websocket.js:218-240）——**无部分继续/降级设计**，与 S3/S4 的"全有或全无"叠加后，单点数据问题=同步整体不可用 | ⚠ 设计耦合 |

---

## 7. 测试盲区清单（为什么两次 PR 后仍漏网）

**已覆盖**（JS 契约测试 + Rust 内联测试 + CI `mobile_sync.yml`）：版本门禁 fail-closed、重复键 JSON 拒绝、final ACK 回显与不伪造、canonicalizer 双端 golden 字节一致、错误注册表 kind/retry 锁定、中央适配器错误映射（根因 code 保留、`CDS_PROTOCOL_MISMATCH` 改名、畸形 Phase 3 成功帧归 `SYNC_PROTOCOL_INVALID`）、pull/push 流式 canonicalize 与背压、invalid history fail-closed（Rust 级）、owner 歧义拒绝、幂等重放保留状态码、原子写并发校验。

**未覆盖**（每个盲区都对应本文至少一个已爆发/待爆发缺陷）：

1. **中央路径 `SYNC_DIFF_RESULTS.phase`**——mock 与断言双双缺失（mobile-sync-central-adapter.test.js:13、63-85）。→ S1，已爆发为真实缺陷。
2. **真实 HTTP/WS 边界的错误保真度**——`1a80189` 后错误契约测试改用 fake express/ws，不再起真实服务器；**没有任何测试验证 CDS `error.rs` 剥掉根因后跨端到底剩什么**，也没有真实 WS close code/帧序/maxPayload 行为测试。→ S2/S7 在此盲区。
3. **毒化爆炸半径**——无"一条脏消息/一个 invalid source/一个 tombstone 不应中止整个 manifest 阶段"的测试；无 legacy 语料（缺 id、结构化 content、浮点 timestamp、重复 id、`status:"removed"`、0 字节 history.json、tombstoned topic/owner）的回归测试。→ S3/S4/S5 全在盲区。
4. **阶段边界**——`PHASE_COMPLETED`→reconcile 门的失败/SERVICE_BUSY 耗尽路径无测试（只测了启动 reconcile 退避）；无"reconcile 恰逢 config.json 写一半不会误墓碑 owner"的测试。→ S6。
5. **版本矩阵**——只有 `createVersionAck` 单测；无 1.1 手机连 1.2 插件、protocol-1 旧 exe 配 1.2 插件的端到端行为测试。→ S8 的"连接拒绝式失败"无覆盖。
6. **墓碑/旧库数据**——无 pre-`d00c10b` `sync_state.db`（含桌面时钟 deletedAt、`file_path=NULL` 行）迁移/共存测试；插件 `cleanupOldDeletedRecords` 与 CDS tombstone 两套过期策略无交叉验证（CDS 侧 expires_at 只写不读，无清理）。
7. **NDJSON 错误帧分裂**（`_error` 放行 vs `_stream_error` 中止）、push 帧 `deletedMessageIds` 丢弃——无测试。→ S9/S10-6。
8. **无桌面↔真机 E2E**；CI 只跑 node --test 与 cargo 门禁。

**方法论结论**：现有测试体系擅长锁"双端一致性"（golden fixture、注册表锁定），但对**"单端内部状态异常如何表现到 wire 上"**几乎零覆盖——而 1.2 时代线上爆发的恰恰全是这一类。

---

## 8. 文档与代码的矛盾点

1. **CDS `README.md:232-233`**：「VCPMobileSync public wire 固定为 1.1」「1.0/1.1 不支持混跑」——该节写于 1.1 时代（16f28ed），**6ea2a4a 升 1.2 时完全没碰这个 README**。实际代码：wire=1.2、插件 manifest=1.2.0、fixture 已改名 `protocol_1_2_golden.json`。同文件 :258「wire 1.1 golden」同样过时。
2. **插件 README 的"三阶段协议 V2 深度揭秘"**（:152-186）仍以 legacy `sync_state.db` 扫描为主线叙事，而默认中央模式下该路径整体停用、消息索引归 CDS——主叙事与默认架构倒挂，新维护者极易误读。
3. **CDS README:235**「缺 Topic、重复 Topic、错误字段类型、无效历史、DB/HTTP/附件错误均终止当前 attempt」是 fail-fast 设计意图的书面证据——但全文没有说明"attempt 终止时手机端能看到什么"。**文档层面就缺少错误可观测性契约**，S2 的黑洞因此长期隐身。
4. **插件 README:120/:146** 钉死移动端配对 commit 与 golden fixture 的 SHA-256——"文档钉死散列"在 fixture 再生成时必须手工同步，本身就是漂移源（1a80189 已演示过一次 SHA 更新）。

---

## 9. 修复路线图（给修复方，按优先级）

### P0 — 立即消除线上阻断（改动最小、收益最大）

1. **tombstone 短路（S3-α/β）**：**✅ 已落地（08-19 第四轮，工作区未 commit）**——`topic_manifest`/`owner_manifest` 对墓碑条目 early-return（hash 三字段空串），跳过 content hash/健康检查/磁盘读；含"PUSH_DELETE 端到端产出"回归测试。部署注意同 P0-6：需 CDS 二进制重建后生效。
2. **CDS 错误分层（S2）**：sync handler 不再一律 `ServiceError::internal`——`validate_*` 失败 → `InvalidRequest(400)`、topic 不存在 → `NotFound(404)`、owner 冲突 → `Ambiguous(409)`；`retryable` 按错误性质赋值。
3. **WS 边界保留上游 origin（S7）**：websocket.js catch 中 `e.origin` 为合法枚举时保留，fallback 仅在缺失时补 `desktop_plugin`。**✅ 已落地（08-18，工作区未 commit）**。
4. **中央路径回填 phase（S1，若 upstream 未修）**：`central.js:156` → `return { ...response, phase: payload.phase }`，并补 phase 与 dataType 的对应校验（topic=2，其余=1）；**注意与群友已做修复核对去重**。**✅ 已落地（08-18，工作区未 commit），`mobile-sync-central-adapter.test.js` 已补 phase mock 与断言**。
5. **排障通道（S2 配套，零协议变更）**：插件收到 CDS 500 时，把 CDS stderr 最近 N 行 tracing 或 anyhow 根因摘要写入插件自身日志（桌面侧可查），不要求上 wire。**✅ 已落地（08-18）——正是它把群友复测的真根因（topicId 冲突）从 S2 黑洞里捞了出来，验证了该设计的价值**。
6. **topicId 校验降级为 frame 权威归一化（S11）**：三端 canonicalizer 同语义改造 + golden fixture 用例迁移 + SHA 钉值同步。**✅ 已落地（08-19）：CDS `sync_wire.rs`、插件 `canonical.js`（+`message.js`/`central.js` 日志接线）、移动端 `pull_executor.rs`；三端测试全绿（CDS 23、插件 57、移动端 325 + `pnpm check`）**。部署注意：CDS 以编译 exe 提交在 VCPChat git 中，需重新 `cargo build --release` 替换二进制后方在桌面端生效。

> ⚠ P0-1 与 P0-4 的依赖关系：若只修 CDS 500 而不修中央路径 phase，Phase 2 通过后会立刻在移动端撞上 `phase must be an integer`（本 checkout 两个 bug 并存）。群友实测 Phase 1 已能走完，说明其分发版已含 phase 修复——合并时务必确认两处都在。

### P1 — 容错与恢复（把"全有或全无"改成条目级降级）

6. **S3-γ/δ/ε**：**✅ 已落地（08-19 第四轮）**——γ 对齐 `message_diff` per-topic 隔离；δ 按"弃用移除"执行（冗余验证通过：活调用方为零）；ε 分层：deleted 行跳过哈希 + 存活行哨兵哈希降级（传输层 fail-closed 不变）。
7. **S4**：manifest 层（Phase 1/2）引入与 Phase 3 同级的条目级隔离；canonicalizer 失败产出"保守重拉 + 诊断 id"。**部分落地（08-19 第五轮 F1-F3）**——条目级隔离已建：topic 级哨兵哈希（F1/F2）、owner 级 degraded→SKIP（F3），毒化半径收敛到条目级且每轮日志可见；ingest 侧规范化（补 id 等）与一次性数据修复工具未做。
8. **S5**：reconcile 启动时自愈"topics 有行、sources 无行、文件存在"毒态；0 字节 history.json 的明确恢复语义；watcher 失败置 `reconcile_required`。
9. **S6**：`reconcile_missing_owners` 区分"解析失败"（skip 本轮，绝不 tombstone）与"连续 N 轮缺席"（才 tombstone）。
10. **测试补盲（§7 的 1/3/4 项）**：中央路径 phase 断言、tombstone/脏数据语料的"不炸整批"回归测试、reconcile 门失败路径测试。

### P2 — 协议与工程硬化

11. **S1 机制层**：CDS 响应 struct ↔ 插件校验的 schema 共享/生成，把字段漂移变成 CI 可见错误。
12. **S8**：版本不匹配统一结构化报错（WS 端口常开、握手期拒绝）；CDS 二进制纳入 release checklist 机制化；消除 `WIRE_PROTOCOL_VERSION` 双源。**部分落地（08-19 第五轮 F5/F6/F7）**——WS 端口常开 + 结构化 `CDS_UNAVAILABLE`、health 去重、非 retryable 熔断均已落地；剩余：版本常量双源与 release checklist 机制化。
13. **S9**：NDJSON 错误分流改用结构化 severity 字段，弃用 code 前缀判断。
14. **S10**：ts 平票仲裁方向双端核对并写进协议文档；消息冲突补 ts 仲裁；墓碑复活窗口收敛；`createPhaseAck` 缺 phase 改报错；幂等键推广到全部写端点；中央 push 处理 `deletedMessageIds` 或协议层面删除该字段。
15. **文档同步（§8）**：CDS README 更新至 wire 1.2；插件 README 主叙事改为中央模式优先；补"错误可观测性契约"章节。

---

## 10. 附录：证据索引（缺陷 → 文件:行号速查）

### CDS（`rust_chat_data_service/src/`）

| 缺陷 | 位置 |
|---|---|
| S3-α 清单查询不滤 tombstone ✅ 第四轮修复 | sync.rs:1162-1164（topic_manifests SQL） |
| S3-α 急切求值 collect ✅ | sync.rs:1183-1186 |
| S3-α topic_manifest 无条件算 hash ✅（墓碑短路） | sync.rs:1189-1213 → 1246 |
| S3-α 健康检查滤 tombstone（无行报错点） ✅（墓碑不再到达此检查） | sync.rs:1268-1291（SQL 在 1271-1277） |
| S3-β owner manifest 对已删 owner 读磁盘 ✅（墓碑短路跳过磁盘读） | sync.rs:1122-1125 → 1141 → 1463-1465 |
| S3-γ topic_hash_diff 容错不一致 ✅（warn+changed 对齐 message_diff） | sync.rs:695-704（对照 message_diff 778-787） |
| S3-δ v1 pull 全有或全无 ✅（端点与包装已移除；v2 不受影响） | 原 sync.rs:836-840、894-900（对照 v2 protocol.rs:586-595） |
| S3-ε message_manifest 单条毒化 ✅（墓碑占位符 + 存活行哨兵哈希） | sync.rs:583-595（SQL 561-565） |
| S2 错误映射 | error.rs:52-92（Internal 在 85-90）、95-114、116-126 |
| S2 handler 一律 internal | protocol.rs:451-507（sync_manifest 在 451-458） |
| S2 校验错误清单 | sync.rs:341-433（validate_manifest_request） |
| S1 ManifestResponse 无 phase | sync.rs:79-86、548-552 |
| S4 canonicalizer bail 点 | sync_wire.rs:184-190；pull 重复 id sync.rs:888-891 |
| S5 健康检查 bail 四种条件 | sync.rs:1283-1290 |
| S5 upsert 不写 history_sources | storage.rs:360-392 |
| S5 0 字节判 invalid | ingest.rs:251-253 → storage.rs:744-765 |
| S5 watcher 不置 reconcile_required | watcher.rs:249-265 |
| S5 READY 先于 reconcile | main.rs:197-256 |
| S6 reconcile 门 Busy | protocol.rs:327-330 |
| S6 owner 误墓碑 | storage.rs:394-487 + ingest.rs:204-214 |
| 墓碑无物理清理（expires_at 只写不读） | storage.rs:121-131、1336-1338 |
| 版本常量 | config.rs:6-7（PROTOCOL_VERSION=2） |
| S11 topicId 归一化（已修复） | sync_wire.rs `canonicalize_message`（原 bail 点 176-183）；golden SHA 常量 `sync_wire.rs` tests 模块 |
| 缺口 A topic_manifests 活条目 source 不健康炸整表 ✅ 第五轮（哨兵条目 + `topic_updated_at_or_now` 保 PULL 偏向） | sync.rs:1231-1290（降级分支 1270-1289）、哨兵 helper 1631、ts helper 1646 |
| 缺口 B owner_manifest 活 owner config 不可读炸整表 ✅ 第五轮（degraded→SKIP） | sync.rs:1158-1228（降级 1196-1225）；`manifest()` SKIP 分支 508-516、555-566；`ManifestItem.degraded` 66 |
| 缺口 C owner_content_hash 聚合冒泡 ✅ 第五轮（per-topic 哨兵） | sync.rs:1339-1380（哨兵接入 1378） |

### 插件（`VCPDistributedServer/Plugin/VCPMobileSync/`）

| 缺陷 | 位置 |
|---|---|
| S1 中央路径不回填 phase | sync/central.js:136-164（形状校验 148-155；错误上下文 157-163） |
| S1 legacy 路径已修对照 | sync/manifest.js:287-292、417-422 |
| S1 MESSAGE_MANIFEST_RESULTS 形状不对称 | sync/central.js:211-222 vs sync/manifest.js:485-489 |
| S7 origin 覆盖 | transport/websocket.js:218-225（手机上报改标在 184-190；语义在 error-contract.js:361-374） |
| S2 插件侧折损 | modules/services/chatDataService/client.js:167-177；脱敏 error-contract.js:175-198 |
| S6 reconcile 门 | index.js:171-179；退避 central.js:107-134 |
| S8 版本门禁 | protocol.js:130-160；index.js:82-88；client.js:308-313 |
| S8 版本常量双源 | protocol.js:3-5 vs sync/canonical.js:6 |
| S9 NDJSON 错误分裂 | central.js:42-46（_stream_error throw）、48-62（_error 放行）、601-622（code 前缀分流） |
| S10-1 ts 平票 | manifest.js:355-367；CDS sync.rs:506-516 |
| S10-2 消息冲突无仲裁 | diff.js:365-375 |
| S10-3 墓碑复活竞态 | diff.js:355-383 |
| S10-4 PHASE_ACK 静默回退 | protocol.js:168-184（回退点 172）；index.js:181 |
| S10-5 幂等纯内存 | core/idempotency.js:6；routes.js:177-206（仅 upload-entity） |
| S10-6 push 丢弃 deletedMessageIds | central.js:500-535 |
| S10-7 SYNC_ACK id 不对称 | index.js:213/297/358 |
| S10-8 裸 Error | core/db.js:116-128；sync/message.js:691-698 |
| avatar 不走 CDS（设计） | index.js:120-131 |
| 错误码注册表（71 项） | error-contract.js:101-173 |
| S11 topicId 归一化（已修复） | sync/canonical.js `canonicalizeMessage`/`canonicalizeTopicFrame`（`topicIdRewrites` 独立计数）；日志接线 sync/message.js（pull 与 ingest 两处）、sync/central.js（CDS pull） |
| S11 分支源头（未动，可选修复） | VCPChat 主体 `modules/chatManager.js:1550`（分支 slice 复制不重写 topicId） |
| golden fixture 与 SHA 钉值 | fixtures/protocol_1_2_golden.json；README.md:146；VCPChat tests/mobile-sync-canonical.test.js |
| 缺口 D 批量上传 ENOENT 错误码 ✅ 第五轮（对齐 SYNC_ENTITY_NOT_FOUND） | sync/entity.js:404-424（对照单条路径 497-510） |
| F5 health 去重版本校验 ✅ 第五轮 | modules/services/chatDataService/client.js:290-313（唯一门禁收敛到 lifecycle.js validateHandshake） |
| F6 非 retryable 熔断 ✅ 第五轮 | modules/services/chatDataService/lifecycle.js:36-38、76、196、213-223、245 |
| F7 降级注册 ✅ 第五轮（S8 核心） | index.js:82-105、109-115；`requireClient()` 抛 CDS_UNAVAILABLE 在 sync/central.js:111-120 |
| stopWsServer（第五轮新增测试导出） | transport/websocket.js:265-289 |

### 移动端对照基准（`VCPMobile`，仅作期望值引用）

| 契约 | 位置 |
|---|---|
| phase 硬门禁 | src-tauri/src/vcp_modules/sync/sync_executor/diff_handler.rs:26-40 |
| 七字段错误对象与校验 | src-tauri/src/vcp_modules/sync/sync_error.rs:58-68、735-764 |
| INTERNAL_ERROR 注册语义 | sync_error.rs:559-566 |
| origin 不得改写规则 | docs/sync/16_Wire_1.2错误契约与排障规范.md:66 |
| 版本精确匹配 | src-tauri/src/vcp_modules/sync/sync_service.rs:30-31、1287-1327 |
| final ACK 四元组 | sync_service.rs:221-245 |
| 断线整 attempt 重开 | sync_service.rs:3147-3190 |
| S11 topicId 归一化（已修复） | sync_executor/pull_executor.rs `parse_topic_ndjson_frame`（重写 + `log::warn!`）；golden 副本 `sync/fixtures/protocol_1_2_golden.json`；SHA 常量与回归测试同文件 tests 模块；文档钉值 docs/sync/04、16 |

---

## 11. 未验证项（如实声明）

1. 本审计基于本地 checkout `5b4bfdd`；群友已合入的两次 PR 不在其中。若 upstream 已修 S1/S3-α，对应条目以 upstream 为准。**08-18/19 三轮修复（S1/S7/P0-5/P0-6/S3 家族）均在本地工作区，尚未 commit**；S2（CDS 错误分层）/S5（history_sources 死锁区）/S6（reconcile 误删风险）等机制性缺陷仍未修复——**但清单/拉取方向的"单点毒化整阶段"放大器（S3）已整体拆除，剩余触发器只能以 per-topic 降级形态出现**。
2. CDS `sync.rs` 全文 2200+ 行中，`push_messages`/`apply_explicit_message_tombstones` 等上行路径的边界条件未逐行穷尽；S3 家族以"清单/拉取"方向为主。
3. 移动端 topic manifest 是否在所有路径都携带 `targetedOwners` 未逐路径核实——若存在不携带的路径，`validate_manifest_request`（sync.rs:354）会经 S2 变成又一个 INTERNAL_ERROR 触发点。
4. 移动端实体层 ts 仲裁方向与桌面端 `remote.ts > local.ts ? PUSH : PULL` 是否镜像，未做双端 diff 级核对（S10-1 的实锤待补）。
5. CDS 的 `README.md` 全文、`modules/services/chatDataService/` 的进程管理（lifecycle.js 启动/崩溃恢复细节）未纳入本次审计范围。
6. **P0-6 与 S3 家族修复的生效均依赖 CDS 二进制重建**（`cargo build --release` 并替换 VCPChat 内提交的 exe）；在替换完成前，中央模式的分支话题同步仍会 500、tombstone 触发器也仍在——legacy 模式（`MobileSyncUseCentralIndex=false`）可作为临时旁路，因为插件侧归一化已生效（但该旁路不覆盖 S3，因为 S3 全在 CDS 内部）。该临时旁路未实测验证。
7. **第五轮修复（F1-F7）状态**：全部在工作区未 commit；F1-F3（CDS sync.rs）叠加在 P0-6/S3 之上、同样需 exe 重建生效，F4-F7（插件/chatDataService）重启桌面端即生效。验证：CDS `cargo test` 33/33、插件 `node --test tests/mobile-sync-*.test.js` 60 pass/1 skip、VCPMobile `pnpm check` 绿（移动端零改动）。缺口 A/B/C 的哨兵/降级语义依赖移动端既有契约（SKIP 记入 changed_owners、mismatchedContent 处理、Phase 2.5/3 per-topic 失败隔离），移动端代码未改动；降级路径的端到端真机验证未做。

---

*审计完成（2026-08-19 第五轮修订）。本文档为修复方底稿：每条缺陷均含代码证据、触发条件、后果与修复方向；P0 六项中 S1/S7/P0-5/P0-6 与 P0-1（S3-α/β）已落地，S3 家族（含 γ/δ/ε）已于第四轮整体关闭，第五轮完成墓碑面全景核查（无遗漏）并关闭同族缺口 A-D（F1-F4）、S8 部分落地（F5-F7）；P0-2（CDS 错误分层）是解除剩余线上风险的下一项，S5/S6 与 P1/P2 决定 1.2 协议的长期可维护性。*
