use crate::vcp_modules::chat_manager::ChatMessage;
use sqlx::{Pool, Sqlite};

/// Internal message repository for DB operations
pub struct MessageRepositoryDb;

impl MessageRepositoryDb {
    pub async fn upsert_message_index(
        pool: &Pool<Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        item_id: &str,
        raw_offset: u64,
        raw_length: u64,
        render_offset: u64,
        render_length: u64,
    ) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        Self::upsert_message_index_tx(
            &mut tx,
            message,
            topic_id,
            item_id,
            raw_offset,
            raw_length,
            render_offset,
            render_length,
        ).await?;
        tx.commit().await.map_err(|e| e.to_string())
    }

    pub async fn upsert_message_index_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        item_id: &str,
        raw_offset: u64,
        raw_length: u64,
        render_offset: u64,
        render_length: u64,
    ) -> Result<(), String> {
        let extra_json = serde_json::to_string(&message.extra).ok();
        let has_attachments = if message.attachments.as_ref().map_or(true, |a| a.is_empty()) { 0 } else { 1 };

        sqlx::query(
            "INSERT OR REPLACE INTO message_index (
                msg_id, topic_id, item_id, role, created_at,
                raw_byte_offset, raw_byte_length, render_byte_offset, render_byte_length,
                has_attachments, is_deleted, extra_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?)"
        )
        .bind(&message.id)
        .bind(topic_id)
        .bind(item_id)
        .bind(&message.role)
        .bind(message.timestamp as i64)
        .bind(raw_offset as i64)
        .bind(raw_length as i64)
        .bind(render_offset as i64)
        .bind(render_length as i64)
        .bind(has_attachments)
        .bind(extra_json)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        // Handle attachments - clear old refs for this message first to ensure consistency on edit/re-import
        sqlx::query("DELETE FROM message_attachment_ref WHERE msg_id = ?")
            .bind(&message.id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(attachments) = &message.attachments {
            for (i, att) in attachments.iter().enumerate() {
                if let Some(hash) = &att.hash {
                    sqlx::query(
                        "INSERT INTO message_attachment_ref (msg_id, attachment_hash, attachment_order)
                         VALUES (?, ?, ?)"
                    )
                    .bind(&message.id)
                    .bind(hash)
                    .bind(i as i32)
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

        // Delete message indices (this should cascade if FKs were set, but let's be explicit)
        sqlx::query("DELETE FROM message_attachment_ref WHERE msg_id IN (SELECT msg_id FROM message_index WHERE topic_id = ?)")
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM message_index WHERE topic_id = ?")
            .bind(topic_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn update_topic_state_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
        item_id: &str,
        timestamp: i64,
    ) -> Result<(), String> {
        // Increment revision and msg_count
        sqlx::query(
            "INSERT INTO topic_state (topic_id, item_id, title, created_at, updated_at, revision, msg_count)
             VALUES (?, ?, 'New Topic', ?, ?, 1, 1)
             ON CONFLICT(topic_id) DO UPDATE SET
                updated_at = excluded.updated_at,
                revision = revision + 1,
                msg_count = msg_count + 1"
        )
        .bind(topic_id)
        .bind(item_id)
        .bind(timestamp)
        .bind(timestamp)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn rebuild_topic_state(
        pool: &Pool<Sqlite>,
        topic_id: &str,
        item_id: &str,
        msg_count: i32,
        last_timestamp: i64,
    ) -> Result<(), String> {
        // Fully reset topic state for this topic
        sqlx::query(
            "INSERT INTO topic_state (topic_id, item_id, title, created_at, updated_at, revision, msg_count)
             VALUES (?, ?, 'New Topic', ?, ?, 1, ?)
             ON CONFLICT(topic_id) DO UPDATE SET
                updated_at = excluded.updated_at,
                revision = revision + 1,
                msg_count = excluded.msg_count"
        )
        .bind(topic_id)
        .bind(item_id)
        .bind(last_timestamp)
        .bind(last_timestamp)
        .bind(msg_count)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(())
    }
}
