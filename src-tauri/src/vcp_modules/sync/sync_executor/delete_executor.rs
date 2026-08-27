use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::{MessageDeleteDecision, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

async fn soft_delete_topic_data(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    now: i64,
) -> Result<Vec<MessageKey>, String> {
    Ok(
        crate::vcp_modules::topic_service::delete_topic_data(pool, key, now)
            .await?
            .map(|result| result.active_messages)
            .unwrap_or_default(),
    )
}

impl DeleteExecutor {
    pub async fn soft_delete_agent<R: Runtime>(
        app: &AppHandle<R>,
        agent_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        if deleted_at < 0 {
            return Err("Agent delete requires a non-negative deletedAt".to_string());
        }
        let state = app.state::<crate::vcp_modules::agent_service::AgentConfigState>();
        crate::vcp_modules::agent_service::delete_agent_internal(
            app,
            &state,
            agent_id,
            Some(deleted_at),
        )
        .await
        .map(|_| ())
    }

    pub async fn soft_delete_group<R: Runtime>(
        app: &AppHandle<R>,
        group_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        if deleted_at < 0 {
            return Err("Group delete requires a non-negative deletedAt".to_string());
        }
        let state = app.state::<crate::vcp_modules::group_service::GroupManagerState>();
        crate::vcp_modules::group_service::delete_group_internal(
            app,
            &state,
            group_id,
            Some(deleted_at),
        )
        .await
        .map(|_| ())
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        key: &TopicKey,
        deleted_at: i64,
    ) -> Result<(), String> {
        if deleted_at < 0 {
            return Err("Topic delete requires a non-negative deletedAt".to_string());
        }
        let db = app.state::<DbState>();

        let active_ids = soft_delete_topic_data(&db.pool, key, deleted_at).await?;
        sqlx::query(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, updated_at,
                config_hash, deleted_at
             ) VALUES (?, ?, ?, '', 0, ?, ?, ?)
             ON CONFLICT(owner_type, owner_id, topic_id) DO NOTHING",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(deleted_at)
        .bind(SYNC_TOMBSTONE_HASH)
        .bind(deleted_at)
        .execute(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

        // Cancellation is intentionally post-commit: late finalizers already require the
        // active row, so the durable tombstone remains the authority if cancellation races.
        if let Some(active_requests) =
            app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
        {
            for key in active_ids {
                if let Err(error) = active_requests.cancel(&key) {
                    log::warn!(
                        "[DeleteExecutor] Failed to cancel generation {} after topic delete: {}",
                        key.msg_id,
                        error
                    );
                }
            }
        }

        Ok(())
    }

    pub async fn soft_delete_messages<R: Runtime>(
        app: &AppHandle<R>,
        key: &TopicKey,
        tombstones: &[MessageDeleteDecision],
    ) -> Result<(), String> {
        if key.topic_id.is_empty()
            || key.owner_id.is_empty()
            || !matches!(key.owner_type.as_str(), "agent" | "group")
            || tombstones.is_empty()
            || tombstones
                .iter()
                .any(|tombstone| tombstone.msg_id.is_empty() || tombstone.deleted_at < 0)
        {
            return Err(
                "Message delete requires topic identity, ids, and non-negative deletedAt"
                    .to_string(),
            );
        }
        let db = app.state::<DbState>();
        let topic_is_live = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL)",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        if !topic_is_live {
            return Ok(());
        }
        let writes = tombstones
            .iter()
            .map(|tombstone| (tombstone.msg_id.clone(), tombstone.deleted_at))
            .collect::<Vec<_>>();
        let active_ids = crate::vcp_modules::message_service::apply_sync_message_tombstones(
            &db.pool, key, &writes,
        )
        .await?;
        if let Some(active_requests) =
            app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
        {
            for active_id in active_ids {
                if let Err(error) =
                    active_requests.cancel(&MessageKey::new(key.clone(), &active_id))
                {
                    log::warn!(
                        "[DeleteExecutor] Failed to cancel generation {active_id} after message delete: {error}"
                    );
                }
            }
        }
        Ok(())
    }

    pub async fn soft_delete_avatar<R: Runtime>(
        app: &AppHandle<R>,
        owner_type: &str,
        owner_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        if deleted_at < 0
            || !crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id)
        {
            return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
        }
        let db = app.state::<DbState>();

        let existing_deleted_at: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT deleted_at FROM avatars WHERE owner_type = ? AND owner_id = ?",
        )
        .bind(owner_type)
        .bind(owner_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        match existing_deleted_at {
            None => {
                sqlx::query(
                    "INSERT INTO avatars (
                        owner_type, owner_id, avatar_hash, mime_type, image_data,
                        updated_at, deleted_at
                     ) VALUES (?, ?, ?, 'application/octet-stream', ?, ?, ?)",
                )
                .bind(owner_type)
                .bind(owner_id)
                .bind(SYNC_TOMBSTONE_HASH)
                .bind(Vec::<u8>::new())
                .bind(deleted_at)
                .bind(deleted_at)
                .execute(&db.pool)
                .await
                .map_err(|e| e.to_string())?;
                return Ok(());
            }
            Some(Some(_)) => return Ok(()),
            Some(None) => {}
        }

        let result = sqlx::query(
            "UPDATE avatars SET deleted_at = ?
                 WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(deleted_at)
        .bind(owner_type)
        .bind(owner_id)
        .execute(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        if result.rows_affected() != 1 {
            return Err(format!(
                "Avatar {owner_type}/{owner_id} disappeared during delete"
            ));
        }

        Ok(())
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;

        // 1. 物理强清除已删除超过安全期（30天）的消息的预渲染缓存
        let render_cache = sqlx::query(
            "DELETE FROM render_cache
                WHERE (owner_type, owner_id, topic_id, msg_id) IN (
                    SELECT owner_type, owner_id, topic_id, msg_id FROM messages
                    WHERE deleted_at IS NOT NULL AND deleted_at < ?
                )",
        )
        .bind(threshold)
        .execute(&db.pool)
        .await
        .map_err(|e| e.to_string())?;

        // 2. 仅清空已删除超过安全期（30天）的消息的正文内容，保留消息的主键、角色与墓碑时间戳（防止多端同步幽灵复活，并释放大文本空间）
        let messages =
            sqlx::query("UPDATE messages SET content = '[已清空]' WHERE deleted_at IS NOT NULL AND deleted_at < ? AND content != '[已清空]'")
                .bind(threshold)
                .execute(&db.pool)
                .await
                .map_err(|e| e.to_string())?;

        log::info!(
            "[DeleteExecutor] Completed safety-period cleanup (older than {} days): cleared_messages_content={}, deleted_render_caches={}",
            days,
            messages.rows_affected(),
            render_cache.rows_affected()
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::soft_delete_topic_data;
    use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent", topic_id)
    }

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query(
            "CREATE TABLE agents (
                owner_type TEXT, agent_id TEXT, content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, agent_id)
             );
             CREATE TABLE groups (
                owner_type TEXT, group_id TEXT, content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, group_id)
             );
             CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT, deleted_at INTEGER
             );
             CREATE TABLE message_attachments (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             CREATE TABLE render_cache (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             CREATE TABLE active_generations (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );
             INSERT INTO agents VALUES ('agent', 'agent', 'owner-before', NULL);
             INSERT INTO topics VALUES ('agent', 'agent', 'topic', 'config', 'content', NULL);
             INSERT INTO messages VALUES ('agent', 'agent', 'topic', 'message', NULL);
             INSERT INTO active_generations VALUES ('agent', 'agent', 'topic', 'message');",
        )
        .execute(&pool)
        .await
        .expect("create delete fixture");
        pool
    }

    #[tokio::test]
    async fn topic_delete_rolls_back_every_step_when_owner_hash_fails() {
        let pool = test_pool().await;
        sqlx::query(
            "CREATE TRIGGER fail_owner_hash
             BEFORE UPDATE OF content_hash ON agents
             BEGIN SELECT RAISE(ABORT, 'owner hash failure'); END;",
        )
        .execute(&pool)
        .await
        .expect("install failure trigger");

        let error = soft_delete_topic_data(&pool, &topic("topic"), 42)
            .await
            .expect_err("late owner hash failure must abort delete");
        assert!(error.contains("owner hash failure"));
        let topic_deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM topics WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read topic");
        let message_deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM messages WHERE msg_id = 'message'")
                .fetch_one(&pool)
                .await
                .expect("read message");
        let active_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM active_generations WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read active generation");
        assert!(topic_deleted_at.is_none());
        assert!(message_deleted_at.is_none());
        assert_eq!(active_count, 1);
    }

    #[tokio::test]
    async fn topic_delete_commits_tombstones_before_returning_active_ids() {
        let pool = test_pool().await;
        let active_ids = soft_delete_topic_data(&pool, &topic("topic"), 42)
            .await
            .expect("atomic delete");
        assert_eq!(active_ids, vec![MessageKey::new(topic("topic"), "message")]);
        let topic_deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM topics WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read topic");
        let active_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM active_generations WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read active generation");
        assert_eq!(topic_deleted_at, Some(42));
        assert_eq!(active_count, 0);
    }

    #[tokio::test]
    async fn missing_and_repeated_topic_delete_are_idempotent() {
        let pool = test_pool().await;
        let missing = soft_delete_topic_data(&pool, &topic("missing"), 42)
            .await
            .expect("missing remote tombstone is a no-op");
        assert!(missing.is_empty());

        soft_delete_topic_data(&pool, &topic("topic"), 42)
            .await
            .expect("first delete");
        let repeated = soft_delete_topic_data(&pool, &topic("topic"), 99)
            .await
            .expect("repeated delete");
        assert!(repeated.is_empty());
        let deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM topics WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read tombstone");
        assert_eq!(deleted_at, Some(42));
    }
}
