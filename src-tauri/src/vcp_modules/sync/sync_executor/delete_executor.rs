use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

async fn soft_delete_topic_data(
    pool: &sqlx::SqlitePool,
    topic_id: &str,
    now: i64,
) -> Result<Vec<MessageKey>, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let Some(parent_row) =
        sqlx::query("SELECT owner_id, owner_type, deleted_at FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let deleted_at: Option<i64> = parent_row
        .try_get("deleted_at")
        .map_err(|error| format!("Topic {topic_id} tombstone decode failed: {error}"))?;
    if deleted_at.is_some() {
        return Ok(Vec::new());
    }
    let owner_id: String = parent_row
        .try_get("owner_id")
        .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
    let owner_type: String = parent_row
        .try_get("owner_type")
        .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
    let key = TopicKey::new(&owner_type, &owner_id, topic_id);
    let active_ids: Vec<String> = sqlx::query_scalar(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(topic_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let deleted = sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if deleted.rows_affected() != 1 {
        return Err(format!(
            "Topic {topic_id} does not exist or is already deleted"
        ));
    }
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&owner_type)
    .bind(&owner_id)
    .bind(topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    match owner_type.as_str() {
        "agent" => HashAggregator::bubble_agent_hash(&mut tx, &owner_id).await?,
        "group" => HashAggregator::bubble_group_hash(&mut tx, &owner_id).await?,
        other => {
            return Err(format!(
                "topic {topic_id} has unsupported owner_type {other}"
            ));
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(active_ids
        .into_iter()
        .map(|msg_id| MessageKey::new(key.clone(), msg_id))
        .collect())
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
        let db = app.state::<DbState>();
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = ?)",
        )
        .bind(agent_id)
        .fetch_one(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        if !exists {
            return Ok(());
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
        let db = app.state::<DbState>();
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = ?)",
        )
        .bind(group_id)
        .fetch_one(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        if !exists {
            return Ok(());
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
        topic_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        if deleted_at < 0 {
            return Err("Topic delete requires a non-negative deletedAt".to_string());
        }
        let db = app.state::<DbState>();

        let active_ids = soft_delete_topic_data(&db.pool, topic_id, deleted_at).await?;

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

    pub async fn soft_delete_message<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
        message_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        if topic_id.is_empty() || message_id.is_empty() || deleted_at < 0 {
            return Err(
                "Message delete requires topicId, id, and non-negative deletedAt".to_string(),
            );
        }
        let db = app.state::<DbState>();
        let topic_owner: Option<(String, String)> = sqlx::query_as(
            "SELECT owner_type, owner_id FROM topics
             WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        let Some((owner_type, owner_id)) = topic_owner else {
            return Ok(());
        };
        let key = TopicKey::new(owner_type, owner_id, topic_id);
        let result = crate::vcp_modules::message_service::delete_messages(
            &db.pool,
            &key,
            vec![message_id.to_string()],
            Some(deleted_at),
        )
        .await?;
        if let Some(active_requests) =
            app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
        {
            for active_id in result.active_ids {
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
            None => return Ok(()),
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

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::query(
            "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER);
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, deleted_at INTEGER
             );
             CREATE TABLE active_generations (topic_id TEXT, msg_id TEXT);
             INSERT INTO agents VALUES ('agent', 'owner-before', NULL);
             INSERT INTO topics VALUES ('topic', 'agent', 'agent', 'config', 'content', NULL);
             INSERT INTO messages VALUES ('topic', 'message', NULL);
             INSERT INTO active_generations VALUES ('topic', 'message');",
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

        let error = soft_delete_topic_data(&pool, "topic", 42)
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
        let active_ids = soft_delete_topic_data(&pool, "topic", 42)
            .await
            .expect("atomic delete");
        assert_eq!(active_ids, vec!["message"]);
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
    async fn invalid_topic_owner_type_rolls_back_delete() {
        let pool = test_pool().await;
        sqlx::query("UPDATE topics SET owner_type = 'unknown' WHERE topic_id = 'topic'")
            .execute(&pool)
            .await
            .expect("corrupt owner type");

        let error = soft_delete_topic_data(&pool, "topic", 42)
            .await
            .expect_err("unknown owner type must not commit a partial tombstone");
        assert!(error.contains("unsupported owner_type"));
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
        assert!(topic_deleted_at.is_none());
        assert_eq!(active_count, 1);
    }

    #[tokio::test]
    async fn missing_and_repeated_topic_delete_are_idempotent() {
        let pool = test_pool().await;
        let missing = soft_delete_topic_data(&pool, "missing", 42)
            .await
            .expect("missing remote tombstone is a no-op");
        assert!(missing.is_empty());

        soft_delete_topic_data(&pool, "topic", 42)
            .await
            .expect("first delete");
        let repeated = soft_delete_topic_data(&pool, "topic", 99)
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
