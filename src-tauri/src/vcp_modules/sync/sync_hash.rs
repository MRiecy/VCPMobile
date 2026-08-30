use crate::vcp_modules::group_types::parse_member_tags;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::{compute_deterministic_hash, compute_merkle_root};
use crate::vcp_modules::topic_types::{
    resolve_topic_activity_updated_at, TopicActivityDto, TopicKey,
};

use sqlx::{Row, SqliteConnection};

pub struct HashAggregator;

impl HashAggregator {
    pub async fn load_topic_activity(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<TopicActivityDto, String> {
        let row = sqlx::query(
            "SELECT msg_count, updated_at, last_message_updated_at, created_at
             FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let topic_updated_at: i64 = row
            .try_get("updated_at")
            .map_err(|error| format!("Topic {} updated_at decode failed: {error}", key.topic_id))?;
        let last_message_updated_at: i64 =
            row.try_get("last_message_updated_at").map_err(|error| {
                format!(
                    "Topic {} last_message_updated_at decode failed: {error}",
                    key.topic_id
                )
            })?;
        let created_at: i64 = row
            .try_get("created_at")
            .map_err(|error| format!("Topic {} created_at decode failed: {error}", key.topic_id))?;
        Ok(TopicActivityDto {
            msg_count: row.try_get("msg_count").map_err(|error| {
                format!("Topic {} msg_count decode failed: {error}", key.topic_id)
            })?,
            updated_at: resolve_topic_activity_updated_at(
                topic_updated_at,
                last_message_updated_at,
                created_at,
            ),
        })
    }

    async fn compute_topic_content_aggregate(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<(String, i32, i64), String> {
        let rows = sqlx::query(
            "SELECT msg_id, content_hash, updated_at FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let msg_count = i32::try_from(rows.len())
            .map_err(|_| format!("Topic {} message count exceeds i32", key.topic_id))?;
        let mut hashes = Vec::with_capacity(rows.len());
        let mut last_message_updated_at = 0_i64;
        for row in rows {
            let message_id: String = row.try_get("msg_id").map_err(|error| {
                format!("Topic {} message id decode failed: {error}", key.topic_id)
            })?;
            let message_hash: String = row.try_get("content_hash").map_err(|error| {
                format!("Topic {} message hash decode failed: {error}", key.topic_id)
            })?;
            let message_updated_at: i64 = row.try_get("updated_at").map_err(|error| {
                format!(
                    "Topic {} message updated_at decode failed: {error}",
                    key.topic_id
                )
            })?;
            last_message_updated_at = last_message_updated_at.max(message_updated_at);
            hashes.push(Self::compute_message_leaf_hash(&message_id, &message_hash));
        }
        Ok((
            compute_merkle_root(hashes),
            msg_count,
            last_message_updated_at,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn compute_message_fingerprint(
        message_id: &str,
        role: &str,
        name: Option<&str>,
        content: &str,
        timestamp: u64,
        agent_id: Option<&str>,
        attachment_hashes: &[String],
    ) -> String {
        let mut sorted_hashes = attachment_hashes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        sorted_hashes.sort_unstable();

        let mut fingerprint_map = serde_json::Map::new();
        fingerprint_map.insert(
            "id".to_string(),
            serde_json::Value::String(message_id.to_string()),
        );
        fingerprint_map.insert(
            "role".to_string(),
            serde_json::Value::String(role.to_string()),
        );
        if let Some(name) = name {
            fingerprint_map.insert(
                "name".to_string(),
                serde_json::Value::String(name.to_string()),
            );
        }
        fingerprint_map.insert(
            "content".to_string(),
            serde_json::Value::String(content.to_string()),
        );
        fingerprint_map.insert(
            "timestamp".to_string(),
            serde_json::Value::Number(timestamp.into()),
        );
        if let Some(agent_id) = agent_id {
            fingerprint_map.insert(
                "agentId".to_string(),
                serde_json::Value::String(agent_id.to_string()),
            );
        }
        if !sorted_hashes.is_empty() {
            fingerprint_map.insert(
                "attachmentHashes".to_string(),
                serde_json::Value::Array(
                    sorted_hashes
                        .into_iter()
                        .map(|hash| serde_json::Value::String(hash.to_string()))
                        .collect(),
                ),
            );
        }

        compute_deterministic_hash(&serde_json::Value::Object(fingerprint_map))
    }

    pub fn compute_message_leaf_hash(message_id: &str, message_hash: &str) -> String {
        compute_deterministic_hash(&serde_json::json!({
            "id": message_id,
            "hash": message_hash,
        }))
    }

    pub fn compute_topic_leaf_hash(
        topic_id: &str,
        config_hash: &str,
        content_hash: &str,
    ) -> String {
        compute_deterministic_hash(&serde_json::json!({
            "topicId": topic_id,
            "configHash": config_hash,
            "contentHash": content_hash,
        }))
    }

    pub fn compute_agent_topic_metadata_hash(dto: &AgentTopicSyncDTO) -> String {
        // 排除 owner_id，仅使用 topic 自身属性计算 hash
        // 确保与桌面端 AGENT_TOPIC_SYNC_FIELDS ["id","name","createdAt","locked","unread"] 一致
        let meta = serde_json::json!({
            "id": &dto.id,
            "name": &dto.name,
            "createdAt": dto.created_at,
            "locked": dto.locked,
            "unread": dto.unread,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_group_topic_metadata_hash(dto: &GroupTopicSyncDTO) -> String {
        // 排除 owner_id，仅使用 topic 自身属性计算 hash
        // 确保与桌面端 GROUP_TOPIC_SYNC_FIELDS ["id","name","createdAt"] 一致
        let meta = serde_json::json!({
            "id": &dto.id,
            "name": &dto.name,
            "createdAt": dto.created_at,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_agent_config_hash(dto: &AgentSyncDTO) -> String {
        // 对 temperature 统一格式化到2位小数，消除 f32/f64 精度差异导致的 hash 不一致
        let meta = serde_json::json!({
            "name": &dto.name,
            "systemPrompt": &dto.system_prompt,
            "model": &dto.model,
            "temperature": (dto.temperature * 100.0).round() / 100.0,
            "contextTokenLimit": dto.context_token_limit,
            "maxOutputTokens": dto.max_output_tokens,
            "streamOutput": dto.stream_output,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_group_config_hash(dto: &GroupSyncDTO) -> String {
        let meta = serde_json::json!({
            "name": &dto.name,
            "members": &dto.members,
            "mode": &dto.mode,
            "memberTags": dto.member_tags.clone().unwrap_or_default(),
            "groupPrompt": dto.group_prompt.as_deref().unwrap_or(""),
            "invitePrompt": dto.invite_prompt.as_deref().unwrap_or(
                "现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。",
            ),
            "useUnifiedModel": dto.use_unified_model,
            "unifiedModel": dto.unified_model.as_deref().unwrap_or(""),
            "tagMatchMode": dto.tag_match_mode.as_deref().unwrap_or("strict"),
            "createdAt": dto.created_at,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_avatar_hash(bytes: &[u8]) -> String {
        crate::vcp_modules::infra::utils::calculate_sha256(bytes)
    }

    pub fn compute_content_hash(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    pub async fn compute_agent_root_hash(
        tx: &mut SqliteConnection,
        agent_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::with_capacity(topic_rows.len());
        for r in topic_rows {
            let topic_id: String = r
                .try_get("topic_id")
                .map_err(|error| format!("Agent {agent_id} topic id decode failed: {error}"))?;
            let config_hash: String = r.try_get("config_hash").map_err(|error| {
                format!("Agent {agent_id} topic config hash decode failed: {error}")
            })?;
            let content_hash: String = r.try_get("content_hash").map_err(|error| {
                format!("Agent {agent_id} topic content hash decode failed: {error}")
            })?;
            hashes.push(Self::compute_topic_leaf_hash(
                &topic_id,
                &config_hash,
                &content_hash,
            ));
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_group_root_hash(
        tx: &mut SqliteConnection,
        group_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT topic_id, config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::with_capacity(topic_rows.len());
        for r in topic_rows {
            let topic_id: String = r
                .try_get("topic_id")
                .map_err(|error| format!("Group {group_id} topic id decode failed: {error}"))?;
            let config_hash: String = r.try_get("config_hash").map_err(|error| {
                format!("Group {group_id} topic config hash decode failed: {error}")
            })?;
            let content_hash: String = r.try_get("content_hash").map_err(|error| {
                format!("Group {group_id} topic content hash decode failed: {error}")
            })?;
            hashes.push(Self::compute_topic_leaf_hash(
                &topic_id,
                &config_hash,
                &content_hash,
            ));
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn bubble_topic_hash(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<TopicActivityDto, String> {
        let (root_hash, msg_count, last_message_updated_at) =
            Self::compute_topic_content_aggregate(tx, key).await?;
        let config_hash = if key.owner_type == "agent" {
            let dto = SyncDtoLoader::load_agent_topic_dto(tx, key).await?;
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if key.owner_type == "group" {
            let dto = SyncDtoLoader::load_group_topic_dto(tx, key).await?;
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {} has unsupported owner type {}",
                key.topic_id, key.owner_type
            ));
        };

        let updated = sqlx::query(
            "UPDATE topics SET content_hash = ?, config_hash = ?, msg_count = ?,
                 last_message_updated_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(config_hash)
        .bind(msg_count)
        .bind(last_message_updated_at)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Topic {} disappeared during hash update",
                key.topic_id
            ));
        }
        Self::load_topic_activity(tx, key).await
    }

    pub async fn bubble_topic_hash_with_meta(
        tx: &mut SqliteConnection,
        key: &TopicKey,
        title: &str,
        created_at: i64,
        locked: bool,
        unread: bool,
    ) -> Result<TopicActivityDto, String> {
        // 1. 一次消息扫描同时计算 content_hash、msg_count 与列表更新时间投影
        let (root_hash, msg_count, last_message_updated_at) =
            Self::compute_topic_content_aggregate(tx, key).await?;

        // 2. 直接根据外部传入的元数据参数计算 config_hash (省去 2 次 SELECT)
        let config_hash = if key.owner_type == "agent" {
            let dto = AgentTopicSyncDTO {
                id: key.topic_id.clone(),
                name: title.to_string(),
                created_at,
                locked,
                unread,
                owner_id: String::new(),
            };
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if key.owner_type == "group" {
            let dto = GroupTopicSyncDTO {
                id: key.topic_id.clone(),
                name: title.to_string(),
                created_at,
                owner_id: String::new(),
            };
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {} has unsupported owner type {}",
                key.topic_id, key.owner_type
            ));
        };

        let updated = sqlx::query(
            "UPDATE topics SET content_hash = ?, config_hash = ?, msg_count = ?,
                 last_message_updated_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(config_hash)
        .bind(msg_count)
        .bind(last_message_updated_at)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Topic {} disappeared during hash update",
                key.topic_id
            ));
        }
        Self::load_topic_activity(tx, key).await
    }

    pub async fn bubble_agent_hash(
        tx: &mut SqliteConnection,
        agent_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_agent_root_hash(tx, agent_id).await?;
        let updated = sqlx::query(
            "UPDATE agents SET content_hash = ?
             WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Agent {agent_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_group_hash(
        tx: &mut SqliteConnection,
        group_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_group_root_hash(tx, group_id).await?;
        let updated = sqlx::query(
            "UPDATE groups SET content_hash = ?
             WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Group {group_id} disappeared during hash update"));
        }
        Ok(())
    }

    /// Group 成员等 DTO 字段在业务事务中变化后，按统一 DTO 合同重算配置哈希与时钟。
    pub async fn recompute_group_config_hash(
        tx: &mut SqliteConnection,
        group_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let dto = SyncDtoLoader::load_group_dto(tx, group_id).await?;
        let config_hash = Self::compute_group_config_hash(&dto);
        let updated = sqlx::query(
            "UPDATE groups SET
                updated_at = CASE
                    WHEN config_hash IS NOT ? THEN ?
                    ELSE updated_at
                END,
                config_hash = ?
             WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL",
        )
        .bind(&config_hash)
        .bind(updated_at)
        .bind(&config_hash)
        .bind(group_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Group {group_id} disappeared during config hash update"
            ));
        }
        Ok(())
    }

    pub async fn bubble_from_topic(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<TopicActivityDto, String> {
        let activity = Self::bubble_topic_hash(tx, key).await?;

        if key.owner_type == "agent" {
            Self::bubble_agent_hash(tx, &key.owner_id).await?;
        } else if key.owner_type == "group" {
            Self::bubble_group_hash(tx, &key.owner_id).await?;
        } else {
            return Err(format!(
                "Topic {} has unsupported owner type {}",
                key.topic_id, key.owner_type
            ));
        }

        Ok(activity)
    }
}

struct SyncDtoLoader;

impl SyncDtoLoader {
    async fn load_agent_topic_dto(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<AgentTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics
             WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentTopicSyncDTO {
            id: row.try_get("topic_id").map_err(|error| {
                format!("Agent topic {} id decode failed: {error}", key.topic_id)
            })?,
            name: row.try_get("title").map_err(|error| {
                format!("Agent topic {} title decode failed: {error}", key.topic_id)
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!(
                    "Agent topic {} created_at decode failed: {error}",
                    key.topic_id
                )
            })?,
            locked: row.try_get::<i64, _>("locked").map_err(|error| {
                format!("Agent topic {} locked decode failed: {error}", key.topic_id)
            })? != 0,
            unread: row.try_get::<i64, _>("unread").map_err(|error| {
                format!("Agent topic {} unread decode failed: {error}", key.topic_id)
            })? != 0,
            owner_id: row.try_get("owner_id").map_err(|error| {
                format!("Agent topic {} owner decode failed: {error}", key.topic_id)
            })?,
        })
    }

    async fn load_group_topic_dto(
        tx: &mut SqliteConnection,
        key: &TopicKey,
    ) -> Result<GroupTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, owner_id FROM topics
             WHERE owner_type = 'group' AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(GroupTopicSyncDTO {
            id: row.try_get("topic_id").map_err(|error| {
                format!("Group topic {} id decode failed: {error}", key.topic_id)
            })?,
            name: row.try_get("title").map_err(|error| {
                format!("Group topic {} title decode failed: {error}", key.topic_id)
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!(
                    "Group topic {} created_at decode failed: {error}",
                    key.topic_id
                )
            })?,
            owner_id: row.try_get("owner_id").map_err(|error| {
                format!("Group topic {} owner decode failed: {error}", key.topic_id)
            })?,
        })
    }

    async fn load_group_dto(
        tx: &mut SqliteConnection,
        group_id: &str,
    ) -> Result<GroupSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, mode, group_prompt, invite_prompt, use_unified_model, unified_model, tag_match_mode, member_tags, created_at
             FROM groups WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let members = Self::load_group_members(tx, group_id).await?;
        let member_tags_raw: String = row
            .try_get("member_tags")
            .map_err(|error| format!("Group {group_id} memberTags decode failed: {error}"))?;
        let member_tags = parse_member_tags(&member_tags_raw)
            .map_err(|error| format!("Group {group_id} memberTags JSON is invalid: {error}"))?;

        Ok(GroupSyncDTO {
            name: row
                .try_get("name")
                .map_err(|error| format!("Group {group_id} name decode failed: {error}"))?,
            members,
            mode: row
                .try_get("mode")
                .map_err(|error| format!("Group {group_id} mode decode failed: {error}"))?,
            member_tags: Some(member_tags),
            group_prompt: row
                .try_get("group_prompt")
                .map_err(|error| format!("Group {group_id} prompt decode failed: {error}"))?,
            invite_prompt: row.try_get("invite_prompt").map_err(|error| {
                format!("Group {group_id} invite prompt decode failed: {error}")
            })?,
            use_unified_model: row
                .try_get::<i64, _>("use_unified_model")
                .map_err(|error| {
                    format!("Group {group_id} unified model flag decode failed: {error}")
                })?
                != 0,
            unified_model: row.try_get("unified_model").map_err(|error| {
                format!("Group {group_id} unified model decode failed: {error}")
            })?,
            tag_match_mode: row.try_get("tag_match_mode").map_err(|error| {
                format!("Group {group_id} tag match mode decode failed: {error}")
            })?,
            created_at: row
                .try_get("created_at")
                .map_err(|error| format!("Group {group_id} created_at decode failed: {error}"))?,
        })
    }

    async fn load_group_members(
        tx: &mut SqliteConnection,
        group_id: &str,
    ) -> Result<Vec<String>, String> {
        let rows = sqlx::query(
            "SELECT agent_id FROM group_members WHERE group_id = ? ORDER BY sort_order",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                row.try_get("agent_id").map_err(|error| {
                    format!("Group {group_id} member agent id decode failed: {error}")
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::sync_dto::{AgentSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO};

    #[test]
    fn message_fingerprint_binds_identity_and_state_but_ignores_attachment_order() {
        let a = HashAggregator::compute_message_fingerprint(
            "message-a",
            "assistant",
            Some("Nova"),
            "hello",
            123,
            Some("agent-a"),
            &["hash-b".to_string(), "hash-a".to_string()],
        );
        let b = HashAggregator::compute_message_fingerprint(
            "message-a",
            "assistant",
            Some("Nova"),
            "hello",
            123,
            Some("agent-a"),
            &["hash-a".to_string(), "hash-b".to_string()],
        );

        assert_eq!(a, b);
        for changed in [
            HashAggregator::compute_message_fingerprint(
                "message-b",
                "assistant",
                Some("Nova"),
                "hello",
                123,
                Some("agent-a"),
                &["hash-a".to_string(), "hash-b".to_string()],
            ),
            HashAggregator::compute_message_fingerprint(
                "message-a",
                "user",
                Some("Nova"),
                "hello",
                123,
                Some("agent-a"),
                &["hash-a".to_string(), "hash-b".to_string()],
            ),
            HashAggregator::compute_message_fingerprint(
                "message-a",
                "assistant",
                Some("Other"),
                "hello",
                123,
                Some("agent-a"),
                &["hash-a".to_string(), "hash-b".to_string()],
            ),
            HashAggregator::compute_message_fingerprint(
                "message-a",
                "assistant",
                Some("Nova"),
                "hello",
                124,
                Some("agent-a"),
                &["hash-a".to_string(), "hash-b".to_string()],
            ),
        ] {
            assert_ne!(a, changed);
        }
    }

    #[test]
    fn keyed_leaves_preserve_message_identity_and_topic_pairing() {
        let message_hash = "same-message-state";
        assert_ne!(
            compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
                "message-a",
                message_hash,
            )]),
            compute_merkle_root(vec![HashAggregator::compute_message_leaf_hash(
                "message-b",
                message_hash,
            )]),
        );

        let original = compute_merkle_root(vec![
            HashAggregator::compute_topic_leaf_hash("topic-a", "config-a", "content-a"),
            HashAggregator::compute_topic_leaf_hash("topic-b", "config-b", "content-b"),
        ]);
        let swapped = compute_merkle_root(vec![
            HashAggregator::compute_topic_leaf_hash("topic-a", "config-a", "content-b"),
            HashAggregator::compute_topic_leaf_hash("topic-b", "config-b", "content-a"),
        ]);
        assert_ne!(original, swapped);
    }

    #[test]
    fn test_agent_config_hash_rounds_temperature_to_two_decimals() {
        let base = AgentSyncDTO {
            name: "Nova".to_string(),
            system_prompt: "system".to_string(),
            model: "model-a".to_string(),
            temperature: 0.704,
            context_token_limit: 1000,
            max_output_tokens: 2000,
            stream_output: true,
        };
        let mut rounded_same = base.clone();
        rounded_same.temperature = 0.70;
        let mut rounded_diff = base.clone();
        rounded_diff.temperature = 0.706;

        assert_eq!(
            HashAggregator::compute_agent_config_hash(&base),
            HashAggregator::compute_agent_config_hash(&rounded_same)
        );
        assert_eq!(
            HashAggregator::compute_agent_config_hash(&base),
            "a0d2b840400413446fb02e237d21747e735ee35af2684c25667a83ac5e066c4a"
        );
        assert_ne!(
            HashAggregator::compute_agent_config_hash(&base),
            HashAggregator::compute_agent_config_hash(&rounded_diff)
        );
    }

    #[test]
    fn test_group_config_hash_normalizes_optional_defaults() {
        let missing = GroupSyncDTO {
            name: "Group".to_string(),
            members: Vec::new(),
            mode: "sequential".to_string(),
            member_tags: None,
            group_prompt: None,
            invite_prompt: None,
            use_unified_model: false,
            unified_model: None,
            tag_match_mode: None,
            created_at: 0,
        };
        let explicit = GroupSyncDTO {
            member_tags: Some(Default::default()),
            group_prompt: Some(String::new()),
            invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
            unified_model: Some(String::new()),
            tag_match_mode: Some("strict".to_string()),
            ..missing.clone()
        };

        assert_eq!(
            HashAggregator::compute_group_config_hash(&missing),
            HashAggregator::compute_group_config_hash(&explicit)
        );
    }

    #[test]
    fn topic_metadata_hashes_exclude_parent_identity() {
        let topic_a = AgentTopicSyncDTO {
            id: "topic-1".to_string(),
            name: "Topic".to_string(),
            created_at: 123,
            locked: true,
            unread: false,
            owner_id: "agent-a".to_string(),
        };
        let mut topic_b = topic_a.clone();
        topic_b.owner_id = "agent-b".to_string();

        assert_eq!(
            HashAggregator::compute_agent_topic_metadata_hash(&topic_a),
            HashAggregator::compute_agent_topic_metadata_hash(&topic_b)
        );

        let group_topic = GroupTopicSyncDTO {
            id: "topic-1".to_string(),
            name: "Topic".to_string(),
            created_at: 123,
            owner_id: "group-a".to_string(),
        };
        let mut same_metadata_other_owner = group_topic.clone();
        same_metadata_other_owner.owner_id = "group-b".to_string();

        assert_eq!(
            HashAggregator::compute_group_topic_metadata_hash(&group_topic),
            HashAggregator::compute_group_topic_metadata_hash(&same_metadata_other_owner)
        );
    }

    async fn compute_owner_root(tx: &mut SqliteConnection, owner_type: &str) -> String {
        if owner_type == "agent" {
            HashAggregator::compute_agent_root_hash(tx, "owner")
                .await
                .expect("compute agent root")
        } else {
            HashAggregator::compute_group_root_hash(tx, "owner")
                .await
                .expect("compute group root")
        }
    }

    async fn assert_owner_root_includes_default(owner_type: &str) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open owner root database");
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             INSERT INTO topics VALUES
                (?, 'owner', 'default', 'default-config', 'default-content', NULL);",
        )
        .bind(owner_type)
        .execute(&pool)
        .await
        .expect("create owner root fixture");
        let mut tx = pool.begin().await.expect("begin owner root transaction");

        sqlx::query(
            "INSERT INTO topics VALUES (?, 'owner', 'topic-a', 'config-a', 'content-a', NULL)",
        )
        .bind(owner_type)
        .execute(&mut *tx)
        .await
        .expect("insert ordinary topic");
        let initial_root = compute_owner_root(&mut tx, owner_type).await;
        assert_eq!(
            initial_root,
            compute_merkle_root(vec![
                HashAggregator::compute_topic_leaf_hash(
                    "default",
                    "default-config",
                    "default-content",
                ),
                HashAggregator::compute_topic_leaf_hash("topic-a", "config-a", "content-a",),
            ])
        );

        sqlx::query("UPDATE topics SET config_hash = 'changed-default' WHERE topic_id = 'default'")
            .execute(&mut *tx)
            .await
            .expect("change default topic");
        let root_after_default_change = compute_owner_root(&mut tx, owner_type).await;
        assert_ne!(root_after_default_change, initial_root);
    }

    #[tokio::test]
    async fn owner_root_hashes_include_default_topics() {
        assert_owner_root_includes_default("agent").await;
    }
}
