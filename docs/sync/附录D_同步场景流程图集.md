---
title: 附录D - 同步场景流程图集
scope: 双端
version: 0.9.13
last_updated: 2026-05-13
---

# 附录D - 同步场景流程图集

> 本附录以 ASCII 流程图形式展示 5 个典型同步场景的完整流程，涵盖 Phase 1（Owner Metadata）、Phase 2（Topic Metadata）、Phase 2.5（Hash Validation）及 Phase 3（Messages）四个阶段。
>
> 阅读对象：需要理解双端同步数据流、排查同步异常或扩展同步协议的开发者。

---

## 场景1：新建 Agent 同步

### 初始状态

```
┌─────────────────┐              ┌─────────────────┐
│   Desktop端     │              │   Mobile端      │
│  Agent A1 存在  │              │   空数据库      │
│  ts=T1 hash=H1  │              │   (首次安装)    │
└─────────────────┘              └─────────────────┘
```

### 操作序列（ASCII 时序图）

```
Mobile                              Desktop
  │                                   │
  │  ┌─ Phase 1: Owner Metadata ─┐   │
  │                                   │
  │  SYNC_MANIFEST                  │
  │  dataType: agent                │
  │  items: []      ───────────────>│
  │                                   │
  │                                 │ ┌─ handleSyncManifest ─┐
  │                                 │ │ localItems = [A1]    │
  │                                 │ │ remoteItems = []     │
  │                                 │ │ → A1 未在远端 → PULL │
  │                                 │ └──────────────────────┘
  │                                   │
  │  <──────────────────────────────  │
  │  SYNC_DIFF_RESULTS                │
  │  [{ id: "A1", action: "PULL" }]  │
  │                                   │
  │  ┌─ 执行 PULL ─┐                  │
  │                                   │
  │  HTTP GET /download-entity?id=A1  │
  │  &type=agent    ───────────────>│
  │                                   │
  │  <──────────────────────────────  │
  │  AgentSyncDTO (JSON)              │
  │                                   │
  │  DbWriteQueue 写入 SQLite         │
  │  HashInitializer 计算本地 hash    │
  │                                   │
```

### Diff 结果

| 实体 | 动作 | 理由 |
|------|------|------|
| A1 | PULL | 移动端无记录，需从桌面端拉取 |

### 最终状态

- 移动端 SQLite `agents` 表写入 A1 的完整配置，`config_hash = H1`
- 双端状态一致，Phase 1 完成 → 自动进入 Phase 2

### 涉及代码文件

- `src-tauri/src/vcp_modules/sync/sync_pipeline/phase1_metadata.rs` — 构建空 Manifest
- `src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs` — 执行 Agent 拉取
- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js` — 桌面端 Diff 计算

### 扩展说明：批量拉取优化

当首次同步存在大量 Agent/Group 时，`PullExecutor::pull_entities_batch` 将 PULL 请求按类型分块：

```
Agent/Group  chunk_size = 50
Topic        chunk_size = 1000
```

单次 HTTP 请求最多拉取 50 个 Agent 或 1000 个 Topic，减少往返次数。

---

## 场景2：双向修改冲突

### 初始状态

```
┌──────────────────────────┐              ┌──────────────────────────┐
│        Desktop端          │              │        Mobile端           │
│  Agent A1                │              │  Agent A1                │
│  updated_at = 1000       │              │  updated_at = 2000       │
│  config_hash = H_old     │              │  config_hash = H_new     │
│  (用户先修改)             │              │  (用户后修改)             │
└──────────────────────────┘              └──────────────────────────┘
```

### 操作序列（ASCII 时序图）

```
Mobile                              Desktop
  │                                   │
  │  SYNC_MANIFEST (agent)            │
  │  [{id:"A1", hash:H_new, ts:2000}] │
  │               ──────────────────> │
  │                                   │
  │                                 │ ┌─ handleSyncManifest ─────────┐
  │                                 │ │ local.ts = 1000              │
  │                                 │ │ remote.ts = 2000             │
  │                                 │ │ remote.ts > local.ts         │
  │                                 │ │ → action = "PUSH"            │
  │                                 │ └──────────────────────────────┘
  │                                   │
  │  <──────────────────────────────  │
  │  SYNC_DIFF_RESULTS                │
  │  [{id:"A1", action:"PUSH"}]       │
  │                                   │
  │  ┌─ 执行 PUSH ─────────────────┐  │
  │  │ PushExecutor::push_agent    │  │
  │  │ 1. 查询本地 A1 DTO           │  │
  │  │ 2. 生成 idempotency_key     │  │
  │  │ 3. HTTP POST /upload-entity │  │
  │  └─────────────────────────────┘  │
  │               ──────────────────> │
  │                                   │
  │                                 │ ┌─ entity.js::handleAgentUpload─┐
  │                                 │ │ if (fileReadSuccess) {        │
  │                                 │ │   applyAgentDTO(config, data); │
  │                                 │ │   // 保留 config.topics 数组   │
  │                                 │ │ }                             │
  │                                 │ └───────────────────────────────┘
  │                                   │
```

### Diff 结果

| 实体 | 动作 | 理由 |
|------|------|------|
| A1 | PUSH | 移动端时间戳更新（2000 > 1000），桌面端接受移动端版本 |

### 最终状态

- 桌面端 `config.json` 被局部覆盖：Agent 名称、模型、Temperature 等字段更新为移动端值
- 桌面端 `config.topics` 数组**不被触碰**（由独立 Phase 2/3 负责 Topic 同步）
- 移动端无需拉取，双端 `config_hash` 最终收敛为 `H_new`

### 涉及代码文件

- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js` — LWW 时间戳比较
- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/entity.js` — `applyAgentDTO` 局部合并
- `src-tauri/src/vcp_modules/sync/sync_executor/push_executor.rs` — 幂等键生成与 HTTP 推送

### 扩展说明：V2 双哈希的防护作用

即使 `config_hash` 冲突解决为 PUSH，`content_hash` 的不一致仍会被标记为 `mismatchedContent = true`：

```javascript
if ((dataType === "agent" || dataType === "group") && localContent !== remoteContent) {
    const existingResult = results.find(r => r.id === remote.id);
    if (existingResult) {
        existingResult.mismatchedContent = true;
    } else {
        results.push({ id: remote.id, action: "SKIP", mismatchedContent: true });
    }
}
```

这确保 Topic 内容差异不会随配置同步被忽略，而是引导 Phase 2 进行定向 Topic 同步。

---

## 场景3：时间戳碰撞仲裁

### 初始状态

```
┌──────────────────────────┐              ┌──────────────────────────┐
│        Desktop端          │              │        Mobile端           │
│  Agent A1                │              │  Agent A1                │
│  updated_at = 1000       │              │  updated_at = 1000       │
│  config_hash = H_d       │              │  config_hash = H_m       │
│  (H_d ≠ H_m)             │              │  (几乎同时修改)            │
└──────────────────────────┘              └──────────────────────────┘
```

### 操作序列（ASCII 时序图）

```
Mobile                              Desktop
  │                                   │
  │  SYNC_MANIFEST (agent)            │
  │  [{id:"A1", hash:H_m, ts:1000}]  │
  │               ──────────────────> │
  │                                   │
  │                                 │ ┌─ handleSyncManifest ────────┐
  │                                 │ │ local.ts = 1000             │
  │                                 │ │ remote.ts = 1000            │
  │                                 │ │ remote.ts > local.ts ? NO   │
  │                                 │ │ → else 分支                 │
  │                                 │ │ → action = "PULL"           │
  │                                 │ │   (桌面端版本默认胜出)        │
  │                                 │ └─────────────────────────────┘
  │                                   │
  │  <──────────────────────────────  │
  │  SYNC_DIFF_RESULTS                │
  │  [{id:"A1", action:"PULL"}]       │
  │                                   │
  │  ┌─ 执行 PULL ─────────────────┐  │
  │  │ PullExecutor::pull_agent    │  │
  │  │ HTTP GET /download-entity   │  │
  │  └─────────────────────────────┘  │
  │               ──────────────────> │
  │                                   │
  │  <──────────────────────────────  │
  │  AgentSyncDTO (Desktop 版本)      │
  │                                   │
  │  DbWriteQueue 覆盖本地 A1         │
  │  (H_m 被 H_d 替换)                │
  │                                   │
```

### Diff 结果

| 实体 | 动作 | 理由 |
|------|------|------|
| A1 | PULL | `remote.ts == local.ts`（1000），进入 `else` 分支，桌面端默认胜出 |

### 最终状态

- 移动端本地修改被桌面端版本**静默覆盖**
- 双端收敛至 `config_hash = H_d`，时间戳保持 `1000`
- **风险说明**：这是 LWW 单向 `>` 比较的已知限制。缓解措施包括：
  1. 高频同步（连接即触发）缩小离线编辑窗口；
  2. V2 双哈希在 Phase 2.5 进一步校验内容一致性；
  3. 移动端写入时强制 `updated_at = max(old_ts + 1, now)` 确保单调递增。

### 涉及代码文件

- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js` — 时间戳比较与 `else` 分支兜底
- `src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs` — 实体拉取与写入

### 扩展说明：为什么不是 Hash 字典序仲裁

早期设计文档（`SYNC_ARCHITECTURE.md`）曾提出以 hash 字符串字典序作为 tiebreaker：

```rust
if local.hash < remote.hash { Pull } else { Push }
```

但实际 `manifest.js` 实现简化为单向 `>` 比较，`remote.ts <= local.ts` 一律 PULL。这是因为：

1. 真实时间戳碰撞概率极低（需精确到毫秒级）；
2. `else` 分支固定为 PULL 使行为更可预测；
3. V2 双哈希在 Phase 2.5 提供了二次校验机会。

---

## 场景4：删除传播

### 初始状态

```
┌──────────────────────────┐              ┌──────────────────────────┐
│        Desktop端          │              │        Mobile端           │
│  Agent A1                │              │  Agent A1                │
│  deleted_at = 5000       │              │  deleted_at = null       │
│  (用户在桌面端删除)        │              │  (仍可见)                 │
└──────────────────────────┘              └──────────────────────────┘
```

### 操作序列（ASCII 时序图）

```
Mobile                              Desktop
  │                                   │
  │  SYNC_MANIFEST (agent)            │
  │  [{id:"A1", hash:H1, ts:1000,    │
  │    deletedAt: null}]              │
  │               ──────────────────> │
  │                                   │
  │                                 │ ┌─ handleSyncManifest ────────┐
  │                                 │ │ local = A1, deletedAt=5000  │
  │                                 │ │ remote = A1, deletedAt=null │
  │                                 │ │ 进入 else if 分支:           │
  │                                 │ │ local.deletedAt &&          │
  │                                 │ │ !remoteDeletedAt            │
  │                                 │ │ → action = "PUSH_DELETE"    │
  │                                 │ └─────────────────────────────┘
  │                                   │
  │  <──────────────────────────────  │
  │  SYNC_DIFF_RESULTS                │
  │  [{id:"A1", action:"PUSH_DELETE", │
  │    deletedAt: 5000}]              │
  │                                   │
  │  ┌─ 执行 PUSH_DELETE ──────────┐  │
  │  │ DeleteExecutor::            │  │
  │  │   soft_delete_agent(A1)     │  │
  │  │ UPDATE agents               │  │
  │  │ SET deleted_at = now()      │  │
  │  │ WHERE agent_id = A1         │  │
  │  │                             │  │
  │  │ HashAggregator::            │  │
  │  │   bubble_agent_hash(A1)     │  │
  │  │                             │  │
  │  │ tx.send(NotifyDelete)       │  │
  │  └─────────────────────────────┘  │
  │                                   │
  │  WebSocket SYNC_ENTITY_DELETE     │
  │  {id:"A1", dataType:"agent"}      │
  │               ──────────────────> │
  │                                   │
  │                                 │ ┌─ 桌面端收到通知 ────────────┐
  │                                 │ │ 执行软删除索引更新           │
  │                                 │ │ (重复删除，幂等忽略)          │
  │                                 │ └─────────────────────────────┘
  │                                   │
```

### Diff 结果

| 实体 | 动作 | 理由 |
|------|------|------|
| A1 | PUSH_DELETE | 桌面端已软删除（`deleted_at = 5000`），移动端未删除，需通知移动端执行删除 |

### 最终状态

- 移动端：`agents` 表 `A1.deleted_at = now()`（软删除）
- 移动端：向上冒泡重新计算 Agent 的 `content_hash`
- 桌面端：收到 `SYNC_ENTITY_DELETE` WS 通知，执行幂等软删除索引更新
- 双端 A1 均进入软删除状态，物理数据保留（需手动清理或等待定期清理任务）

### 涉及代码文件

- `src-tauri/src/vcp_modules/sync/sync_service.rs` — `PUSH_DELETE` 处理与 `NotifyDelete` 发送
- `src-tauri/src/vcp_modules/sync/sync_executor/delete_executor.rs` — `soft_delete_agent` 与 Hash 冒泡
- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/manifest.js` — 删除判定分支

### 扩展说明：DELETE 与 PUSH_DELETE 的区别

| 动作 | 发起端 | 移动端行为 | 桌面端行为 |
|------|--------|-----------|-----------|
| DELETE | Desktop (manifest diff) | 软删除本地实体 | 已删除，无操作 |
| PUSH_DELETE | Desktop (manifest diff) | 软删除 + WS 通知 | 收到 WS 后更新索引 |
| SYNC_ENTITY_DELETE | Mobile (WS 主动通知) | 已删除，无操作 | 更新索引 |

`PUSH_DELETE` 的冗余 WS 通知是为了处理桌面端未将删除持久化到索引的边界情况。

---

## 场景5：带附件的消息同步

### 初始状态

```
┌─────────────────────────────────────────┐      ┌─────────────────────────────┐
│              Desktop端                   │      │           Mobile端           │
│  Topic T1                               │      │  Topic T1                   │
│  └─ Message M1                          │      │  └─ (无 M1)                 │
│      content: "See attached"            │      │                             │
│      attachment: att.jpg                │      │                             │
│      attachment_hash: H_att             │      │                             │
└─────────────────────────────────────────┘      └─────────────────────────────┘
```

### 操作序列（ASCII 时序图）

```
Mobile                              Desktop
  │                                   │
  │  ┌─ Phase 3: Messages ─────────┐  │
  │                                   │
  │  SYNC_MESSAGE_DIFF_BATCH          │
  │  topics: {                        │
  │    T1: {                          │
  │      topicHash: "...",            │
  │      messages: {}     // 空       │
  │    }                              │
  │  }              ────────────────> │
  │                                   │
  │                                 │ ┌─ handleSyncMessageDiffBatch ─┐
  │                                 │ │ T1 本地有 M1，远端无记录      │
  │                                 │ │ → toPull: ["M1"]             │
  │                                 │ │ → toPush: false              │
  │                                 │ └──────────────────────────────┘
  │                                   │
  │  <──────────────────────────────  │
  │  SYNC_DIFF_RESULTS_BATCH          │
  │  { T1: { toPull: ["M1"],         │
  │          toPush: false } }        │
  │                                   │
  │  ┌─ 执行 Pull ─────────────────┐  │
  │  │ PullExecutor::              │  │
  │  │   pull_messages_batch       │  │
  │  │ HTTP POST /download-messages│  │
  │  │ Body: {topicId:"T1",        │  │
  │  │        msgIds:["M1"]}       │  │
  │  └─────────────────────────────┘  │
  │               ──────────────────> │
  │                                   │
  │  <──────────────────────────────  │
  │  [{id:"M1", role:"user",         │
  │    content:"See attached",        │
  │    attachments:[{hash:H_att}]}]   │
  │                                   │
  │  process_topic_messages(M1)       │
  │  ┌─ 附件路径解析 ──────────────┐  │
  │  │ SELECT hash, internal_path   │  │
  │  │ FROM attachments             │  │
  │  │ WHERE hash IN (H_att)        │  │
  │  │ → 无记录                      │  │
  │  │                              │  │
  │  │ 填充占位符:                   │  │
  │  │ internalPath = "attachments/ │  │
  │  │               H_att"         │  │
  │  │ src = "file://attachments/   │  │
  │  │        H_att"                │  │
  │  │ status = "ready" // 乐观标记 │  │
  │  └─────────────────────────────┘  │
  │                                   │
  │  INSERT messages (M1)             │
  │  INSERT message_attachments 关联   │
  │                                   │
  │  bubble_topic_hash(T1)            │
  │  bubble_agent_hash(owner)         │
  │                                   │
```

### Diff 结果

| Topic | toPull | toPush | 理由 |
|-------|--------|--------|------|
| T1 | [M1] | false | 移动端缺少 M1 |

### 最终状态

- 移动端 `messages` 表写入 M1，内容与桌面端一致
- 附件 `att.jpg` **尚未下载**，`attachments` 表无 `H_att` 记录
- 消息附件路径为占位符 `file://attachments/H_att`，前端显示加载占位图
- 用户点击附件或后续独立附件同步任务触发时，才执行实际下载：
  ```
  GET /api/mobile-sync/download-attachment?hash=H_att
  ```
- Topic Hash 经冒泡后双端一致

### 涉及代码文件

- `src-tauri/src/vcp_modules/sync/sync_service.rs` — Phase 3 分批 Diff 与 `SYNC_DIFF_RESULTS_BATCH` 处理
- `src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs` — `pull_messages_batch` 与 `process_topic_messages`
- `VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/diff.js` — 消息差异比对

### 扩展说明：先写消息、后补附件策略

该策略的核心目的是**避免大文件阻塞消息同步流水线**：

1. 消息文本数据通常 < 10 KB，可快速完成 Phase 3；
2. 附件可能达数 MB 甚至上百 MB，按需下载不影响同步进度；
3. 前端通过 `status = "ready"` 展示占位图，用户感知为"已同步，加载中"。

当用户点击附件时，前端检查 `attachments` 表：

```
if (本地存在 H_att 文件) {
    直接打开
} else {
    触发下载 → 更新 attachments 表 → 刷新 UI
}
```

---

## 附录：通用机制详解

### A. 三阶段流水线状态机

```
┌─────────┐    Phase 1     ┌─────────────┐    Phase 2     ┌─────────────┐
│  Idle   │ ─────────────> │   Owner     │ ─────────────> │   Topic     │
│         │                │  Metadata   │                │  Metadata   │
└─────────┘                └─────────────┘                └─────────────┘
                                                                │
                                                                │ Phase 2.5
                                                                ↓
┌─────────┐    Phase 3     ┌─────────────┐              ┌─────────────┐
│Completed│ <───────────── │  Messages   │ <────────────│  Validation │
│         │                │             │              │  (V2 Hash)  │
└─────────┘                └─────────────┘              └─────────────┘
```

阶段切换由 `SyncCommand` 通过内部 mpsc channel 驱动：

```rust
pub enum SyncCommand {
    StartTopicMetadata,   // Phase 1 → Phase 2
    StartTopicValidation, // Phase 2 → Phase 2.5
    StartMessages,        // Phase 2.5 → Phase 3
    Finalize,             // Phase 3 → Completed
}
```

### B. Diff 动作决策树（桌面端 manifest.js）

```
                    ┌─────────────────┐
                    │ handleSyncManifest
                    └────────┬────────┘
                             │
         ┌───────────────────┼───────────────────┐
         │ remote.deletedAt? │                   │
         └────────┬──────────┘                   │
              是 /   \ 否                        │
               /     \                           │
    ┌─────────┐   ┌─────────────────────────────┐
    │ DELETE  │   │ local 存在？                 │
    └─────────┘   └───────────┬─────────────────┘
                          是 /   \ 否
                           /     \  
            ┌──────────────┐   ┌──────────┐
            │ local.deleted│   │  PUSH    │
            │ At && !remote│   │ (通知远端 │
            │   .deletedAt?│   │  推送到我)│
            └──────┬───────┘   └──────────┘
               是 /   \ 否
                /     \ 
       ┌─────────┐  ┌────────────────────────┐
       │PUSH_DELETE│ │ configHash 比较         │
       └─────────┘  └───────────┬────────────┘
                             相等 /   \ 不等
                                  /     \
                           ┌────────┐  ┌────────────┐
                           │  SKIP  │  │ ts 比较     │
                           └────────┘  └─────┬──────┘
                                        remote.ts > local.ts?
                                         是 /   \ 否
                                          /     \
                                   ┌────────┐  ┌────────┐
                                   │  PUSH  │  │  PULL  │
                                   └────────┘  └────────┘
```

### C. 哈希冒泡路径

任何涉及 Topic 内容变更的操作都会触发 Hash 冒泡：

```
Message 变更 (insert/update/delete)
    │
    ▼
┌─────────────────────────────┐
│ bubble_topic_hash(topic_id) │
│   重新聚合 Topic 下所有消息   │
│   的 hash，计算 content_hash │
└─────────────┬───────────────┘
              │
              ▼
┌─────────────────────────────┐
│ bubble_agent_hash(agent_id) │
│   或                        │
│ bubble_group_hash(group_id) │
│   重新聚合 Owner 下所有 Topic │
│   的 hash，计算 content_hash │
└─────────────────────────────┘
```

### D. 五场景核心指标对照表

| 场景 | 触发阶段 | 核心动作 | 仲裁依据 | 数据流向 | 风险等级 |
|------|---------|---------|---------|---------|---------|
| 新建 Agent | Phase 1 | PULL | 远端无记录 | Desktop → Mobile | 低 |
| 双向修改 | Phase 1 | PUSH | `remote.ts > local.ts` | Mobile → Desktop | 低 |
| 时间戳碰撞 | Phase 1 | PULL | `else` 分支兜底 | Desktop → Mobile | **高** |
| 删除传播 | Phase 1 | PUSH_DELETE | `deletedAt` 存在性 | Desktop → Mobile | 中 |
| 附件消息 | Phase 3 | toPull | 消息 hash 缺失 | Desktop → Mobile | 低 |

### E. 幂等性保障

移动端所有 PUSH 操作携带幂等键：

```
idempotency_key = SHA256(action + entity_type + id + minute_timestamp)
```

桌面端在 5 分钟 TTL 内缓存该键，重复请求直接返回上次结果，避免网络重试导致的数据重复。

### F. 看门狗死锁恢复

`sync_service.rs` 每 10 秒检查 `pending_tasks`：

```rust
if stuck_count >= 6 {  // 约 60 秒无进展
    // 强制触发阶段过渡
    tx.send(SyncCommand::StartTopicMetadata);
}
```

副作用：可能跳过未完成任务，但下一轮同步的幂等性与双哈希校验会自动补齐差异。

### G. 分批传输机制

Phase 3 消息 diff 按消息数量分批，防止单条 WebSocket 消息超过 2 MB：

```rust
const MAX_MESSAGES_PER_BATCH: usize = 10000;

// 每批约 1.5-2 MB JSON
// 处理完一批后，自动发送下一批
```

分批队列在断线重连时自动清空，防止发送过时数据。

### H. 双哈希 V2 协议说明

V2 协议将配置哈希与内容哈希分离，避免配置变更与消息变更互相干扰：

| 层级 | Mobile 字段 | Desktop 字段 | 作用 |
|------|------------|-------------|------|
| 配置层 | `config_hash` | `hash` | DTO 序列化指纹 |
| 内容层 | `content_hash` | `aggregated_hash` | 消息聚合指纹 |

Phase 2.5 的 `SYNC_TOPIC_HASH_BATCH_V2` 对双哈希执行严格相等判断，确保内容级不一致能被检测到。

---

*本文档基于 VCPMobile v0.9.13 同步协议编写。所有 ASCII 流程图均映射到实际代码路径，可作为调试同步异常时的参照基准。*
