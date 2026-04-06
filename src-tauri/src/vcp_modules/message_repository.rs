use crate::vcp_modules::chat_manager::ChatMessage;
// use sqlx::Sqlite; (Removed unused import)

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
        let is_group_message = message
            .extra
            .get("isGroupMessage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let avatar_url = message.extra.get("avatarUrl").and_then(|v| v.as_str());
        let avatar_color = message.extra.get("avatarColor").and_then(|v| v.as_str());

        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, name, agent_id, content, timestamp,
                is_thinking, is_group_message, group_id, avatar_url, avatar_color,
                render_format, render_content, render_version, extra_json,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
             ON CONFLICT(msg_id) DO UPDATE SET
                content = excluded.content,
                role = excluded.role,
                name = excluded.name,
                is_thinking = excluded.is_thinking,
                render_format = excluded.render_format,
                render_content = excluded.render_content,
                extra_json = excluded.extra_json,
                updated_at = excluded.updated_at,
                deleted_at = NULL",
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

    // 已移除 clear_topic_data 和 rebuild_topic_data_state，
    // 因为全量删除再重建的逻辑在数据库架构下是不安全且非必要的。
    // 请直接使用 upsert_message 处理单条消息或批量循环处理。
}
