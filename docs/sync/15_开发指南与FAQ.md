---
title: 开发指南与FAQ
scope: 双端
related_files:
  - src-tauri/src/vcp_modules/sync/sync_service.rs
  - src-tauri/src/vcp_modules/sync/sync_pipeline/*.rs
  - src-tauri/src/vcp_modules/sync/sync_executor/*.rs
  - src-tauri/src/vcp_modules/sync/sync_hash.rs
  - src-tauri/src/vcp_modules/sync/sync_types.rs
  - src-tauri/src/vcp_modules/sync/sync_dto.rs
  - VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/index.js
  - VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/sync/*.js
  - VCPChat/VCPDistributedServer/Plugin/VCPMobileSync/transport/*.js
version: 0.9.13
last_updated: 2026-05-13
---

# 开发指南与FAQ

## 1. 如何新增一种同步实体

### 1.1 步骤清单

将一种新实体纳入同步体系（如 `settings`、`prompt_preset` 等），需要修改双端共 **7 个文件**。以下是强制检查清单：

| 步骤 | 文件 | 修改内容 | 优先级 |
|------|------|---------|--------|
| 1 | `sync_types.rs` | 在 `SyncDataType` 枚举新增变体 | P0 |
| 2 | `sync_dto.rs` | 新建该实体的 `XxxSyncDTO` 结构体 | P0 |
| 3 | `sync_hash.rs` | 新增 `compute_xxx_config_hash()` 函数 | P0 |
| 4 | `db_manager.rs` | 新建/修改表定义，添加 `config_hash`、`content_hash` 字段 | P0 |
| 5 | `sync_manifest/manifest_builder.rs` | 新增 `build_xxx_manifest()` 函数 | P0 |
| 6 | 桌面端 `dto/*.js` | 新增字段白名单 `XXX_SYNC_FIELDS` 与提取函数 | P0 |
| 7 | 桌面端 `sync/entity.js` | 新增上传/下载/删除处理分支 | P0 |

### 1.2 Rust 端详细步骤

**第一步：扩展 `SyncDataType`**

在 `sync_types.rs` 的枚举中添加新变体，使用 `#[serde(rename_all = "lowercase")]` 确保序列化值为小写字符串：

```rust
pub enum SyncDataType {
    Agent,
    Group,
    Avatar,
    Topic,
    Message,
    PromptPreset, // 新增
}
```

**第二步：定义 DTO**

在 `sync_dto.rs` 中定义该实体参与同步的最小字段集。DTO（Data Transfer Object，数据传输对象）仅包含需要跨端同步的字段，排除本地 UI 状态（如 `current_topic_id`）：

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PromptPresetSyncDTO {
    pub name: String,
    pub content: String,
    pub category: String,
}
```

**第三步：注册哈希函数**

在 `sync_hash.rs` 的 `HashAggregator` 中新增配置哈希计算函数。必须遵循**字段白名单**原则——仅提取参与同步的字段，按字典序排序键名后计算 SHA-256：

```rust
pub fn compute_prompt_preset_hash(dto: &PromptPresetSyncDTO) -> String {
    let meta = serde_json::json!({
        "name": &dto.name,
        "content": &dto.content,
        "category": &dto.category,
    });
    compute_deterministic_hash(&meta)
}
```

**第四步：修改 ManifestBuilder**

在 `sync_manifest/manifest_builder.rs` 中新增清单构建函数。该函数从 SQLite 查询实体的 `id`、`config_hash`、`updated_at`、`deleted_at`，并组装为 `Vec<EntityState>`：

```rust
pub async fn build_prompt_preset_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
    // SELECT preset_id, config_hash, updated_at, deleted_at FROM prompt_presets
    // 映射为 EntityState 数组
}
```

### 1.3 桌面端（Node.js）详细步骤

**第五步：字段白名单与 DTO 提取**

在桌面端新建 `dto/preset.dto.js`，定义白名单数组与提取函数：

```javascript
const PRESET_SYNC_FIELDS = ["name", "content", "category"];

function extractPresetDTO(config) {
  const dto = {};
  PRESET_SYNC_FIELDS.forEach((field) => {
    dto[field] = config[field] ?? "";
  });
  return dto;
}
```

**第六步：实体传输处理**

在 `sync/entity.js` 的 `uploadEntity` 与 `downloadEntity` 中新增 `prompt_preset` 分支。若新实体需要独立文件存储（类似 Agent/Group），还需在 `config/defaults.js` 中定义默认值工厂。

### 1.4 注意事项

| 风险点 | 说明 | 缓解措施 |
|--------|------|---------|
| 字段白名单漂移 | 新增字段后，若仅更新一端，hash 将永远不一致 | 使用双端对照表（见本文档附录C）逐字段核对 |
| 默认值不一致 | 双端对缺失字段的默认值不同，导致首次同步即冲突 | 桌面端 `extractXxxDTO` 必须使用 `?? DEFAULTS[field]` 填充 |
| 哈希字段循环依赖 | `config_hash` 字段本身被纳入 hash 计算 | 确保 DTO 中不含 `config_hash`、`content_hash` 等元数据字段 |
| 数据库迁移 | 存量用户缺少新表/新字段 | 在 `db_manager.rs` 的 `setup_tables` 中编写 `ALTER TABLE` 兜底语句 |

---

## 2. 如何调试 Hash 不一致

### 2.1 排查流程（决策树）

```
收到 "hash mismatch" 报告
    |
    ▼
+-----------------------------------+
| 步骤1：确认哪一层级 mismatch      |
| Agent/Group → Topic → Message     |
+-----------------------------------+
    |
    ├─ Agent/Group 层级
    |   ▼
    |   检查 config_hash（配置变更）
    |   检查 content_hash（子 Topic 变更）
    |   ▼
    |   导出双端 DTO JSON，对比差异字段
    |
    ├─ Topic 层级
    |   ▼
    |   检查 config_hash（title/locked/unread）
    |   检查 content_hash（消息聚合）
    |   ▼
    |   用 stable_stringify 对比双端序列化结果
    |
    └─ Message 层级
        ▼
        检查 content + attachmentHashes
        ▼
        确认附件哈希列表排序一致
        ▼
        检查 temperature 精度截断（2位小数）
```

### 2.2 常用工具与关键检查点

**工具一：移动端日志**

在 `sync_service.rs` 的 `SYNC_DIFF_RESULTS` 处理分支中，移动端会输出以下日志：

```
[{}] Diff: pull={} push={} delete={} push_delete={}
```

通过 `vcp-log` 事件或 `sync_logs/` 目录下的日志文件，可定位具体实体的 Action 类型。

**工具二：桌面端日志**

桌面端 `SyncLogger` 输出结构化日志：

```
[owner_metadata] diff agent success: push=3 pull=2 delete=1 push_delete=0
```

**工具三：双端 Hash 对比脚本**

编写独立脚本分别调用双端的 hash 计算函数，输入相同 DTO，对比输出：

| 检查点 | 移动端位置 | 桌面端位置 | 常见差异原因 |
|--------|-----------|-----------|-------------|
| Agent config hash | `sync_hash.rs:54` | `core/hash.js:95` | temperature 精度、字段缺失 |
| Topic metadata hash | `sync_hash.rs:30` | `dto/topic.dto.js` | locked/unread 默认值、owner_id 混入 |
| Message fingerprint | `sync_hash.rs:11` | `core/hash.js:65` | 附件哈希排序、空附件列表处理 |
| Merkle Root | `sync_types.rs:22` | `core/hash.js:119` | 空集合返回 `""` vs `null`、排序规则 |

**关键检查点清单**：

1. **字段白名单一致性**：双端参与 hash 的字段数量与名称必须完全一致。
2. **类型归一化**：`temperature` 必须截断到 2 位小数；`createdAt`/`contextTokenLimit` 必须转为整数。
3. **空值处理**：`null` / `undefined` / `Option::None` 必须被跳过，不参与序列化。
4. **排序规则**：Object 键按字典序；Array 元素按业务规则（消息按 `timestamp ASC, msg_id ASC`）。
5. **编码一致**：双端均使用 UTF-8，无 BOM 头。

### 2.3 快速修复模板

当发现 hash 不一致时，按以下顺序修复：

| 优先级 | 操作 | 预期结果 |
|--------|------|---------|
| P0 | 删除移动端 `sync_state.db`（如有）并重启 | 重建索引基线 |
| P0 | 删除桌面端 `sync_state.db` 并重启 VCPDistributedServer | 重建索引基线 |
| P1 | 运行 `pnpm check` 确保无编译错误 | 消除类型层面差异 |
| P1 | 在双端分别打印 `stable_stringify` 中间结果 | 定位逐字节差异 |
| P2 | 检查 `AGENTS.md` 默认值对照表 | 修复默认值漂移 |

---

## 3. 如何扩展同步阶段

### 3.1 新增 PipelinePhase 的步骤

当前流水线（Pipeline）包含 Phase 1/2/2.5/3/Finalize。若需新增 Phase（例如 `Phase4Attachments` 用于独立附件同步），按以下步骤操作：

**步骤一：扩展枚举**

在 `sync_pipeline/pipeline_state.rs` 的 `PipelinePhase` 中添加新变体：

```rust
pub enum PipelinePhase {
    // ... 现有变体
    Phase4Attachments {
        progress: PhaseProgress,
    },
}
```

**步骤二：扩展 PipelineCommand**

在 `sync_pipeline/pipeline.rs` 的 `PipelineCommand` 枚举中添加指令：

```rust
pub enum PipelineCommand {
    // ... 现有指令
    StartAttachmentSync,
}
```

**步骤三：实现状态转换函数**

在 `SyncPipeline` 实现中新增转换方法：

```rust
pub async fn on_attachment_phase_ready(&self) -> Result<(), String> {
    {
        let mut state = self.state.write().await;
        *state = PipelinePhase::Phase4Attachments {
            progress: PhaseProgress::new(),
        };
    }
    let _ = self.command_tx.send(PipelineCommand::StartAttachmentSync);
    Ok(())
}
```

**步骤四：在 sync_service.rs 中处理新指令**

在 `run_sync_session` 的 `pipeline_rx.recv()` 分支中，为 `StartAttachmentSync` 添加处理逻辑：构建附件清单、发送 WS 消息、派发 HTTP 任务。

### 3.2 风险点

| 风险 | 说明 | 缓解措施 |
|------|------|---------|
| 状态机分支爆炸 | 每新增一个 Phase，前端需同步更新进度条组件 | 保持 Phase 数量 ≤ 6，复杂逻辑用子阶段（如 Phase 2.5）实现 |
| 前端兼容性问题 | 旧版前端不认识新 Phase 枚举值 | 使用 `#[serde(rename_all = "camelCase")]` 确保命名稳定；前端对未知 Phase 显示"同步中..." |
| 阶段间数据传递 | 新 Phase 可能需要前一阶段的上下文 | 通过 `Arc<Mutex<T>>` 共享状态容器传递，避免全局变量 |
| Watchdog 超时 | 新 Phase 执行时间超过 60 秒 | 在长时间任务中定期更新 `pending_tasks` 或调整看门狗阈值 |

---

## 4. 常见问题排查流程

### 4.1 同步完全失败的决策树

```
点击"同步"后无反应或立即报错
    |
    ▼
+-----------------------------------+
| 检查1：桌面端插件是否启动？        |
| 日志中是否出现"WebSocket 同步总线已启动" |
+-----------------------------------+
    |
    ├─ 否 → 检查 VCPDistributedServer 是否运行
    |       检查 plugin-manifest.json 版本是否为 0.9.13
    |       检查 5975 端口是否被占用
    |
    └─ 是 → 继续
            |
            ▼
    +-------------------------------+
    | 检查2：移动端能否连接 WS？     |
    | vcp-sync-status 是否为 connecting → error |
    +-------------------------------+
        |
        ├─ 连接失败 → 检查防火墙/同局域网
        |           检查 sync_server_url 是否为 ws://IP:5975
        |           检查 sync_token 是否一致
        |
        └─ 连接成功 → 继续
                    |
                    ▼
        +---------------------------+
        | 检查3：版本校验是否通过？  |
        | 日志是否出现"版本验证通过" |
        +---------------------------+
            |
            ├─ 版本不匹配 → 升级桌面端插件或移动端 App
            |
            └─ 版本通过 → 继续
                        |
                        ▼
            +-----------------------+
            | 检查4：Hash 初始化是否成功？ |
            | 是否有"数据库初始化失败"日志 |
            +-----------------------+
                |
                ├─ 初始化失败 → 检查 SQLite 文件权限/磁盘空间
                |
                └─ 成功 → 进入 Phase 1/2/3 具体排查
```

### 4.2 同步卡住（进度条不动）的排查

| 现象 | 可能原因 | 排查方法 |
|------|---------|---------|
| Phase 1 卡住 | `pending_tasks` 未归零，某条 Pull/Push 死锁 | 检查日志中最后一条 `[PullExecutor]` 或 `[PushExecutor]` 记录 |
| Phase 2 卡住 | Topic Manifest 响应未收到 | 检查桌面端 `handleSyncManifest` 是否抛出异常 |
| Phase 2.5 卡住 | `SYNC_TOPIC_HASH_RESULTS` 未返回 | 检查桌面端 `diff.js` 是否崩溃 |
| Phase 3 卡住 | `Phase3Tracker` 计数未达标 | 检查是否有 Topic 的 `mark_completed` 未被调用 |
| Finalize 卡住 | `DbWriteQueue` flush 阻塞 | 检查 SQLite WAL 锁竞争 |

**看门狗（Watchdog）机制**：当 `pending_tasks` 连续 60 秒无变化时，系统会强制推进相位。若日志中出现 `"Watchdog: forcing phase advance"`，说明此前存在任务死锁。

### 4.3 数据不一致的排查

| 现象 | 可能原因 | 修复步骤 |
|------|---------|---------|
| 某 Agent 配置双端不同 | LWW 时间戳碰撞导致覆盖 | 手动修改一次配置，触发新的 `updated_at` |
| 某 Topic 消息缺失 | Phase 3 Fast Path 误判 | 删除桌面端 `sync_state.db` 重建索引 |
| 附件无法显示 | 附件哈希在移动端缺失 | 检查 `attachments` 表是否有该 hash；若无，手动触发附件上传 |
| 已删除实体重新出现 | 软删除未正确传播 | 检查双端 `deleted_at` 是否一致；不一致时手动删除桌面端物理目录 |

---

## 5. 版本兼容性策略

### 5.1 协议版本协商

VCP Mobile 采用**严格版本匹配**策略，不支持向前/向后兼容的松散协商。

**协商流程**：

| 步骤 | 方向 | 消息 | 字段 | 处理位置 |
|------|------|------|------|---------|
| 1 | 移动 → 桌面 | `VERSION_CHECK` | `mobileVersion: string` | `sync_service.rs` |
| 2 | 桌面 → 移动 | `VERSION_ACK` | `version: string` | `index.js` (读取 plugin-manifest.json) |
| 3 | 移动端校验 | — | 字符串精确匹配 | `sync_service.rs` |

**校验规则**：

- 版本一致：继续同步流程。
- 版本不一致：移动端断开 WS，状态置为 `error`，提示用户更新插件。
- 超时未收到 `VERSION_ACK`（5秒）：视为桌面端插件过旧，同样断开。

### 5.2 字段增减的平滑升级

虽然版本号要求严格匹配，但在**同版本内**增加字段时，可通过以下机制实现平滑升级：

**机制一：DTO 可选字段**

Rust 端使用 `#[serde(default)]` 或 `Option<T>`，桌面端使用 `?? DEFAULTS[field]`：

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub new_field: Option<String>,
```

**机制二：数据库 ALTER TABLE 兜底**

在 `db_manager.rs` 的表初始化函数中，对每个表执行 `PRAGMA table_info` 检查，缺失字段自动 `ALTER TABLE ADD COLUMN`：

```rust
"ALTER TABLE agents ADD COLUMN new_field TEXT DEFAULT ''"
```

**机制三：桌面端索引宽容解析**

桌面端 `entity_index` 的 `hash` 计算仅依赖白名单字段，新增字段默认不参与 hash，因此旧索引在新字段存在时仍有效。

### 5.3 版本升级 checklist

当需要发布新版本（如 `0.9.13 → 0.9.14`）时，必须同步修改：

| 位置 | 当前值 | 修改后 | 说明 |
|------|--------|--------|------|
| `sync_service.rs` | `EXPECTED_PLUGIN_VERSION` | 新版本号 | 移动端期望版本 |
| `plugin-manifest.json` | `version` | 新版本号 | 桌面端实际版本 |
| `package.json` | `version` | 新版本号 | 移动端应用版本 |
| `Cargo.toml` | `version` | 新版本号 | Rust 编译版本 |

---

## 6. FAQ

### Q1: 同步为什么慢？

**常见原因与优化建议**：

| 瓶颈环节 | 症状 | 优化方案 |
|---------|------|---------|
| 首次同步全量拉取 | Phase 1/2/3 均需全量传输 | 正常现象，后续增量同步会大幅加速 |
| 消息量过大 | Phase 3 的 `detailed` 比例高 | 检查 Hash 冒泡是否正常；若不正常，重建桌面端索引 |
| 网络延迟高 | 每个 HTTP 请求 RTT > 200ms | 确保手机与电脑在同一局域网；避免使用移动数据 |
| SQLite 锁竞争 | `database is locked` 日志 | 减少并发写入；确保 WAL 模式已启用 |
| 附件重复上传 | `uploaded_hashes` 未共享 | 检查 `Arc::clone` 是否正确使用 |

### Q2: Hash 不一致怎么办？

参见本文档第 2 节「如何调试 Hash 不一致」。快速修复：双端均删除 `sync_state.db`（移动端如有缓存）并重启，强制重建索引基线。

### Q3: 多设备支持吗？

**当前版本（0.9.13）不支持多移动端同时连接。** 桌面端插件未实现多客户端会话隔离与并发写入排队。若手机 A 与手机 B 同时修改同一实体，后完成的写入会覆盖前者。

**缓解措施**：一次只连接一台手机；更换设备前断开旧连接或重启桌面端。

### Q4: 同步会覆盖我的本地数据吗？

**取决于时间戳（LWW 策略）**：

- 若桌面端数据更新（`updated_at` 更大），移动端会被覆盖。
- 若移动端数据更新，桌面端会被覆盖。
- 若时间戳相等，默认桌面端胜出（`PULL`）。

**保护措施**：所有删除均为软删除（`deleted_at` 标记），30 天内可通过清理任务恢复；实体写入采用原子写入（`tmp → rename`），防止半写入损坏。

### Q5: 为什么桌面端插件启动后需要等待几秒才能同步？

桌面端插件在开放 WebSocket/HTTP 端口前，必须完成 `reconcileLocalFiles` 全量索引扫描。这是核心安全约束——防止移动端在索引未就绪时发起同步，导致差异计算错误。数据量极大时（数万条消息），扫描可能需要 3-10 秒。

### Q6: 同步过程中断网会怎样？

1. WebSocket 断开 → 移动端捕获错误 → 指数退避重试（最多 3 次）。
2. 若重试耗尽 → 发布 `vcp-sync-status` = `error`。
3. 重新连接后 → 整个同步流水线从头开始（Manifest 重新发送）。
4. 幂等性键（Idempotency Key）保证重复上传不会导致数据重复。

### Q7: 附件为什么有时显示不出来？

VCP Mobile 采用「先写消息、后补附件」的懒加载策略：

1. 消息同步时，若本地缺失附件，会填充占位符路径 `file://attachments/{hash}`。
2. 附件实际传输在消息同步完成后，按需通过 `upload-attachment` / `download-attachment` 端点补齐。
3. 若附件始终未补齐，检查：
   - 移动端 `attachments` 表是否包含该 hash；
   - 桌面端 `attachment_index` 是否指向正确的物理路径；
   - 文件是否超过 100MB 限制。

### Q8: 如何清理已删除的数据？

移动端：`DeleteExecutor::cleanup_old_deleted_records(app, days)` 会物理删除超过 `days` 天的软删除记录。桌面端：插件每小时自动执行一次 `cleanupOldDeletedRecords()`，默认清理 30 天前的记录。

**手动触发**：重启桌面端插件或移动端应用，清理任务会在启动时执行。

### Q9: 可以只同步某个 Agent 吗？

当前协议不支持单实体同步。同步的最小粒度是**一次完整的 Sync Session**（Phase 1 → 2 → 2.5 → 3 → Finalize）。但 V2 的 `targetedOwners` 机制会自动将 Phase 2 的范围缩小到**仅变更的 Owner**，避免全量 Topic 比对。

### Q10: 如何确认双端数据已完全一致？

**方法**：对比双端的 Agent/Group/Topic 聚合哈希（Merkle Root）。

| 层级 | 移动端位置 | 桌面端位置 |
|------|-----------|-----------|
| Agent | `agents.content_hash` | `entity_index.aggregated_hash` (type=agent) |
| Topic | `topics.content_hash` | `entity_index.aggregated_hash` (type=topic) |

若所有对应实体的哈希均一致，则可认为数据完全一致（概率意义上，SHA-256 碰撞可忽略）。

### Q11: 修改了同步相关代码后必须做什么？

**强制检查清单**：

1. 运行 `pnpm check`（`vue-tsc --noEmit && cargo check`）。
2. 若修改了 DTO 或 Hash 计算，执行双端全量同步测试，验证相同配置的哈希值是否一致。
3. 若修改了 `plans/` 目录，执行 `pnpm memory:refresh`。
4. 若涉及超过 500 行的文件重构，先执行 `git add . && git commit -m "save"`。

### Q12: 桌面端 `sync_state.db` 可以删除吗？

**可以，且是官方推荐的修复手段之一。** `sync_state.db` 是纯索引缓存，不存储实际业务数据。删除后插件在下次启动时会自动重新执行 `reconcileLocalFiles`，重建完整索引。

**必须删除的场景**：插件版本升级、`AppData` 路径变更、发现索引异常、首次安装后的兼容性问题。

---

*本文档基于 VCPMobile v0.9.13 同步模块源代码编写。各节引用的代码位置以实际源码为准。*
