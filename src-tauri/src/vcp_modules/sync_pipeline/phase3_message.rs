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
    /// 获取所有 topic 的 content_hash（轻量，用于 Phase 2 快速筛选）
    pub async fn get_all_topic_content_hashes(
        pool: &SqlitePool,
    ) -> Result<HashMap<String, String>, String> {
        let rows =
            sqlx::query("SELECT topic_id, content_hash FROM topics WHERE deleted_at IS NULL")
                .fetch_all(pool)
                .await
                .map_err(|e| e.to_string())?;

        let mut result = HashMap::new();
        for row in rows {
            result.insert(
                row.get::<String, _>("topic_id"),
                row.get::<String, _>("content_hash"),
            );
        }
        Ok(result)
    }

    /// 批量获取指定 topic 的本地消息哈希，用于发送给桌面端计算 diff
    pub async fn get_topic_message_hashes(
        pool: &SqlitePool,
        topic_ids: &[String],
    ) -> Result<HashMap<String, TopicLocalState>, String> {
        if topic_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = topic_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");

        // 1. 批量查询所有 topic 的 content_hash
        let topic_query = format!(
            "SELECT topic_id, content_hash FROM topics WHERE topic_id IN ({}) AND deleted_at IS NULL",
            placeholders
        );
        let mut q = sqlx::query(&topic_query);
        for id in topic_ids {
            q = q.bind(id);
        }
        let topic_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        let mut result: HashMap<String, TopicLocalState> = HashMap::new();
        for row in topic_rows {
            let tid: String = row.get("topic_id");
            let hash: String = row.get("content_hash");
            result.insert(
                tid,
                TopicLocalState {
                    topic_hash: hash,
                    messages: HashMap::new(),
                },
            );
        }

        // 2. 批量查询所有消息 hash
        let msg_query = format!(
            "SELECT topic_id, msg_id, content_hash FROM messages WHERE topic_id IN ({}) AND deleted_at IS NULL",
            placeholders
        );
        let mut q = sqlx::query(&msg_query);
        for id in topic_ids {
            q = q.bind(id);
        }
        let msg_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for row in msg_rows {
            let tid: String = row.get("topic_id");
            let msg_id: String = row.get("msg_id");
            let hash: String = row.get("content_hash");
            if let Some(state) = result.get_mut(&tid) {
                state.messages.insert(msg_id, hash);
            }
        }

        Ok(result)
    }
}
