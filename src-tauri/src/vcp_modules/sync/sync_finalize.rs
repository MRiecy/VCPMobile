use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::SyncPipeline;
use crate::vcp_modules::sync_service::emit_sync_log;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

pub struct SyncFinalizer;

struct TopicBubbleMeta {
    title: String,
    created_at: i64,
    locked: bool,
    unread: bool,
}

#[derive(Debug)]
struct FinalizationStats {
    bubbled_topics: usize,
    affected_agents: usize,
    affected_groups: usize,
}

const SQLITE_TOPIC_CHUNK: usize = 300;

async fn finalize_modified_topics(
    pool: &sqlx::SqlitePool,
    modified_topics: &HashSet<TopicKey>,
) -> Result<FinalizationStats, String> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("开启同步收尾事务失败: {error}"))?;
    let mut meta_map = std::collections::HashMap::new();
    let topic_keys = modified_topics.iter().collect::<Vec<_>>();
    for topic_chunk in topic_keys.chunks(SQLITE_TOPIC_CHUNK) {
        let placeholders = topic_chunk
            .iter()
            .map(|_| "(?, ?, ?)")
            .collect::<Vec<_>>()
            .join(",");
        let query_sql = format!(
            "SELECT topic_id, owner_id, owner_type, title, created_at, locked, unread
             FROM topics WHERE deleted_at IS NULL
               AND (owner_type, owner_id, topic_id) IN ({placeholders})"
        );
        let mut query = sqlx::query(&query_sql);
        for key in topic_chunk {
            query = query
                .bind(&key.owner_type)
                .bind(&key.owner_id)
                .bind(&key.topic_id);
        }
        let rows = query
            .fetch_all(&mut *tx)
            .await
            .map_err(|error| format!("读取同步收尾话题元数据失败: {error}"))?;
        for row in rows {
            let topic_id: String = row
                .try_get("topic_id")
                .map_err(|error| format!("解码同步收尾 topic_id 失败: {error}"))?;
            let owner_id: String = row
                .try_get("owner_id")
                .map_err(|error| format!("解码同步收尾 owner_id 失败: {error}"))?;
            let owner_type: String = row
                .try_get("owner_type")
                .map_err(|error| format!("解码同步收尾 owner_type 失败: {error}"))?;
            let key = TopicKey::new(owner_type, owner_id, &topic_id);
            if meta_map
                .insert(
                    key,
                    TopicBubbleMeta {
                        title: row
                            .try_get("title")
                            .map_err(|error| format!("解码同步收尾 title 失败: {error}"))?,
                        created_at: row
                            .try_get("created_at")
                            .map_err(|error| format!("解码同步收尾 created_at 失败: {error}"))?,
                        locked: row
                            .try_get::<i64, _>("locked")
                            .map_err(|error| format!("解码同步收尾 locked 失败: {error}"))?
                            != 0,
                        unread: row
                            .try_get::<i64, _>("unread")
                            .map_err(|error| format!("解码同步收尾 unread 失败: {error}"))?
                            != 0,
                    },
                )
                .is_some()
            {
                return Err(format!("同步收尾话题元数据重复: {topic_id}"));
            }
        }
    }

    let actual_topics = meta_map.keys().cloned().collect::<HashSet<_>>();
    if actual_topics != *modified_topics {
        let mut missing = modified_topics
            .difference(&actual_topics)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        return Err(format!("同步收尾缺少 live 话题元数据: {missing:?}"));
    }

    for (key, _meta) in &meta_map {
        let result = sqlx::query(
            "UPDATE topics SET msg_count = (
                SELECT COUNT(*) FROM messages
                WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             ) WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| format!("更新同步收尾消息计数失败: {error}"))?;
        if result.rows_affected() != 1 {
            return Err(format!("同步收尾消息计数未更新话题 {}", key.topic_id));
        }
    }

    let mut affected_agents = HashSet::new();
    let mut affected_groups = HashSet::new();
    let mut bubbled_topics = 0usize;
    for (key, meta) in &meta_map {
        HashAggregator::bubble_topic_hash_with_meta(
            &mut tx,
            &key,
            &meta.title,
            meta.created_at,
            meta.locked,
            meta.unread,
        )
        .await
        .map_err(|error| format!("冒泡同步话题哈希失败 ({}): {error}", key.topic_id))?;
        bubbled_topics += 1;
        match key.owner_type.as_str() {
            "agent" => {
                affected_agents.insert(key.owner_id.clone());
            }
            "group" => {
                affected_groups.insert(key.owner_id.clone());
            }
            other => {
                return Err(format!(
                    "同步话题 {} 的 owner_type 非法: {other}",
                    key.topic_id
                ))
            }
        }
    }

    for agent_id in &affected_agents {
        HashAggregator::bubble_agent_hash(&mut tx, agent_id)
            .await
            .map_err(|error| format!("冒泡同步 Agent 哈希失败 ({agent_id}): {error}"))?;
    }
    for group_id in &affected_groups {
        HashAggregator::bubble_group_hash(&mut tx, group_id)
            .await
            .map_err(|error| format!("冒泡同步 Group 哈希失败 ({group_id}): {error}"))?;
    }

    tx.commit()
        .await
        .map_err(|error| format!("提交同步收尾事务失败: {error}"))?;
    Ok(FinalizationStats {
        bubbled_topics,
        affected_agents: affected_agents.len(),
        affected_groups: affected_groups.len(),
    })
}

pub fn invalidate_sync_entity_caches(app_handle: &AppHandle) {
    if let Some(state) =
        app_handle.try_state::<crate::vcp_modules::agent_service::AgentConfigState>()
    {
        state.invalidate_cache();
    }
    if let Some(state) =
        app_handle.try_state::<crate::vcp_modules::group_service::GroupManagerState>()
    {
        state.invalidate_cache();
    }
}

impl SyncFinalizer {
    pub async fn execute(
        app_handle: &AppHandle,
        db: &DbState,
        write_queue: &DbWriteQueue,
        pipeline: &SyncPipeline,
        logger: &Arc<Mutex<SyncLogger>>,
        modified_topics: HashSet<TopicKey>,
    ) -> Result<(), String> {
        // 1. 强制落盘数据库写队列
        write_queue
            .flush()
            .await
            .map_err(|error| format!("同步写队列落盘失败: {error}"))?;

        // 2. 全局 Hash 冒泡
        if !modified_topics.is_empty() {
            let start_instant = std::time::Instant::now();
            log::info!(
                "[SyncFinalizer] Finalizing {} modified topics (recalculating hashes)...",
                modified_topics.len()
            );
            emit_sync_log(
                app_handle,
                "info",
                &format!("正在校验 {} 个话题的一致性...", modified_topics.len()),
            );

            let stats = match finalize_modified_topics(&db.pool, &modified_topics).await {
                Ok(stats) => stats,
                Err(error) => {
                    if let Ok(mut sync_logger) = logger.lock() {
                        sync_logger.log(LogLevel::Error, "finalize", &error);
                    }
                    emit_sync_log(app_handle, "error", &error);
                    return Err(error);
                }
            };
            let elapsed = start_instant.elapsed();
            let success_msg = format!(
                "[SyncFinalizer] 一致性校验成功！耗时: {:?}. 冒泡话题: {}, 级联智能体: {}, 级联群组: {}.",
                elapsed,
                stats.bubbled_topics,
                stats.affected_agents,
                stats.affected_groups
            );
            log::info!("{}", success_msg);
            emit_sync_log(app_handle, "success", &success_msg);
        }

        // 同步写队列绕过业务 Facade；完成后统一失效配置缓存，避免继续命中同步前快照。
        invalidate_sync_entity_caches(app_handle);

        // 3. 推进 Pipeline 状态
        pipeline
            .on_messages_done()
            .await
            .map_err(|error| format!("推进同步收尾状态失败: {error}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::finalize_modified_topics;
    use std::collections::HashSet;

    #[tokio::test]
    async fn finalizer_updates_content_without_advancing_topic_config_time() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, title TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
                updated_at INTEGER, config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, timestamp INTEGER,
                content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('agent', 'owner-before', NULL);
             INSERT INTO topics VALUES
                ('topic', 'agent', 'agent', 'Topic', 1, 1, 0, 0, 77,
                 'config-before', 'content-before', NULL);
             INSERT INTO messages VALUES ('topic', 'message', 1, 'message-hash', NULL);",
        )
        .execute(&pool)
        .await
        .expect("create finalizer fixture");

        finalize_modified_topics(&pool, &HashSet::from(["topic".to_string()]))
            .await
            .expect("finalize topic");
        let state: (i64, i64, String, String) = sqlx::query_as(
            "SELECT t.updated_at, t.msg_count, t.content_hash, a.content_hash
             FROM topics t JOIN agents a ON a.agent_id = t.owner_id
             WHERE t.topic_id = 'topic'",
        )
        .fetch_one(&pool)
        .await
        .expect("read finalized state");
        assert_eq!(state.0, 77);
        assert_eq!(state.1, 1);
        assert_ne!(state.2, "content-before");
        assert_ne!(state.3, "owner-before");
    }

    #[tokio::test]
    async fn late_owner_hash_failure_rolls_back_all_finalizer_updates() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, title TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
                updated_at INTEGER, config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, timestamp INTEGER,
                content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('agent', 'owner-before', NULL);
             INSERT INTO topics VALUES
                ('topic', 'agent', 'agent', 'Topic', 1, 1, 0, 0, 1,
                 'config-before', 'content-before', NULL);
             INSERT INTO messages VALUES ('topic', 'message', 1, 'message-hash', NULL);
             CREATE TRIGGER fail_owner_hash
             BEFORE UPDATE OF content_hash ON agents
             BEGIN SELECT RAISE(ABORT, 'owner hash failure'); END;",
        )
        .execute(&pool)
        .await
        .expect("create finalizer fixture");

        let error = finalize_modified_topics(&pool, &HashSet::from(["topic".to_string()]))
            .await
            .expect_err("owner hash failure must fail finalization");
        assert!(error.contains("owner hash failure"));
        let row: (i64, String, String) = sqlx::query_as(
            "SELECT msg_count, config_hash, content_hash FROM topics WHERE topic_id = 'topic'",
        )
        .fetch_one(&pool)
        .await
        .expect("read rolled-back topic");
        assert_eq!(row.0, 0);
        assert_eq!(row.1, "config-before");
        assert_eq!(row.2, "content-before");
    }

    #[tokio::test]
    async fn metadata_query_failure_is_not_reported_as_success() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query("CREATE TABLE topics (topic_id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("create malformed topics table");

        let error = finalize_modified_topics(&pool, &HashSet::from(["topic".to_string()]))
            .await
            .expect_err("malformed metadata query must fail closed");
        assert!(error.contains("话题元数据"));
    }

    #[tokio::test]
    async fn missing_or_tombstoned_repair_topic_fails_before_updates() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, title TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
                updated_at INTEGER, config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, timestamp INTEGER,
                content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO topics VALUES
                ('live', 'agent', 'agent', 'Live', 1, 1, 0, 7, 1, '', '', NULL),
                ('deleted', 'agent', 'agent', 'Deleted', 1, 1, 0, 9, 1, '', '', 8);",
        )
        .execute(&pool)
        .await
        .expect("create finalizer fixture");

        for missing in ["missing", "deleted"] {
            let error = finalize_modified_topics(
                &pool,
                &HashSet::from(["live".to_string(), missing.to_string()]),
            )
            .await
            .expect_err("repair set must have exact live metadata coverage");
            assert!(error.contains(missing));
            let msg_count: i64 =
                sqlx::query_scalar("SELECT msg_count FROM topics WHERE topic_id = 'live'")
                    .fetch_one(&pool)
                    .await
                    .expect("read unchanged topic");
            assert_eq!(msg_count, 7);
        }
    }
}
