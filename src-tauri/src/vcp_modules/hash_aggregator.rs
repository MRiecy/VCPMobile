use crate::vcp_modules::sync_types::{compute_deterministic_hash, compute_merkle_root};
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO};
use sqlx::{Sqlite, Transaction, Row};
use sha2::{Digest, Sha256};

/// =================================================================
/// vcp_modules/hash_aggregator.rs - 聚合哈希计算与冒泡更新中心
/// =================================================================

pub struct HashAggregator;

impl HashAggregator {
    // === 内容指纹计算 (纯函数) ===

    /// 计算消息的指纹 (与桌面端 JS 逻辑严格对齐)
    pub fn compute_message_fingerprint(content: &str, attachment_hashes: &[String]) -> String {
        let mut sorted_hashes = attachment_hashes.to_vec();
        sorted_hashes.sort();
        
        let mut fingerprint_map = serde_json::Map::new();
        fingerprint_map.insert("content".to_string(), serde_json::Value::String(content.to_string()));
        if !sorted_hashes.is_empty() {
            fingerprint_map.insert("attachmentHashes".to_string(), serde_json::to_value(sorted_hashes).unwrap());
        }
        
        compute_deterministic_hash(&serde_json::Value::Object(fingerprint_map))
    }

    /// 计算 Agent Topic 元数据哈希
    pub fn compute_agent_topic_metadata_hash(dto: &AgentTopicSyncDTO) -> String {
        compute_deterministic_hash(dto)
    }

    /// 计算 Group Topic 元数据哈希
    pub fn compute_group_topic_metadata_hash(dto: &GroupTopicSyncDTO) -> String {
        compute_deterministic_hash(dto)
    }

    /// 计算智能体配置哈希
    pub fn compute_agent_config_hash(dto: &AgentSyncDTO) -> String {
        compute_deterministic_hash(dto)
    }

    /// 计算群组配置哈希
    pub fn compute_group_config_hash(dto: &GroupSyncDTO) -> String {
        compute_deterministic_hash(dto)
    }

    /// 计算头像哈希
    pub fn compute_avatar_hash(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    // === Manifest 聚合 (用于 sync_manager 阶段一广播) ===

    pub fn aggregate_agent_manifest_hash(config_hash: &str, content_hash: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(config_hash.as_bytes());
        hasher.update(content_hash.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn aggregate_group_manifest_hash(config_hash: &str, content_hash: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(config_hash.as_bytes());
        hasher.update(content_hash.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn aggregate_topic_manifest_hash(metadata_hash: &str, content_hash: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(metadata_hash.as_bytes());
        hasher.update(content_hash.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // === 聚合哈希计算 (读取 DB) ===

    /// 重新计算话题的聚合哈希
    pub async fn compute_topic_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<String, String> {
        let rows = sqlx::query("SELECT content_hash FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY msg_id ASC")
            .bind(topic_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;

        let hashes: Vec<String> = rows.iter().map(|r| r.get::<String, _>("content_hash")).collect();
        Ok(compute_merkle_root(hashes))
    }

    /// 重新计算 Agent 的聚合哈希 (Config + Topics)
    pub async fn compute_agent_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<String, String> {
        let agent_row = sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ?")
            .bind(agent_id).fetch_optional(&mut **tx).await.map_err(|e| e.to_string())?;
        
        let config_hash = match agent_row {
            Some(r) => r.get::<String, _>("config_hash"),
            None => {
                println!("[HashAggregator] WARN: Agent {} metadata missing during root hash calc", agent_id);
                "EMPTY_CONFIG".to_string()
            }
        };

        let topic_rows = sqlx::query("SELECT content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL ORDER BY topic_id ASC")
            .bind(agent_id).fetch_all(&mut **tx).await.map_err(|e| e.to_string())?;
        
        let mut hashes = vec![config_hash];
        for r in topic_rows { hashes.push(r.get("content_hash")); }

        Ok(compute_merkle_root(hashes))
    }

    /// 重新计算 Group 的聚合哈希 (Config + Topics)
    pub async fn compute_group_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<String, String> {
        let group_row = sqlx::query("SELECT config_hash FROM groups WHERE group_id = ?")
            .bind(group_id).fetch_optional(&mut **tx).await.map_err(|e| e.to_string())?;
        
        let config_hash = match group_row {
            Some(r) => r.get::<String, _>("config_hash"),
            None => {
                println!("[HashAggregator] WARN: Group {} metadata missing during root hash calc", group_id);
                "EMPTY_CONFIG".to_string()
            }
        };

        let topic_rows = sqlx::query("SELECT content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL ORDER BY topic_id ASC")
            .bind(group_id).fetch_all(&mut **tx).await.map_err(|e| e.to_string())?;
        
        let mut hashes = vec![config_hash];
        for r in topic_rows { hashes.push(r.get("content_hash")); }

        Ok(compute_merkle_root(hashes))
    }

    // === 冒泡更新 (写入 DB) ===

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

    /// 从 Topic 直接冒泡到其所有者 (Agent 或 Group)
    pub async fn bubble_from_topic(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        // 1. 冒泡更新 Topic 本身
        Self::bubble_topic_hash(tx, topic_id).await?;

        // 2. 获取 Owner 信息
        let topic_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        
        let owner_id: String = topic_row.get("owner_id");
        let owner_type: String = topic_row.get("owner_type");

        // 3. 向上冒泡到 Agent 或 Group
        if owner_type == "agent" {
            Self::bubble_agent_hash(tx, &owner_id).await?;
        } else if owner_type == "group" {
            Self::bubble_group_hash(tx, &owner_id).await?;
        }

        Ok(())
    }
}
