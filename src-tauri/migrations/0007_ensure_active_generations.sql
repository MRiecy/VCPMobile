-- Migration 0007: 修复 1.1.2 及更早血统的存量数据库缺失 active_generations 表的问题。
--
-- 背景：1.1.2 时代的建表代码（db_manager.setup_tables）只创建 14 张业务表，
-- 没有 active_generations，也没有 _sqlx_migrations 追踪表。1.1.3 引入 sqlx
-- migrator 时，bootstrap_legacy_if_needed 会将 v1 标记为"已应用"并跳过
-- 0001 整份文件，而 active_generations 只在 0001 中定义，后续迁移均未补建，
-- 导致该表对老库永远不存在。1.1.4 的 begin_stream_message 在发起 VCP 请求前
-- 会向该表 INSERT，老库因此报 "no such table: active_generations"，
-- 命令失败且前端静默吞错，表现为"发消息后 Agent 完全无反应"。
--
-- 对全新安装与 1.1.3+ 创建的库，本迁移为 no-op（IF NOT EXISTS）。
-- DDL 必须与 0001_create_initial_tables.sql 中的定义保持一致。
CREATE TABLE IF NOT EXISTS active_generations (
    msg_id TEXT PRIMARY KEY,
    topic_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL,
    created_at BIGINT NOT NULL
);
