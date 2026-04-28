use crate::vcp_modules::sync_pipeline::phase2_topic::Phase2Topic;
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashMap;

pub struct Phase3Message;

#[derive(Debug)]
pub struct TopicLocalState {
    pub topic_hash: String,
    pub messages: HashMap<String, String>,
}

impl Phase3Message {
    pub async fn get_all_active_topic_ids(pool: &SqlitePool) -> Result<Vec<String>, String> {
        Phase2Topic::get_all_topic_ids(pool).await
    }

    /// 批量获取所有 topic 的本地消息哈希，用于发送给桌面端计算 diff
    pub async fn get_all_topic_message_hashes(
        pool: &SqlitePool,
    ) -> Result<HashMap<String, TopicLocalState>, String> {
        let topic_rows =
            sqlx::query("SELECT topic_id, content_hash FROM topics WHERE deleted_at IS NULL")
                .fetch_all(pool)
                .await
                .map_err(|e| e.to_string())?;

        let mut result = HashMap::new();
        for row in topic_rows {
            let topic_id: String = row.get("topic_id");
            let topic_hash: String = row.get("content_hash");

            let msg_rows = sqlx::query(
                "SELECT msg_id, content_hash FROM messages WHERE topic_id = ? AND deleted_at IS NULL"
            )
            .bind(&topic_id)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;

            let mut messages = HashMap::new();
            for r in msg_rows {
                messages.insert(
                    r.get::<String, _>("msg_id"),
                    r.get::<String, _>("content_hash"),
                );
            }

            result.insert(
                topic_id,
                TopicLocalState {
                    topic_hash,
                    messages,
                },
            );
        }

        Ok(result)
    }
}
