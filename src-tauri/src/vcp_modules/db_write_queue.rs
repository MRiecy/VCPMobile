use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_service;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum DbWriteTask {
    UpsertAgent {
        id: String,
        dto: AgentSyncDTO,
    },
    UpsertGroup {
        id: String,
        dto: GroupSyncDTO,
    },
    UpsertAvatar {
        owner_type: String,
        owner_id: String,
        bytes: Vec<u8>,
    },
    UpsertAgentTopic {
        topic_id: String,
        dto: AgentTopicSyncDTO,
    },
    UpsertGroupTopic {
        topic_id: String,
        dto: GroupTopicSyncDTO,
    },
    UpsertMessages {
        topic_id: String,
        owner_id: String,
        owner_type: String,
        messages: Vec<ChatMessage>,
    },
}

pub struct DbWriteQueue {
    sender: mpsc::Sender<DbWriteTask>,
}

impl DbWriteQueue {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        let (tx, mut rx) = mpsc::channel(256);

        tokio::spawn(async move {
            println!("[DbWriteQueue] Worker started");

            while let Some(task) = rx.recv().await {
                match task {
                    DbWriteTask::UpsertAgent { id, dto } => {
                        if let Err(e) = Self::upsert_agent(&pool, &id, &dto).await {
                            println!("[DbWriteQueue] Agent {} error: {}", id, e);
                        }
                    }
                    DbWriteTask::UpsertGroup { id, dto } => {
                        if let Err(e) = Self::upsert_group(&pool, &id, &dto).await {
                            println!("[DbWriteQueue] Group {} error: {}", id, e);
                        }
                    }
                    DbWriteTask::UpsertAvatar {
                        owner_type,
                        owner_id,
                        bytes,
                    } => {
                        if let Err(e) =
                            Self::upsert_avatar(&pool, &owner_type, &owner_id, &bytes).await
                        {
                            println!(
                                "[DbWriteQueue] Avatar {}:{} error: {}",
                                owner_type, owner_id, e
                            );
                        }
                    }
                    DbWriteTask::UpsertAgentTopic { topic_id, dto } => {
                        if let Err(e) = Self::upsert_agent_topic(&pool, &topic_id, &dto).await {
                            println!("[DbWriteQueue] AgentTopic {} error: {}", topic_id, e);
                        }
                    }
                    DbWriteTask::UpsertGroupTopic { topic_id, dto } => {
                        if let Err(e) = Self::upsert_group_topic(&pool, &topic_id, &dto).await {
                            println!("[DbWriteQueue] GroupTopic {} error: {}", topic_id, e);
                        }
                    }
                    DbWriteTask::UpsertMessages {
                        topic_id,
                        owner_id,
                        owner_type,
                        messages,
                    } => {
                        if let Err(e) = Self::upsert_messages(
                            &pool,
                            &topic_id,
                            &owner_id,
                            &owner_type,
                            &messages,
                        )
                        .await
                        {
                            println!("[DbWriteQueue] Messages for {} error: {}", topic_id, e);
                        }
                    }
                }
            }

            println!("[DbWriteQueue] Worker stopped");
        });

        Self { sender: tx }
    }

    pub async fn submit(&self, task: DbWriteTask) {
        if let Err(e) = self.sender.send(task).await {
            println!("[DbWriteQueue] Submit error: {}", e);
        }
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
                tag_match_mode, config_hash, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(group_id) DO UPDATE SET
                name = excluded.name,
                mode = excluded.mode,
                group_prompt = excluded.group_prompt,
                invite_prompt = excluded.invite_prompt,
                use_unified_model = excluded.use_unified_model,
                unified_model = excluded.unified_model,
                tag_match_mode = excluded.tag_match_mode,
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

        for member in &dto.members {
            sqlx::query(
                "INSERT INTO group_members (group_id, agent_id, sort_order, updated_at) VALUES (?, ?, 0, ?)"
            )
            .bind(id)
            .bind(member)
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
        // 先尝试写入 topic，如果 owner 不存在则跳过哈希更新
        let now = chrono::Utc::now().timestamp_millis();

        let result = sqlx::query(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
            VALUES (?, ?, ?, 'agent', ?, ?, ?, ?)
            ON CONFLICT(topic_id) DO UPDATE SET
            title=excluded.title, locked=excluded.locked, unread=excluded.unread, updated_at=excluded.updated_at"
        )
        .bind(topic_id)
        .bind(&dto.name)
        .bind(&dto.owner_id)
        .bind(dto.created_at)
        .bind(if dto.locked { 1 } else { 0 })
        .bind(if dto.unread { 1 } else { 0 })
        .bind(now)
        .execute(pool)
        .await;

        match result {
            Ok(_) => {
                // 只有当 owner 存在时才更新哈希
                let owner_exists: bool = sqlx::query_scalar(
                    "SELECT COUNT(*) > 0 FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
                )
                .bind(&dto.owner_id)
                .fetch_one(pool)
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
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
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
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    async fn upsert_messages(
        pool: &sqlx::SqlitePool,
        topic_id: &str,
        owner_id: &str,
        owner_type: &str,
        messages: &[ChatMessage],
    ) -> Result<(), String> {
        // 检查 topic 是否存在，如果不存在则跳过更新 topic 的操作
        let topic_exists: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM topics WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        for msg in messages {
            if topic_exists {
                let _ = message_service::patch_single_message_no_app(
                    pool,
                    owner_id,
                    owner_type,
                    topic_id.to_string(),
                    msg.clone(),
                    false,
                )
                .await;
            } else {
                // Topic 还不存在，直接写入消息（不更新 topic）
                let _ = message_service::patch_single_message_no_app(
                    pool,
                    owner_id,
                    owner_type,
                    topic_id.to_string(),
                    msg.clone(),
                    true, // skip_bubble = true，因为 topic 不存在
                )
                .await;
            }
        }

        // 只有 topic 存在时才更新 msg_count
        if topic_exists {
            let count: i32 = sqlx::query_scalar::<sqlx::Sqlite, i64>(
                "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
            )
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0) as i32;

            let _ = sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?")
                .bind(count)
                .bind(topic_id)
                .execute(pool)
                .await;
        } else {
            println!(
                "[DbWriteQueue] Messages for topic {} inserted, but topic not yet available (will sync later)",
                topic_id
            );
        }

        Ok(())
    }
}
