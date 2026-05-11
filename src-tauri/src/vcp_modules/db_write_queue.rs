use crate::vcp_modules::avatar_service::extract_dominant_color_from_bytes;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_repository::MessageRepository;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::SyncLogger;
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
    AgentTopicBatch {
        topics: Vec<(String, AgentTopicSyncDTO)>,
    },
    GroupTopic {
        topic_id: String,
        dto: GroupTopicSyncDTO,
    },
    GroupTopicBatch {
        topics: Vec<(String, GroupTopicSyncDTO)>,
    },
    TopicMessages {
        topic_id: String,
        messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
        render_bytes: Vec<Vec<u8>>,
        skip_bubble: bool,
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
            println!("[DbWriteQueue] Worker started (Greedy Transactional Mode)");

            let mut success_count = 0u32;
            let mut error_count = 0u32;

            while let Some(first_task) = rx.recv().await {
                // 如果第一个任务就是 Flush，无需启动事务，直接确认
                if let DbWriteTask::Flush { tx } = first_task {
                    let _ = tx.send(());
                    continue;
                }

                // 启动超级事务
                let mut tx = match pool.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        println!("[DbWriteQueue] Failed to start super transaction: {}", e);
                        continue;
                    }
                };

                let mut tasks_in_this_tx = vec![first_task];
                let mut flush_tx_opt: Option<oneshot::Sender<()>> = None;

                // [Optimization] 真正贪婪模式：如果批次不满，等待最多 50ms 抓取更多并发任务
                // 这样能确保在 30 线程并发分发时，Worker 能一次性吃掉几十个话题的任务
                while tasks_in_this_tx.len() < 500 {
                    let next_res = tokio::time::timeout(
                        std::time::Duration::from_millis(50),
                        rx.recv()
                    ).await;

                    match next_res {
                        Ok(Some(DbWriteTask::Flush { tx })) => {
                            flush_tx_opt = Some(tx);
                            break;
                        }
                        Ok(Some(task)) => tasks_in_this_tx.push(task),
                        _ => break, // 超时或 Channel 关闭，直接提交当前批次
                    }
                }

                let mut affected_owners = std::collections::HashSet::new();
                let mut affected_topics = std::collections::HashSet::new();

                for task in tasks_in_this_tx {
                    let (task_type, id, result) = match task {
                        // ... (Agent/Group/Avatar cases remain same) ...
                        DbWriteTask::Agent { id, dto } => {
                            let res = Self::upsert_agent_in_tx(&mut tx, &id, &dto).await;
                            affected_owners.insert((id.clone(), "agent".to_string()));
                            ("agent", id, res)
                        }
                        DbWriteTask::Group { id, dto } => {
                            let res = Self::upsert_group_in_tx(&mut tx, &id, &dto).await;
                            affected_owners.insert((id.clone(), "group".to_string()));
                            ("group", id, res)
                        }
                        DbWriteTask::Avatar {
                            owner_type,
                            owner_id,
                            bytes,
                        } => {
                            let res =
                                Self::upsert_avatar_in_tx(&mut tx, &owner_type, &owner_id, &bytes)
                                    .await;
                            ("avatar", format!("{}:{}", owner_type, owner_id), res)
                        }
                        DbWriteTask::AgentTopic { topic_id, dto } => {
                            let res =
                                Self::upsert_agent_topic_in_tx(&mut tx, &topic_id, &dto).await;
                            affected_owners.insert((dto.owner_id.clone(), "agent".to_string()));
                            ("agent_topic", topic_id, res)
                        }
                        DbWriteTask::AgentTopicBatch { topics } => {
                            let count = topics.len();
                            for (tid, dto) in &topics {
                                affected_owners.insert((dto.owner_id.clone(), "agent".to_string()));
                                affected_topics.insert(tid.clone());
                            }
                            let res = Self::upsert_agent_topic_batch_in_tx(&mut tx, topics).await;
                            ("agent_topic_batch", format!("{} items", count), res)
                        }
                        DbWriteTask::GroupTopic { topic_id, dto } => {
                            let res =
                                Self::upsert_group_topic_in_tx(&mut tx, &topic_id, &dto).await;
                            affected_owners.insert((dto.owner_id.clone(), "group".to_string()));
                            affected_topics.insert(topic_id.clone());
                            ("group_topic", topic_id, res)
                        }
                        DbWriteTask::GroupTopicBatch { topics } => {
                            let count = topics.len();
                            for (tid, dto) in &topics {
                                affected_owners.insert((dto.owner_id.clone(), "group".to_string()));
                                affected_topics.insert(tid.clone());
                            }
                            let res = Self::upsert_group_topic_batch_in_tx(&mut tx, topics).await;
                            ("group_topic_batch", format!("{} items", count), res)
                        }
                        DbWriteTask::TopicMessages {
                            topic_id,
                            messages,
                            render_bytes,
                            skip_bubble,
                        } => {
                            let count = messages.len();
                            if !skip_bubble {
                                affected_topics.insert(topic_id.clone());
                            }
                            let res = Self::upsert_topic_messages_in_tx(
                                &mut tx,
                                &topic_id,
                                messages,
                                render_bytes,
                                skip_bubble, // 传递此参数
                            )
                            .await;
                            (
                                "topic_messages",
                                format!("{} msgs in {}", count, topic_id),
                                res,
                            )
                        }
                        DbWriteTask::Flush { .. } => unreachable!(),
                    };

                    match result {
                        Ok(_) => success_count += 1,
                        Err(e) => {
                            error_count += 1;
                            println!("[DbWriteQueue] {} {} execution error: {}", task_type, id, e);
                        }
                    }
                }
                
                // ... (bubbling logic remains same) ...

                // 在同一事务末尾统一执行 Hash 冒泡，确保原子性且最小化开销
                for (owner_id, owner_type) in affected_owners {
                    if owner_type == "agent" {
                        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM agents WHERE agent_id = ? AND deleted_at IS NULL").bind(&owner_id).fetch_one(&mut *tx).await.unwrap_or(false);
                        if exists {
                            let _ = HashAggregator::bubble_agent_hash(&mut tx, &owner_id).await;
                        }
                    } else {
                        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM groups WHERE group_id = ? AND deleted_at IS NULL").bind(&owner_id).fetch_one(&mut *tx).await.unwrap_or(false);
                        if exists {
                            let _ = HashAggregator::bubble_group_hash(&mut tx, &owner_id).await;
                        }
                    }
                }
                for topic_id in affected_topics {
                    let _ = HashAggregator::bubble_from_topic(&mut tx, &topic_id).await;
                }

                // 统一提交
                if let Err(e) = tx.commit().await {
                    println!("[DbWriteQueue] Super transaction commit failed: {}", e);
                }
                // Flush 确认必须在事务落盘之后
                if let Some(tx) = flush_tx_opt {
                    let _ = tx.send(());
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

    // --- 内部事务级 Upsert 方法 ---

    async fn upsert_agent_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_config_hash(dto);

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
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_group_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_config_hash(dto);

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
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM group_members WHERE group_id = ?")
            .bind(id)
            .execute(&mut **tx)
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
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn upsert_avatar_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        owner_type: &str,
        owner_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let hash = HashAggregator::compute_avatar_hash(bytes);
        let dominant_color = extract_dominant_color_from_bytes(bytes).ok();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query(
            "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, dominant_color, updated_at) 
             VALUES (?, ?, ?, 'image/png', ?, ?, ?) 
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET 
             avatar_hash=excluded.avatar_hash, image_data=excluded.image_data, dominant_color=excluded.dominant_color, updated_at=excluded.updated_at"
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(&hash)
        .bind(bytes)
        .bind(&dominant_color)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_agent_topic_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        dto: &AgentTopicSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query(
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
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_group_topic_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        dto: &GroupTopicSyncDTO,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query(
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
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn upsert_agent_topic_batch_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topics: Vec<(String, AgentTopicSyncDTO)>,
    ) -> Result<(), String> {
        for (topic_id, dto) in &topics {
            Self::upsert_agent_topic_in_tx(tx, topic_id, dto).await?;
        }
        Ok(())
    }

    async fn upsert_group_topic_batch_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topics: Vec<(String, GroupTopicSyncDTO)>,
    ) -> Result<(), String> {
        for (topic_id, dto) in &topics {
            Self::upsert_group_topic_in_tx(tx, topic_id, dto).await?;
        }
        Ok(())
    }

    async fn upsert_topic_messages_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        messages: Vec<ChatMessage>,
        render_bytes: Vec<Vec<u8>>,
        skip_metadata_update: bool,
    ) -> Result<(), String> {
        // 将消息和渲染字节对齐
        let items: Vec<(&ChatMessage, Vec<u8>)> = messages.iter().zip(render_bytes).collect();
        MessageRepository::upsert_messages_batch(tx, topic_id, &items).await?;

        // 如果是同步期间，跳过这些昂贵的单话题 SQL 操作
        if skip_metadata_update {
            return Ok(());
        }

        // 更新 topic 元数据 (仅用于平时普通聊天消息落盘)
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("UPDATE topics SET updated_at = ? WHERE topic_id = ?")
            .bind(now)
            .bind(topic_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        let count: i32 = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_optional(&mut **tx)
        .await
        .ok()
        .flatten()
        .unwrap_or(0) as i32;

        sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?")
            .bind(count)
            .bind(topic_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}
