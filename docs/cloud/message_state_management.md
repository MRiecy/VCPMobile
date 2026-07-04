# VCP Mobile 消息状态管理与云端恢复体检报告 (基于活跃生成注册表重构)

> **v1.1.3 更新**：当前代码已实现活跃生成注册表（`active_generations`）以及配套的 `get_active_generations` / `recover_active_generation` / `resume_stream` 接口。本文档结合历史分析背景与当前真实实现进行描述。

根据 v1.1.3 之前自查函数的实际运行输出：
* 历史数据中存在 **232 条 `assistant` 消息** 且 `finish_reason IS NULL`。
* 存在 **2420 条 `user` 消息** 且 `finish_reason IS NULL`。

**结论**: 由于历史版本的遗留问题，我们**绝不能**简单地使用 `finish_reason IS NULL` 作为冷启动时判定“未完成/异常中断消息”的依据，否则会导致大量历史已完成的正常消息被误判为“活跃生成中”，从而引发频繁的无效网络同步甚至接口报错。

为了彻底解决“历史数据污染”并保证极低的重构风险，v1.1.3 引入了 **“活跃生成注册表（Active Generation Registry）”** 架构设计。

---

## 1. 核心设计：活跃生成注册表 (`active_generations` 表)

我们不在已有的、数据量巨大的 `messages` 表上进行复杂的字段修改或数据清洗，而是引入一张专门的**轻量级事务状态表**：`active_generations`。

该表充当 AI 生成任务的 **“前向写日志（Write-Ahead Log / WAL）”**。它只记录当前**正在发生**的流式生成任务。一旦任务结束，该记录即被清除。

### 1.1 注册表 Schema 设计
建表语句位于 `src-tauri/migrations/0001_create_initial_tables.sql`（v1.1.3 起由 sqlx 迁移引擎管理）：
```sql
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,          -- 正在生成的助手消息 ID
    topic_id TEXT NOT NULL,           -- 话题 ID
    owner_id TEXT NOT NULL,           -- 智能体/群组 ID
    owner_type TEXT NOT NULL,         -- 'agent' | 'group'
    created_at BIGINT NOT NULL        -- 创建时间戳
);
```

### 1.2 为什么这是最佳方案？
1. **零历史 baggage 污染**: 历史遗留的 232 条 `assistant` 消息绝对不会出现在该表中，自检时实现**精确度 100% 的物理隔离**。
2. **免去表结构变更风险**: 无需对拥有数千条记录的 `messages` 主表执行 `ALTER TABLE` 或数据订正，规避了 SQLite 锁表或升级失败的风险。
3. **极低的 I/O 开销**: 只有在生成“开始”时插入一条记录，“结束”时删除一条记录，流式生成过程中**无任何写盘操作**。

---

## 2. 重构后的完整生命周期时序

```
[ 客户端开始生成 ]
       │
       ├─► 1. 插入 messages 骨架 (content: "")
       ├─► 2. 注册活跃状态: INSERT INTO active_generations (msg_id, ...)
       ▼
[ 流式生成中 ] (内存高频更新，物理零写盘)
       │
   ┌───┴────────────────────────┐
   ▼ (正常结束 / 用户 Abort)      ▼ (App 意外被杀 / 锁屏断网)
[ 3a. 收尾落盘与注销 ]          [ 3b. 状态遗留在注册表中 ]
   │                            - 内存丢失，连接断开
   ├─► 更新 messages             - active_generations 记录保留
   ├─► DELETE FROM active_generations
```

### 2.1 正常生成与收尾 (Normal Path)
1. **生成开始**:
   在 `triggerGeneration` 时，伴随向 `messages` 插入骨架消息，同步执行：
   `INSERT INTO active_generations (msg_id, topic_id, owner_id, owner_type, created_at) VALUES (?, ?, ?, ?, ?)`。
2. **流式传输**:
   数据在前端内存（Pinia）中高频追加，本地不写盘。
3. **生成结束**:
   在 `finalize_stream_message` 中，通过 SQLite **事务（Transaction）** 确保两步操作的原子性：
   * 更新 `messages` 表，写入完整文本，将 `finish_reason` 置为 `"completed"`。
   * **注销活跃任务**：`DELETE FROM active_generations WHERE msg_id = ?`。

### 2.2 冷启动自检与恢复 (Cold Start Recovery Path)
当用户强杀 App 重新打开时：
1. **极速自检**:
   启动时，消息引擎仅需查询注册表：
   `SELECT msg_id, topic_id, owner_id, owner_type FROM active_generations`
   *(由于正常结束的记录已被删除，此查询在 99.9% 的场景下返回空，耗时 < 1ms)*。
2. **精准对齐**:
   若查询到有遗留的 `msg_id`（说明上次运行时该生成被异常打断）：
   * 客户端携带该 `msg_id` 向服务端发起状态查询：`GET /api/chat/messages/{msg_id}`。
     > **待确认：** 服务端接口 `GET /api/chat/messages/{msg_id}` 与 `GET /api/chat/stream?msg_id={msg_id}` 定义于 `VCPToolBox` 工程，当前仓库无法直接校验其路径、响应字段与缓存策略，需人工对照服务端代码确认。
   * **若云端已生成完毕 (`completed`)**:
     服务端返回全量文本，客户端更新本地 `messages` 文本并置 `finish_reason = 'completed'`，随后从 `active_generations` 中 `DELETE` 该记录。
   * **若云端仍在生成中 (`streaming`)**:
     服务端返回当前已生成文本。客户端在内存中重建响应式状态，发起 SSE 重连（携带 `Last-Event-ID = 已生成文本长度`）继续接续流。
   * **若云端任务已失效 (`failed` / `not_found`)**:
     客户端将本地消息置为 `"error"`，并从 `active_generations` 中 `DELETE` 该记录。

---

## 3. 双端改造具体实现规格

### 3.1 移动端 SQLite 建表与注入
在 `db_manager.rs` 中：
1. `active_generations` 建表 SQL 位于 `src-tauri/migrations/0001_create_initial_tables.sql`，由 `run_migrations(&pool).await?` 在启动时统一应用，不再在 `db_manager.rs` 的硬编码 `setup_tables` 中维护。
2. 将启动自查函数修改为扫描 `active_generations` 表，而不是扫描 `messages` 表。

### 3.2 移动端消息写入与注销
在 `message_service.rs` 中：
1. **`append_single_message`**:
   如果是 `assistant` 角色且处于流式发起阶段，在同一个 DB 事务中向 `active_generations` 插入一条记录。
2. **`finalize_stream_message`**:
   在提交最终文本的事务中，追加 `DELETE FROM active_generations WHERE msg_id = ?`。

### 3.3 服务端 (VCPToolBox) 对齐接口
* **`GET /api/chat/messages/{msg_id}`**:
  读取 `message_cache` 表，返回 `{ status: "streaming"|"completed"|"failed", content: "..." }`。
* **`GET /api/chat/stream?msg_id={msg_id}`**:
  支持从 `Last-Event-ID` 偏移量处截取缓存文本进行补发，并桥接后台持续运行的生成任务。

---

*最后更新：2026-07-04 | VCP Mobile v1.1.3*
