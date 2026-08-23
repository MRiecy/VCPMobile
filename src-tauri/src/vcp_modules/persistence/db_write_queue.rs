use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_types::is_valid_avatar_owner;
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum DbWriteTask {
    Agent {
        id: String,
        dto: AgentSyncDTO,
    },
    Group {
        id: String,
        dto: GroupSyncDTO,
    },
    Avatar {
        owner_type: String,
        owner_id: String,
        bytes: Vec<u8>,
    },
    AgentTopicBatch {
        topics: Vec<(TopicKey, AgentTopicSyncDTO)>,
    },
    GroupTopicBatch {
        topics: Vec<(TopicKey, GroupTopicSyncDTO)>,
    },
    TopicMessages {
        key: TopicKey,
        messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
        contents: Vec<String>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
        skip_bubble: bool,
    },
    Flush {
        tx: oneshot::Sender<Result<(), String>>,
    },
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // Existing test module intentionally sits beside the task enum.
mod tests {
    use super::DbWriteQueue;
    use crate::vcp_modules::chat_manager::ChatMessage;
    use crate::vcp_modules::sync_dto::{
        AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
    };
    use crate::vcp_modules::sync_hash::HashAggregator;

    #[test]
    fn flush_error_summary_is_reported_once() {
        let mut errors = vec![
            "batch one failed".to_string(),
            "batch two failed".to_string(),
        ];
        let first = DbWriteQueue::take_pending_errors(&mut errors)
            .expect_err("flush must surface all prior batch failures");
        assert!(first.contains("batch one failed"));
        assert!(first.contains("batch two failed"));
        assert!(errors.is_empty());
        assert!(DbWriteQueue::take_pending_errors(&mut errors).is_ok());
    }

    fn assert_queued_owner_root_excludes_default(owner_type: &str) {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open owner root database");
        conn.execute_batch(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );",
        )
        .expect("create owner root schema");
        let tx = conn.transaction().expect("begin owner root transaction");
        if owner_type == "agent" {
            tx.execute("INSERT INTO agents VALUES ('owner', '', NULL)", [])
                .expect("insert agent");
        } else {
            tx.execute("INSERT INTO groups VALUES ('owner', '', NULL)", [])
                .expect("insert group");
        }
        tx.execute(
            "INSERT INTO topics VALUES ('default', 'owner', ?, 'default-config', 'default-content', NULL)",
            [owner_type],
        )
        .expect("insert default topic");

        let bubble = |tx: &rusqlite::Transaction<'_>| {
            if owner_type == "agent" {
                DbWriteQueue::rusqlite_bubble_agent_hash(tx, "owner")
            } else {
                DbWriteQueue::rusqlite_bubble_group_hash(tx, "owner")
            }
        };
        let read_root = |tx: &rusqlite::Transaction<'_>| {
            let sql = if owner_type == "agent" {
                "SELECT content_hash FROM agents WHERE agent_id = 'owner'"
            } else {
                "SELECT content_hash FROM groups WHERE group_id = 'owner'"
            };
            tx.query_row(sql, [], |row| row.get::<_, String>(0))
        };

        tx.execute(
            "INSERT INTO topics VALUES ('topic-a', 'owner', ?, 'config-a', 'content-a', NULL)",
            [owner_type],
        )
        .expect("insert ordinary topic");
        bubble(&tx).expect("bubble ordinary topic");
        let ordinary_root = read_root(&tx).expect("read ordinary root");
        assert_eq!(
            ordinary_root,
            crate::vcp_modules::sync_types::compute_merkle_root(vec![
                HashAggregator::compute_topic_leaf_hash("topic-a", "config-a", "content-a",),
            ])
        );

        tx.execute(
            "UPDATE topics SET content_hash = 'changed-default' WHERE topic_id = 'default'",
            [],
        )
        .expect("change default topic");
        bubble(&tx).expect("bubble changed default topic");
        assert_eq!(read_root(&tx).expect("read unchanged root"), ordinary_root);
    }

    #[test]
    fn queued_owner_root_hashes_exclude_default_topics() {
        assert_queued_owner_root_excludes_default("agent");
        assert_queued_owner_root_excludes_default("group");
    }

    #[test]
    fn sync_entity_upserts_never_rewrite_tombstones_or_children() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open test database");
        conn.execute_batch(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, name TEXT, system_prompt TEXT, model TEXT,
                temperature REAL, context_token_limit INTEGER, max_output_tokens INTEGER,
                stream_output INTEGER, config_hash TEXT, content_hash TEXT,
                updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, name TEXT, mode TEXT, group_prompt TEXT,
                invite_prompt TEXT, use_unified_model INTEGER, unified_model TEXT,
                tag_match_mode TEXT, created_at INTEGER, config_hash TEXT,
                content_hash TEXT, updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, member_tag TEXT, sort_order INTEGER,
                updated_at INTEGER
             );
             CREATE TABLE avatars (
                owner_type TEXT, owner_id TEXT, avatar_hash TEXT, mime_type TEXT,
                image_data BLOB, dominant_color TEXT, updated_at INTEGER,
                deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id)
             );
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, title TEXT, owner_id TEXT, owner_type TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, config_hash TEXT,
                updated_at INTEGER, deleted_at INTEGER
             );
             INSERT INTO agents VALUES
                ('agent-live', 'live', '', '', 0, 0, 0, 0, '', '', 1, NULL),
                ('agent-deleted', 'deleted', '', '', 0, 0, 0, 0, '', '', 1, 9);
             INSERT INTO groups VALUES
                ('group-deleted', 'deleted-group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, '', '', 1, 9),
                ('group-live', 'live-group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, '', '', 1, NULL);
             INSERT INTO group_members VALUES ('group-deleted', 'old-member', NULL, 0, 1);
             INSERT INTO avatars VALUES
                ('agent', 'agent-deleted', 'old-hash', 'image/png', x'09', NULL, 1, 9);
             INSERT INTO topics VALUES
                ('topic-deleted', 'deleted-topic', 'agent-live', 'agent', 1, 1, 0, '', 1, 9);",
        )
        .expect("create tombstone fixture");

        let tx = conn.transaction().expect("begin transaction");
        DbWriteQueue::rusqlite_upsert_agent(
            &tx,
            "agent-deleted",
            &AgentSyncDTO {
                name: "stale-agent".into(),
                system_prompt: "stale".into(),
                model: "stale".into(),
                temperature: 1.0,
                context_token_limit: 1,
                max_output_tokens: 1,
                stream_output: true,
            },
        )
        .expect_err("tombstoned agent upsert must fail closed");
        DbWriteQueue::rusqlite_upsert_group(
            &tx,
            "group-deleted",
            &GroupSyncDTO {
                name: "stale-group".into(),
                members: vec!["new-member".into()],
                mode: "round".into(),
                member_tags: None,
                group_prompt: None,
                invite_prompt: None,
                use_unified_model: false,
                unified_model: None,
                tag_match_mode: None,
                created_at: 2,
            },
        )
        .expect_err("tombstoned group upsert must fail closed");
        DbWriteQueue::rusqlite_upsert_avatar(&tx, "agent", "agent-deleted", &[1, 2, 3])
            .expect_err("tombstoned avatar upsert must fail closed");
        DbWriteQueue::rusqlite_upsert_avatar(&tx, "agent", "missing-agent", &[1])
            .expect_err("orphan agent avatar must fail closed");
        DbWriteQueue::rusqlite_upsert_avatar(&tx, "group", "agent-live", &[1])
            .expect_err("wrong-type avatar owner must fail closed");
        DbWriteQueue::rusqlite_upsert_avatar(&tx, "system", "system", &[1])
            .expect_err("unsupported avatar owner must fail closed");
        DbWriteQueue::rusqlite_upsert_avatar(&tx, "user", "user_avatar", &[1])
            .expect("fixed user avatar owner must remain supported");
        DbWriteQueue::rusqlite_upsert_agent_topic(
            &tx,
            "topic-deleted",
            &AgentTopicSyncDTO {
                id: "topic-deleted".into(),
                name: "stale-topic".into(),
                created_at: 2,
                locked: false,
                unread: true,
                owner_id: "agent-live".into(),
            },
        )
        .expect_err("tombstoned topic upsert must fail closed");
        DbWriteQueue::rusqlite_upsert_group_topic(
            &tx,
            "topic-under-deleted-owner",
            &GroupTopicSyncDTO {
                id: "topic-under-deleted-owner".into(),
                name: "stale-child".into(),
                created_at: 2,
                owner_id: "group-deleted".into(),
            },
        )
        .expect_err("topic with deleted owner must fail closed");

        assert_eq!(
            tx.query_row(
                "SELECT name FROM agents WHERE agent_id = 'agent-deleted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read agent"),
            "deleted"
        );
        assert_eq!(
            tx.query_row(
                "SELECT name FROM groups WHERE group_id = 'group-deleted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read group"),
            "deleted-group"
        );
        assert_eq!(
            tx.query_row(
                "SELECT agent_id FROM group_members WHERE group_id = 'group-deleted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read member"),
            "old-member"
        );
        assert_eq!(
            tx.query_row(
                "SELECT image_data FROM avatars WHERE owner_type = 'agent' AND owner_id = 'agent-deleted'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .expect("read avatar"),
            vec![9]
        );
        assert_eq!(
            tx.query_row(
                "SELECT title FROM topics WHERE topic_id = 'topic-deleted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read topic"),
            "deleted-topic"
        );
        let child_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM topics WHERE topic_id = 'topic-under-deleted-owner'",
                [],
                |row| row.get(0),
            )
            .expect("count child topics");
        assert_eq!(child_count, 0);
    }

    #[test]
    fn sync_message_batch_does_not_repopulate_tombstoned_side_tables() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open test database");
        conn.execute_batch(
            "CREATE TABLE topics (topic_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, content TEXT, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE render_cache (
                topic_id TEXT, msg_id TEXT, render_content BLOB,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE messages_fts (msg_id TEXT, topic_id TEXT, content TEXT);
             CREATE TABLE message_attachments (topic_id TEXT, msg_id TEXT, hash TEXT);
             INSERT INTO topics VALUES ('topic', NULL);
             INSERT INTO messages VALUES ('topic', 'message', '[deleted]', 9);
             INSERT INTO render_cache VALUES ('topic', 'message', x'09');
             INSERT INTO messages_fts VALUES ('message', 'topic', 'deleted-index');
             INSERT INTO message_attachments VALUES ('topic', 'message', 'deleted-hash');",
        )
        .expect("create message tombstone fixture");

        let tx = conn.transaction().expect("begin transaction");
        let stale = ChatMessage {
            id: "message".into(),
            role: "assistant".into(),
            content: "stale remote body".into(),
            timestamp: 10,
            ..Default::default()
        };
        DbWriteQueue::rusqlite_upsert_messages_batch(
            &tx,
            "topic",
            vec![stale],
            vec!["stale remote body".into()],
            vec![vec![1, 2, 3]],
            vec!["stale-hash".into()],
        )
        .expect("guarded message batch");

        let (content, deleted_at): (String, Option<i64>) = tx
            .query_row(
                "SELECT content, deleted_at FROM messages WHERE topic_id = 'topic' AND msg_id = 'message'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read tombstoned message");
        assert_eq!(content, "[deleted]");
        assert_eq!(deleted_at, Some(9));
        let render: Vec<u8> = tx
            .query_row("SELECT render_content FROM render_cache", [], |row| {
                row.get(0)
            })
            .expect("read render cache");
        assert_eq!(render, vec![9]);
        let fts: String = tx
            .query_row("SELECT content FROM messages_fts", [], |row| row.get(0))
            .expect("read fts");
        assert_eq!(fts, "deleted-index");
        let relation: String = tx
            .query_row("SELECT hash FROM message_attachments", [], |row| row.get(0))
            .expect("read relation");
        assert_eq!(relation, "deleted-hash");
    }

    #[test]
    fn sync_topic_and_message_writes_require_live_matching_parents() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open test database");
        conn.execute_batch(
            "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at INTEGER);
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, title TEXT, owner_id TEXT, owner_type TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, updated_at INTEGER,
                deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('agent-a', NULL), ('agent-b', NULL);
             INSERT INTO topics VALUES
                ('topic', 'Topic', 'agent-a', 'agent', 1, 1, 0, 1, NULL);",
        )
        .expect("create parent fixture");
        let tx = conn.transaction().expect("begin transaction");

        let owner_conflict = DbWriteQueue::rusqlite_upsert_agent_topic(
            &tx,
            "topic",
            &AgentTopicSyncDTO {
                id: "topic".into(),
                name: "Conflict".into(),
                created_at: 1,
                locked: true,
                unread: false,
                owner_id: "agent-b".into(),
            },
        )
        .expect_err("live topic owner must be immutable during sync pull");
        assert!(owner_conflict.to_string().contains("owner conflicts"));

        let missing_parent = DbWriteQueue::rusqlite_upsert_messages_batch(
            &tx,
            "missing-topic",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect_err("even an empty message batch requires its live topic");
        assert!(missing_parent.to_string().contains("parent topic"));
    }
}

pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
    logger: Option<Arc<Mutex<SyncLogger>>>,
    db_path: std::path::PathBuf,
    _worker: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for DbWriteQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            logger: self.logger.clone(),
            db_path: self.db_path.clone(),
            _worker: None,
        }
    }
}

impl DbWriteQueue {
    fn sync_contract_error(message: impl Into<String>) -> rusqlite::Error {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.into(),
        )))
    }

    pub fn new(_pool: sqlx::SqlitePool, db_path: std::path::PathBuf) -> Self {
        let (tx, mut rx) = mpsc::channel(256);
        let db_path_for_worker = db_path.clone();

        // 核心优化：利用 Mutex 持有持久连接，确保 spawn_blocking 之间 prepare_cached 缓存不失效
        let conn_holder: Arc<Mutex<Option<rusqlite::Connection>>> = Arc::new(Mutex::new(None));

        let worker = tokio::spawn(async move {
            log::info!("[DbWriteQueue] Worker started (Turbo rusqlite Mode)");

            let mut success_count = 0u32;
            let mut error_count = 0u32;
            let mut pending_errors: Vec<String> = Vec::new();

            while let Some(first_task) = rx.recv().await {
                // 如果第一个任务就是 Flush，直接确认
                if let DbWriteTask::Flush { tx } = first_task {
                    let _ = tx.send(Self::take_pending_errors(&mut pending_errors));
                    continue;
                }

                let mut tasks_in_this_tx = vec![first_task];
                let mut total_msg_count = 0u32;

                if let DbWriteTask::TopicMessages { messages, .. } = &tasks_in_this_tx[0] {
                    total_msg_count += messages.len() as u32;
                }

                let mut flush_tx_opt: Option<oneshot::Sender<Result<(), String>>> = None;

                while tasks_in_this_tx.len() < 32 && total_msg_count < 500 {
                    let next_res =
                        tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv()).await;

                    match next_res {
                        Ok(Some(DbWriteTask::Flush { tx })) => {
                            flush_tx_opt = Some(tx);
                            break;
                        }
                        Ok(Some(task)) => {
                            if let DbWriteTask::TopicMessages { messages, .. } = &task {
                                total_msg_count += messages.len() as u32;
                            }
                            tasks_in_this_tx.push(task);
                        }
                        _ => break,
                    }
                }

                let db_path = db_path_for_worker.clone();
                let ch = conn_holder.clone();

                // [Turbo Phase 3] 使用 spawn_blocking + rusqlite 进行极致写入
                let result = tokio::task::spawn_blocking(move || {
                    let mut guard = ch.lock().unwrap();
                    if guard.is_none() {
                        let conn = rusqlite::Connection::open(&db_path)?;
                        // 极致性能调优 (仅在初始化连接时执行一次)
                        conn.pragma_update(None, "journal_mode", "WAL")?;
                        conn.pragma_update(None, "synchronous", "NORMAL")?;
                        // SQLite 内建 busy handler 做有界退避；2 秒后把错误交给 flush，
                        // 不再用 30 秒长事务阻塞正常聊天写入。
                        conn.busy_timeout(std::time::Duration::from_millis(2000))?;
                        *guard = Some(conn);
                    }
                    let conn = guard.as_mut().unwrap();
                    let tx = conn.transaction()?;

                    let mut affected_owners = HashSet::new();
                    let mut affected_topics = HashSet::new();

                    for task in tasks_in_this_tx {
                        match task {
                            DbWriteTask::Agent { id, dto } => {
                                Self::rusqlite_upsert_agent(&tx, &id, &dto)?;
                                affected_owners.insert(OwnerKey::new("agent", id));
                            }
                            DbWriteTask::Group { id, dto } => {
                                Self::rusqlite_upsert_group(&tx, &id, &dto)?;
                                affected_owners.insert(OwnerKey::new("group", id));
                            }
                            DbWriteTask::Avatar { owner_type, owner_id, bytes } => {
                                Self::rusqlite_upsert_avatar(&tx, &owner_type, &owner_id, &bytes)?;
                            }
                            DbWriteTask::AgentTopicBatch { topics } => {
                                for (key, dto) in topics {
                                    affected_owners.insert(OwnerKey::new("agent", &key.owner_id));
                                    Self::rusqlite_upsert_agent_topic(&tx, &key, &dto)?;
                                }
                            }
                            DbWriteTask::GroupTopicBatch { topics } => {
                                for (key, dto) in topics {
                                    affected_owners.insert(OwnerKey::new("group", &key.owner_id));
                                    Self::rusqlite_upsert_group_topic(&tx, &key, &dto)?;
                                }
                            }
                            DbWriteTask::TopicMessages { key, messages, contents, render_bytes, content_hashes, skip_bubble } => {
                                if !skip_bubble {
                                    affected_topics.insert(key.clone());
                                }
                                Self::rusqlite_upsert_messages_batch(&tx, &key, messages, contents, render_bytes, content_hashes)?;
                            }
                            DbWriteTask::Flush { .. } => unreachable!(),
                        }
                    }

                    // [Phase 5] 统一冒泡：分层去重，批量校验存在，确保最小化开销
                    for key in affected_topics {
                        Self::rusqlite_bubble_topic_hash(&tx, &key)?;
                    }

                    // 批量提取 Owner 并去重校验
                    let mut unique_agents = HashSet::new();
                    let mut unique_groups = HashSet::new();
                    for owner in affected_owners {
                        if owner.owner_type == "agent" {
                            unique_agents.insert(owner.owner_id);
                        } else if owner.owner_type == "group" {
                            unique_groups.insert(owner.owner_id);
                        }
                    }

                    if !unique_agents.is_empty() {
                        let mut requested = unique_agents.into_iter().collect::<Vec<_>>();
                        requested.sort();
                        let mut valid_ids = HashSet::new();
                        for chunk in requested.chunks(400) {
                            let placeholders = vec!["?"; chunk.len()].join(",");
                            let sql = format!(
                                "SELECT agent_id FROM agents
                                 WHERE owner_type = 'agent' AND agent_id IN ({}) AND deleted_at IS NULL",
                                placeholders
                            );
                            let mut stmt = tx.prepare(&sql)?;
                            let decoded = stmt
                                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| row.get(0))?
                                .collect::<rusqlite::Result<Vec<String>>>()?;
                            valid_ids.extend(decoded);
                        }
                        let expected = requested.iter().cloned().collect::<HashSet<_>>();
                        if valid_ids != expected {
                            let mut missing = expected.difference(&valid_ids).cloned().collect::<Vec<_>>();
                            missing.sort();
                            return Err(Self::sync_contract_error(format!(
                                "Agent hash bubble is missing live owners {missing:?}"
                            )));
                        }
                        for aid in requested {
                            Self::rusqlite_bubble_agent_hash(&tx, &aid)?;
                        }
                    }

                    if !unique_groups.is_empty() {
                        let mut requested = unique_groups.into_iter().collect::<Vec<_>>();
                        requested.sort();
                        let mut valid_ids = HashSet::new();
                        for chunk in requested.chunks(400) {
                            let placeholders = vec!["?"; chunk.len()].join(",");
                            let sql = format!(
                                "SELECT group_id FROM groups
                                 WHERE owner_type = 'group' AND group_id IN ({}) AND deleted_at IS NULL",
                                placeholders
                            );
                            let mut stmt = tx.prepare(&sql)?;
                            let decoded = stmt
                                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| row.get(0))?
                                .collect::<rusqlite::Result<Vec<String>>>()?;
                            valid_ids.extend(decoded);
                        }
                        let expected = requested.iter().cloned().collect::<HashSet<_>>();
                        if valid_ids != expected {
                            let mut missing = expected.difference(&valid_ids).cloned().collect::<Vec<_>>();
                            missing.sort();
                            return Err(Self::sync_contract_error(format!(
                                "Group hash bubble is missing live owners {missing:?}"
                            )));
                        }
                        for gid in requested {
                            Self::rusqlite_bubble_group_hash(&tx, &gid)?;
                        }
                    }

                    tx.commit()?;
                    Ok::<(), rusqlite::Error>(())
                }).await;

                match result {
                    Ok(Ok(_)) => success_count += 1,
                    Ok(Err(e)) => {
                        error_count += 1;
                        log::error!("[DbWriteQueue] rusqlite execution error: {}", e);
                        pending_errors.push(format!("rusqlite execution error: {e}"));
                    }
                    Err(e) => {
                        error_count += 1;
                        log::error!("[DbWriteQueue] spawn_blocking error: {}", e);
                        pending_errors.push(format!("write worker join error: {e}"));
                    }
                }

                if let Some(tx) = flush_tx_opt {
                    let _ = tx.send(Self::take_pending_errors(&mut pending_errors));
                }
            }

            log::info!(
                "[DbWriteQueue] Worker stopped. Total: success={}, errors={}",
                success_count,
                error_count
            );
        });

        Self {
            sender: tx,
            logger: None,
            db_path,
            _worker: Some(worker),
        }
    }

    pub fn set_logger(&mut self, logger: Arc<Mutex<SyncLogger>>) {
        self.logger = Some(logger);
    }

    pub async fn submit(&self, task: DbWriteTask) -> Result<(), String> {
        self.sender
            .send(task)
            .await
            .map_err(|e| format!("DbWriteQueue submit failed: {e}"))
    }

    pub async fn flush(&self) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(DbWriteTask::Flush { tx })
            .await
            .map_err(|e| format!("DbWriteQueue flush submit failed: {e}"))?;
        rx.await
            .map_err(|e| format!("DbWriteQueue flush acknowledgement failed: {e}"))??;
        log::debug!("[DbWriteQueue] Flush completed");
        Ok(())
    }

    fn take_pending_errors(errors: &mut Vec<String>) -> Result<(), String> {
        if errors.is_empty() {
            Ok(())
        } else {
            Err(std::mem::take(errors).join(" | "))
        }
    }

    // --- rusqlite 事务级方法 ---

    fn rusqlite_upsert_agent(
        tx: &rusqlite::Transaction,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> rusqlite::Result<()> {
        if id.is_empty() {
            return Err(Self::sync_contract_error(
                "Agent upsert requires a non-empty id",
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_config_hash(dto);

        let changed = tx.execute(
            "INSERT INTO agents (
                owner_type, agent_id, name, system_prompt, model, temperature,
                context_token_limit, max_output_tokens, 
                stream_output, config_hash, updated_at
            ) VALUES ('agent', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, agent_id) DO UPDATE SET
                name = excluded.name, 
                system_prompt = excluded.system_prompt, 
                model = excluded.model, 
                temperature = excluded.temperature, 
                context_token_limit = excluded.context_token_limit, 
                max_output_tokens = excluded.max_output_tokens, 
                stream_output = excluded.stream_output, 
                updated_at = CASE
                    WHEN agents.config_hash IS NOT excluded.config_hash THEN excluded.updated_at
                    ELSE agents.updated_at
                END,
                config_hash = excluded.config_hash
             WHERE agents.deleted_at IS NULL",
            rusqlite::params![
                id,
                &dto.name,
                &dto.system_prompt,
                &dto.model,
                dto.temperature,
                dto.context_token_limit,
                dto.max_output_tokens,
                if dto.stream_output { 1 } else { 0 },
                &config_hash,
                now
            ],
        )?;

        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Agent {id} is tombstoned"
            )));
        }

        Ok(())
    }

    fn rusqlite_upsert_group(
        tx: &rusqlite::Transaction,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> rusqlite::Result<()> {
        if id.is_empty() {
            return Err(Self::sync_contract_error(
                "Group upsert requires a non-empty id",
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_config_hash(dto);

        let changed = tx.execute(
            "INSERT INTO groups (
                owner_type, group_id, name, mode,
                group_prompt, invite_prompt, use_unified_model, unified_model,
                tag_match_mode, created_at, config_hash, updated_at
            ) VALUES ('group', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, group_id) DO UPDATE SET
                name = excluded.name,
                mode = excluded.mode,
                group_prompt = excluded.group_prompt,
                invite_prompt = excluded.invite_prompt,
                use_unified_model = excluded.use_unified_model,
                unified_model = excluded.unified_model,
                tag_match_mode = excluded.tag_match_mode,
                created_at = excluded.created_at,
                updated_at = CASE
                    WHEN groups.config_hash IS NOT excluded.config_hash THEN excluded.updated_at
                    ELSE groups.updated_at
                END,
                config_hash = excluded.config_hash
             WHERE groups.deleted_at IS NULL",
            rusqlite::params![
                id,
                &dto.name,
                &dto.mode,
                &dto.group_prompt,
                &dto.invite_prompt,
                if dto.use_unified_model { 1 } else { 0 },
                &dto.unified_model,
                &dto.tag_match_mode,
                dto.created_at,
                &config_hash,
                now
            ],
        )?;

        // A local tombstone is monotonic. A stale remote snapshot must fail the
        // attempt instead of being counted as a successful no-op.
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Group {id} is tombstoned"
            )));
        }

        tx.execute("DELETE FROM group_members WHERE group_id = ?", [id])?;

        let member_tags = dto.member_tags.as_ref().and_then(|v| v.as_object());

        for (sort_order, member) in dto.members.iter().enumerate() {
            let tag = member_tags
                .and_then(|m| m.get(member))
                .and_then(|v| v.as_str());
            tx.execute(
                "INSERT INTO group_members (group_id, agent_id, member_tag, sort_order, updated_at) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![id, member, tag, sort_order as i64, now]
            )?;
        }

        Ok(())
    }

    fn rusqlite_upsert_avatar(
        tx: &rusqlite::Transaction,
        owner_type: &str,
        owner_id: &str,
        bytes: &[u8],
    ) -> rusqlite::Result<()> {
        let hash = HashAggregator::compute_avatar_hash(bytes);
        let dominant_color: Option<String> = None;
        let now = chrono::Utc::now().timestamp_millis();

        if !is_valid_avatar_owner(owner_type, owner_id) {
            return Err(Self::sync_contract_error(
                "Avatar requires a non-empty owner id and a supported owner type",
            ));
        }
        let parent_is_live = match owner_type {
            "agent" => tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM agents
                    WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
                 )",
                [owner_id],
                |row| row.get::<_, bool>(0),
            )?,
            "group" => tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM groups
                    WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL
                 )",
                [owner_id],
                |row| row.get::<_, bool>(0),
            )?,
            "user" => true,
            _ => false,
        };
        if !parent_is_live {
            return Err(Self::sync_contract_error(format!(
                "Avatar owner {owner_type}/{owner_id} is missing or deleted"
            )));
        }
        let changed = tx.execute(
            "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, dominant_color, updated_at) 
             VALUES (?, ?, ?, 'image/png', ?, ?, ?) 
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET 
             avatar_hash=excluded.avatar_hash, image_data=excluded.image_data, dominant_color=excluded.dominant_color, updated_at=excluded.updated_at
             WHERE avatars.deleted_at IS NULL",
            rusqlite::params![owner_type, owner_id, &hash, bytes, &dominant_color, now]
        )?;

        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Avatar {owner_type}/{owner_id} is tombstoned"
            )));
        }

        Ok(())
    }

    fn rusqlite_upsert_agent_topic(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
        dto: &AgentTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        if key.owner_type != "agent"
            || !key.is_valid()
            || dto.id != key.topic_id
            || dto.owner_id != key.owner_id
        {
            return Err(Self::sync_contract_error(
                "Agent topic requires an exact agent topic identity",
            ));
        }
        let topic_id = &key.topic_id;
        let owner_exists: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM agents
                WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
             )",
            [&dto.owner_id],
            |row| row.get(0),
        )?;
        if !owner_exists {
            return Err(Self::sync_contract_error(format!(
                "Agent topic {topic_id} owner {} is missing or deleted",
                dto.owner_id
            )));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_topic_metadata_hash(dto);

        let changed = tx.execute(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, config_hash, updated_at)
            SELECT ?, ?, ?, 'agent', ?, ?, ?, ?, ?
            WHERE EXISTS (
                SELECT 1 FROM agents
                WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL
            )
            ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
            title=excluded.title, created_at=excluded.created_at,
            locked=excluded.locked, unread=excluded.unread,
            updated_at=CASE
                WHEN topics.config_hash IS NOT excluded.config_hash THEN excluded.updated_at
                ELSE topics.updated_at
            END,
            config_hash=excluded.config_hash
            WHERE topics.deleted_at IS NULL",
            rusqlite::params![
                topic_id, &dto.name, &dto.owner_id, dto.created_at,
                if dto.locked { 1 } else { 0 },
                if dto.unread { 1 } else { 0 },
                &config_hash, now, &dto.owner_id
            ]
        )?;
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Agent topic {topic_id} upsert affected {changed} rows"
            )));
        }

        Ok(())
    }

    fn rusqlite_upsert_group_topic(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
        dto: &GroupTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        if key.owner_type != "group"
            || !key.is_valid()
            || dto.id != key.topic_id
            || dto.owner_id != key.owner_id
        {
            return Err(Self::sync_contract_error(
                "Group topic requires an exact group topic identity",
            ));
        }
        let topic_id = &key.topic_id;
        let owner_exists: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM groups
                WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL
             )",
            [&dto.owner_id],
            |row| row.get(0),
        )?;
        if !owner_exists {
            return Err(Self::sync_contract_error(format!(
                "Group topic {topic_id} owner {} is missing or deleted",
                dto.owner_id
            )));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_topic_metadata_hash(dto);

        let changed = tx.execute(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, config_hash, updated_at)
            SELECT ?, ?, ?, 'group', ?, 1, 0, ?, ?
            WHERE EXISTS (
                SELECT 1 FROM groups
                WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL
            )
            ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
            title=excluded.title, created_at=excluded.created_at,
            updated_at=CASE
                WHEN topics.config_hash IS NOT excluded.config_hash THEN excluded.updated_at
                ELSE topics.updated_at
            END,
            config_hash=excluded.config_hash
            WHERE topics.deleted_at IS NULL",
            rusqlite::params![
                topic_id, &dto.name, &dto.owner_id, dto.created_at,
                &config_hash, now, &dto.owner_id
            ]
        )?;
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Group topic {topic_id} upsert affected {changed} rows"
            )));
        }

        Ok(())
    }

    fn rusqlite_upsert_messages_batch(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
        messages: Vec<ChatMessage>,
        contents: Vec<String>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
    ) -> rusqlite::Result<()> {
        if messages.len() != contents.len()
            || messages.len() != render_bytes.len()
            || messages.len() != content_hashes.len()
        {
            return Err(Self::sync_contract_error(format!(
                "Topic {} message batch vectors have inconsistent lengths",
                key.topic_id
            )));
        }

        let topic_is_live: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM topics
                WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             )",
            rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
            |row| row.get(0),
        )?;
        if !topic_is_live {
            return Err(Self::sync_contract_error(format!(
                "Message batch parent topic {} is missing or deleted",
                key.topic_id
            )));
        }
        if messages.is_empty() {
            return Ok(());
        }
        if messages.iter().any(|message| {
            message
                .topic_id
                .as_deref()
                .is_some_and(|message_topic| message_topic != key.topic_id)
        }) {
            return Err(Self::sync_contract_error(format!(
                "Message batch contains a topic id conflicting with {}",
                key.topic_id
            )));
        }

        // Pull batches do not carry a causally newer restore marker. Filter local
        // tombstones once up front so no message, FTS, render, or attachment side table
        // can be repopulated by a stale remote snapshot.
        let mut tombstoned_ids: HashSet<String> = HashSet::new();
        for chunk in messages.chunks(998) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql = format!(
                "SELECT msg_id FROM messages
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                   AND deleted_at IS NOT NULL AND msg_id IN ({})",
                placeholders
            );
            let mut params = Vec::with_capacity(chunk.len() + 3);
            params.push(key.owner_type.clone());
            params.push(key.owner_id.clone());
            params.push(key.topic_id.clone());
            params.extend(chunk.iter().map(|message| message.id.clone()));
            let mut statement = tx.prepare_cached(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(params), |row| row.get(0))?;
            for row in rows {
                tombstoned_ids.insert(row?);
            }
        }
        if !tombstoned_ids.is_empty() {
            let kept_indices: Vec<usize> = messages
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    (!tombstoned_ids.contains(&message.id)).then_some(index)
                })
                .collect();
            if kept_indices.is_empty() {
                return Ok(());
            }
            let filtered_messages = kept_indices
                .iter()
                .map(|index| messages[*index].clone())
                .collect();
            let filtered_contents = kept_indices
                .iter()
                .map(|index| contents[*index].clone())
                .collect();
            let filtered_render_bytes = kept_indices
                .iter()
                .map(|index| render_bytes[*index].clone())
                .collect();
            let filtered_content_hashes = kept_indices
                .iter()
                .map(|index| content_hashes[*index].clone())
                .collect();
            return Self::rusqlite_upsert_messages_batch(
                tx,
                key,
                filtered_messages,
                filtered_contents,
                filtered_render_bytes,
                filtered_content_hashes,
            );
        }

        let now = chrono::Utc::now().timestamp_millis();

        // Phase 3: Turbo Mode - Chunked Bulk Insert
        const MAX_PARAMS: usize = 999;
        const PARAMS_PER_MSG: usize = 15;
        let chunk_size = MAX_PARAMS / PARAMS_PER_MSG;

        for chunk_indices in messages
            .iter()
            .enumerate()
            .collect::<Vec<_>>()
            .chunks(chunk_size)
        {
            // 1. 批量插入 messages 表 (不含 render_content)
            let mut sql_msgs = String::from(
                "INSERT INTO messages (
                    owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content, timestamp,
                    is_group_message, group_id, finish_reason,
                    content_hash, created_at, updated_at
                ) VALUES ",
            );

            for i in 0..chunk_indices.len() {
                if i > 0 {
                    sql_msgs.push_str(", ");
                }
                sql_msgs.push_str("(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)");
            }

            sql_msgs.push_str(
                " ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                    content = excluded.content,
                    role = excluded.role,
                    name = excluded.name,
                    agent_id = excluded.agent_id,
                    is_group_message = excluded.is_group_message,
                    group_id = excluded.group_id,
                    finish_reason = excluded.finish_reason,
                    content_hash = excluded.content_hash,
                    timestamp = excluded.timestamp,
                    updated_at = excluded.updated_at
                 WHERE messages.deleted_at IS NULL",
            );

            let mut stmt_msgs = tx.prepare_cached(&sql_msgs)?;
            let mut params_msgs: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            for (idx, msg) in chunk_indices {
                let updated_at = msg.updated_at.unwrap_or(msg.timestamp);
                if updated_at > (1_u64 << 53) - 1 {
                    return Err(Self::sync_contract_error(format!(
                        "Message {}/{} updatedAt exceeds the safe integer range",
                        key.topic_id, msg.id
                    )));
                }
                params_msgs.push(Box::new(key.owner_type.clone()));
                params_msgs.push(Box::new(key.owner_id.clone()));
                params_msgs.push(Box::new(key.topic_id.clone()));
                params_msgs.push(Box::new(msg.id.clone()));
                params_msgs.push(Box::new(msg.role.clone()));
                params_msgs.push(Box::new(msg.name.clone()));
                params_msgs.push(Box::new(msg.agent_id.clone()));
                params_msgs.push(Box::new(contents[*idx].clone()));
                params_msgs.push(Box::new(msg.timestamp as i64));
                params_msgs.push(Box::new(msg.is_group_message.unwrap_or(false)));
                params_msgs.push(Box::new(msg.group_id.clone()));
                params_msgs.push(Box::new(msg.finish_reason.clone()));
                params_msgs.push(Box::new(content_hashes[*idx].clone()));
                params_msgs.push(Box::new(msg.timestamp as i64));
                params_msgs.push(Box::new(updated_at as i64));
            }

            let refs_msgs: Vec<&dyn rusqlite::ToSql> =
                params_msgs.iter().map(|p| p.as_ref()).collect();
            stmt_msgs.execute(&*refs_msgs)?;

            // 2. 批量插入 render_cache 表
            // 先失效本批全部旧缓存；预渲染关闭或编译失败时不能继续沿用旧内容。
            let cache_placeholders = vec!["?"; chunk_indices.len()].join(", ");
            let delete_cache_sql = format!(
                "DELETE FROM render_cache
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id IN ({})",
                cache_placeholders
            );
            let mut delete_cache = tx.prepare_cached(&delete_cache_sql)?;
            let mut delete_params: Vec<String> = Vec::with_capacity(chunk_indices.len() + 3);
            delete_params.push(key.owner_type.clone());
            delete_params.push(key.owner_id.clone());
            delete_params.push(key.topic_id.clone());
            delete_params.extend(chunk_indices.iter().map(|(_, msg)| msg.id.clone()));
            let delete_refs: Vec<&dyn rusqlite::ToSql> = delete_params
                .iter()
                .map(|value| value as &dyn rusqlite::ToSql)
                .collect();
            delete_cache.execute(&*delete_refs)?;

            // 过滤出有实际预渲染内容的消息（当预渲染关闭时，所有 render_bytes 均为空）
            let render_chunk: Vec<_> = chunk_indices
                .iter()
                .map(|&(idx, msg)| (idx, msg))
                .filter(|(idx, _)| !render_bytes[*idx].is_empty())
                .collect();

            if !render_chunk.is_empty() {
                let mut sql_render = String::from(
                    "INSERT INTO render_cache (
                        owner_type, owner_id, topic_id, msg_id, render_content, content_hash,
                        renderer_schema_version, updated_at
                     ) VALUES ",
                );

                for i in 0..render_chunk.len() {
                    if i > 0 {
                        sql_render.push_str(", ");
                    }
                    sql_render.push_str("(?, ?, ?, ?, ?, ?, ?, ?)");
                }

                sql_render.push_str(
                    " ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                        render_content = excluded.render_content,
                        content_hash = excluded.content_hash,
                        renderer_schema_version = excluded.renderer_schema_version,
                        updated_at = excluded.updated_at",
                );

                let mut stmt_render = tx.prepare_cached(&sql_render)?;
                let mut params_render: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

                for (idx, msg) in render_chunk {
                    params_render.push(Box::new(key.owner_type.clone()));
                    params_render.push(Box::new(key.owner_id.clone()));
                    params_render.push(Box::new(key.topic_id.clone()));
                    params_render.push(Box::new(msg.id.clone()));
                    params_render.push(Box::new(render_bytes[idx].clone()));
                    params_render.push(Box::new(content_hashes[idx].clone()));
                    params_render.push(Box::new(
                        crate::vcp_modules::message_repository::RENDERER_SCHEMA_VERSION,
                    ));
                    params_render.push(Box::new(now));
                }

                let refs_render: Vec<&dyn rusqlite::ToSql> =
                    params_render.iter().map(|p| p.as_ref()).collect();
                stmt_render.execute(&*refs_render)?;
            }
        }

        // Phase 3.5: 全文检索 FTS5 批量同步
        let msg_ids_for_fts: Vec<String> = messages.iter().map(|msg| msg.id.clone()).collect();
        for chunk in msg_ids_for_fts.chunks(998) {
            // SQLite 参数上限，预留 1 个给 topic_id
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql_del_fts = format!(
                "DELETE FROM messages_fts
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id IN ({})",
                placeholders
            );
            let mut stmt_del_fts = tx.prepare_cached(&sql_del_fts)?;

            let mut params: Vec<String> = Vec::with_capacity(chunk.len() + 3);
            params.push(key.owner_type.clone());
            params.push(key.owner_id.clone());
            params.push(key.topic_id.clone());
            for id in chunk {
                params.push(id.clone());
            }
            stmt_del_fts.execute(rusqlite::params_from_iter(params))?;
        }

        const PARAMS_PER_FTS: usize = 5;
        let fts_chunk_size = MAX_PARAMS / PARAMS_PER_FTS;
        for chunk in messages.chunks(fts_chunk_size) {
            let mut sql_ins_fts =
                String::from(
                    "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id) VALUES ",
                );
            for i in 0..chunk.len() {
                if i > 0 {
                    sql_ins_fts.push_str(", ");
                }
                sql_ins_fts.push_str("(?, ?, ?, ?, ?)");
            }
            let mut stmt_ins_fts = tx.prepare_cached(&sql_ins_fts)?;
            let mut params_fts: Vec<String> = Vec::new();
            for msg in chunk {
                // trigram 分词器（migration 0008 起）直接索引原文，无需 CJK 预处理
                params_fts.push(msg.id.clone());
                params_fts.push(key.topic_id.clone());
                params_fts.push(msg.content.clone());
                params_fts.push(key.owner_type.clone());
                params_fts.push(key.owner_id.clone());
            }
            stmt_ins_fts.execute(rusqlite::params_from_iter(params_fts))?;
        }

        // Phase 4: Attachment Optimization
        let mut msg_ids = Vec::new();
        let mut all_relations = Vec::new();

        for msg in messages.iter() {
            msg_ids.push(msg.id.clone());
            if let Some(ref attachments) = msg.attachments {
                for (i, att) in attachments.iter().enumerate() {
                    let hash = att.hash.clone().ok_or_else(|| {
                        Self::sync_contract_error(format!(
                            "Attachment {} on message {} is missing its SHA-256 hash",
                            att.name, msg.id
                        ))
                    })?;
                    if hash != hash.to_ascii_lowercase()
                        || !crate::vcp_modules::infra::utils::is_valid_cas_hash(&hash)
                    {
                        return Err(Self::sync_contract_error(format!(
                            "Attachment {} on message {} has an invalid SHA-256 hash",
                            att.name, msg.id
                        )));
                    }
                    let status = att.status.as_deref().ok_or_else(|| {
                        Self::sync_contract_error(format!(
                            "Attachment {hash} on message {} is missing status",
                            msg.id
                        ))
                    })?;
                    if !matches!(status, "ready" | "desktop_only") {
                        return Err(Self::sync_contract_error(format!(
                            "Attachment {hash} on message {} has invalid status {status}",
                            msg.id
                        )));
                    }

                    Self::rusqlite_upsert_attachment_core(tx, &hash, att, msg.timestamp as i64)?;

                    // Resolve readiness inside the same write transaction as the relation.
                    // If CAS registration committed before us, the preserved core path wins;
                    // if it commits after us, its promotion UPDATE observes this relation.
                    let current_path: String = tx.query_row(
                        "SELECT internal_path FROM attachments WHERE hash = ?",
                        [&hash],
                        |row| row.get(0),
                    )?;
                    let clean_path = current_path.trim_start_matches("file://");
                    let verified_path = if clean_path.is_empty() {
                        None
                    } else {
                        match std::fs::metadata(clean_path) {
                            Ok(metadata) if metadata.is_file() => Some(clean_path),
                            Ok(_) => None,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                            Err(error) => {
                                return Err(Self::sync_contract_error(format!(
                                    "Failed to inspect local attachment {hash}: {error}"
                                )));
                            }
                        }
                    };
                    let (relation_src, relation_status) = match verified_path {
                        Some(path) => (format!("file://{path}"), "ready".to_string()),
                        None => {
                            if !current_path.trim().is_empty() {
                                tx.execute(
                                    "UPDATE attachments SET internal_path = '' WHERE hash = ?",
                                    [&hash],
                                )?;
                            }
                            (String::new(), "desktop_only".to_string())
                        }
                    };

                    all_relations.push((
                        msg.id.clone(),
                        hash,
                        i as i32,
                        att.name.clone(),
                        relation_src,
                        relation_status,
                        msg.timestamp as i64,
                    ));
                }
            }
        }

        // Chunked Delete
        for chunk in msg_ids.chunks(999) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                "DELETE FROM message_attachments
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                   AND msg_id IN ({})",
                placeholders
            );
            let mut stmt = tx.prepare_cached(&sql)?;
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            params.push(Box::new(key.owner_type.clone()));
            params.push(Box::new(key.owner_id.clone()));
            params.push(Box::new(key.topic_id.clone()));
            for id in chunk {
                params.push(Box::new(id.clone()));
            }
            let params_refs: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            stmt.execute(&*params_refs)?;
        }

        // Chunked Relation Insert
        if !all_relations.is_empty() {
            const PARAMS_PER_REL: usize = 10;
            let rel_chunk_size = MAX_PARAMS / PARAMS_PER_REL;
            for chunk in all_relations.chunks(rel_chunk_size) {
                let mut sql = String::from(
                    "INSERT INTO message_attachments (
                    owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                    display_name, src, status, created_at
                ) VALUES ",
                );
                for i in 0..chunk.len() {
                    if i > 0 {
                        sql.push_str(", ");
                    }
                    sql.push_str("(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)");
                }
                let mut stmt = tx.prepare_cached(&sql)?;
                let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                for rel in chunk {
                    params.push(Box::new(key.owner_type.clone()));
                    params.push(Box::new(key.owner_id.clone()));
                    params.push(Box::new(key.topic_id.clone()));
                    params.push(Box::new(rel.0.clone()));
                    params.push(Box::new(rel.1.clone()));
                    params.push(Box::new(rel.2));
                    params.push(Box::new(rel.3.clone()));
                    params.push(Box::new(rel.4.clone()));
                    params.push(Box::new(rel.5.clone()));
                    params.push(Box::new(rel.6));
                }
                let params_refs: Vec<&dyn rusqlite::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();
                stmt.execute(&*params_refs)?;
            }
        }

        Ok(())
    }

    fn rusqlite_bubble_topic_hash(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
    ) -> rusqlite::Result<()> {
        // 1. 计算 content_hash (消息聚合)
        let mut stmt = tx.prepare(
            "SELECT msg_id, content_hash FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             ORDER BY timestamp ASC, msg_id ASC",
        )?;
        let hashes: Vec<String> = stmt
            .query_map(
                rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
                |row| {
                    let message_id = row.get::<_, String>(0)?;
                    let message_hash = row.get::<_, String>(1)?;
                    Ok(HashAggregator::compute_message_leaf_hash(
                        &message_id,
                        &message_hash,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);

        let config_hash = if key.owner_type == "agent" {
            let dto = Self::rusqlite_load_agent_topic_dto(tx, key)?;
            HashAggregator::compute_agent_topic_metadata_hash(&dto)
        } else if key.owner_type == "group" {
            let dto = Self::rusqlite_load_group_topic_dto(tx, key)?;
            HashAggregator::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(Self::sync_contract_error(format!(
                "Topic {} has unsupported owner type {}",
                key.topic_id, key.owner_type
            )));
        };

        let changed = tx.execute(
            "UPDATE topics SET content_hash = ?, config_hash = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
            rusqlite::params![
                root_hash,
                config_hash,
                &key.owner_type,
                &key.owner_id,
                &key.topic_id
            ],
        )?;
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Topic {} disappeared during hash update",
                key.topic_id
            )));
        }
        Ok(())
    }

    fn rusqlite_bubble_agent_hash(
        tx: &rusqlite::Transaction,
        agent_id: &str,
    ) -> rusqlite::Result<()> {
        let mut stmt = tx.prepare("SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND topic_id <> 'default' AND deleted_at IS NULL ORDER BY topic_id ASC")?;
        let mut rows = stmt.query([agent_id])?;
        let mut hashes = Vec::new();
        while let Some(row) = rows.next()? {
            hashes.push(HashAggregator::compute_topic_leaf_hash(
                &row.get::<_, String>(0)?,
                &row.get::<_, String>(1)?,
                &row.get::<_, String>(2)?,
            ));
        }
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);
        let changed = tx.execute(
            "UPDATE agents SET content_hash = ?
             WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
            [root_hash, agent_id.to_string()],
        )?;
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Agent {agent_id} disappeared during hash update"
            )));
        }
        Ok(())
    }

    fn rusqlite_bubble_group_hash(
        tx: &rusqlite::Transaction,
        group_id: &str,
    ) -> rusqlite::Result<()> {
        let mut stmt = tx.prepare("SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND topic_id <> 'default' AND deleted_at IS NULL ORDER BY topic_id ASC")?;
        let mut rows = stmt.query([group_id])?;
        let mut hashes = Vec::new();
        while let Some(row) = rows.next()? {
            hashes.push(HashAggregator::compute_topic_leaf_hash(
                &row.get::<_, String>(0)?,
                &row.get::<_, String>(1)?,
                &row.get::<_, String>(2)?,
            ));
        }
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);
        let changed = tx.execute(
            "UPDATE groups SET content_hash = ?
             WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL",
            [root_hash, group_id.to_string()],
        )?;
        if changed != 1 {
            return Err(Self::sync_contract_error(format!(
                "Group {group_id} disappeared during hash update"
            )));
        }
        Ok(())
    }

    fn rusqlite_load_agent_topic_dto(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
    ) -> rusqlite::Result<AgentTopicSyncDTO> {
        tx.query_row(
            "SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics
             WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ?",
            rusqlite::params![&key.owner_id, &key.topic_id],
            |row| {
                Ok(AgentTopicSyncDTO {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    locked: row.get::<_, i64>(3)? != 0,
                    unread: row.get::<_, i64>(4)? != 0,
                    owner_id: row.get(5)?,
                })
            },
        )
    }

    fn rusqlite_load_group_topic_dto(
        tx: &rusqlite::Transaction,
        key: &TopicKey,
    ) -> rusqlite::Result<GroupTopicSyncDTO> {
        tx.query_row(
            "SELECT topic_id, title, created_at, owner_id FROM topics
             WHERE owner_type = 'group' AND owner_id = ? AND topic_id = ?",
            rusqlite::params![&key.owner_id, &key.topic_id],
            |row| {
                Ok(GroupTopicSyncDTO {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    owner_id: row.get(3)?,
                })
            },
        )
    }

    fn rusqlite_upsert_attachment_core(
        tx: &rusqlite::Transaction,
        hash: &str,
        att: &crate::vcp_modules::chat_manager::Attachment,
        timestamp: i64,
    ) -> rusqlite::Result<()> {
        let image_frames = att
            .image_frames
            .as_ref()
            .and_then(|frames| serde_json::to_string(frames).ok());

        tx.execute(
            "INSERT INTO attachments (
                hash, mime_type, size, internal_path, extracted_text, image_frames, thumbnail_path,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(hash) DO UPDATE SET
                mime_type = excluded.mime_type,
                size = excluded.size,
                internal_path = CASE
                    WHEN excluded.internal_path <> '' THEN excluded.internal_path
                    ELSE attachments.internal_path
                END,
                extracted_text = COALESCE(attachments.extracted_text, excluded.extracted_text),
                image_frames = COALESCE(attachments.image_frames, excluded.image_frames),
                thumbnail_path = COALESCE(attachments.thumbnail_path, excluded.thumbnail_path),
                updated_at = excluded.updated_at",
            rusqlite::params![
                hash,
                &att.r#type,
                att.size as i64,
                &att.internal_path,
                &att.extracted_text,
                image_frames,
                &att.thumbnail_path,
                timestamp,
                timestamp
            ],
        )?;

        Ok(())
    }
}
