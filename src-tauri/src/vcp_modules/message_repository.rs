use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_hash::HashAggregator;
use sha2::Digest;

/// Internal message repository for DB operations
pub struct MessageRepository;

impl MessageRepository {
    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String> {
        // 1. 计算核心内容指纹 (通过 HashAggregator)
        let attachment_hashes: Vec<String> = message
            .attachments
            .as_ref()
            .map(|atts| {
                atts.iter()
                    .map(|a| a.hash.clone().unwrap_or_default())
                    .filter(|h| !h.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let content_hash =
            HashAggregator::compute_message_fingerprint(&message.content, &attachment_hashes);

        // 2. 插入或更新消息
        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, name, agent_id, content, timestamp,
                is_thinking, is_group_message, group_id, finish_reason,
                render_content,
                content_hash,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(msg_id) DO UPDATE SET
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
        )
        .bind(&message.id)
        .bind(topic_id)
        .bind(&message.role)
        .bind(&message.name)
        .bind(&message.agent_id)
        .bind(&message.content)
        .bind(message.timestamp as i64)
        .bind(message.is_thinking.unwrap_or(false))
        .bind(message.is_group_message.unwrap_or(false))
        .bind(&message.group_id)
        .bind(&message.finish_reason)
        .bind(render_content)
        .bind(&content_hash)
        .bind(message.timestamp as i64) // created_at
        .bind(message.timestamp as i64) // updated_at
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // Handle attachments
        if let Some(ref attachments) = message.attachments {
            Self::upsert_attachments_for_message(
                tx,
                &message.id,
                message.timestamp as i64,
                attachments,
            )
            .await?;
        } else {
            sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
                .bind(&message.id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        }

        // 3. 触发聚合哈希冒泡 (通过 HashAggregator 统一处理)
        if !skip_bubble {
            HashAggregator::bubble_from_topic(tx, topic_id).await?;
        }

        Ok(())
    }

    /// 批量 upsert 消息（VALUES 批量插入），附件保持逐条处理
    pub async fn upsert_messages_batch(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        messages: &[(&ChatMessage, Vec<u8>)],
    ) -> Result<(), String> {
        const BATCH_SIZE: usize = 50;

        if messages.is_empty() {
            return Ok(());
        }

        // Step 1: 按 BATCH_SIZE 分块，批量 upsert messages 主表
        let single_values = "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        for chunk in messages.chunks(BATCH_SIZE) {
            let placeholders: Vec<&str> = std::iter::repeat_n(single_values, chunk.len()).collect();
            let values_clause = placeholders.join(", ");

            let sql = format!(
                "INSERT INTO messages (
                    msg_id, topic_id, role, name, agent_id, content, timestamp,
                    is_thinking, is_group_message, group_id, finish_reason,
                    render_content, content_hash, created_at, updated_at
                ) VALUES {}
                ON CONFLICT(msg_id) DO UPDATE SET
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
                values_clause
            );

            let mut q = sqlx::query(&sql);
            for (msg, render_bytes) in chunk {
                let attachment_hashes: Vec<String> = msg
                    .attachments
                    .as_ref()
                    .map(|atts| {
                        atts.iter()
                            .map(|a| a.hash.clone().unwrap_or_default())
                            .filter(|h| !h.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();

                let content_hash =
                    HashAggregator::compute_message_fingerprint(&msg.content, &attachment_hashes);

                q = q
                    .bind(&msg.id)
                    .bind(topic_id)
                    .bind(&msg.role)
                    .bind(&msg.name)
                    .bind(&msg.agent_id)
                    .bind(&msg.content)
                    .bind(msg.timestamp as i64)
                    .bind(msg.is_thinking.unwrap_or(false))
                    .bind(msg.is_group_message.unwrap_or(false))
                    .bind(&msg.group_id)
                    .bind(&msg.finish_reason)
                    .bind(render_bytes.as_slice())
                    .bind(content_hash)
                    .bind(msg.timestamp as i64)
                    .bind(msg.timestamp as i64);
            }
            q.execute(&mut **tx).await.map_err(|e| e.to_string())?;
        }

        // Step 2: 逐条处理附件（数量级通常远低于消息数）
        for (msg, _) in messages {
            if let Some(ref attachments) = msg.attachments {
                Self::upsert_attachments_for_message(
                    tx,
                    &msg.id,
                    msg.timestamp as i64,
                    attachments,
                )
                .await?;
            } else {
                sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
                    .bind(&msg.id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    async fn upsert_attachments_for_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        msg_id: &str,
        timestamp: i64,
        attachments: &[crate::vcp_modules::chat_manager::Attachment],
    ) -> Result<(), String> {
        sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
            .bind(msg_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        for (i, att) in attachments.iter().enumerate() {
            let hash = att.hash.clone().unwrap_or_else(|| {
                let mut hasher = sha2::Sha256::new();
                sha2::Digest::update(&mut hasher, att.src.as_bytes());
                format!("{:x}", sha2::Digest::finalize(hasher))
            });

            let image_frames = att
                .image_frames
                .as_ref()
                .and_then(|frames| serde_json::to_string(frames).ok());

            sqlx::query(
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
                    updated_at = excluded.updated_at"
            )
            .bind(&hash)
            .bind(&att.r#type)
            .bind(att.size as i64)
            .bind(&att.internal_path)
            .bind(&att.extracted_text)
            .bind(image_frames)
            .bind(&att.thumbnail_path)
            .bind(timestamp)
            .bind(timestamp)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

            sqlx::query(
                "INSERT INTO message_attachments (
                    msg_id, hash, attachment_order, display_name, src, status, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(msg_id)
            .bind(&hash)
            .bind(i as i32)
            .bind(&att.name)
            .bind(&att.src)
            .bind(&att.status)
            .bind(timestamp)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        Ok(())
    }
}
