use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::{compute_deterministic_hash, compute_merkle_root};

use sqlx::{Row, Sqlite, Transaction};

pub struct HashAggregator;

impl HashAggregator {
    pub fn compute_message_fingerprint(content: &str, attachment_hashes: &[String]) -> String {
        let mut sorted_hashes = attachment_hashes.to_vec();
        sorted_hashes.sort();

        let mut fingerprint_map = serde_json::Map::new();
        fingerprint_map.insert(
            "content".to_string(),
            serde_json::Value::String(content.to_string()),
        );
        if !sorted_hashes.is_empty() {
            fingerprint_map.insert(
                "attachmentHashes".to_string(),
                serde_json::to_value(sorted_hashes).unwrap(),
            );
        }

        compute_deterministic_hash(&serde_json::Value::Object(fingerprint_map))
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
        compute_deterministic_hash(dto)
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

    pub async fn compute_topic_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<String, String> {
        let rows = sqlx::query(
            "SELECT content_hash FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp ASC, msg_id ASC",
        )
        .bind(topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::with_capacity(rows.len());
        for row in rows {
            hashes.push(row.try_get("content_hash").map_err(|error| {
                format!("Topic {topic_id} message hash decode failed: {error}")
            })?);
        }
        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_agent_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND topic_id <> 'default' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(agent_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::new();
        for r in topic_rows {
            // 将 topic 的元数据 hash 和内容 hash 同时作为叶子节点，确保任何一方变动都会向上冒泡
            hashes.push(r.try_get("config_hash").map_err(|error| {
                format!("Agent {agent_id} topic config hash decode failed: {error}")
            })?);
            hashes.push(r.try_get("content_hash").map_err(|error| {
                format!("Agent {agent_id} topic content hash decode failed: {error}")
            })?);
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_group_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND topic_id <> 'default' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::new();
        for r in topic_rows {
            hashes.push(r.try_get("config_hash").map_err(|error| {
                format!("Group {group_id} topic config hash decode failed: {error}")
            })?);
            hashes.push(r.try_get("content_hash").map_err(|error| {
                format!("Group {group_id} topic content hash decode failed: {error}")
            })?);
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn bubble_topic_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        // 1. 计算并更新 content_hash (消息聚合)
        let root_hash = Self::compute_topic_root_hash(tx, topic_id).await?;

        // 2. 计算并更新 config_hash (元数据)
        let row =
            sqlx::query("SELECT owner_type FROM topics WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(topic_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let owner_type: String = row
            .try_get("owner_type")
            .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
        let config_hash = if owner_type == "agent" {
            let dto = HashInitializer::load_agent_topic_dto(tx, topic_id).await?;
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if owner_type == "group" {
            let dto = HashInitializer::load_group_topic_dto(tx, topic_id).await?;
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        };

        let updated =
            sqlx::query("UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(root_hash)
                .bind(config_hash)
                .bind(topic_id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Topic {topic_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_topic_hash_with_meta(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
        owner_type: &str,
        title: &str,
        created_at: i64,
        locked: bool,
        unread: bool,
    ) -> Result<(), String> {
        // 1. 计算并更新 content_hash (消息聚合)
        let root_hash = Self::compute_topic_root_hash(tx, topic_id).await?;

        // 2. 直接根据外部传入的元数据参数计算 config_hash (省去 2 次 SELECT)
        let config_hash = if owner_type == "agent" {
            let dto = AgentTopicSyncDTO {
                id: topic_id.to_string(),
                name: title.to_string(),
                created_at,
                locked,
                unread,
                owner_id: String::new(),
            };
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if owner_type == "group" {
            let dto = GroupTopicSyncDTO {
                id: topic_id.to_string(),
                name: title.to_string(),
                created_at,
                owner_id: String::new(),
            };
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        };

        let updated =
            sqlx::query("UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(root_hash)
                .bind(config_hash)
                .bind(topic_id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Topic {topic_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_agent_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_agent_root_hash(tx, agent_id).await?;
        let updated = sqlx::query(
            "UPDATE agents SET content_hash = ? WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Agent {agent_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_group_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_group_root_hash(tx, group_id).await?;
        let updated = sqlx::query(
            "UPDATE groups SET content_hash = ? WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Group {group_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_from_topic(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        Self::bubble_topic_hash(tx, topic_id).await?;

        let topic_row = sqlx::query(
            "SELECT owner_id, owner_type FROM topics WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let owner_id: String = topic_row
            .try_get("owner_id")
            .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
        let owner_type: String = topic_row
            .try_get("owner_type")
            .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;

        if owner_type == "agent" {
            Self::bubble_agent_hash(tx, &owner_id).await?;
        } else if owner_type == "group" {
            Self::bubble_group_hash(tx, &owner_id).await?;
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        }

        Ok(())
    }
}

pub struct HashInitializer;

impl HashInitializer {
    pub async fn ensure_agent_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let row =
            sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ? AND deleted_at IS NULL")
                .bind(agent_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let r = row.ok_or_else(|| format!("Agent {agent_id} is missing or deleted"))?;
        let config_hash: Option<String> = r
            .try_get("config_hash")
            .map_err(|error| format!("Agent {agent_id} hash decode failed: {error}"))?;
        if config_hash
            .as_deref()
            .is_none_or(|hash| hash.is_empty() || hash == "PENDING")
        {
            let dto = Self::load_agent_dto(tx, agent_id).await?;
            let new_hash = HashAggregator::compute_agent_config_hash(&dto);
            let updated = sqlx::query(
                "UPDATE agents SET config_hash = ? WHERE agent_id = ? AND deleted_at IS NULL",
            )
            .bind(&new_hash)
            .bind(agent_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
            if updated.rows_affected() != 1 {
                return Err(format!(
                    "Agent {agent_id} disappeared during hash initialization"
                ));
            }
            log::debug!(
                "[HashInitializer] Initialized config_hash for Agent {}",
                agent_id
            );
        }

        Ok(())
    }

    pub async fn ensure_group_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let row =
            sqlx::query("SELECT config_hash FROM groups WHERE group_id = ? AND deleted_at IS NULL")
                .bind(group_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let r = row.ok_or_else(|| format!("Group {group_id} is missing or deleted"))?;
        let config_hash: Option<String> = r
            .try_get("config_hash")
            .map_err(|error| format!("Group {group_id} hash decode failed: {error}"))?;
        if config_hash
            .as_deref()
            .is_none_or(|hash| hash.is_empty() || hash == "PENDING")
        {
            let dto = Self::load_group_dto(tx, group_id).await?;
            let new_hash = HashAggregator::compute_group_config_hash(&dto);
            let updated = sqlx::query(
                "UPDATE groups SET config_hash = ? WHERE group_id = ? AND deleted_at IS NULL",
            )
            .bind(&new_hash)
            .bind(group_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
            if updated.rows_affected() != 1 {
                return Err(format!(
                    "Group {group_id} disappeared during hash initialization"
                ));
            }
            log::debug!(
                "[HashInitializer] Initialized config_hash for Group {}",
                group_id
            );
        }

        Ok(())
    }

    pub async fn ensure_all_agent_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let rows = sqlx::query(
            "SELECT agent_id FROM agents
             WHERE deleted_at IS NULL
               AND (config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING')",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        for row in rows {
            let agent_id: String = row
                .try_get("agent_id")
                .map_err(|error| format!("Agent hash id decode failed: {error}"))?;
            Self::ensure_agent_hashes(&mut tx, &agent_id)
                .await
                .map_err(|error| {
                    format!("Failed to initialize hash for Agent {agent_id}: {error}")
                })?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        log::info!("[HashInitializer] Ensured all Agent hashes");
        Ok(())
    }

    pub async fn ensure_all_group_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let rows = sqlx::query(
            "SELECT group_id FROM groups
             WHERE deleted_at IS NULL
               AND (config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING')",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        for row in rows {
            let group_id: String = row
                .try_get("group_id")
                .map_err(|error| format!("Group hash id decode failed: {error}"))?;
            Self::ensure_group_hashes(&mut tx, &group_id)
                .await
                .map_err(|error| {
                    format!("Failed to initialize hash for Group {group_id}: {error}")
                })?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        log::info!("[HashInitializer] Ensured all Group hashes");
        Ok(())
    }

    pub async fn load_agent_topic_dto(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<AgentTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentTopicSyncDTO {
            id: row
                .try_get("topic_id")
                .map_err(|error| format!("Agent topic {topic_id} id decode failed: {error}"))?,
            name: row
                .try_get("title")
                .map_err(|error| format!("Agent topic {topic_id} title decode failed: {error}"))?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!("Agent topic {topic_id} created_at decode failed: {error}")
            })?,
            locked: row
                .try_get::<i64, _>("locked")
                .map_err(|error| format!("Agent topic {topic_id} locked decode failed: {error}"))?
                != 0,
            unread: row
                .try_get::<i64, _>("unread")
                .map_err(|error| format!("Agent topic {topic_id} unread decode failed: {error}"))?
                != 0,
            owner_id: row
                .try_get("owner_id")
                .map_err(|error| format!("Agent topic {topic_id} owner decode failed: {error}"))?,
        })
    }

    pub async fn load_group_topic_dto(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<GroupTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'group' AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(GroupTopicSyncDTO {
            id: row
                .try_get("topic_id")
                .map_err(|error| format!("Group topic {topic_id} id decode failed: {error}"))?,
            name: row
                .try_get("title")
                .map_err(|error| format!("Group topic {topic_id} title decode failed: {error}"))?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!("Group topic {topic_id} created_at decode failed: {error}")
            })?,
            owner_id: row
                .try_get("owner_id")
                .map_err(|error| format!("Group topic {topic_id} owner decode failed: {error}"))?,
        })
    }

    async fn load_agent_dto(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<AgentSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output
             FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentSyncDTO {
            name: row
                .try_get("name")
                .map_err(|error| format!("Agent {agent_id} name decode failed: {error}"))?,
            system_prompt: row.try_get("system_prompt").map_err(|error| {
                format!("Agent {agent_id} system prompt decode failed: {error}")
            })?,
            model: row
                .try_get("model")
                .map_err(|error| format!("Agent {agent_id} model decode failed: {error}"))?,
            temperature: row
                .try_get("temperature")
                .map_err(|error| format!("Agent {agent_id} temperature decode failed: {error}"))?,
            context_token_limit: row.try_get("context_token_limit").map_err(|error| {
                format!("Agent {agent_id} context token limit decode failed: {error}")
            })?,
            max_output_tokens: row.try_get("max_output_tokens").map_err(|error| {
                format!("Agent {agent_id} max output tokens decode failed: {error}")
            })?,
            stream_output: row.try_get::<i64, _>("stream_output").map_err(|error| {
                format!("Agent {agent_id} stream output decode failed: {error}")
            })? != 0,
        })
    }

    async fn load_group_dto(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<GroupSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, mode, group_prompt, invite_prompt, use_unified_model, unified_model, tag_match_mode, created_at
             FROM groups WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let members = Self::load_group_members(tx, group_id).await?;
        let member_tags = Self::load_member_tags(tx, group_id).await?;

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
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<Vec<String>, String> {
        let rows = sqlx::query(
            "SELECT agent_id FROM group_members WHERE group_id = ? ORDER BY sort_order",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
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

    async fn load_member_tags(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<serde_json::Value, String> {
        let rows = sqlx::query(
            "SELECT agent_id, member_tag FROM group_members WHERE group_id = ? AND member_tag IS NOT NULL",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;

        let mut tags = serde_json::Map::new();
        for row in rows {
            let agent_id: String = row.try_get("agent_id").map_err(|error| {
                format!("Group {group_id} member tag agent id decode failed: {error}")
            })?;
            let tag: String = row
                .try_get("member_tag")
                .map_err(|error| format!("Group {group_id} member tag decode failed: {error}"))?;
            tags.insert(agent_id, serde_json::Value::String(tag));
        }

        Ok(serde_json::Value::Object(tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::sync_dto::{AgentSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO};

    #[test]
    fn test_message_fingerprint_ignores_attachment_order() {
        let a = HashAggregator::compute_message_fingerprint(
            "hello",
            &["hash-b".to_string(), "hash-a".to_string()],
        );
        let b = HashAggregator::compute_message_fingerprint(
            "hello",
            &["hash-a".to_string(), "hash-b".to_string()],
        );

        assert_eq!(a, b);
        assert_ne!(
            a,
            HashAggregator::compute_message_fingerprint("hello!", &["hash-a".to_string()])
        );
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
        assert_ne!(
            HashAggregator::compute_agent_config_hash(&base),
            HashAggregator::compute_agent_config_hash(&rounded_diff)
        );
    }

    #[test]
    fn test_topic_metadata_hash_excludes_owner_id() {
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
    }

    #[test]
    fn test_group_topic_metadata_hash_excludes_locked_unread_conceptually() {
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

    async fn compute_owner_root(tx: &mut Transaction<'_, Sqlite>, owner_type: &str) -> String {
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

    async fn assert_owner_root_excludes_default(owner_type: &str) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open owner root database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO topics VALUES
                ('default', 'owner', ?, 'default-config', 'default-content', NULL);",
        )
        .bind(owner_type)
        .execute(&pool)
        .await
        .expect("create owner root fixture");
        let mut tx = pool.begin().await.expect("begin owner root transaction");

        let root_with_only_default = compute_owner_root(&mut tx, owner_type).await;
        assert_eq!(root_with_only_default, "");

        sqlx::query(
            "INSERT INTO topics VALUES ('topic-a', 'owner', ?, 'config-a', 'content-a', NULL)",
        )
        .bind(owner_type)
        .execute(&mut *tx)
        .await
        .expect("insert ordinary topic");
        let ordinary_root = compute_owner_root(&mut tx, owner_type).await;
        assert_eq!(
            ordinary_root,
            "1e33dc5103370a9970e5c719697e29dcc8bff3a3196de13fcbaaf1029c0436c4"
        );

        sqlx::query("UPDATE topics SET config_hash = 'changed-default' WHERE topic_id = 'default'")
            .execute(&mut *tx)
            .await
            .expect("change default topic");
        let root_after_default_change = compute_owner_root(&mut tx, owner_type).await;
        assert_eq!(root_after_default_change, ordinary_root);

        sqlx::query("UPDATE topics SET content_hash = 'content-b' WHERE topic_id = 'topic-a'")
            .execute(&mut *tx)
            .await
            .expect("change ordinary topic");
        let root_after_ordinary_change = compute_owner_root(&mut tx, owner_type).await;
        assert_ne!(root_after_ordinary_change, ordinary_root);
    }

    #[tokio::test]
    async fn owner_root_hashes_exclude_default_topics() {
        assert_owner_root_excludes_default("agent").await;
        assert_owner_root_excludes_default("group").await;
    }

    #[tokio::test]
    async fn hash_initializer_rolls_back_and_surfaces_row_errors() {
        let agent_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open agent database");
        sqlx::query(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, config_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('broken-agent', 'PENDING', NULL);",
        )
        .execute(&agent_pool)
        .await
        .expect("create broken agent fixture");
        let agent_error = HashInitializer::ensure_all_agent_hashes(&agent_pool)
            .await
            .expect_err("per-agent query errors must abort initialization");
        assert!(agent_error.contains("broken-agent"));

        let group_pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open group database");
        sqlx::query(
            "CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, config_hash TEXT, name TEXT, mode TEXT,
                group_prompt TEXT, invite_prompt TEXT, use_unified_model INTEGER,
                unified_model TEXT, tag_match_mode TEXT, created_at INTEGER,
                deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, sort_order INTEGER
             );
             INSERT INTO groups VALUES
                ('broken-group', 'PENDING', 'Group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, NULL);
             INSERT INTO group_members VALUES ('broken-group', 'agent', 0);",
        )
        .execute(&group_pool)
        .await
        .expect("create broken group fixture");
        let group_error = HashInitializer::ensure_all_group_hashes(&group_pool)
            .await
            .expect_err("member-tag query errors must abort initialization");
        assert!(group_error.contains("broken-group"));
    }

    #[tokio::test]
    async fn hash_initializer_accepts_legacy_null_hashes_without_panicking() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, name TEXT, system_prompt TEXT, model TEXT,
                temperature REAL, context_token_limit INTEGER, max_output_tokens INTEGER,
                stream_output INTEGER, config_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, name TEXT, mode TEXT, group_prompt TEXT,
                invite_prompt TEXT, use_unified_model INTEGER, unified_model TEXT,
                tag_match_mode TEXT, created_at INTEGER, config_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, member_tag TEXT, sort_order INTEGER
             );
             INSERT INTO agents VALUES
                ('legacy-agent', 'Agent', '', 'model', 1, 100, 20, 1, NULL, NULL);
             INSERT INTO groups VALUES
                ('legacy-group', 'Group', 'fixed', NULL, NULL, 0, NULL, NULL, 1, NULL, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create legacy fixture");

        HashInitializer::ensure_all_agent_hashes(&pool)
            .await
            .expect("initialize agent hash");
        HashInitializer::ensure_all_group_hashes(&pool)
            .await
            .expect("initialize group hash");
        let agent_hash: String =
            sqlx::query_scalar("SELECT config_hash FROM agents WHERE agent_id = 'legacy-agent'")
                .fetch_one(&pool)
                .await
                .expect("read agent hash");
        let group_hash: String =
            sqlx::query_scalar("SELECT config_hash FROM groups WHERE group_id = 'legacy-group'")
                .fetch_one(&pool)
                .await
                .expect("read group hash");
        assert!(!agent_hash.is_empty());
        assert!(!group_hash.is_empty());
    }
}
