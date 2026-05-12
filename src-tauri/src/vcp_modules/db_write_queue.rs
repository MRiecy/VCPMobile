use crate::vcp_modules::avatar_service::extract_dominant_color_from_bytes;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::SyncLogger;
use sha2::Digest;
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
        content_hashes: Vec<String>,
        skip_bubble: bool,
    },
    Flush {
        tx: oneshot::Sender<()>,
    },
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
    pub fn new(_pool: sqlx::SqlitePool, db_path: std::path::PathBuf) -> Self {
        let (tx, mut rx) = mpsc::channel(256);
        let db_path_for_worker = db_path.clone();

        let worker = tokio::spawn(async move {
            println!("[DbWriteQueue] Worker started (Turbo rusqlite Mode)");

            let mut success_count = 0u32;
            let mut error_count = 0u32;

            while let Some(first_task) = rx.recv().await {
                // 如果第一个任务就是 Flush，直接确认
                if let DbWriteTask::Flush { tx } = first_task {
                    let _ = tx.send(());
                    continue;
                }

                let mut tasks_in_this_tx = vec![first_task];
                let mut total_msg_count = 0u32;
                
                if let DbWriteTask::TopicMessages { messages, .. } = &tasks_in_this_tx[0] {
                    total_msg_count += messages.len() as u32;
                }

                let mut flush_tx_opt: Option<oneshot::Sender<()>> = None;

                while tasks_in_this_tx.len() < 200 && total_msg_count < 5000 {
                    let next_res = tokio::time::timeout(
                        std::time::Duration::from_millis(50),
                        rx.recv()
                    ).await;

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
                // [Turbo Phase 3] 使用 spawn_blocking + rusqlite 进行极致写入
                let result = tokio::task::spawn_blocking(move || {
                    let mut conn = rusqlite::Connection::open(&db_path)?;
                    
                    // 极致性能调优
                    conn.execute("PRAGMA journal_mode = WAL", [])?;
                    conn.execute("PRAGMA synchronous = NORMAL", [])?;
                    conn.execute("PRAGMA busy_timeout = 30000", [])?;

                    let tx = conn.transaction()?;
                    
                    let mut affected_owners = HashSet::new();
                    let mut affected_topics = HashSet::new();

                    for task in tasks_in_this_tx {
                        match task {
                            DbWriteTask::Agent { id, dto } => {
                                Self::rusqlite_upsert_agent(&tx, &id, &dto)?;
                                affected_owners.insert((id, "agent".to_string()));
                            }
                            DbWriteTask::Group { id, dto } => {
                                Self::rusqlite_upsert_group(&tx, &id, &dto)?;
                                affected_owners.insert((id, "group".to_string()));
                            }
                            DbWriteTask::Avatar { owner_type, owner_id, bytes } => {
                                Self::rusqlite_upsert_avatar(&tx, &owner_type, &owner_id, &bytes)?;
                            }
                            DbWriteTask::AgentTopic { topic_id, dto } => {
                                Self::rusqlite_upsert_agent_topic(&tx, &topic_id, &dto)?;
                                affected_owners.insert((dto.owner_id, "agent".to_string()));
                            }
                            DbWriteTask::AgentTopicBatch { topics } => {
                                for (tid, dto) in topics {
                                    affected_owners.insert((dto.owner_id.clone(), "agent".to_string()));
                                    Self::rusqlite_upsert_agent_topic(&tx, &tid, &dto)?;
                                }
                            }
                            DbWriteTask::GroupTopic { topic_id, dto } => {
                                Self::rusqlite_upsert_group_topic(&tx, &topic_id, &dto)?;
                                affected_owners.insert((dto.owner_id, "group".to_string()));
                            }
                            DbWriteTask::GroupTopicBatch { topics } => {
                                for (tid, dto) in topics {
                                    affected_owners.insert((dto.owner_id.clone(), "group".to_string()));
                                    Self::rusqlite_upsert_group_topic(&tx, &tid, &dto)?;
                                }
                            }
                            DbWriteTask::TopicMessages { topic_id, messages, render_bytes, content_hashes, skip_bubble } => {
                                if !skip_bubble {
                                    affected_topics.insert(topic_id.clone());
                                }
                                Self::rusqlite_upsert_messages_batch(&tx, &topic_id, messages, render_bytes, content_hashes)?;
                            }
                            DbWriteTask::Flush { .. } => unreachable!(),
                        }
                    }

                    // [Phase 5] 统一冒泡：分层去重，批量校验存在，确保最小化开销
                    for topic_id in affected_topics {
                        Self::rusqlite_bubble_topic_hash(&tx, &topic_id)?;
                    }

                    // 批量提取 Owner 并去重校验
                    let mut unique_agents = HashSet::new();
                    let mut unique_groups = HashSet::new();
                    for (id, owner_type) in affected_owners {
                        if owner_type == "agent" {
                            unique_agents.insert(id);
                        } else if owner_type == "group" {
                            unique_groups.insert(id);
                        }
                    }

                    if !unique_agents.is_empty() {
                        let placeholders = vec!["?"; unique_agents.len()].join(",");
                        let sql = format!("SELECT agent_id FROM agents WHERE agent_id IN ({}) AND deleted_at IS NULL", placeholders);
                        let mut stmt = tx.prepare(&sql)?;
                        let valid_ids: Vec<String> = stmt.query_map(rusqlite::params_from_iter(unique_agents.iter()), |r| r.get(0))?
                            .filter_map(|r| r.ok()).collect();
                        for aid in valid_ids {
                            Self::rusqlite_bubble_agent_hash(&tx, &aid)?;
                        }
                    }

                    if !unique_groups.is_empty() {
                        let placeholders = vec!["?"; unique_groups.len()].join(",");
                        let sql = format!("SELECT group_id FROM groups WHERE group_id IN ({}) AND deleted_at IS NULL", placeholders);
                        let mut stmt = tx.prepare(&sql)?;
                        let valid_ids: Vec<String> = stmt.query_map(rusqlite::params_from_iter(unique_groups.iter()), |r| r.get(0))?
                            .filter_map(|r| r.ok()).collect();
                        for gid in valid_ids {
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
                        println!("[DbWriteQueue] rusqlite execution error: {}", e);
                    }
                    Err(e) => {
                        error_count += 1;
                        println!("[DbWriteQueue] spawn_blocking error: {}", e);
                    }
                }

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
            db_path,
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

    // --- rusqlite 事务级方法 ---

    fn rusqlite_upsert_agent(
        tx: &rusqlite::Transaction,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_config_hash(dto);

        tx.execute(
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
            rusqlite::params![
                id, &dto.name, &dto.system_prompt, &dto.model,
                dto.temperature, dto.context_token_limit, dto.max_output_tokens,
                if dto.stream_output { 1 } else { 0 },
                &config_hash, now
            ],
        )?;

        Ok(())
    }

    fn rusqlite_upsert_group(
        tx: &rusqlite::Transaction,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_config_hash(dto);

        tx.execute(
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
            rusqlite::params![
                id, &dto.name, &dto.mode, &dto.group_prompt, &dto.invite_prompt,
                if dto.use_unified_model { 1 } else { 0 },
                &dto.unified_model, &dto.tag_match_mode, dto.created_at,
                &config_hash, now
            ],
        )?;

        tx.execute("DELETE FROM group_members WHERE group_id = ?", [id])?;

        let member_tags = dto.member_tags.as_ref().and_then(|v| v.as_object());

        for member in &dto.members {
            let tag = member_tags
                .and_then(|m| m.get(member))
                .and_then(|v| v.as_str());
            tx.execute(
                "INSERT INTO group_members (group_id, agent_id, member_tag, sort_order, updated_at) VALUES (?, ?, ?, 0, ?)",
                rusqlite::params![id, member, tag, now]
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
        let dominant_color = extract_dominant_color_from_bytes(bytes).ok();
        let now = chrono::Utc::now().timestamp_millis();

        tx.execute(
            "INSERT INTO avatars (owner_type, owner_id, avatar_hash, mime_type, image_data, dominant_color, updated_at) 
             VALUES (?, ?, ?, 'image/png', ?, ?, ?) 
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET 
             avatar_hash=excluded.avatar_hash, image_data=excluded.image_data, dominant_color=excluded.dominant_color, updated_at=excluded.updated_at",
            rusqlite::params![owner_type, owner_id, &hash, bytes, &dominant_color, now]
        )?;

        Ok(())
    }

    fn rusqlite_upsert_agent_topic(
        tx: &rusqlite::Transaction,
        topic_id: &str,
        dto: &AgentTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().timestamp_millis();

        tx.execute(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
            VALUES (?, ?, ?, 'agent', ?, ?, ?, ?)
            ON CONFLICT(topic_id) DO UPDATE SET
            title=excluded.title, locked=excluded.locked, unread=excluded.unread, updated_at=excluded.updated_at",
            rusqlite::params![
                topic_id, &dto.name, &dto.owner_id, dto.created_at,
                if dto.locked { 1 } else { 0 },
                if dto.unread { 1 } else { 0 },
                now
            ]
        )?;

        Ok(())
    }

    fn rusqlite_upsert_group_topic(
        tx: &rusqlite::Transaction,
        topic_id: &str,
        dto: &GroupTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        let now = chrono::Utc::now().timestamp_millis();

        tx.execute(
            "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
            VALUES (?, ?, ?, 'group', ?, 1, 0, ?)
            ON CONFLICT(topic_id) DO UPDATE SET
            title=excluded.title, updated_at=excluded.updated_at",
            rusqlite::params![topic_id, &dto.name, &dto.owner_id, dto.created_at, now]
        )?;

        Ok(())
    }

    fn rusqlite_upsert_messages_batch(
        tx: &rusqlite::Transaction,
        topic_id: &str,
        messages: Vec<ChatMessage>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
    ) -> rusqlite::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }

        // Phase 3: Turbo Mode - Chunked Bulk Insert
        const MAX_PARAMS: usize = 999;
        const PARAMS_PER_MSG: usize = 15;
        let chunk_size = MAX_PARAMS / PARAMS_PER_MSG;

        for chunk_indices in messages.iter().enumerate().collect::<Vec<_>>().chunks(chunk_size) {
            let mut sql = String::from(
                "INSERT INTO messages (
                    msg_id, topic_id, role, name, agent_id, content, timestamp,
                    is_thinking, is_group_message, group_id, finish_reason,
                    render_content, content_hash, created_at, updated_at
                ) VALUES ",
            );

            for i in 0..chunk_indices.len() {
                if i > 0 { sql.push_str(", "); }
                sql.push_str("(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)");
            }

            sql.push_str(
                " ON CONFLICT(msg_id) DO UPDATE SET
                    content = excluded.content,
                    role = excluded.role,
                    name = excluded.name,
                    is_thinking = excluded.is_thinking,
                    agent_id = excluded.agent_id,
                    is_group_message = excluded.is_group_message,
                    group_id = excluded.group_id,
                    finish_reason = excluded.finish_reason,
                    render_content = excluded.render_content,
                    content_hash = excluded.content_hash,
                    updated_at = excluded.updated_at,
                    deleted_at = NULL",
            );

            let mut stmt = tx.prepare_cached(&sql)?;
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            for (idx, msg) in chunk_indices {
                params.push(Box::new(msg.id.clone()));
                params.push(Box::new(topic_id.to_string()));
                params.push(Box::new(msg.role.clone()));
                params.push(Box::new(msg.name.clone()));
                params.push(Box::new(msg.agent_id.clone()));
                params.push(Box::new(msg.content.clone()));
                params.push(Box::new(msg.timestamp as i64));
                params.push(Box::new(msg.is_thinking.unwrap_or(false)));
                params.push(Box::new(msg.is_group_message.unwrap_or(false)));
                params.push(Box::new(msg.group_id.clone()));
                params.push(Box::new(msg.finish_reason.clone()));
                params.push(Box::new(render_bytes[*idx].clone()));
                params.push(Box::new(content_hashes[*idx].clone()));
                params.push(Box::new(msg.timestamp as i64));
                params.push(Box::new(msg.timestamp as i64));
            }
            
            let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            stmt.execute(&*params_refs)?;
        }

        // Phase 4: Attachment Optimization
        let mut msg_ids = Vec::new();
        let mut all_relations = Vec::new();

        for (_idx, msg) in messages.iter().enumerate() {
            msg_ids.push(msg.id.clone());
            if let Some(ref attachments) = msg.attachments {
                for (i, att) in attachments.iter().enumerate() {
                    let hash = att.hash.clone().unwrap_or_else(|| {
                        let mut hasher = sha2::Sha256::new();
                        hasher.update(att.src.as_bytes());
                        format!("{:x}", hasher.finalize())
                    });

                    Self::rusqlite_upsert_attachment_core(tx, &hash, att, msg.timestamp as i64)?;

                    all_relations.push((
                        msg.id.clone(),
                        hash,
                        i as i32,
                        att.name.clone(),
                        att.src.clone(),
                        att.status.clone().unwrap_or_else(|| "ready".to_string()),
                        msg.timestamp as i64
                    ));
                }
            }
        }

        // Chunked Delete
        for chunk in msg_ids.chunks(999) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!("DELETE FROM message_attachments WHERE msg_id IN ({})", placeholders);
            let mut stmt = tx.prepare_cached(&sql)?;
            let params_refs: Vec<&dyn rusqlite::ToSql> = chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            stmt.execute(&*params_refs)?;
        }

        // Chunked Relation Insert
        if !all_relations.is_empty() {
            const PARAMS_PER_REL: usize = 7;
            let rel_chunk_size = MAX_PARAMS / PARAMS_PER_REL;
            for chunk in all_relations.chunks(rel_chunk_size) {
                let mut sql = String::from("INSERT INTO message_attachments (
                    msg_id, hash, attachment_order, display_name, src, status, created_at
                ) VALUES ");
                for i in 0..chunk.len() {
                    if i > 0 { sql.push_str(", "); }
                    sql.push_str("(?, ?, ?, ?, ?, ?, ?)");
                }
                let mut stmt = tx.prepare_cached(&sql)?;
                let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                for rel in chunk {
                    params.push(Box::new(rel.0.clone()));
                    params.push(Box::new(rel.1.clone()));
                    params.push(Box::new(rel.2));
                    params.push(Box::new(rel.3.clone()));
                    params.push(Box::new(rel.4.clone()));
                    params.push(Box::new(rel.5.clone()));
                    params.push(Box::new(rel.6));
                }
                let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
                stmt.execute(&*params_refs)?;
            }
        }

        Ok(())
    }

    fn rusqlite_bubble_topic_hash(
        tx: &rusqlite::Transaction,
        topic_id: &str,
    ) -> rusqlite::Result<()> {
        // 1. 计算 content_hash (消息聚合)
        let mut stmt = tx.prepare("SELECT content_hash FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp ASC, msg_id ASC")?;
        let hashes: Vec<String> = stmt.query_map([topic_id], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);

        // 2. 计算 config_hash (元数据)
        let owner_type: String = tx.query_row("SELECT owner_type FROM topics WHERE topic_id = ?", [topic_id], |r| r.get(0))?;
        
        let config_hash = if owner_type == "agent" {
            let dto = Self::rusqlite_load_agent_topic_dto(tx, topic_id)?;
            HashAggregator::compute_agent_topic_metadata_hash(&dto)
        } else {
            let dto = Self::rusqlite_load_group_topic_dto(tx, topic_id)?;
            HashAggregator::compute_group_topic_metadata_hash(&dto)
        };

        tx.execute("UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ?", rusqlite::params![root_hash, config_hash, topic_id])?;
        Ok(())
    }

    fn rusqlite_bubble_agent_hash(
        tx: &rusqlite::Transaction,
        agent_id: &str,
    ) -> rusqlite::Result<()> {
        let mut stmt = tx.prepare("SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL ORDER BY topic_id ASC")?;
        let mut rows = stmt.query([agent_id])?;
        let mut hashes = Vec::new();
        while let Some(row) = rows.next()? {
            hashes.push(row.get::<_, String>(0)?);
            hashes.push(row.get::<_, String>(1)?);
        }
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);
        tx.execute("UPDATE agents SET content_hash = ? WHERE agent_id = ?", [root_hash, agent_id.to_string()])?;
        Ok(())
    }

    fn rusqlite_bubble_group_hash(
        tx: &rusqlite::Transaction,
        group_id: &str,
    ) -> rusqlite::Result<()> {
        let mut stmt = tx.prepare("SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL ORDER BY topic_id ASC")?;
        let mut rows = stmt.query([group_id])?;
        let mut hashes = Vec::new();
        while let Some(row) = rows.next()? {
            hashes.push(row.get::<_, String>(0)?);
            hashes.push(row.get::<_, String>(1)?);
        }
        let root_hash = crate::vcp_modules::sync_types::compute_merkle_root(hashes);
        tx.execute("UPDATE groups SET content_hash = ? WHERE group_id = ?", [root_hash, group_id.to_string()])?;
        Ok(())
    }

    fn rusqlite_load_agent_topic_dto(
        tx: &rusqlite::Transaction,
        topic_id: &str,
    ) -> rusqlite::Result<AgentTopicSyncDTO> {
        tx.query_row(
            "SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics WHERE topic_id = ?",
            [topic_id],
            |row| Ok(AgentTopicSyncDTO {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                locked: row.get::<_, i64>(3)? != 0,
                unread: row.get::<_, i64>(4)? != 0,
                owner_id: row.get(5)?,
            })
        )
    }

    fn rusqlite_load_group_topic_dto(
        tx: &rusqlite::Transaction,
        topic_id: &str,
    ) -> rusqlite::Result<GroupTopicSyncDTO> {
        tx.query_row(
            "SELECT topic_id, title, created_at, owner_id FROM topics WHERE topic_id = ?",
            [topic_id],
            |row| Ok(GroupTopicSyncDTO {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                owner_id: row.get(3)?,
            })
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
                internal_path = excluded.internal_path,
                extracted_text = excluded.extracted_text,
                image_frames = excluded.image_frames,
                thumbnail_path = excluded.thumbnail_path,
                updated_at = excluded.updated_at",
            rusqlite::params![
                hash, &att.r#type, att.size as i64, &att.internal_path,
                &att.extracted_text, image_frames, &att.thumbnail_path,
                timestamp, timestamp
            ]
        )?;

        Ok(())
    }
}
