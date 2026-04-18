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
        render_format: &str,
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
                render_format, render_content, render_version,
                content_hash,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
             ON CONFLICT(msg_id) DO UPDATE SET
                content = excluded.content,
                role = excluded.role,
                name = excluded.name,
                is_thinking = excluded.is_thinking,
                agent_id = excluded.agent_id,
                is_group_message = excluded.is_group_message,
                group_id = excluded.group_id,
                finish_reason = excluded.finish_reason,
                render_format = excluded.render_format,
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
        .bind(message.is_thinking)
        .bind(message.is_group_message.unwrap_or(false))
        .bind(&message.group_id)
        .bind(&message.finish_reason)
        .bind(render_format)
        .bind(render_content)
        .bind(&content_hash)
        .bind(message.timestamp as i64) // created_at
        .bind(message.timestamp as i64) // updated_at
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // Handle attachments (保持现状)
        sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
            .bind(&message.id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(attachments) = &message.attachments {
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
                .bind(message.timestamp as i64)
                .bind(message.timestamp as i64)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

                sqlx::query(
                    "INSERT INTO message_attachments (
                        msg_id, hash, attachment_order, display_name, src, status, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&message.id)
                .bind(&hash)
                .bind(i as i32)
                .bind(&att.name)
                .bind(&att.src)
                .bind(&att.status)
                .bind(message.timestamp as i64)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
            }
        }

        // 3. 触发聚合哈希冒泡 (通过 HashAggregator 统一处理)
        if !skip_bubble {
            HashAggregator::bubble_from_topic(tx, topic_id).await?;
        }

        Ok(())
    }
}
