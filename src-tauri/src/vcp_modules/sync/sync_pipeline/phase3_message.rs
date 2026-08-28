use crate::vcp_modules::sync_types::{
    MessageDeletedState, MessageLiveState, MessageVersionState, OwnerType,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use futures_util::TryStreamExt;
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap, HashSet};

const SQLITE_BIND_CHUNK: usize = 400;
const MAX_PHASE3_MESSAGES_PER_TOPIC: usize = 10_000;
const MAX_PHASE3_MESSAGES: usize = 100_000;
const MAX_PHASE3_STATE_BYTES: usize = 64 * 1024 * 1024;

pub struct Phase3Message;

#[derive(Debug)]
pub struct TargetedTopicHashState {
    pub config_hash: String,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct TopicLocalState {
    pub content_hash: String,
    pub messages: BTreeMap<String, MessageVersionState>,
}

#[derive(Default)]
struct Phase3StateBudget {
    messages: usize,
    bytes: usize,
}

impl Phase3StateBudget {
    fn observe_message(
        &mut self,
        topic_id: &str,
        topic_messages: usize,
        raw_bytes: usize,
    ) -> Result<(), String> {
        if topic_messages > MAX_PHASE3_MESSAGES_PER_TOPIC {
            return Err(format!(
                "Phase 3 topic {topic_id} exceeds the {MAX_PHASE3_MESSAGES_PER_TOPIC}-message limit"
            ));
        }
        self.messages = self
            .messages
            .checked_add(1)
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
            return Err("Phase 3 state exceeds the 64 MiB payload budget".to_string());
        }
        Ok(())
    }
}

impl Phase3Message {
    /// V2: 获取指定 owner 下所有 topic 的 config_hash 和 content_hash
    pub async fn get_targeted_topic_hashes(
        pool: &SqlitePool,
        owners: &[OwnerKey],
    ) -> Result<HashMap<TopicKey, TargetedTopicHashState>, String> {
        if owners.is_empty() {
            return Ok(HashMap::new());
        }

        let expected_owners = owners.iter().cloned().collect::<HashSet<_>>();
        let mut result = HashMap::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let predicates = owner_chunk
                .iter()
                .map(|_| "(owner_type = ? AND owner_id = ?)")
                .collect::<Vec<_>>()
                .join(" OR ");
            let query_str = format!(
                "SELECT topic_id, owner_type, owner_id, config_hash, content_hash
                 FROM topics WHERE ({predicates}) AND deleted_at IS NULL"
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(query_str));
            for owner in owner_chunk {
                query = query.bind(&owner.owner_type).bind(&owner.owner_id);
            }
            let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
            for row in rows {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Targeted topic id decode failed: {error}"))?;
                let config_hash: String = row.try_get("config_hash").map_err(|error| {
                    format!("Targeted topic {topic_id} config hash decode failed: {error}")
                })?;
                let content_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Targeted topic {topic_id} content hash decode failed: {error}")
                })?;
                let raw_owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Targeted topic {topic_id} owner type decode failed: {error}")
                })?;
                let owner_type = OwnerType::try_from(raw_owner_type.as_str())
                    .map_err(|_| format!("Targeted topic {topic_id} has invalid owner identity"))?;
                let owner_id: String = row.try_get("owner_id").map_err(|error| {
                    format!("Targeted topic {topic_id} owner id decode failed: {error}")
                })?;
                if owner_id.is_empty()
                    || !expected_owners.contains(&OwnerKey::new(owner_type.as_str(), &owner_id))
                {
                    return Err(format!(
                        "Targeted topic {topic_id} has invalid owner identity"
                    ));
                }
                let key = TopicKey::new(owner_type.as_str(), owner_id, &topic_id);
                if result
                    .insert(
                        key,
                        TargetedTopicHashState {
                            config_hash,
                            content_hash,
                        },
                    )
                    .is_some()
                {
                    return Err(format!(
                        "Targeted topic hash query returned duplicate topic identity for {topic_id}"
                    ));
                }
            }
        }
        Ok(result)
    }

    /// 批量获取指定 topic 的本地消息哈希，用于发送给桌面端计算 diff
    pub async fn get_topic_message_hashes(
        pool: &SqlitePool,
        topic_keys: &[TopicKey],
    ) -> Result<HashMap<TopicKey, TopicLocalState>, String> {
        if topic_keys.is_empty() {
            return Ok(HashMap::new());
        }

        let expected = topic_keys.iter().cloned().collect::<HashSet<_>>();
        if expected.len() != topic_keys.len()
            || expected.iter().any(|key| {
                !matches!(key.owner_type.as_str(), "agent" | "group")
                    || key.owner_id.is_empty()
                    || key.topic_id.is_empty()
            })
        {
            return Err("Topic message hash request contains invalid or duplicate topics".into());
        }

        let mut tx = pool
            .begin()
            .await
            .map_err(|error| format!("Phase 3 state snapshot failed: {error}"))?;
        let mut result: HashMap<TopicKey, TopicLocalState> = HashMap::new();
        for topic_chunk in topic_keys.chunks(SQLITE_BIND_CHUNK / 3) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "(?, ?, ?)")
                .collect::<Vec<_>>()
                .join(", ");
            let topic_query = format!(
                "SELECT topic_id, owner_type, owner_id, content_hash
                 FROM topics WHERE (owner_type, owner_id, topic_id) IN ({}) AND deleted_at IS NULL",
                placeholders
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(topic_query));
            for key in topic_chunk {
                query = query
                    .bind(&key.owner_type)
                    .bind(&key.owner_id)
                    .bind(&key.topic_id);
            }
            for row in query.fetch_all(&mut *tx).await.map_err(|e| e.to_string())? {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Topic hash id decode failed: {error}"))?;
                let content_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Topic {topic_id} content hash decode failed: {error}")
                })?;
                let raw_owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Topic {topic_id} owner type decode failed: {error}")
                })?;
                let owner_type = OwnerType::try_from(raw_owner_type.as_str())
                    .map_err(|_| format!("Topic {topic_id} has invalid owner identity"))?;
                let owner_id: String = row
                    .try_get("owner_id")
                    .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
                if owner_id.is_empty() {
                    return Err(format!("Topic {topic_id} has invalid owner identity"));
                }
                let key = TopicKey::new(owner_type.as_str(), owner_id, &topic_id);
                if result
                    .insert(
                        key,
                        TopicLocalState {
                            content_hash,
                            messages: BTreeMap::new(),
                        },
                    )
                    .is_some()
                {
                    return Err(format!(
                        "Topic message hash query returned duplicate topic identity for {topic_id}"
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

        // Stream the actual state once and enforce budgets before retaining each decoded row.
        let mut budget = Phase3StateBudget::default();
        // 批量查询所有消息 hash (包含已软删除的消息)
        for topic_chunk in topic_keys.chunks(SQLITE_BIND_CHUNK / 3) {
            let placeholders = topic_chunk
                .iter()
                .map(|_| "(?, ?, ?)")
                .collect::<Vec<_>>()
                .join(", ");
            let msg_query = format!(
                "SELECT owner_type, owner_id, topic_id, msg_id, content_hash, updated_at, deleted_at
                 FROM messages WHERE (owner_type, owner_id, topic_id) IN ({})",
                placeholders
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(msg_query));
            for key in topic_chunk {
                query = query
                    .bind(&key.owner_type)
                    .bind(&key.owner_id)
                    .bind(&key.topic_id);
            }
            let mut rows = query.fetch(&mut *tx);
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
                let owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Message hash owner type decode failed for {topic_id}: {error}")
                })?;
                let owner_id: String = row.try_get("owner_id").map_err(|error| {
                    format!("Message hash owner id decode failed for {topic_id}: {error}")
                })?;
                let key = TopicKey::new(owner_type, owner_id, &topic_id);
                let state = result.get_mut(&key).ok_or_else(|| {
                    format!("Message hash query returned an unknown topic {topic_id}")
                })?;
                let (version, effective_updated_at) = if let Some(deleted_at) = deleted_at {
                    (
                        MessageVersionState::Deleted(MessageDeletedState { deleted_at }),
                        deleted_at,
                    )
                } else {
                    (
                        MessageVersionState::Live(MessageLiveState {
                            message_hash: hash,
                            updated_at,
                        }),
                        updated_at,
                    )
                };
                if !(0..=(1_i64 << 53) - 1).contains(&effective_updated_at) {
                    return Err(format!(
                        "Message update time is invalid for {topic_id}/{msg_id}"
                    ));
                }
                let topic_messages = state.messages.len().saturating_add(1);
                let state_bytes = msg_id.len().saturating_add(match &version {
                    MessageVersionState::Live(live) => live.message_hash.len() + 48,
                    MessageVersionState::Deleted(_) => 32,
                });
                budget.observe_message(&topic_id, topic_messages, state_bytes)?;
                if state.messages.insert(msg_id.clone(), version).is_some() {
                    return Err(format!(
                        "Message hash query returned duplicate message {msg_id} for {topic_id}"
                    ));
                }
            }
        }

        tx.commit()
            .await
            .map_err(|error| format!("Phase 3 state snapshot close failed: {error}"))?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MessageDeletedState, MessageLiveState, MessageVersionState, Phase3Message,
        Phase3StateBudget, MAX_PHASE3_MESSAGES_PER_TOPIC,
    };
    use crate::vcp_modules::topic_types::TopicKey;

    fn topic(topic_id: &str) -> TopicKey {
        TopicKey::new("agent", "agent-a", topic_id)
    }

    #[test]
    fn phase3_state_budget_rejects_an_oversized_single_topic() {
        let mut budget = Phase3StateBudget::default();
        let error = budget
            .observe_message("topic", MAX_PHASE3_MESSAGES_PER_TOPIC + 1, 1)
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
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             INSERT INTO topics VALUES
                ('agent', 'agent-a', 'live', 'topic-hash', NULL),
                ('agent', 'agent-a', 'deleted', 'deleted-hash', 9);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        for missing in ["missing", "deleted"] {
            let error =
                Phase3Message::get_topic_message_hashes(&pool, &[topic("live"), topic(missing)])
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
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                content_hash TEXT, updated_at INTEGER, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             INSERT INTO topics VALUES ('agent', 'agent-a', 'topic', 'topic-hash', NULL);
             INSERT INTO messages VALUES
                ('agent', 'agent-a', 'topic', 'live', 'live-hash', 123, NULL),
                ('agent', 'agent-a', 'topic', 'deleted', 'old-hash', 50, 456);",
        )
        .execute(&pool)
        .await
        .expect("create fixture");

        let key = topic("topic");
        let states = Phase3Message::get_topic_message_hashes(&pool, std::slice::from_ref(&key))
            .await
            .expect("load message states");
        assert_eq!(
            states[&key].messages["live"],
            MessageVersionState::Live(MessageLiveState {
                message_hash: "live-hash".to_string(),
                updated_at: 123,
            })
        );
        assert_eq!(
            states[&key].messages["deleted"],
            MessageVersionState::Deleted(MessageDeletedState { deleted_at: 456 })
        );
    }
}
