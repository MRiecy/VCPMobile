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
    /// V2: 获取指定 owner 下所有 topic 的 config_hash 和 content_hash
    pub async fn get_targeted_topic_hashes(
        pool: &SqlitePool,
        owners: &[String],
    ) -> Result<HashMap<String, (String, String)>, String> {
        if owners.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = owners.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let query_str = format!(
            "SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id IN ({}) AND deleted_at IS NULL",
            placeholders
        );

        let mut q = sqlx::query(&query_str);
        for owner_id in owners {
            q = q.bind(owner_id);
        }

        let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        let mut result = HashMap::new();
        for row in rows {
            let topic_id: String = row.get("topic_id");
            if topic_id == "default" {
                continue;
            }
            result.insert(
                topic_id,
                (
                    row.get::<String, _>("config_hash"),
                    row.get::<String, _>("content_hash"),
                ),
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

        // 2. 批量查询所有消息 hash (包含已软删除的消息)
        let msg_query = format!(
            "SELECT topic_id, msg_id, content_hash, deleted_at FROM messages WHERE topic_id IN ({})",
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
            let deleted_at: Option<i64> = row.get("deleted_at");
            if let Some(state) = result.get_mut(&tid) {
                if deleted_at.is_some() {
                    state.messages.insert(msg_id, "DELETED".to_string());
                } else {
                    state.messages.insert(msg_id, hash);
                }
            }
        }

        Ok(result)
    }
}
