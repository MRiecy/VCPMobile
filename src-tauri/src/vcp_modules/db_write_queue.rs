use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_retry::{retry_on_db_locked, RetryConfig};
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
    AgentTopic {
        topic_id: String,
        dto: AgentTopicSyncDTO,
    },
    GroupTopic {
        topic_id: String,
        dto: GroupTopicSyncDTO,
    },
    Flush {
        tx: oneshot::Sender<()>,
    },
}

pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
    logger: Option<Arc<Mutex<SyncLogger>>>,
    _worker: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for DbWriteQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            logger: self.logger.clone(),
            _worker: None,
        }
    }
}

impl DbWriteQueue {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        let (tx, mut rx) = mpsc::channel(256);

        let worker = tokio::spawn(async move {
            println!("[DbWriteQueue] Worker started");

            let mut success_count = 0u32;
            let mut error_count = 0u32;

            while let Some(task) = rx.recv().await {
                let (task_type, id, result) = match task {
                    DbWriteTask::Agent { id, dto } => {
                        let result = Self::upsert_agent(&pool, &id, &dto).await;
                        ("agent", id, result)
                    }
                    DbWriteTask::Group { id, dto } => {
                        let result = Self::upsert_group(&pool, &id, &dto).await;
                        ("group", id, result)
                    }
                    DbWriteTask::Avatar {
                        owner_type,
                        owner_id,
                        bytes,
                    } => {
                        let result =
                            Self::upsert_avatar(&pool, &owner_type, &owner_id, &bytes).await;
                        ("avatar", format!("{}:{}", owner_type, owner_id), result)
                    }
                    DbWriteTask::AgentTopic { topic_id, dto } => {
                        let result = Self::upsert_agent_topic(&pool, &topic_id, &dto).await;
                        ("agent_topic", topic_id, result)
                    }
                    DbWriteTask::GroupTopic { topic_id, dto } => {
                        let result = Self::upsert_group_topic(&pool, &topic_id, &dto).await;
                        ("group_topic", topic_id, result)
                    }
                    DbWriteTask::Flush { tx } => {
                        let _ = tx.send(());
                        continue;
                    }
                };

                match result {
                    Ok(_) => {
                        success_count += 1;
                    }
                    Err(e) => {
                        error_count += 1;
                        println!("[DbWriteQueue] {} {} error: {}", task_type, id, e);
                    }
                }
            }

            println!(
                "[DbWriteQueue] Worker stopped. Total: success={}, errors={}",
                success_count, error_count
            );
        });

        Self {
            sender: tx,
            logger: None,
            _worker: Some(worker),
        }
    }

    pub fn set_logger(&mut self, logger: Arc<Mutex<SyncLogger>>) {
        self.logger = Some(logger);
    }

    pub async fn submit(&self, task: DbWriteTask) {
        if let Err(e) = self.sender.send(task).await {
            println!("[DbWriteQueue] Submit error: {}", e);
        }
    }

    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self.sender.send(DbWriteTask::Flush { tx }).await {
            println!("[DbWriteQueue] Flush submit error: {}", e);
            return;
        }
        let _ = rx.await;
        println!("[DbWriteQueue] Flush completed");
    }

    async fn upsert_agent(
        pool: &sqlx::SqlitePool,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_config_hash(dto);

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        sqlx::query(
            "INSERT INTO agents (
                agent_id, name, system_prompt, model, temperature, 
                context_token_limit, max_output_tokens, 
                stream_output, config_hash, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(agent_id) DO UPDATE SET
                name = excluded.name, 
                system_prompt = excluded.system_prompt, 
                model = excluded.model, 
                temperature = excluded.temperature, 
                context_token_limit = excluded.context_token_limit, 
                max_output_tokens = excluded.max_output_tokens, 
                stream_output = excluded.stream_output, 
                config_hash = excluded.config_hash,
                updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(&dto.name)
        .bind(&dto.system_prompt)
        .bind(&dto.model)
        .bind(dto.temperature)
        .bind(dto.context_token_limit)
        .bind(dto.max_output_tokens)
        .bind(if dto.stream_output { 1 } else { 0 })
        .bind(&config_hash)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;

        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_agent_hash(&mut bubble_tx, id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_group(
        pool: &sqlx::SqlitePool,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_config_hash(dto);

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        sqlx::query(
            "INSERT INTO groups (
                group_id, name, mode,
                group_prompt, invite_prompt, use_unified_model, unified_model,
                tag_match_mode, created_at, config_hash, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(group_id) DO UPDATE SET
                name = excluded.name,
                mode = excluded.mode,
                group_prompt = excluded.group_prompt,
                invite_prompt = excluded.invite_prompt,
                use_unified_model = excluded.use_unified_model,
                unified_model = excluded.unified_model,
                tag_match_mode = excluded.tag_match_mode,
                created_at = excluded.created_at,
                config_hash = excluded.config_hash,
                updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(&dto.name)
        .bind(&dto.mode)
        .bind(&dto.group_prompt)
        .bind(&dto.invite_prompt)
        .bind(if dto.use_unified_model { 1 } else { 0 })
        .bind(&dto.unified_model)
        .bind(&dto.tag_match_mode)
        .bind(dto.created_at)
        .bind(&config_hash)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM group_members WHERE group_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        let member_tags = dto.member_tags.as_ref().and_then(|v| v.as_object());

        for member in &dto.members {
            let tag = member_tags
                .and_then(|m| m.get(member))
                .and_then(|v| v.as_str());
            sqlx::query(
                "INSERT INTO group_members (group_id, agent_id, member_tag, sort_order, updated_at) VALUES (?, ?, ?, 0, ?)"
            )
            .bind(id)
            .bind(member)
            .bind(tag)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;

        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_group_hash(&mut bubble_tx, id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_avatar(
        pool: &sqlx::SqlitePool,
        owner_type: &str,
        owner_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let hash = HashAggregator::compute_avatar_hash(bytes);
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query(
            "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at) 
             VALUES (?, ?, ?, 'image/png', ?, ?) 
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET 
             avatar_hash=excluded.avatar_hash, image_data=excluded.image_data, updated_at=excluded.updated_at"
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(&hash)
        .bind(bytes)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_agent_topic(
        pool: &sqlx::SqlitePool,
        topic_id: &str,
        dto: &AgentTopicSyncDTO,
    ) -> Result<(), String> {
        let config = RetryConfig::default();
        let operation_name = format!("upsert_agent_topic[{}]", topic_id);

        retry_on_db_locked(&config, || {
            let pool = pool.clone();
            let topic_id = topic_id.to_string();
            let dto = dto.clone();
            let now = chrono::Utc::now().timestamp_millis();

            async move {
                // 先尝试写入 topic，如果 owner 不存在则跳过哈希更新
                let result = sqlx::query(
                    "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
                    VALUES (?, ?, ?, 'agent', ?, ?, ?, ?)
                    ON CONFLICT(topic_id) DO UPDATE SET
                    title=excluded.title, locked=excluded.locked, unread=excluded.unread, updated_at=excluded.updated_at"
                )
                .bind(&topic_id)
                .bind(&dto.name)
                .bind(&dto.owner_id)
                .bind(dto.created_at)
                .bind(if dto.locked { 1 } else { 0 })
                .bind(if dto.unread { 1 } else { 0 })
                .bind(now)
                .execute(&pool)
                .await;

                match result {
                    Ok(_) => {
                        // 只有当 owner 存在时才更新哈希
                        let owner_exists: bool = sqlx::query_scalar(
                            "SELECT COUNT(*) > 0 FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
                        )
                        .bind(&dto.owner_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap_or(false);

                        if owner_exists {
                            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
                            HashAggregator::bubble_agent_hash(&mut tx, &dto.owner_id).await?;
                            tx.commit().await.map_err(|e| e.to_string())?;
                        } else {
                            println!(
                                "[DbWriteQueue] AgentTopic {} inserted, but owner {} not yet available (will sync later)",
                                topic_id, dto.owner_id
                            );
                        }

                        // 补偿：如果该 topic 已有消息（因竞态先写入了消息），更新 msg_count
                        let current_msg_count: i32 = sqlx::query_scalar("SELECT msg_count FROM topics WHERE topic_id = ?")
                            .bind(&topic_id)
                            .fetch_one(&pool)
                            .await
                            .unwrap_or(0);

                        let actual_count: i32 = sqlx::query_scalar::<_, i64>(
                            "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
                        )
                        .bind(&topic_id)
                        .fetch_optional(&pool)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(0) as i32;

                        if actual_count > 0 && actual_count != current_msg_count {
                            let _ = sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?")
                                .bind(actual_count)
                                .bind(&topic_id)
                                .execute(&pool)
                                .await;
                            println!(
                                "[DbWriteQueue] Topic {} msg_count compensated to {}",
                                topic_id, actual_count
                            );
                        }

                        Ok(())
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
        }, &operation_name).await
    }

    async fn upsert_group_topic(
        pool: &sqlx::SqlitePool,
        topic_id: &str,
        dto: &GroupTopicSyncDTO,
    ) -> Result<(), String> {
        // 先尝试写入 topic，如果 owner 不存在则跳过哈希更新
        let now = chrono::Utc::now().timestamp_millis();

        let result = sqlx::query(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
            VALUES (?, ?, ?, 'group', ?, 1, 0, ?)
            ON CONFLICT(topic_id) DO UPDATE SET
            title=excluded.title, updated_at=excluded.updated_at"
        )
        .bind(topic_id)
        .bind(&dto.name)
        .bind(&dto.owner_id)
        .bind(dto.created_at)
        .bind(now)
        .execute(pool)
        .await;

        match result {
            Ok(_) => {
                // 只有当 owner 存在时才更新哈希
                let owner_exists: bool = sqlx::query_scalar(
                    "SELECT COUNT(*) > 0 FROM groups WHERE group_id = ? AND deleted_at IS NULL",
                )
                .bind(&dto.owner_id)
                .fetch_one(pool)
                .await
                .unwrap_or(false);

                if owner_exists {
                    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
                    HashAggregator::bubble_group_hash(&mut tx, &dto.owner_id).await?;
                    tx.commit().await.map_err(|e| e.to_string())?;
                } else {
                    println!(
                        "[DbWriteQueue] GroupTopic {} inserted, but owner {} not yet available (will sync later)",
                        topic_id, dto.owner_id
                    );
                }

                // 补偿：如果该 topic 已有消息（因竞态先写入了消息），更新 msg_count
                let current_msg_count: i32 =
                    sqlx::query_scalar("SELECT msg_count FROM topics WHERE topic_id = ?")
                        .bind(topic_id)
                        .fetch_one(pool)
                        .await
                        .unwrap_or(0);

                let actual_count: i32 = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
                )
                .bind(topic_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(0) as i32;

                if actual_count > 0 && actual_count != current_msg_count {
                    let _ = sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?")
                        .bind(actual_count)
                        .bind(topic_id)
                        .execute(pool)
                        .await;
                    println!(
                        "[DbWriteQueue] Topic {} msg_count compensated to {}",
                        topic_id, actual_count
                    );
                }

                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
