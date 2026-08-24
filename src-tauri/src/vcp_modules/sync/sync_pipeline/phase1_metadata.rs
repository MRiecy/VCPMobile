use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, EntityState, SyncDataType, SyncManifest,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashSet;

const SQLITE_BIND_CHUNK: usize = 400;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_agent_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT agent_id, config_hash, content_hash, updated_at, deleted_at 
             FROM agents WHERE owner_type = 'agent'",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r
                .try_get("agent_id")
                .map_err(|error| format!("Agent manifest id decode failed: {error}"))?;
            let conf_h: String = r.try_get("config_hash").map_err(|error| {
                format!("Agent manifest config hash decode failed for {id}: {error}")
            })?;
            let cont_h: String = r.try_get("content_hash").map_err(|error| {
                format!("Agent manifest content hash decode failed for {id}: {error}")
            })?;
            items.push(EntityState {
                id,
                hash: conf_h.clone(), // 兼容旧版，默认使用 config_hash
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Agent manifest timestamp decode failed: {error}"))?,
                deleted_at: r
                    .try_get("deleted_at")
                    .map_err(|error| format!("Agent manifest tombstone decode failed: {error}"))?,
                owner_type: None,
                owner_id: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Agent,
            items,
        })
    }

    pub async fn build_group_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT group_id, config_hash, content_hash, updated_at, deleted_at 
             FROM groups WHERE owner_type = 'group'",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r
                .try_get("group_id")
                .map_err(|error| format!("Group manifest id decode failed: {error}"))?;
            let conf_h: String = r.try_get("config_hash").map_err(|error| {
                format!("Group manifest config hash decode failed for {id}: {error}")
            })?;
            let cont_h: String = r.try_get("content_hash").map_err(|error| {
                format!("Group manifest content hash decode failed for {id}: {error}")
            })?;
            items.push(EntityState {
                id,
                hash: conf_h.clone(),
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Group manifest timestamp decode failed: {error}"))?,
                deleted_at: r
                    .try_get("deleted_at")
                    .map_err(|error| format!("Group manifest tombstone decode failed: {error}"))?,
                owner_type: None,
                owner_id: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Group,
            items,
        })
    }

    pub async fn build_targeted_topic_manifest(
        pool: &SqlitePool,
        owners: &[OwnerKey],
    ) -> Result<SyncManifest, String> {
        if owners.is_empty() {
            return Ok(SyncManifest {
                data_type: SyncDataType::Topic,
                items: Vec::new(),
            });
        }

        let expected_owners = owners.iter().cloned().collect::<HashSet<_>>();
        if expected_owners.len() != owners.len()
            || expected_owners.iter().any(|owner| {
                !matches!(owner.owner_type.as_str(), "agent" | "group") || owner.owner_id.is_empty()
            })
        {
            return Err("Targeted topic manifest contains invalid or duplicate owners".into());
        }

        let mut items = Vec::new();
        let mut seen_topics = HashSet::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let predicates = owner_chunk
                .iter()
                .map(|_| "(owner_type = ? AND owner_id = ?)")
                .collect::<Vec<_>>()
                .join(" OR ");
            let query_str = format!(
                "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, owner_id, deleted_at
                 FROM topics WHERE {predicates}"
            );
            let mut query = sqlx::query(&query_str);
            for owner in owner_chunk {
                query = query.bind(&owner.owner_type).bind(&owner.owner_id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let id: String = row
                    .try_get("topic_id")
                    .map_err(|error| format!("Topic manifest id decode failed: {error}"))?;
                let config_hash: String = row.try_get("config_hash").map_err(|error| {
                    format!("Topic manifest config hash decode failed for {id}: {error}")
                })?;
                let content_hash: String = row.try_get("content_hash").map_err(|error| {
                    format!("Topic manifest content hash decode failed for {id}: {error}")
                })?;
                let owner_type: String = row.try_get("owner_type").map_err(|error| {
                    format!("Topic manifest owner type decode failed for {id}: {error}")
                })?;
                if !matches!(owner_type.as_str(), "agent" | "group") {
                    return Err(format!(
                        "Topic manifest {id} has unsupported owner type {owner_type}"
                    ));
                }
                let owner_id: String = row.try_get("owner_id").map_err(|error| {
                    format!("Topic manifest owner id decode failed for {id}: {error}")
                })?;
                let topic_key = TopicKey::new(&owner_type, &owner_id, &id);
                if owner_id.is_empty()
                    || !expected_owners.contains(&OwnerKey::new(&owner_type, &owner_id))
                {
                    return Err(format!(
                        "Topic manifest {id} returned unexpected owner {owner_type}/{owner_id}"
                    ));
                }
                if !seen_topics.insert(topic_key) {
                    return Err(format!(
                        "Targeted topic manifest returned duplicate topic {owner_type}/{owner_id}/{id}"
                    ));
                }
                let updated_at = row.try_get("updated_at").map_err(|error| {
                    format!("Topic manifest timestamp decode failed for {id}: {error}")
                })?;
                let deleted_at = row.try_get("deleted_at").map_err(|error| {
                    format!("Topic manifest tombstone decode failed for {id}: {error}")
                })?;
                items.push(EntityState {
                    id,
                    hash: config_hash.clone(),
                    config_hash: Some(config_hash),
                    content_hash: Some(content_hash),
                    ts: updated_at,
                    deleted_at,
                    owner_type: Some(owner_type),
                    owner_id: Some(owner_id),
                });
            }
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Topic,
            items,
        })
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT owner_id, owner_type, avatar_hash, updated_at, deleted_at
             FROM avatars",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let owner_type: String = r
                .try_get("owner_type")
                .map_err(|error| format!("Avatar manifest owner type decode failed: {error}"))?;
            let owner_id: String = r
                .try_get("owner_id")
                .map_err(|error| format!("Avatar manifest owner id decode failed: {error}"))?;
            if !is_valid_avatar_owner(&owner_type, &owner_id) {
                return Err(format!(
                    "Avatar manifest has invalid owner {owner_type}/{owner_id}"
                ));
            }
            let deleted_at: Option<i64> = r
                .try_get("deleted_at")
                .map_err(|error| format!("Avatar manifest tombstone decode failed: {error}"))?;
            let parent_is_live = deleted_at.is_some() || match owner_type.as_str() {
                "agent" => sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM agents WHERE owner_type = 'agent' AND agent_id = ? AND deleted_at IS NULL)",
                )
                .bind(&owner_id)
                .fetch_one(pool)
                .await
                .map_err(|error| {
                    format!("Avatar manifest agent owner lookup failed for {owner_id}: {error}")
                })?,
                "group" => sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM groups WHERE owner_type = 'group' AND group_id = ? AND deleted_at IS NULL)",
                )
                .bind(&owner_id)
                .fetch_one(pool)
                .await
                .map_err(|error| {
                    format!("Avatar manifest group owner lookup failed for {owner_id}: {error}")
                })?,
                "user" => true,
                _ => false,
            };
            if !parent_is_live {
                return Err(format!(
                    "Avatar manifest owner {owner_type}/{owner_id} is missing or deleted"
                ));
            }
            items.push(EntityState {
                id: format!("{}:{}", owner_type, owner_id),
                hash: r.try_get("avatar_hash").map_err(|error| {
                    format!(
                        "Avatar manifest hash decode failed for {owner_type}/{owner_id}: {error}"
                    )
                })?,
                config_hash: None,
                content_hash: None,
                ts: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Avatar manifest timestamp decode failed: {error}"))?,
                deleted_at,
                owner_type: None,
                owner_id: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Avatar,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Phase1Metadata;
    use crate::vcp_modules::topic_types::OwnerKey;

    #[tokio::test]
    async fn avatar_manifest_preserves_tombstones() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE avatars (
                owner_id TEXT, owner_type TEXT, avatar_hash TEXT,
                updated_at INTEGER, deleted_at INTEGER
             );
             INSERT INTO avatars VALUES
                ('agent-a', 'agent', 'hash', 10, 9),
                ('user_avatar', 'user', 'user-hash', 11, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create avatar fixture");

        let manifest = Phase1Metadata::build_avatar_manifest(&pool)
            .await
            .expect("build avatar manifest");
        assert_eq!(manifest.items.len(), 2);
        assert_eq!(manifest.items[0].id, "agent:agent-a");
        assert_eq!(manifest.items[0].deleted_at, Some(9));
        assert_eq!(manifest.items[1].id, "user:user_avatar");
        assert_eq!(manifest.items[1].deleted_at, None);
    }

    #[tokio::test]
    async fn targeted_topic_manifest_carries_exact_owner_identity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                config_hash TEXT, content_hash TEXT, updated_at INTEGER,
                deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             INSERT INTO topics VALUES
                ('agent', 'agent-a', 'topic-a', 'config-hash',
                 'content-hash', 10, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create topic fixture");

        let manifest = Phase1Metadata::build_targeted_topic_manifest(
            &pool,
            &[OwnerKey::new("agent", "agent-a")],
        )
        .await
        .expect("build topic manifest");
        assert_eq!(manifest.items.len(), 1);
        assert_eq!(manifest.items[0].owner_type.as_deref(), Some("agent"));
        assert_eq!(manifest.items[0].owner_id.as_deref(), Some("agent-a"));
    }
}
