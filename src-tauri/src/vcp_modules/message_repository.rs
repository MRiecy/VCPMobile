use crate::vcp_modules::chat_manager::ChatMessage;
use sqlx::{Pool, Sqlite};

/// Internal message repository for DB operations
pub struct MessageRepository;

impl MessageRepository {
    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_format: &str,
        render_content: &[u8],
    ) -> Result<(), String> {
        let extra_json = serde_json::to_string(&message.extra).ok();
        
        let agent_id = message.extra.get("agentId").and_then(|v| v.as_str());
        let group_id = message.extra.get("groupId").and_then(|v| v.as_str());
        let is_group_message = message.extra.get("isGroupMessage").and_then(|v| v.as_bool()).unwrap_or(false);
        let avatar_url = message.extra.get("avatarUrl").and_then(|v| v.as_str());
        let avatar_color = message.extra.get("avatarColor").and_then(|v| v.as_str());

        sqlx::query(
            "INSERT OR REPLACE INTO messages (
                msg_id, topic_id, role, name, agent_id, content, timestamp,
                is_thinking, is_group_message, group_id, avatar_url, avatar_color,
                render_format, render_content, render_version, extra_json,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)"
        )
        .bind(&message.id)
        .bind(topic_id)
        .bind(&message.role)
        .bind(&message.name)
        .bind(agent_id)
        .bind(&message.content)
        .bind(message.timestamp as i64)
        .bind(message.is_thinking)
        .bind(is_group_message)
        .bind(group_id)
        .bind(avatar_url)
        .bind(avatar_color)
        .bind(render_format)
        .bind(render_content)
        .bind(extra_json)
        .bind(message.timestamp as i64) // created_at
        .bind(message.timestamp as i64) // updated_at
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // Handle attachments
        sqlx::query("DELETE FROM message_attachments WHERE msg_id = ?")
            .bind(&message.id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(attachments) = &message.attachments {
            for (i, att) in attachments.iter().enumerate() {
                if let Some(hash) = &att.hash {
                    sqlx::query(
                        "INSERT INTO message_attachments (msg_id, attachment_hash, attachment_order, created_at)
                         VALUES (?, ?, ?, ?)"
                    )
                    .bind(&message.id)
                    .bind(hash)
                    .bind(i as i32)
                    .bind(message.timestamp as i64)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        Ok(())
    }

    pub async fn clear_topic_data(
        pool: &Pool<Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        // Delete message attachments first
        sqlx::query("DELETE FROM message_attachments WHERE msg_id IN (SELECT msg_id FROM messages WHERE topic_id = ?)")
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM messages WHERE topic_id = ?")
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn rebuild_topic_data_state(
        pool: &Pool<Sqlite>,
        topic_id: &str,
        owner_type: &str,
        owner_id: &str,
        msg_count: i32,
        last_timestamp: i64,
    ) -> Result<(), String> {
        // Fully reset topic state for this topic in the new topics table
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at, revision, msg_count)
             VALUES (?, ?, ?, 'New Topic', ?, ?, 1, ?)
             ON CONFLICT(topic_id) DO UPDATE SET
                updated_at = excluded.updated_at,
                revision = revision + 1,
                msg_count = excluded.msg_count"
        )
        .bind(topic_id)
        .bind(owner_type)
        .bind(owner_id)
        .bind(last_timestamp)
        .bind(last_timestamp)
        .bind(msg_count)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}
