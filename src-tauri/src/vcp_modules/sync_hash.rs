use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::{compute_deterministic_hash, compute_merkle_root};
use sha2::{Digest, Sha256};
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
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
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

        let hashes: Vec<String> = rows
            .iter()
            .map(|r| r.get::<String, _>("content_hash"))
            .collect();
        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_agent_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<String, String> {
        let agent_row = sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        let config_hash = match agent_row {
            Some(r) => r.get::<String, _>("config_hash"),
            None => {
                println!(
                    "[HashAggregator] WARN: Agent {} metadata missing during root hash calc",
                    agent_id
                );
                "EMPTY_CONFIG".to_string()
            }
        };

        let topic_rows = sqlx::query(
            "SELECT content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(agent_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = vec![config_hash];
        for r in topic_rows {
            hashes.push(r.get("content_hash"));
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_group_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<String, String> {
        let group_row = sqlx::query("SELECT config_hash FROM groups WHERE group_id = ?")
            .bind(group_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        let config_hash = match group_row {
            Some(r) => r.get::<String, _>("config_hash"),
            None => {
                println!(
                    "[HashAggregator] WARN: Group {} metadata missing during root hash calc",
                    group_id
                );
                "EMPTY_CONFIG".to_string()
            }
        };

        let topic_rows = sqlx::query(
            "SELECT content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = vec![config_hash];
        for r in topic_rows {
            hashes.push(r.get("content_hash"));
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn bubble_topic_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_topic_root_hash(tx, topic_id).await?;
        sqlx::query("UPDATE topics SET content_hash = ? WHERE topic_id = ?")
            .bind(root_hash)
            .bind(topic_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn bubble_agent_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_agent_root_hash(tx, agent_id).await?;
        sqlx::query("UPDATE agents SET content_hash = ? WHERE agent_id = ?")
            .bind(root_hash)
            .bind(agent_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn bubble_group_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_group_root_hash(tx, group_id).await?;
        sqlx::query("UPDATE groups SET content_hash = ? WHERE group_id = ?")
            .bind(root_hash)
            .bind(group_id)
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn bubble_from_topic(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        Self::bubble_topic_hash(tx, topic_id).await?;

        let topic_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        let owner_id: String = topic_row.get("owner_id");
        let owner_type: String = topic_row.get("owner_type");

        if owner_type == "agent" {
            Self::bubble_agent_hash(tx, &owner_id).await?;
        } else if owner_type == "group" {
            Self::bubble_group_hash(tx, &owner_id).await?;
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
        let row = sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(r) = row {
            let config_hash: String = r.get("config_hash");
            if config_hash.is_empty() || config_hash == "PENDING" {
                let dto = Self::load_agent_dto(tx, agent_id).await?;
                let new_hash = HashAggregator::compute_agent_config_hash(&dto);
                sqlx::query("UPDATE agents SET config_hash = ? WHERE agent_id = ?")
                    .bind(&new_hash)
                    .bind(agent_id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| e.to_string())?;
                println!(
                    "[HashInitializer] Initialized config_hash for Agent {}",
                    agent_id
                );
            }
        }

        Ok(())
    }

    pub async fn ensure_group_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let row = sqlx::query("SELECT config_hash FROM groups WHERE group_id = ?")
            .bind(group_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(r) = row {
            let config_hash: String = r.get("config_hash");
            if config_hash.is_empty() || config_hash == "PENDING" {
                let dto = Self::load_group_dto(tx, group_id).await?;
                let new_hash = HashAggregator::compute_group_config_hash(&dto);
                sqlx::query("UPDATE groups SET config_hash = ? WHERE group_id = ?")
                    .bind(&new_hash)
                    .bind(group_id)
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| e.to_string())?;
                println!(
                    "[HashInitializer] Initialized config_hash for Group {}",
                    group_id
                );
            }
        }

        Ok(())
    }

    pub async fn ensure_all_agent_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let rows = sqlx::query(
            "SELECT agent_id FROM agents WHERE config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING'",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        if rows.is_empty() {
            return Ok(());
        }

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        for row in rows {
            let agent_id: String = row.get("agent_id");
            if let Err(e) = Self::ensure_agent_hashes(&mut tx, &agent_id).await {
                println!(
                    "[HashInitializer] Failed to ensure hash for Agent {}: {}",
                    agent_id, e
                );
            }
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        println!("[HashInitializer] Ensured all Agent hashes");
        Ok(())
    }

    pub async fn ensure_all_group_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let rows = sqlx::query(
            "SELECT group_id FROM groups WHERE config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING'",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        if rows.is_empty() {
            return Ok(());
        }

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        for row in rows {
            let group_id: String = row.get("group_id");
            if let Err(e) = Self::ensure_group_hashes(&mut tx, &group_id).await {
                println!(
                    "[HashInitializer] Failed to ensure hash for Group {}: {}",
                    group_id, e
                );
            }
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        println!("[HashInitializer] Ensured all Group hashes");
        Ok(())
    }

    async fn load_agent_dto(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<AgentSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output FROM agents WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentSyncDTO {
            name: row.get("name"),
            system_prompt: row.get("system_prompt"),
            model: row.get("model"),
            temperature: row.get::<f64, _>("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i64, _>("stream_output") != 0,
        })
    }

    async fn load_group_dto(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<GroupSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, mode, group_prompt, invite_prompt, use_unified_model, unified_model, tag_match_mode, created_at FROM groups WHERE group_id = ?",
        )
        .bind(group_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let members = Self::load_group_members(tx, group_id).await?;
        let member_tags = Self::load_member_tags(tx, group_id).await;

        Ok(GroupSyncDTO {
            name: row.get("name"),
            members,
            mode: row.get("mode"),
            member_tags: Some(member_tags),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i64, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            tag_match_mode: row.get("tag_match_mode"),
            created_at: row.get("created_at"),
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

        Ok(rows.iter().map(|r| r.get("agent_id")).collect())
    }

    async fn load_member_tags(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> serde_json::Value {
        let rows = sqlx::query(
            "SELECT agent_id, member_tag FROM group_members WHERE group_id = ? AND member_tag IS NOT NULL",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .unwrap_or_default();

        let mut tags = serde_json::Map::new();
        for row in rows {
            let agent_id: String = row.get("agent_id");
            let tag: String = row.get("member_tag");
            tags.insert(agent_id, serde_json::Value::String(tag));
        }

        serde_json::Value::Object(tags)
    }
}
