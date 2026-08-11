use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

const SQLITE_BIND_CHUNK: usize = 400;

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

        let mut result = HashMap::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = owner_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                "SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id IN ({}) AND deleted_at IS NULL",
                placeholders
            );
            let mut query = sqlx::query(&query_str);
            for owner_id in owner_chunk {
                query = query.bind(owner_id);
            }
            let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
            for row in rows {
                let topic_id: String = row.get("topic_id");
                if topic_id == "default" {
                    continue;
                }
                if result
                    .insert(
                        topic_id.clone(),
                        (
                            row.get::<String, _>("config_hash"),
                            row.get::<String, _>("content_hash"),
                        ),
                    )
                    .is_some()
                {
                    return Err(format!(
                        "Targeted topic hash query returned duplicate topic {topic_id}"
                    ));
                }
            }
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

        let expected = topic_ids.iter().cloned().collect::<HashSet<_>>();
        if expected.len() != topic_ids.len() || expected.iter().any(|id| id.is_empty()) {
            return Err("Topic message hash request contains empty or duplicate topic ids".into());
        }

        let mut result: HashMap<String, TopicLocalState> = HashMap::new();
        for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let topic_query = format!(
                "SELECT topic_id, content_hash FROM topics WHERE topic_id IN ({}) AND deleted_at IS NULL",
                placeholders
            );
            let mut query = sqlx::query(&topic_query);
            for id in topic_chunk {
                query = query.bind(id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let topic_id: String = row.get("topic_id");
                if result
                    .insert(
                        topic_id.clone(),
                        TopicLocalState {
                            topic_hash: row.get("content_hash"),
                            messages: HashMap::new(),
                        },
                    )
                    .is_some()
                {
                    return Err(format!(
                        "Topic message hash query returned duplicate topic {topic_id}"
                    ));
                }
            }
        }
        let actual = result.keys().cloned().collect::<HashSet<_>>();
        if actual != expected {
            let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
            missing.sort();
            return Err(format!(
                "Topic message hash query is missing live topics {missing:?}"
            ));
        }

        // 2. 批量查询所有消息 hash (包含已软删除的消息)
        for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let msg_query = format!(
                "SELECT topic_id, msg_id, content_hash, deleted_at FROM messages WHERE topic_id IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&msg_query);
            for id in topic_chunk {
                query = query.bind(id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let topic_id: String = row.get("topic_id");
                let msg_id: String = row.get("msg_id");
                let hash: String = row.get("content_hash");
                let deleted_at: Option<i64> = row.get("deleted_at");
                let state = result.get_mut(&topic_id).ok_or_else(|| {
                    format!("Message hash query returned an unknown topic {topic_id}")
                })?;
                let effective_hash = if deleted_at.is_some() {
                    "DELETED".to_string()
                } else {
                    hash
                };
                if state
                    .messages
                    .insert(msg_id.clone(), effective_hash)
                    .is_some()
                {
                    return Err(format!(
                        "Message hash query returned duplicate message {msg_id} for {topic_id}"
                    ));
                }
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::Phase3Message;

    #[tokio::test]
    async fn requested_topic_hashes_require_exact_live_coverage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             INSERT INTO topics VALUES
                ('live', 'topic-hash', NULL),
                ('deleted', 'deleted-hash', 9);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        for missing in ["missing", "deleted"] {
            let error = Phase3Message::get_topic_message_hashes(
                &pool,
                &["live".to_string(), missing.to_string()],
            )
            .await
            .expect_err("missing or tombstoned topic must fail closed");
            assert!(error.contains(missing));
        }
    }
}
