use futures_util::TryStreamExt;
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

const SQLITE_BIND_CHUNK: usize = 400;
const MAX_PHASE3_MESSAGES_PER_TOPIC: usize = 10_000;
const MAX_PHASE3_MESSAGES: usize = 100_000;
const MAX_PHASE3_STATE_BYTES: usize = 64 * 1024 * 1024;

pub struct Phase3Message;

#[derive(Debug)]
pub struct TargetedTopicHashState {
    pub owner_type: String,
    pub owner_id: String,
    pub config_hash: String,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct TopicLocalState {
    pub owner_type: String,
    pub owner_id: String,
    pub topic_hash: String,
    pub messages: HashMap<String, MessageVersionState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageVersionState {
    pub hash: String,
    pub updated_at: i64,
}

#[derive(Default)]
struct Phase3StateBudget {
    messages: usize,
    bytes: usize,
}

impl Phase3StateBudget {
    fn observe_topic(
        &mut self,
        topic_id: &str,
        messages: usize,
        raw_bytes: usize,
    ) -> Result<(), String> {
        if messages > MAX_PHASE3_MESSAGES_PER_TOPIC {
            return Err(format!(
                "Phase 3 topic {topic_id} exceeds the {MAX_PHASE3_MESSAGES_PER_TOPIC}-message limit"
            ));
        }
        self.messages = self
            .messages
            .checked_add(messages)
            .ok_or_else(|| "Phase 3 message count overflow".to_string())?;
        if self.messages > MAX_PHASE3_MESSAGES {
            return Err(format!(
                "Phase 3 state exceeds the {MAX_PHASE3_MESSAGES}-message limit"
            ));
        }
        self.bytes = self
            .bytes
            .checked_add(raw_bytes)
            .ok_or_else(|| "Phase 3 state size overflow".to_string())?;
        if self.bytes > MAX_PHASE3_STATE_BYTES {
            return Err("Phase 3 state exceeds the 64 MiB memory budget".to_string());
        }
        Ok(())
    }
}

impl Phase3Message {
    /// V2: 获取指定 owner 下所有 topic 的 config_hash 和 content_hash
    pub async fn get_targeted_topic_hashes(
        pool: &SqlitePool,
        owners: &[String],
    ) -> Result<HashMap<String, TargetedTopicHashState>, String> {
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
                "SELECT topic_id, owner_type, owner_id, config_hash, content_hash
                 FROM topics WHERE owner_id IN ({}) AND deleted_at IS NULL",
                placeholders
            );
            let mut query = sqlx::query(&query_str);
            for owner_id in owner_chunk {
                query = query.bind(owner_id);
            }
            let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
            for row in rows {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Targeted topic id decode failed: {error}"))?;
                if topic_id == "default" {
                    continue;
                }
                let config_hash: String = row.try_get("config_hash").map_err(|error| {
                    format!("Targeted topic {topic_id} config hash decode failed: {error}")
                })?;
                let content_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Targeted topic {topic_id} content hash decode failed: {error}")
                })?;
                let owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Targeted topic {topic_id} owner type decode failed: {error}")
                })?;
                let owner_id: String = row.try_get("owner_id").map_err(|error| {
                    format!("Targeted topic {topic_id} owner id decode failed: {error}")
                })?;
                if !matches!(owner_type.as_str(), "agent" | "group")
                    || owner_id.is_empty()
                    || !owners.contains(&owner_id)
                {
                    return Err(format!(
                        "Targeted topic {topic_id} has invalid owner identity"
                    ));
                }
                if result
                    .insert(
                        topic_id.clone(),
                        TargetedTopicHashState {
                            owner_type,
                            owner_id,
                            config_hash,
                            content_hash,
                        },
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
                "SELECT topic_id, owner_type, owner_id, content_hash
                 FROM topics WHERE topic_id IN ({}) AND deleted_at IS NULL",
                placeholders
            );
            let mut query = sqlx::query(&topic_query);
            for id in topic_chunk {
                query = query.bind(id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Topic hash id decode failed: {error}"))?;
                let topic_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Topic {topic_id} content hash decode failed: {error}")
                })?;
                let owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Topic {topic_id} owner type decode failed: {error}")
                })?;
                let owner_id: String = row
                    .try_get("owner_id")
                    .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
                if !matches!(owner_type.as_str(), "agent" | "group") || owner_id.is_empty() {
                    return Err(format!("Topic {topic_id} has invalid owner identity"));
                }
                if result
                    .insert(
                        topic_id.clone(),
                        TopicLocalState {
                            owner_type,
                            owner_id,
                            topic_hash,
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

        // Bound the state before loading message IDs/hashes into memory. SQLite LENGTH over BLOB
        // values counts UTF-8 bytes rather than characters, matching the wire budget.
        let mut budget = Phase3StateBudget::default();
        for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let count_query = format!(
                "SELECT topic_id, COUNT(*) AS message_count,
                        COALESCE(SUM(
                            LENGTH(CAST(msg_id AS BLOB)) +
                            LENGTH(CAST(content_hash AS BLOB)) + 48
                        ), 0) AS state_bytes
                 FROM messages
                 WHERE topic_id IN ({placeholders})
                 GROUP BY topic_id"
            );
            let mut query = sqlx::query(&count_query);
            for id in topic_chunk {
                query = query.bind(id);
            }
            for row in query
                .fetch_all(pool)
                .await
                .map_err(|error| format!("Phase 3 message budget query failed: {error}"))?
            {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Phase 3 budget topic id decode failed: {error}"))?;
                let message_count: i64 = row.try_get("message_count").map_err(|error| {
                    format!("Phase 3 message count decode failed for {topic_id}: {error}")
                })?;
                let message_count = usize::try_from(message_count)
                    .map_err(|_| format!("Phase 3 message count is invalid for {topic_id}"))?;
                let state_bytes: i64 = row.try_get("state_bytes").map_err(|error| {
                    format!("Phase 3 state size decode failed for {topic_id}: {error}")
                })?;
                let state_bytes = usize::try_from(state_bytes)
                    .map_err(|_| format!("Phase 3 state size is invalid for {topic_id}"))?;
                budget.observe_topic(&topic_id, message_count, state_bytes)?;
            }
        }

        // 2. 批量查询所有消息 hash (包含已软删除的消息)
        for topic_chunk in topic_ids.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let msg_query = format!(
                "SELECT topic_id, msg_id, content_hash, updated_at, deleted_at FROM messages WHERE topic_id IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&msg_query);
            for id in topic_chunk {
                query = query.bind(id);
            }
            let mut rows = query.fetch(pool);
            while let Some(row) = rows
                .try_next()
                .await
                .map_err(|error| format!("Phase 3 message hash query failed: {error}"))?
            {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Message hash topic id decode failed: {error}"))?;
                let msg_id: String = row
                    .try_get("msg_id")
                    .map_err(|error| format!("Message id decode failed for {topic_id}: {error}"))?;
                let hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Message hash decode failed for {topic_id}/{msg_id}: {error}")
                })?;
                let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(|error| {
                    format!("Message tombstone decode failed for {topic_id}/{msg_id}: {error}")
                })?;
                let updated_at: i64 = row.try_get("updated_at").map_err(|error| {
                    format!("Message update time decode failed for {topic_id}/{msg_id}: {error}")
                })?;
                let state = result.get_mut(&topic_id).ok_or_else(|| {
                    format!("Message hash query returned an unknown topic {topic_id}")
                })?;
                let (effective_hash, effective_updated_at) = if let Some(deleted_at) = deleted_at {
                    ("DELETED".to_string(), deleted_at)
                } else {
                    (hash, updated_at)
                };
                if !(0..=(1_i64 << 53) - 1).contains(&effective_updated_at) {
                    return Err(format!(
                        "Message update time is invalid for {topic_id}/{msg_id}"
                    ));
                }
                if state
                    .messages
                    .insert(
                        msg_id.clone(),
                        MessageVersionState {
                            hash: effective_hash,
                            updated_at: effective_updated_at,
                        },
                    )
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
    use super::{
        MessageVersionState, Phase3Message, Phase3StateBudget, MAX_PHASE3_MESSAGES_PER_TOPIC,
    };

    #[test]
    fn phase3_state_budget_rejects_an_oversized_single_topic_before_loading_rows() {
        let mut budget = Phase3StateBudget::default();
        let error = budget
            .observe_topic("topic", MAX_PHASE3_MESSAGES_PER_TOPIC + 1, 1)
            .expect_err("oversized topic must fail before hash materialization");
        assert!(error.contains("topic"));
        assert!(error.contains("message limit"));
    }

    #[tokio::test]
    async fn requested_topic_hashes_require_exact_live_coverage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_type TEXT, owner_id TEXT,
                content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             INSERT INTO topics VALUES
                ('live', 'agent', 'agent-a', 'topic-hash', NULL),
                ('deleted', 'agent', 'agent-a', 'deleted-hash', 9);",
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

    #[tokio::test]
    async fn message_states_use_live_update_time_and_tombstone_time() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_type TEXT, owner_id TEXT,
                content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             INSERT INTO topics VALUES ('topic', 'agent', 'agent-a', 'topic-hash', NULL);
             INSERT INTO messages VALUES
                ('topic', 'live', 'live-hash', 123, NULL),
                ('topic', 'deleted', 'old-hash', 50, 456);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        let states = Phase3Message::get_topic_message_hashes(&pool, &["topic".to_string()])
            .await
            .expect("load message states");
        assert_eq!(
            states["topic"].messages["live"],
            MessageVersionState {
                hash: "live-hash".to_string(),
                updated_at: 123,
            }
        );
        assert_eq!(
            states["topic"].messages["deleted"],
            MessageVersionState {
                hash: "DELETED".to_string(),
                updated_at: 456,
            }
        );
    }
}
