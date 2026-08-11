use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

async fn soft_delete_topic_data(
    pool: &sqlx::SqlitePool,
    topic_id: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let parent_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let active_ids: Vec<String> =
        sqlx::query_scalar("SELECT msg_id FROM active_generations WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

    sqlx::query("UPDATE topics SET deleted_at = ? WHERE topic_id = ?")
        .bind(now)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id = ? AND deleted_at IS NULL")
        .bind(now)
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query("DELETE FROM active_generations WHERE topic_id = ?")
        .bind(topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(row) = parent_row {
        let owner_id: String = row.get("owner_id");
        let owner_type: String = row.get("owner_type");
        match owner_type.as_str() {
            "agent" => HashAggregator::bubble_agent_hash(&mut tx, &owner_id).await?,
            "group" => HashAggregator::bubble_group_hash(&mut tx, &owner_id).await?,
            other => {
                return Err(format!(
                    "topic {topic_id} has unsupported owner_type {other}"
                ));
            }
        }
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(active_ids)
}

impl DeleteExecutor {
    pub async fn soft_delete_agent<R: Runtime>(
        app: &AppHandle<R>,
        agent_id: &str,
    ) -> Result<(), String> {
        let state = app.state::<crate::vcp_modules::agent_service::AgentConfigState>();
        crate::vcp_modules::agent_service::delete_agent_internal(app, &state, agent_id).await
    }

    pub async fn soft_delete_group<R: Runtime>(
        app: &AppHandle<R>,
        group_id: &str,
    ) -> Result<(), String> {
        let state = app.state::<crate::vcp_modules::group_service::GroupManagerState>();
        crate::vcp_modules::group_service::delete_group_internal(app, &state, group_id).await
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();
        let active_ids = soft_delete_topic_data(&db.pool, topic_id, now).await?;

        // Cancellation is intentionally post-commit: late finalizers already require the
        // active row, so the durable tombstone remains the authority if cancellation races.
        if let Some(active_requests) =
            app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
        {
            for msg_id in active_ids {
                if let Err(error) = active_requests.cancel(&msg_id) {
                    log::warn!(
                        "[DeleteExecutor] Failed to cancel generation {} after topic delete: {}",
                        msg_id,
                        error
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
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE avatars SET deleted_at = ? WHERE owner_type = ? AND owner_id = ?")
            .bind(now)
            .bind(owner_type)
            .bind(owner_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;

        // 1. 物理强清除已删除超过安全期（30天）的消息的预渲染缓存
        let render_cache =
            sqlx::query("DELETE FROM render_cache WHERE (topic_id, msg_id) IN (SELECT topic_id, msg_id FROM messages WHERE deleted_at IS NOT NULL AND deleted_at < ?)")
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
            "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, content_hash TEXT);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, content_hash TEXT);
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, deleted_at INTEGER
             );
             CREATE TABLE active_generations (topic_id TEXT, msg_id TEXT);
             INSERT INTO agents VALUES ('agent', 'owner-before');
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
}
