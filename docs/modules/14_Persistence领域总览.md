---
id: MOD-PERSISTENCE-014
version: "1.1.3"
date: 2026-08-11
module: persistence/
scope: src-tauri/src/vcp_modules/persistence/
related: [db_manager.rs, db_write_queue.rs, message_repository.rs, sync_service.rs, chat_manager.rs, message_service.rs]
---

# 14_持久化层与数据访问（Persistence 领域总览）

## 1. 概述

### 1.1 领域定位

`persistence/` 是 VCP Mobile 重组后的 7 大领域目录之一，位于 `src-tauri/src/vcp_modules/persistence/`。该领域统管所有本地数据的持久化存储与访问，是前端 Vue 3 状态、Rust 核心业务逻辑与 SQLite 物理文件之间的唯一正式通道。

在 Double-Track 3-Tier 架构中，persistence/ 处于**底层数据轨道**的核心位置：
- 向上为 `chat/`、`agent/`、`sync/` 等领域提供类型安全的数据操作接口。
- 向下直接操作 SQLite 文件，通过 WAL 模式、批量事务、内存映射等手段将移动端磁盘 I/O 的延迟降至最低。
- 横向与 `infra/`（文件管理器）协作，完成附件元数据与物理文件的联合存储。

### 1.2 职责边界

| 模块 | 文件 | 核心职责 | 关键设计决策 |
|------|------|---------|-------------|
| 数据库管理器 | `db_manager.rs` | 连接池生命周期、Schema 初始化与迁移、PRAGMA 调优 | sqlx 异步连接池 (`max_connections=5`) + WAL 模式 |
| 写入队列 | `db_write_queue.rs` | 单工作线程批量写入、消除 SQLite 并发锁竞争、同步哈希冒泡 | mpsc 队列 + `spawn_blocking` + rusqlite 直连 |
| 消息仓储 | `message_repository.rs` | 消息读写、渲染编译、内容压缩、全量重建/压缩维护任务 | `MessageRenderCompiler` + `ContentCompressor` + 三段流水线 |

### 1.3 整体数据流

```text
Vue 3 前端 / Rust 业务层
    │
    ├─→ 普通查询（如加载话题列表、读取消息）
    │   ↓
    │   sqlx::Pool<Sqlite>（DbState.pool）
    │   ↓
    │   直接返回结果
    │
    └─→ 写入/同步（如收到同步消息、创建话题）
        ↓
        DbWriteQueue.submit(DbWriteTask::TopicMessages { ... })
        ↓
        mpsc::channel(256) → 单 Worker 线程
        ↓
        spawn_blocking → rusqlite::Connection::open(db_path)
        ↓
        批量事务合并 → 统一哈希冒泡 → 提交
```

**为什么查询与写入走不同通道？**
- 查询需要高并发、低延迟、异步友好 → sqlx 连接池是最佳选择。
- 写入在 SQLite 上天然串行（即使 WAL 模式，写操作仍需 `WAL_WRITE_LOCK`）。若所有业务层直接通过 sqlx 并发写入，会导致大量 `BUSY` 错误和重试。
- 写入队列将并发写入请求收敛为单线程顺序批量事务，配合 rusqlite 的同步事务语义，实现吞吐量最大化与锁竞争最小化。

---

## 2. 数据库管理器（`db_manager.rs`）

`db_manager.rs`（约 740 行）是 persistence/ 领域的入口模块，负责 SQLite 数据库的初始化、连接池配置、**基于 sqlx 迁移引擎的版本化 Schema 管理**、页面大小优化与损坏自愈。

### 2.1 DbState

```rust
pub struct DbState {
    pub pool: Pool<Sqlite>,
    pub path: std::path::PathBuf,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `pool` | `Pool<Sqlite>` | sqlx 异步连接池，供全应用普通查询使用 |
| `path` | `PathBuf` | 数据库文件的绝对物理路径，供 `DbWriteQueue` 直接打开 rusqlite 连接 |

`DbState` 以 Tauri `State` 形式挂载到 `AppHandle`，通过 `app_handle.state::<DbState>()` 在任意 Tauri Command 中获取（L6–L9）。

### 2.2 连接池初始化

```rust
pub async fn init_db(app_handle: &AppHandle) -> Result<(Pool<Sqlite>, PathBuf), String>
```

**流程**（L11–L64）：

1. **路径解析**：通过 `app_handle.path().app_config_dir()` 获取配置目录，追加 `vcp_avatar.db`（L26–L39）。在 Android 上，该路径通常为 `/data/user/0/com.vcp.avatar/files/vcp_avatar.db`。
2. **连接选项配置**（L44–L65）：链式配置 SQLite PRAGMA。
3. **连接池创建**：`SqlitePoolOptions::new().max_connections(5).connect_with(...)`（L67）。
4. **运行版本化迁移**：调用 `run_migrations(&pool).await?`（L85），由 `sqlx::migrate!("./migrations")` 应用 `src-tauri/migrations/` 下的全部 SQL 迁移。
5. **返回**：`(pool, db_path)`，由调用方（`lib.rs` 生命周期管理器）组装为 `DbState` 并挂载到 App State。

### 2.3 损坏检测与恢复单元

数据库自愈遵循 fail-closed 边界，不能把任意 open/check 错误解释为损坏：

1. 在任何 SQLite open 之前检查文件集合。主 DB 缺失但 `-wal` 或 `-shm` 任一存在时，立即停止初始化并保留现场；不得让 `create_if_missing` 先创建空主库、间接清掉旁车；
2. `PRAGMA quick_check(1)` 只有返回非 `ok`，或错误码 primary code 为 `SQLITE_CORRUPT(11)` / `SQLITE_NOTADB(26)` 时，才授权恢复；BUSY、LOCKED、IOERR 等全部归类为“暂不可用”，不重建；
3. 已确认损坏时先关闭连接池，再把主 DB、WAL、SHM 移入同一个唯一恢复目录。普通 rename 失败会反向回滚此前移动，不删除任何旁车；
4. 归档成功后才创建干净数据库。恢复通知及 archive path 保存在 `LifecycleState.database_recovery`，并通过 `get_system_snapshot` 交给前端持久警告，避免启动早期一次性事件丢失。

这套流程保证常规错误路径不会静默丢弃最近已提交但尚未 checkpoint 的 WAL。三文件 rename 仍不是掉电原子操作；遇到异常断电留下的不完整恢复单元必须人工保全处理，不能自动猜测。

### 2.4 WAL 模式与深度性能调优

连接选项通过链式 `pragma` 调用实现 9 项深度优化（`db_manager.rs:56-65`）：

| PRAGMA | 设置值 | 作用 |
|--------|--------|------|
| `journal_mode` | `WAL` | 启用 Write-Ahead Logging，允许读操作与写操作并发，极大降低 UI 卡顿 |
| `synchronous` | `NORMAL` | WAL 模式下兼顾安全性与速度（每次 checkpoint 同步，而非每次事务） |
| `busy_timeout` | `30000` ms | 锁冲突时自动等待 30 秒，避免立刻抛出 `database is locked` |
| `mmap_size` | `268435456` (256 MB) | 开启内存映射 I/O，将磁盘读取转为内存访问 |
| `temp_store` | `2` (MEMORY) | 临时表与排序操作强制在内存中执行 |
| `page_size` | `16384` (16 KB) | 匹配现代闪存页大小，提升顺序 I/O 效率 |
| `cache_size` | `-8000` (8000 页 ≈ 128 MB) | 负值表示以页为单位，增大数据页缓存 |
| `auto_vacuum` | `2` (INCREMENTAL) | 增量清理逻辑，配合后续维护任务物理回收空间 |
| `foreign_keys` | `1` | 开启外键约束，支持级联删除 |

**WAL 模式的移动端优势**：
- 传统 `DELETE` 日志模式下，写操作会阻塞所有读操作。
- WAL 模式下，读操作可以从旧的快照继续执行，写操作仅追加到独立的 `.wal` 文件。
- 这允许前端在同步大批量写入时，仍然流畅地滚动浏览历史消息。
- checkpoint 操作（将 WAL 内容刷回主数据库）由 SQLite 自动管理，通常在 WAL 文件达到 1000 页时触发。

### 2.5 连接池容量决策

`max_connections = 5` 并非随意选取，而是基于 SQLite 的并发特性和移动端资源约束的权衡：

| 因素 | 分析 |
|------|------|
| SQLite 写串行性 | 即使有多个连接，写操作在底层仍按顺序执行，过多连接只会增加锁竞争 |
| 移动端内存 | 每个连接占用页缓存（默认约 2 MB），5 个连接约 10 MB，在 Android 低端机上可接受 |
| 读并发需求 | 前端同时可能进行：消息列表加载、话题列表加载、附件查询、设置读取，5 个连接足以覆盖 |
| Tauri IPC 并发 | 前端同时发起的 invoke 调用通常不超过 3–4 个，5 个连接有充足余量 |

### 2.6 Schema 初始化与版本迁移（v1.1.3 重构）

v1.1.3 起，数据库 Schema 不再通过 `setup_tables` 函数硬编码创建，而是使用 **`sqlx::migrate!("./migrations")`** 进行版本化迁移管理，配合 `db_manager.rs` 中的 `run_migrations()` 与 `bootstrap_legacy_if_needed()` 实现从 1.1.2 平滑升级。

**迁移文件清单**（`src-tauri/migrations/`）：

| 文件 | 版本 | 说明 |
|------|------|------|
| `0001_create_initial_tables.sql` | 1 | 初始化全部业务表与 9 个索引 |
| `0002_add_deleted_at_to_message_attachments.sql` | 2 | 为 `message_attachments` 增加 `deleted_at` 软删除字段 |
| `0003_create_messages_fts.sql` | 3 | 创建 `messages_fts` FTS5 虚拟表及删除同步触发器 |
| `0004_fix_fts_triggers.sql` | 4 | 修复触发器使用复合主键 `(topic_id, msg_id)`，避免跨话题误删索引 |

**核心表结构**（按创建顺序）：

| 表名 | 主键 | 核心用途 |
|------|------|---------|
| `avatars` | `(owner_type, owner_id)` | 全局多态头像，含二进制 BLOB 与预计算主色调 |
| `agents` | `agent_id` | 智能体配置，含 `mobile_system_prompt` / `use_temperature` / `config_hash` / `content_hash` |
| `groups` | `group_id` | 群组配置，含成员关系外键 |
| `group_members` | `(group_id, agent_id)` | 群组成员与标签 |
| `topics` | `topic_id` | 话题元数据，`owner_type` + `owner_id` 区分归属 |
| `messages` | `(topic_id, msg_id)` | 消息历史（复合主键），`content` 为明文 TEXT |
| `render_cache` | `(topic_id, msg_id)` | 预渲染 AST 二进制缓存，独立表避免消息表膨胀 |
| `message_attachments` | `(topic_id, msg_id, attachment_order)` | 消息-附件关联关系（v1.1.3 新增 `deleted_at`） |
| `attachments` | `hash` | 附件物理文件真理之源（内容寻址） |
| `settings` | `key` | 全局键值配置 |
| `model_favorites` | `model_id` | 收藏模型 |
| `model_usage_stats` | `model_id` | 模型使用计数 |
| `emoticon_library` | `id` (AUTOINCREMENT) | 表情包修复库 |
| `tarven_rules` | `id` | VCPChatTarven 规则库 |
| `active_generations` | `msg_id` | 活跃生成注册表（断点续传事务日志） |
| `messages_fts` | 虚拟表 | FTS5 全文搜索索引 |

**关键迁移逻辑 1：1.1.2 遗留用户桥接**（`bootstrap_legacy_if_needed`，`db_manager.rs:268-367`）

- **检测**：存在 `messages` 表但不存在 `_sqlx_migrations` 表时，判定为 1.1.2 遗留数据库。
- **推断**：通过 `PRAGMA table_info(message_attachments)` 检查 `deleted_at` 列，以及 `messages_fts` 表存在性，推断 Migration 2/3 是否已实际执行。
- **播种**：手动创建 `_sqlx_migrations` 表，并插入对应版本的虚拟记录（checksum 取自 `migrator.migrations[i].checksum`），让 sqlx 迁移器跳过已执行的迁移。
- **安全**：仅对真实存在的 Schema 状态播种，避免重复执行 DDL。

**关键迁移逻辑 2：页面大小升级与 VACUUM**（`db_manager.rs:87-162`）

- 启动时检测 `PRAGMA page_size`，若不等于 `16384`：
  1. 关闭当前 WAL 连接池。
  2. 以 `journal_mode = DELETE` 打开临时单连接。
  3. 执行 `PRAGMA page_size = 16384; VACUUM;`。
  4. 关闭临时连接，重新打开标准 WAL 连接池。
- 该过程会触发 `CoreStatus::Optimizing` 状态广播到前端。

**关键迁移逻辑 3：历史压缩数据解压**（`decompress_database_migration`，`db_manager.rs:521-730`）

- v1.1.3 将 `messages.content` 从 zstd 压缩 BLOB 改回明文 TEXT。
- 启动时若检测到 `typeof(content) = 'blob'` 的记录，进入解压迁移：
  1. 分批读取压缩消息（每批 200 条）。
  2. 校验 zstd 魔数头 `[0x28, 0xB5, 0x2F, 0xFD]`，解压缩后写回 `TEXT`。
  3. 同步重建 `messages_fts` 全文索引（仅对未删除消息）。
  4. 最后执行 `VACUUM` 回收空间，并进入 `DecompressionComplete` 状态等待用户重启。

### 2.7 关键表字段详解

#### messages 表

```sql
CREATE TABLE messages (
    msg_id TEXT NOT NULL,
    topic_id TEXT NOT NULL,
    role TEXT NOT NULL,
    name TEXT,
    agent_id TEXT,
    content TEXT NOT NULL,          -- 明文消息文本（v1.1.3 起由压缩 BLOB 解压为 TEXT）
    timestamp BIGINT NOT NULL,
    is_group_message INTEGER NOT NULL DEFAULT 0,
    group_id TEXT,
    finish_reason TEXT,
    content_hash TEXT NOT NULL DEFAULT '',
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    deleted_at BIGINT,
    PRIMARY KEY (topic_id, msg_id)
);
```

| 字段 | 说明 |
|------|------|
| `content` | 存储明文消息文本。v1.1.3 起由 zstd 压缩 BLOB 迁移为 TEXT，以支持 FTS5 全文搜索与直接读取 |
| `content_hash` | 消息内容与附件 hash 的聚合指纹，用于同步 Diff；同步下载时若桌面端已提供则直接复用 |
| `is_thinking` | **已弃用**。v0.9.14 起数据库 `messages` 表已移除该列；应用层 `ChatMessage` / Sync DTO 仍保留字段，读取时固定为 `Some(false)` |
| `finish_reason` | 流式输出的结束原因（如 `stop`、`length`、`error`） |
| `deleted_at` | 软删除时间戳，`NULL` 表示未删除。同步系统依赖此字段识别删除状态 |

#### render_cache 表

```sql
CREATE TABLE render_cache (
    topic_id TEXT NOT NULL,
    msg_id TEXT NOT NULL,
    render_content BLOB,            -- zstd 压缩的 JSON AST
    content_hash TEXT NOT NULL,      -- 编译时对应的消息正文指纹
    renderer_schema_version INTEGER NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (topic_id, msg_id),
    FOREIGN KEY (topic_id, msg_id) REFERENCES messages(topic_id, msg_id) ON DELETE CASCADE
);
```

| 字段 | 说明 |
|------|------|
| `render_content` | `MessageRenderCompiler::serialize` 生成的 zstd 压缩 JSON，存储 `Vec<ContentBlock>` |
| `content_hash` | 与 `messages.content_hash` 相等时缓存才可命中；正文变化会自动失效 |
| `renderer_schema_version` | 渲染器协议版本；版本不匹配时按 cache miss 重编译 |
| `ON DELETE CASCADE` | 消息被删除时，渲染缓存自动级联删除，无需应用层处理 |

### 2.8 新增表：`active_generations`（v1.1.3）

```sql
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
```

| 字段 | 说明 |
|------|------|
| `msg_id` | 正在生成的助手消息 ID |
| `topic_id` / `owner_id` / `owner_type` | 消息归属，用于恢复时定位上下文 |
| `created_at` | 注册时间戳，辅助判断超时 |

**生命周期**：
- **INSERT**：`message_service::append_single_message` 在收到 `thinking` 事件、写入 `messages` 骨架时，对 `role = assistant` 且 `finish_reason IS NULL` 的消息执行 `INSERT OR REPLACE`（`message_service.rs:669-682`）。
- **DELETE**：`message_service::finalize_stream_message` 在流正常结束/中止/错误后执行 `DELETE FROM active_generations WHERE msg_id = ?`（`message_service.rs:1065-1070`）。
- **清理**：`delete_executor` 在软删除 Agent/Group/Topic 时级联清理相关活跃生成记录；`topic_service` 删除话题时清理。

### 2.9 索引策略

迁移 `0001_create_initial_tables.sql` 创建 9 个索引，覆盖高频查询路径：

| 索引名 | 字段 | 服务场景 |
|--------|------|---------|
| `idx_topics_owner` | `(owner_id, owner_type, created_at DESC)` | 加载某个 Agent/Group 的话题列表 |
| `idx_messages_topic_time` | `(topic_id, timestamp DESC)` | 按话题加载消息时间线 |
| `idx_messages_updated_at` | `(updated_at)` | 同步增量扫描（按更新时间筛选） |
| `idx_group_members_agent` | `(agent_id)` | 查询某 Agent 所属的所有群组 |
| `idx_message_attachments_hash` | `(hash)` | 根据 hash 反查关联消息 |
| `idx_message_attachments_msg` | `(topic_id, msg_id)` | 加载单条消息的附件列表 |
| `idx_render_cache_msg` | `(topic_id, msg_id)` | 快速命中渲染缓存 |
| `idx_emoticon_category` | `(category)` | 表情包按分类检索 |
| `idx_tarven_rules_active` | `(rule_type, is_enabled, sort_order ASC)` | 按规则类型与启用状态加载 Tarven 规则 |

**索引设计原则**：所有索引均围绕**实体归属**（owner_id）、**时间线排序**（timestamp DESC）、**同步扫描**（updated_at）三大查询模式构建，避免过度索引带来的写入开销。

---

## 3. 写入队列（`db_write_queue.rs`）

`db_write_queue.rs`（784 行）是 persistence/ 领域的**写入咽喉**，负责将所有上层写入请求串行化、批量化、事务化。

### 3.1 单线程 Worker 模型

```rust
pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
    logger: Option<Arc<Mutex<SyncLogger>>>,
    db_path: std::path::PathBuf,
    _worker: Option<tokio::task::JoinHandle<()>>,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `sender` | `mpsc::Sender<DbWriteTask>` | 外部提交写入任务的唯一入口，通道容量 256 |
| `logger` | `Option<Arc<Mutex<SyncLogger>>>` | 可选的同步日志器，用于记录写入审计 |
| `db_path` | `PathBuf` | 数据库物理路径，Worker 独立打开 rusqlite 连接 |
| `_worker` | `Option<JoinHandle<()>>` | 后台 Worker 句柄，Drop 时自动清理 |

**`Clone` 语义**（L61–L70）：
- `DbWriteQueue` 实现了 `Clone`，但克隆体仅复制 `sender`、`logger`、`db_path`，`_worker` 置为 `None`。
- 这意味着任意数量的业务模块可以持有 `DbWriteQueue` 的克隆体提交任务，但底层始终只有**一个** Worker 线程消费通道。

**Worker 启动**（L73–L247）：
1. 创建 `mpsc::channel(256)`。
2. `tokio::spawn` 启动异步 Worker 协程。
3. Worker 内部使用 `while let Some(first_task) = rx.recv().await` 循环等待任务。
4. 每轮循环收集一批任务后，通过 `tokio::task::spawn_blocking` 将实际数据库操作转移到阻塞线程池，避免阻塞 Tokio 异步调度器。

### 3.2 DbWriteTask 枚举

```rust
#[derive(Debug)]
pub enum DbWriteTask {
    Agent { id: String, dto: AgentSyncDTO },
    Group { id: String, dto: GroupSyncDTO },
    Avatar { owner_type: String, owner_id: String, bytes: Vec<u8> },
    AgentTopicBatch { topics: Vec<(String, AgentTopicSyncDTO)> },
    GroupTopicBatch { topics: Vec<(String, GroupTopicSyncDTO)> },
    TopicMessages {
        topic_id: String,
        messages: Vec<ChatMessage>,
        contents: Vec<String>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
        skip_bubble: bool,
    },
    Flush { tx: oneshot::Sender<()> },
}
```

| 变体 | 数据来源 | 写入目标 |
|------|---------|---------|
| `Agent` / `Group` | 同步服务（Sync Service） | `agents` / `groups` 表 |
| `Avatar` | 同步服务 / 头像上传 | `avatars` 表（BLOB） |
| `AgentTopic` / `GroupTopic` | 同步服务 | `topics` 表 |
| `AgentTopicBatch` / `GroupTopicBatch` | 同步服务批量同步 | `topics` 表（批量） |
| `TopicMessages` | 同步服务 / 聊天发送 | `messages` + `render_cache` + `message_attachments` |
| `Flush` | 同步服务（如 SyncPipeline 结束阶段） | 无实际写入，仅作为事务边界信号 |

`TopicMessages` 是最复杂的变体，承载了消息正文、渲染缓存、附件关联的三重写入。其中 `render_bytes`、`contents`、`content_hashes` 均与 `messages` 一一对应：`contents` 为明文消息正文，供批量插入时直接绑定；`render_bytes` 由调用方预先通过 `MessageRenderCompiler::serialize` 生成；`content_hashes` 供消息指纹直接入库；`skip_bubble` 用于控制是否跳过该话题的哈希冒泡（如初始化填充历史数据时不需要实时冒泡）。

### 3.3 批量事务合并

Worker 的核心设计是**将短时间窗口内的多个独立写入请求合并为单个 SQLite 事务**，大幅降低磁盘 fsync 次数。

**合并策略**（L83–L116）：

```text
接收第一个任务 first_task
│
├─→ 若 first_task 是 Flush → 立即确认，本轮结束
│
└─→ 初始化 tasks_in_this_tx = [first_task]
    初始化 total_msg_count = 该任务包含的消息数
    初始化 flush_tx_opt = None
    │
    └─→ 循环尝试拉取更多任务（最多 10ms 超时）
        ├─→ 收到 Flush → 记录 sender，中断拉取
        ├─→ 收到普通任务 → 加入批次（限制：总任务数 < 32，总消息数 < 500）
        └─→ 超时或通道空 → 中断拉取
```

| 限制项 | 阈值 | 设计理由 |
|--------|------|---------|
| 最大任务数 | 32 | 缩短同步事务占锁时间，给聊天终态写入留出调度窗口 |
| 最大消息数 | 500 | 限制消息解析结果与 SQL 参数的单事务峰值 |
| 合并窗口 | 10 ms | 保留批量吞吐，同时降低交互写等待 |

**事务执行**（L120–L216）：
- 在 `spawn_blocking` 闭包内：`rusqlite::Connection::open(&db_path)` 打开独立连接。
- 重复设置 WAL / NORMAL / busy_timeout（确保与 sqlx 侧一致）。
- `conn.transaction()?` 开启事务。
- 遍历 `tasks_in_this_tx`，按变体分发到对应的 `rusqlite_upsert_*` 私有方法。
- 收集 `affected_owners`（Agent/Group ID）和 `affected_topics`（Topic ID）。
- 统一冒泡哈希（见 §3.6）。
- `tx.commit()?` 提交。
- 根据执行结果累加 `success_count` 或 `error_count`，并在 Worker 停止时输出统计（L235–L238）。

### 3.4 Flush 穿透语义

`Flush` 是写入队列中唯一的**控制信号**而非数据负载，用于解决同步 pipeline 中的**时序确定性**问题。

```rust
pub async fn flush(&self) -> Result<(), String> {
    let (tx, rx) = oneshot::channel();
    if let Err(e) = self.sender.send(DbWriteTask::Flush { tx }).await { ... }
    rx.await.map_err(...)??;
    Ok(())
}
```

**两种穿透场景**（L84–L88, L104–L106, L230–L232）：

| 场景 | 行为 |
|------|------|
| **首任务即 Flush** | 事务队列为空，返回此前尚未结算的 batch 错误；无错误时返回 `Ok(())` |
| **合并中收到 Flush** | 终止当前批次收集；事务提交后返回本批与此前积累错误的汇总 Result |

**设计意图**：同步服务在批量发送 TopicMessages 后调用 `flush().await?`，不仅确认此前任务已处理，还把后台事务失败显式传回阶段状态机。任一 flush/finalizer 失败都会阻止 `PHASE_COMPLETED`，避免 UI 显示完成但数据库缺项。

### 3.5 Turbo rusqlite 模式

`db_write_queue.rs` 被注释为 **"Turbo rusqlite Mode"**（L78），其核心是绕过 sqlx 连接池，直接使用 rusqlite 进行同步批量写入。

**为什么不用 sqlx 执行批量写入？**

| 维度 | sqlx Pool | rusqlite Direct |
|------|-----------|-----------------|
| 连接获取 | 异步竞争，可能等待 | 独占打开，立即可用 |
| 参数绑定 | `sqlx::query` 动态绑定 | `prepare_cached` 复用语句句柄 |
| 批量插入 | 逐条 `execute` | 单条多值 `INSERT ... VALUES (...), (...), ...` |
| 事务控制 | 需 `pool.begin()` 获取连接 | `conn.transaction()` 原生支持 |
| 编译时检查 | SQL 语法在编译期验证 | 运行时验证 |

对于同步流水线中的批量写入场景，**运行时性能优先于编译时安全**，因此选用 rusqlite。

**消息批量插入的极限优化**（`rusqlite_upsert_messages_batch`，L431–L635）：
- `MAX_PARAMS = 999`：SQLite 单条 SQL 的参数上限。
- `PARAMS_PER_MSG = 13`：messages 表每条记录需 13 个参数（msg_id, topic_id, role, name, agent_id, content, timestamp, is_group_message, group_id, finish_reason, content_hash, created_at, updated_at）。v0.9.14 移除 `is_thinking`。
- 计算 `chunk_size = 999 / 13 = 76`，即单条 SQL 最多插入 76 条消息。
- 使用 `String` 拼接动态 SQL，构造 `VALUES (?,?...), (?,?...), ...` 形式。
- `prepare_cached` 缓存编译后语句，在同事务内的多个 chunk 间复用。
- 消息正文以明文 `TEXT` 直接绑定（v1.1.3 起由压缩 BLOB 迁移为 TEXT，以支持 FTS5 全文搜索）。

**附件批量处理**（L553–L632）：
- 对同一批消息，先执行 `Chunked Delete`：按 `msg_id` IN 列表分块删除旧关系，上限 999 个 ID。
- 再执行 `Chunked Relation Insert`：`message_attachments` 表单条记录 8 参数（topic_id, msg_id, hash, attachment_order, display_name, src, status, created_at），chunk_size = 124。
- 附件本体（`attachments` 表）通过 `rusqlite_upsert_attachment_core` 逐条 UPSERT，冲突键为 `hash`。

### 3.6 哈希冒泡与分层去重

批量事务提交数据后，必须**自底向上重新计算并更新聚合哈希指纹**，以供同步子系统快速 Diff。

**冒泡层次**（L176–L212）：

```text
事务内所有写入完成后
│
├─→ 1. Topic 层冒泡（affected_topics）
│   └─→ rusqlite_bubble_topic_hash：
│       ├─→ 读取该 topic 下所有现存消息的 content_hash，按 timestamp ASC, msg_id ASC 排序
│       ├─→ 调用 compute_merkle_root(hashes) 计算消息根哈希
│       ├─→ 读取 topic 元数据，按 owner_type 分别构造 AgentTopicSyncDTO / GroupTopicSyncDTO
│       ├─→ 调用 HashAggregator::compute_agent_topic_metadata_hash 或 compute_group_topic_metadata_hash 计算 config_hash
│       └─→ UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ?
│
├─→ 2. Owner 层冒泡（affected_owners）
│   └─→ 先批量去重校验存在性：
│       ├─→ Agent: SELECT agent_id FROM agents WHERE agent_id IN (...) AND deleted_at IS NULL
│       └─→ Group: SELECT group_id FROM groups WHERE group_id IN (...) AND deleted_at IS NULL
│       仅对确实存在的 Owner 执行冒泡：
│       ├─→ rusqlite_bubble_agent_hash：汇总该 Agent 下所有 topics 的 config_hash + content_hash，计算 Merkle Root → 更新 agents.content_hash
│       └─→ rusqlite_bubble_group_hash：同理更新 groups.content_hash
```

**为什么需要批量存在性校验？**
- 同步流可能包含已删除 Owner 的残余 Topic 数据（如 Topic 在远端被删除但消息仍下发）。
- 若直接对不存在的 Agent/Group 执行 `UPDATE`，虽然 SQL 层面无影响，但会增加无意义计算。
- 通过 `IN (...)` 批量查询一次性过滤有效 ID，将冒泡调用次数降至最少。
- 使用 `deleted_at IS NULL` 条件进一步排除已软删除的实体。

**哈希算法**：`crate::vcp_modules::sync_types::compute_merkle_root`，对有序哈希列表计算 Merkle Root（L647, L682, L701）。该算法确保子集的任何增删改都会改变根哈希，使同步 Diff 可以精确识别变更范围。

### 3.7 各实体 UPSERT 方法详解

#### Agent 写入（`rusqlite_upsert_agent`，L271–L310）

```rust
fn rusqlite_upsert_agent(
    tx: &rusqlite::Transaction,
    id: &str,
    dto: &AgentSyncDTO,
) -> rusqlite::Result<()>
```

- 计算 `config_hash = HashAggregator::compute_agent_config_hash(dto)`。
- `INSERT ... ON CONFLICT(agent_id) DO UPDATE SET ...`。
- 更新字段：name, system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output, config_hash, updated_at。
- 注意：不更新 `mobile_system_prompt` 和 `use_temperature`。这两个字段在 `agents` 表 Schema 中存在（默认值 `''` / `0`），但当前同步写入使用数据库默认值，不随 `AgentSyncDTO` 更新；`current_topic_id` 已不再是数据库字段，仅作为前端 `sessionStore` 运行时状态存在。

#### Group 写入（`rusqlite_upsert_group`，L312–L367）

- 计算 `config_hash = HashAggregator::compute_group_config_hash(dto)`。
- 先 `DELETE FROM group_members WHERE group_id = ?` 清理旧成员（L352）。
- 再按 `dto.members` 列表重新插入成员关系（L356–L364）。
- `member_tags` 为可选 JSON 对象，通过 `as_object()` 提取后按成员 ID 查找对应标签。

#### Avatar 写入（`rusqlite_upsert_avatar`，L369–L388）

- 计算 `avatar_hash = HashAggregator::compute_avatar_hash(bytes)`。
- 提取主色调：`extract_dominant_color_from_bytes(bytes)`（来自 `avatar_service.rs`）。
- 固定 MIME 类型为 `image/png`，所有头像统一转换为 PNG 后入库。
- BLOB 数据直接写入 `image_data` 字段。

#### Topic 写入（`rusqlite_upsert_agent_topic` / `rusqlite_upsert_group_topic`，L390–L429）

- AgentTopic：写入 `owner_type = 'agent'`，保留 `locked` 和 `unread` 字段。
- GroupTopic：写入 `owner_type = 'group'`，`locked` 固定为 1，`unread` 固定为 0（群组话题无未读概念）。

### 3.8 错误传播与统计

DbWriteQueue Worker 可以继续消费后续批次，但任何批次失败都会进入 `pending_errors`，并由下一次 `flush()` 汇总返回；继续运行不等于把失败当作成功：

```rust
match result {
    Ok(Ok(_)) => success_count += 1,
    Ok(Err(e)) => pending_errors.push(e.to_string()),
    Err(e) => pending_errors.push(e.to_string()),
}
```

| 错误类型 | 原因 | 处理策略 |
|---------|------|---------|
| rusqlite 执行错误 | SQL 语法错误、约束冲突、磁盘满 | 事务回滚、记录错误；下一次 Flush 返回 `Err`，同步阶段不得完成 |
| spawn_blocking 错误 | 线程池 panic、OS 级资源耗尽 | 记录 JoinError；下一次 Flush 返回 `Err` |

**统计输出**：Worker 停止时（通道关闭或应用退出）输出总成功数和错误数（L235–L238），便于诊断同步数据丢失问题。

---

## 4. 消息仓储（`message_repository.rs`）

`message_repository.rs`（~580 行）负责消息的**渲染编译**、**仓储操作**以及**全量维护任务**（预渲染重建）。内容压缩（`compress_all_contents`）已在 v0.9.14 移除。

### 4.1 MessageRenderCompiler

```rust
pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    pub fn compile(content: &str) -> Vec<ContentBlock>;
    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String>;
    pub fn deserialize(bytes: &[u8]) -> Result<Vec<ContentBlock>, String>;
}
```

`MessageRenderCompiler` 是前端消息渲染的**Rust 侧预编译器**，将原始 Markdown/HTML 混合文本转换为结构化 AST（`ContentBlock` 列表），并以二进制形式缓存到 `render_cache` 表。

| 方法 | 输入 | 输出 | 说明 |
|------|------|------|------|
| `compile` | `&str`（原始消息内容） | `Vec<ContentBlock>` | 调用 `content_parser::parse_content`，支持原生 HTML |
| `serialize` | `&[ContentBlock]` | `Vec<u8>`（zstd 压缩 JSON） | 先 `serde_json::to_vec`，再 `zstd::bulk::compress`（level=3） |
| `deserialize` | `&[u8]`（zstd 二进制） | `Vec<ContentBlock>` | 先 `zstd::bulk::decompress`（上限 16 MB），再 `serde_json::from_slice` |

**为什么需要预渲染缓存？**
- 移动端解析长文本的 Markdown/代码块/HTML 是 CPU 密集型操作。
- 首次渲染时由 Rust 编译为 AST 二进制并入库；后续加载直接从 `render_cache` 反序列化，前端无需重复解析。
- 尤其利好长对话回溯场景：切换话题时消息列表秒开。
- `render_cache` 作为独立表，允许全量重建而不影响 `messages` 表的主数据。

### 4.2 process_message_content

```rust
#[tauri::command]
pub async fn process_message_content(
    _app_handle: AppHandle,
    content: String,
) -> Result<Vec<ContentBlock>, String>
```

这是一个暴露给前端的 Tauri Command（L55–L64），用于**实时预解析**用户输入或接收到的消息内容。

- 前端在消息发送前或接收后可调用此命令，提前获得 AST 结构。
- 与 `render_cache` 的区别：此命令不操作数据库，仅为单次解析服务。
- 返回值 `Vec<ContentBlock>` 直接通过 Tauri IPC 的 JSON 序列化传递回前端。

### 4.3 MessageRepository

```rust
pub struct MessageRepository;

impl MessageRepository {
    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String>;
}
```

`MessageRepository::upsert_message` 是**非批量场景**下的消息写入标准接口（例如用户发送单条消息、流式补全结束落盘）。它直接在调用方提供的 `sqlx::Transaction` 上执行，与 `db_write_queue.rs` 的 rusqlite 批量模式形成互补。

**流程**（L407–L509）：

1. **计算指纹**（L414–L428）：
   - 提取附件 hash 列表（过滤空值）。
   - 调用 `HashAggregator::compute_message_fingerprint(&message.content, &attachment_hashes)` 生成 `content_hash`。
   - 该指纹用于同步 Diff 时快速判断消息内容是否变更。

2. **写入 messages 表**（L430–L466）：
   - `INSERT ... ON CONFLICT(topic_id, msg_id) DO UPDATE SET ...`。
   - `content` 字段通过 `ContentCompressor::compress(&message.content)?` 存储为 zstd 二进制。
   - `deleted_at = NULL`：若消息曾被软删除，本次 UPSERT 恢复。
   - `created_at` 和 `updated_at` 均使用 `message.timestamp`（保证同一条消息在不同场景下时间戳一致）。

3. **写入 render_cache 表**（L469–L482）：
   - `INSERT ... ON CONFLICT(topic_id, msg_id) DO UPDATE SET render_content = excluded.render_content, updated_at = excluded.updated_at`。
   - `render_content` 由调用方预先通过 `MessageRenderCompiler::serialize` 生成。
   - 独立表设计使得渲染缓存可以单独重建、清理或迁移，而不影响消息正文。

4. **处理附件**（L484–L501）：
   - 若消息含附件：调用 `upsert_attachments_for_message` 删除旧关系并插入新关系。
   - 若无附件：`DELETE FROM message_attachments WHERE topic_id = ? AND msg_id = ?`，确保旧关联被清理。
   - 这种"先删后插"策略保证附件列表的强一致性：即使消息编辑后附件数量减少，残留关系也会被清除。

5. **哈希冒泡**（L504–L506）：
   - 若 `!skip_bubble`：调用 `HashAggregator::bubble_from_topic(tx, topic_id).await?`。
   - 该操作在 `sqlx::Transaction` 内异步执行，与 `db_write_queue.rs` 中的 rusqlite 冒泡逻辑等价但接口不同。

### 4.3a 懒渲染缓存策略（Lazy Render Cache）

v0.9.14 对消息加载流程进行了重大重构，引入**懒渲染缓存策略**：

```
加载消息时
│
├─→ 查询 LEFT JOIN render_cache
│
├─→ render_content 命中 ?
│   ├─Yes 且 hash/schema 匹配─→ deserialize_render_async(rb) → blocks
│   │        跳过编译，直接返回
│   │
│   └─No──→ ContentCompressor::decompress(content)
│            有界 spawn_blocking 编译/序列化
│            serde_json::to_value(&compiled) → blocks
│            │
│            └─→ 异步 CAS 写回 render_cache
│                 （仅正文 content_hash 仍等于编译起点时提交）
```

- **受控命中**：只有正文 hash 和渲染 schema 同时匹配才反序列化；损坏缓存按 miss 处理，不返回空白正文。
- **有界编译**：解析、序列化、反序列化共用 1–4 permit 的 `spawn_blocking` 门禁，不占用 Tokio IO worker。
- **CAS 回写**：慢编译只能在消息仍保持起始 hash 时写回，不能覆盖更新后的正文缓存。

### 4.3b re_render_message 命令

```rust
#[tauri::command]
pub async fn re_render_message(
    app_handle: tauri::AppHandle,
    message_id: String,
    topic_id: String,
) -> Result<serde_json::Value, String>
```

**触发场景**：内容解析器升级后，单条消息的 `render_cache` 可能与新渲染逻辑不兼容。前端通过 MessageRenderer 的上下文菜单「重新渲染」调用此命令。

**流程**：
1. 从 `messages` 表读取 `content` 并解压。
2. `MessageRenderCompiler::compile` 重新编译。
3. `MessageRenderCompiler::serialize` 序列化。
4. UPSERT 到 `render_cache` 表。
5. 返回编译后的 `Vec<ContentBlock>` JSON，前端立即替换本地 `blocks`。

**`upsert_attachments_for_message` 实现细节**（L511–L583）：
- 先 `DELETE FROM message_attachments WHERE topic_id = ? AND msg_id = ?` 清理旧关系。
- 遍历附件列表，对每个附件：
  - 若 `hash` 存在则直接使用；否则对 `src` 计算 SHA-256 作为兜底 hash。
  - `image_frames`（视频帧或 PDF 图片列表）通过 `serde_json::to_string` 序列化为 JSON 字符串存入。
  - 对 `attachments` 表执行 UPSERT（冲突键 `hash`）。
  - 对 `message_attachments` 表执行 INSERT（关联关系无冲突处理，因为已预先删除）。

**与 `DbWriteQueue` 的差异**：

| 维度 | `MessageRepository::upsert_message` | `DbWriteQueue::rusqlite_upsert_messages_batch` |
|------|-------------------------------------|-----------------------------------------------|
| 数据库接口 | sqlx::Transaction（异步） | rusqlite::Transaction（同步） |
| 适用场景 | 单条消息实时写入 | 同步批量消息写入 |
| 调用方 | `chat_manager.rs`（发送消息） | `sync_service.rs`（同步流水线） |
| 批量优化 | 逐条执行 | Chunked 批量插入（71 条/批） |
| 附件处理 | 逐条 DELETE + INSERT | Chunked Delete + Chunked Insert |
| 事务来源 | 调用方提供（可跨多个操作共享） | Worker 内部新建（每批次独立） |

### 4.4 预渲染重建三段流水线

`message_repository.rs` 的预渲染重建采用 Reader → Processor → Writer 三段流水线。旧的内容压缩维护任务已移除，因此这里不是任意 SQL 的通用 writer。

```rust
// Stage 1: Reader
async fn stream_cached_message_contents(
    pool: &sqlx::SqlitePool,
    tx: mpsc::Sender<(String, String, String, String)>,
) -> Result<(), String>;

// Stage 3: Writer
fn run_render_cache_update_writer(
    db_path: &Path,
    rx: mpsc::Receiver<Vec<(String, String, String, Vec<u8>)>>,
    progress_event: &str,
    app_handle: AppHandle,
    total: usize,
) -> JoinHandle<Result<(), String>>;
```

**Reader（`stream_cached_message_contents`）**：
- 按 `rowid` 正序分页读取，每页 `FETCH_SIZE = 500`。
- 读取明文 `content` 与当时的 `content_hash`，传递 `(topic_id, msg_id, content, source_hash)`。
- 当发送端关闭（`tx.send` 失败）时优雅退出，不报错。
- 使用 `rowid` 而非 `OFFSET` 分页，避免大数据量下的偏移性能衰减。

**Writer（`run_render_cache_update_writer`）**：
- 在 `spawn_blocking` 中运行，使用 rusqlite 直连。
- 每收到一个 batch 开启一个事务，逐项调用 `update_render_cache_if_current`。
- 写入携带 Reader 捕获的 `source_hash`；仅当 messages 当前 hash 仍相等且未删除时更新缓存。
- 进度发射：每 32ms 或处理完成时，通过 `app_handle.emit` 向前端推送 `RebuildProgress`。
- 32ms 间隔对应约 30 FPS，确保进度条视觉流畅且不频繁触发 IPC。

**Processor**：
- 由 `rebuild_all_pre_renders` 创建多个 `spawn_blocking` 并行 Worker。
- 并发度：`std::thread::available_parallelism().clamp(2, 12)`，根据设备核心数自适应，最低 2 线程保证基础并行，最高 12 线程防止线程爆炸。

### 4.5 全量预渲染重建

```rust
#[tauri::command]
pub async fn rebuild_all_pre_renders(app_handle: AppHandle) -> Result<(), String>
```

**触发场景**：内容解析器升级后，旧消息的 `render_cache` 可能与新前端渲染逻辑不兼容，需要全量重建。

**三段流水线执行**（L181–L293）：

```text
Stage 1: Reader (tokio::spawn 异步)
    └─→ stream_cached_message_contents
        └─→ 按 rowid 分页读取已有 render_cache 的 messages.content + content_hash
        └─→ mpsc::send 到 Compiler Stage

Stage 2: Parallel Compiler Workers (spawn_blocking × N)
    └─→ 每个 Worker 持有 rx_compiler (Arc<Mutex<mpsc::Receiver>>)
    └─→ MessageRenderCompiler::compile(content) → AST
    └─→ MessageRenderCompiler::serialize(&blocks) → zstd 二进制
    └─→ 每满 50 条 batch → mpsc::send 到 Writer Stage
    └─→ 通道关闭时发送残余 batch

Stage 3: Writer (spawn_blocking)
    └─→ run_render_cache_update_writer
        └─→ 每 batch 一个 rusqlite 事务
        └─→ 仅当 messages.content_hash == source_hash 且未删除时 UPDATE
        └─→ 发射事件 "render_rebuild_progress"
```

Reader 将读取时的 `content_hash` 随编译结果传到 Writer。Writer 通过 `update_render_cache_if_current` 做 hash-CAS；若正文在编译期间被编辑或删除，该旧结果更新 0 行，不能覆盖新缓存。普通懒渲染与全量重建因此遵守同一缓存提交条件。

**优雅停机机制**：
- Reader 完成后 `drop(tx_compiler)`，Compiler Workers 的 `blocking_recv()` 收到 `None` 后发送残余 batch 并退出。
- 所有 Compiler Workers 完成后 `drop(tx_writer)`，Writer 的 `blocking_recv()` 收到 `None` 后退出。
- 使用 `futures_util::future::join_all` 等待所有 Compiler Worker 结束，确保无数据在传输中途丢失。

**进度补偿**：Writer 结束后，显式发射一次 `current == total` 的进度事件（L285–L291），确保前端进度条达到 100%。

### 4.6 性能特征

| 操作 | 主导开销 | 大致耗时（典型数据） |
|------|---------|---------------------|
| 单条消息写入 (`upsert_message`) | zstd 压缩 + 2 次 UPSERT + 附件处理 | 5–20 ms |
| 批量消息写入 (71 条/批) | 动态 SQL 拼接 + 事务提交 | 10–50 ms/批 |
| 预渲染重建 (三段流水线) | AST 编译（CPU 密集型） | 1000 条消息约 1–3 秒 |
| 消息查询（带缓存，命中） | render_cache 反序列化 | 单条 <1 ms |
| 消息查询（带缓存，未命中） | content 解压 + compile + 异步回写 | 单条 5–20 ms |
| ~~内容压缩~~ | ~~已在 v0.9.14 移除~~ | — |

---

## 5. 模块依赖关系

### 5.1 persistence/ 内部协作

```text
persistence/
├── mod.rs
│   └─→ 导出 db_manager, db_write_queue, message_repository
│
├── db_manager.rs
│   ├─→ 被 db_write_queue.rs 引用：db_path 用于 rusqlite::Connection::open
│   └─→ 被 message_repository.rs 引用：DbState.pool 用于普通查询
│
├── db_write_queue.rs
│   ├─→ 引用 message_repository.rs：ContentCompressor::compress（消息内容压缩）
│   ├─→ 引用 sync_dto.rs：AgentSyncDTO, GroupSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO
│   ├─→ 引用 sync_hash.rs：HashAggregator（配置/元数据哈希计算）
│   ├─→ 引用 sync_types.rs：compute_merkle_root（Merkle Root 计算）
│   ├─→ 引用 sync_logger.rs：SyncLogger（可选审计日志）
│   ├─→ 引用 avatar_service.rs：extract_dominant_color_from_bytes（头像主色调）
│   └─→ 引用 chat_manager.rs：ChatMessage, Attachment（消息与附件类型）
│
└── message_repository.rs
    ├─→ 引用 chat_manager.rs：ChatMessage
    ├─→ 引用 content_parser.rs：parse_content, ContentBlock（渲染编译）
    ├─→ 引用 sync_hash.rs：HashAggregator（消息指纹与冒泡）
    └─→ 被 db_write_queue.rs 引用：ContentCompressor::compress
```

### 5.2 跨领域依赖

| 依赖领域 | 具体模块 | 依赖方向 | 说明 |
|---------|---------|---------|------|
| **sync/** | `sync_service.rs`, `sync_pipeline/` | `sync/` → `db_write_queue.rs` | 同步流水线将解析后的 DTO 批量提交给写入队列 |
| **chat/** | `chat_manager.rs` | 双向 | `chat_manager` 定义 `ChatMessage` / `Attachment` 类型，被 persistence/ 消费；`chat_manager` 调用 `MessageRepository::upsert_message` 实时落盘 |
| **sync/** | `sync_hash.rs`, `sync_types.rs` | `db_write_queue.rs` / `message_repository.rs` → `sync/` | 哈希计算与 Merkle Root 工具由同步子系统提供 |
| **parser/** | `content_parser.rs` | `message_repository.rs` → `content_parser` | 渲染编译依赖内容解析器 |

**依赖原则**：persistence/ 作为底层领域，**不主动调用**上层业务逻辑。所有上层写入均通过 `DbWriteQueue` 或 `MessageRepository` 的显式 API 进入 persistence/。

### 5.3 数据一致性边界

| 一致性级别 | 保证机制 | 说明 |
|-----------|---------|------|
| **单机原子性** | SQLite 事务 | 单条 `upsert_message` 或单批 `DbWriteTask` 批次均为原子操作 |
| **跨表一致性** | 外键 + 事务 | `render_cache` 和 `message_attachments` 均声明外键，`ON DELETE CASCADE` 保证消息删除时关联数据自动清理 |
| **读写一致性** | WAL 模式快照读 | 读操作不会看到未提交事务的中间状态 |
| **跨层一致性** | Flush 屏障 | `DbWriteQueue::flush` 确保调用方收到确认时，此前所有写入已落盘 |
| **最终一致性（同步）** | 哈希冒泡 | 同步写入后通过 Merkle Root 冒泡，确保聚合哈希最终收敛到正确值 |

### 5.4 同步流水线协作时序

```text
Sync Pipeline (sync_service.rs / sync_executor.rs)
    │
    ├─→ 接收远程同步数据
    │   └─→ 解析为 AgentSyncDTO / GroupSyncDTO / TopicSyncDTO / ChatMessage
    │
    ├─→ 批量提交 DbWriteTask::AgentTopicBatch { ... }
    ├─→ 批量提交 DbWriteTask::TopicMessages { ... }
    │
    ├─→ DbWriteQueue.flush().await
    │   └─→ 确保所有 TopicMessages 已落盘
    │
    └─→ 向前端发射 "sync_completed" 事件
        └─→ Vue 3 收到后重新加载话题/消息列表
```

在这个时序中，`flush()` 是同步 pipeline 与 persistence/ 之间的**契约点**：没有 flush 的确认，sync service 不会宣告同步完成。

Agent/Group 同步写绕过业务 Facade，因此 session 成功或失败退出前都会调用 `invalidate_sync_entity_caches()`。配置缓存带 generation 与短提交锁：异步 read/write/create 在数据库工作前捕获代次，只有代次未变化才允许填充；同步失效在同一短锁内 clear 并推进 generation，旧快照最多造成一次 cache miss，不会在 clear 后复活。

---

## 6. 术语速查表

| 术语 | 定义 | 出现位置 |
|------|------|---------|
| **WAL 模式** | Write-Ahead Logging，SQLite 的日志模式，允许读并发 | `db_manager.rs` L44 |
| **DbState** | 数据库状态单例，包含 sqlx 连接池与数据库路径 | `db_manager.rs` L6 |
| **DbWriteQueue** | 单工作线程批量写入队列，消除并发锁竞争 | `db_write_queue.rs` L54 |
| **DbWriteTask** | 写入任务枚举，涵盖 Agent/Group/Avatar/Topic/Messages/Flush | `db_write_queue.rs` L14 |
| **Flush** | 穿透式屏障信号，确保此前所有写入已落盘 | `db_write_queue.rs` L49 |
| **Turbo rusqlite 模式** | 绕过 sqlx，直接用 rusqlite 执行同步批量事务 | `db_write_queue.rs` L78 |
| **批量事务合并** | 将 10ms 窗口内最多 32 个任务/500 条消息合并为 SQLite 事务 | `db_write_queue.rs` |
| **Chunked Insert** | 受 SQLite 参数上限约束的分块批量插入 | `db_write_queue.rs` L445 |
| **哈希冒泡** | 自底向上重新计算并更新 content_hash / config_hash | `db_write_queue.rs` L637 |
| **Merkle Root** | 对有序哈希列表计算出的聚合根哈希 | `sync_types.rs` |
| **MessageRenderCompiler** | 消息渲染编译器，将文本转为 AST 二进制缓存 | `message_repository.rs` L10 |
| **ContentCompressor** | zstd 文本压缩/解压器，用于 messages.content 存储与读取 | `message_service.rs` |
| **render_cache** | 独立表，存储预编译的 AST zstd 二进制；v0.9.14 起条件写入（仅非空时插入） | `db_manager.rs` L242 |
| **三段流水线** | Reader → Processor → Writer 的通用全量维护架构；仅保留预渲染重建 | `message_repository.rs` L74 |
| **复合主键** | messages 表主键为 `(topic_id, msg_id)`，支持按话题分片 | `db_manager.rs` L219 |
| **内容寻址** | attachments 表以 SHA-256 hash 为主键，物理文件同名存储 | `db_manager.rs` L401 |
| **增量迁移** | 通过检测列/主键存在性，对存量数据库执行渐进式升级 | `db_manager.rs` L188 |
| **懒渲染缓存** | render_cache 命中直接反序列化 blocks；未命中编译后异步回写 | `message_service.rs` |
| **re_render_message** | 手动强制重新编译单条消息并更新 render_cache 的 Tauri 命令 | `message_service.rs` |
| **快照读** | WAL 模式下读操作基于事务开始时的数据库快照 | `db_manager.rs` L44 |

---

*最后更新：2026-08-11 | VCP Mobile v1.1.3*

> **关键设计决策备忘**
>
> 1. **双通道数据库访问**：查询走 sqlx 异步连接池，写入走 DbWriteQueue + rusqlite 同步直连。两者共享同一物理数据库文件，通过 WAL 模式协调并发。
> 2. **批量事务合并**：DbWriteQueue Worker 以 10ms 窗口 + 32 任务/500 消息上限合并事务，在吞吐与交互写延迟之间取平衡。
> 3. **render_cache 独立表**：将预渲染 AST 二进制从 messages 表剥离，避免消息表膨胀，同时使全量重建可独立进行而不影响消息正文。
> 4. **zstd 压缩全链路**：messages.content 和 render_cache.render_content 均以 zstd level=3 压缩存储，纯文本压缩比通常 3–10 倍。
> 5. **哈希冒泡分层去重**：事务提交后先批量校验 Owner 存在性，再执行 Topic → Owner 的两层冒泡，避免对幽灵数据做无意义计算。
> 6. **懒渲染缓存闭环**：加载时 render_cache 命中即走；未命中实时编译并异步回写，确保首次访问后的后续加载均为 O(1) 反序列化。
> 7. **渐进式 Schema 迁移**：通过检测 `pragma_table_info` 和 `ALTER TABLE ... ADD COLUMN` 的幂等执行，实现无版本号数据库升级。
