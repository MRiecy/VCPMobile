use crate::vcp_modules::db_manager::{begin_immediate_write, DbState};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

async fn soft_delete_topic_data(
    db_state: &DbState,
    key: &TopicKey,
    now: i64,
) -> Result<Vec<MessageKey>, String> {
    Ok(
        crate::vcp_modules::topic_service::delete_topic_data(db_state, key, now)
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

        let active_ids = soft_delete_topic_data(&db, key, deleted_at).await?;

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
        let writes = tombstones
            .iter()
            .map(|tombstone| (tombstone.msg_id.clone(), tombstone.deleted_at))
            .collect::<Vec<_>>();
        let active_ids =
            crate::vcp_modules::message_service::apply_sync_message_tombstones(&db, key, &writes)
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
        let write_permit = db.write_gate.acquire("sync.delete.avatar").await?;
        let mut tx = begin_immediate_write(&db.pool)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO avatars (
                owner_type, owner_id, avatar_hash, mime_type, image_data,
                dominant_color, updated_at, deleted_at
             ) VALUES (?, ?, ?, 'application/octet-stream', ?, NULL, ?, ?)
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET
                avatar_hash = excluded.avatar_hash,
                mime_type = excluded.mime_type,
                image_data = excluded.image_data,
                dominant_color = NULL,
                updated_at = MAX(avatars.updated_at, excluded.updated_at),
                deleted_at = CASE
                    WHEN avatars.deleted_at IS NULL
                      OR avatars.deleted_at < excluded.deleted_at
                    THEN excluded.deleted_at
                    ELSE avatars.deleted_at
                END",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(SYNC_TOMBSTONE_HASH)
        .bind(Vec::<u8>::new())
        .bind(deleted_at)
        .bind(deleted_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(write_permit);

        Ok(())
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;
        let (write_permit, mut tx) = db.begin_write("maintenance.tombstone-cleanup").await?;

        // 1. 物理强清除已删除超过安全期（30天）的消息的预渲染缓存
        let render_cache = sqlx::query(
            "DELETE FROM render_cache
                WHERE (owner_type, owner_id, topic_id, msg_id) IN (
                    SELECT owner_type, owner_id, topic_id, msg_id FROM messages
                    WHERE deleted_at IS NOT NULL AND deleted_at < ?
                )",
        )
        .bind(threshold)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        // 2. 仅清空已删除超过安全期（30天）的消息的正文内容，保留消息的主键、角色与墓碑时间戳（防止多端同步幽灵复活，并释放大文本空间）
        let messages =
            sqlx::query("UPDATE messages SET content = '[已清空]' WHERE deleted_at IS NOT NULL AND deleted_at < ? AND content != '[已清空]'")
                .bind(threshold)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        drop(write_permit);

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
    use super::{soft_delete_topic_data, DeleteExecutor};
    use crate::vcp_modules::agent_service::AgentConfigState;
    use crate::vcp_modules::db_manager::DbState;
    use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
    use crate::vcp_modules::sync_dto::AgentSyncDTO;
    use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
    use tauri::Manager;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent", topic_id)
    }

    async fn test_db() -> DbState {
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
                title TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
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
             INSERT INTO topics (
                owner_type, owner_id, topic_id, config_hash, content_hash, deleted_at
             ) VALUES ('agent', 'agent', 'topic', 'config', 'content', NULL);
             INSERT INTO messages VALUES ('agent', 'agent', 'topic', 'message', NULL);
             INSERT INTO active_generations VALUES ('agent', 'agent', 'topic', 'message');",
        )
        .execute(&pool)
        .await
        .expect("create delete fixture");
        DbState::new(pool, std::path::PathBuf::new())
    }

    #[tokio::test]
    async fn empty_mobile_applies_nine_pulls_two_missing_deletes_and_business_write() {
        let temp_dir = tempfile::tempdir().expect("create full sync database directory");
        let db_path = temp_dir.path().join("empty-mobile-full-sync.sqlite");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(2));
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .expect("open full sync SQLx pool");
        sqlx::raw_sql(include_str!("../../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .expect("create full sync baseline");

        let app = tauri::test::mock_app();
        let db_state = DbState::new(pool.clone(), db_path.clone());
        let gate = db_state.write_gate.clone();
        assert!(app.manage(db_state));
        assert!(app.manage(AgentConfigState::new()));
        let queue = DbWriteQueue::new(pool.clone(), db_path, gate, 9001);
        for index in 0..9 {
            queue
                .submit(DbWriteTask::Agent {
                    id: format!("live-{index}"),
                    dto: AgentSyncDTO {
                        name: format!("Live {index}"),
                        system_prompt: String::new(),
                        model: "model".to_string(),
                        temperature: 0.7,
                        context_token_limit: 4096,
                        max_output_tokens: 1024,
                        stream_output: true,
                    },
                })
                .await
                .expect("submit owner Pull");
        }

        let handle = app.handle().clone();
        let first_delete = DeleteExecutor::soft_delete_agent(&handle, "missing-delete-1", 100);
        let second_delete = DeleteExecutor::soft_delete_agent(&handle, "missing-delete-2", 101);
        let ordinary_write = async {
            let db = handle.state::<DbState>();
            let (write_permit, mut tx) = db.begin_write("test.business-writer").await?;
            sqlx::query(
                "INSERT INTO settings (key, value, updated_at)
                 VALUES ('concurrent-business-write', 'ok', 1)",
            )
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
            tx.commit().await.map_err(|error| error.to_string())?;
            drop(write_permit);
            Ok::<(), String>(())
        };
        let (first_result, second_result, business_result, flush_result) =
            tokio::join!(first_delete, second_delete, ordinary_write, queue.flush());
        first_result.expect("apply first missing PULL_DELETE");
        second_result.expect("apply second missing PULL_DELETE");
        business_result.expect("apply ordinary writer");
        flush_result.expect("flush nine owner Pull writes");

        let counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT COUNT(*) FROM agents WHERE deleted_at IS NULL),
                (SELECT COUNT(*) FROM agents WHERE deleted_at IS NOT NULL),
                (SELECT COUNT(*) FROM settings WHERE key = 'concurrent-business-write')",
        )
        .fetch_one(&pool)
        .await
        .expect("read converged full sync state");
        assert_eq!(counts, (9, 2, 1));
    }

    #[tokio::test]
    async fn topic_delete_rolls_back_every_step_when_owner_hash_fails() {
        let db = test_db().await;
        let pool = db.pool.clone();
        sqlx::query(
            "CREATE TRIGGER fail_owner_hash
             BEFORE UPDATE OF content_hash ON agents
             BEGIN SELECT RAISE(ABORT, 'owner hash failure'); END;",
        )
        .execute(&pool)
        .await
        .expect("install failure trigger");

        let error = soft_delete_topic_data(&db, &topic("topic"), 42)
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
        let db = test_db().await;
        let pool = db.pool.clone();
        let active_ids = soft_delete_topic_data(&db, &topic("topic"), 42)
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
        let db = test_db().await;
        let pool = db.pool.clone();
        let missing = soft_delete_topic_data(&db, &topic("missing"), 42)
            .await
            .expect("missing remote tombstone must be durable");
        assert!(missing.is_empty());
        let missing_deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM topics WHERE topic_id = 'missing'")
                .fetch_one(&pool)
                .await
                .expect("read missing topic tombstone");
        assert_eq!(missing_deleted_at, Some(42));

        soft_delete_topic_data(&db, &topic("topic"), 42)
            .await
            .expect("first delete");
        let repeated = soft_delete_topic_data(&db, &topic("topic"), 99)
            .await
            .expect("repeated delete");
        assert!(repeated.is_empty());
        let deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM topics WHERE topic_id = 'topic'")
                .fetch_one(&pool)
                .await
                .expect("read tombstone");
        assert_eq!(deleted_at, Some(99));
    }
}
