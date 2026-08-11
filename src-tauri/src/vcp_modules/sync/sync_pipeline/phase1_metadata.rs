use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashSet;

const SQLITE_BIND_CHUNK: usize = 400;

pub struct Phase1Metadata;

impl Phase1Metadata {
    pub async fn build_agent_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT agent_id, config_hash, content_hash, updated_at, deleted_at 
             FROM agents",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let conf_h: String = r.get("config_hash");
            let cont_h: String = r.get("content_hash");
            items.push(EntityState {
                id: r.get("agent_id"),
                hash: conf_h.clone(), // 兼容旧版，默认使用 config_hash
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r.get("updated_at"),
                deleted_at: r.get("deleted_at"),
                owner_type: None,
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
             FROM groups",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let conf_h: String = r.get("config_hash");
            let cont_h: String = r.get("content_hash");
            items.push(EntityState {
                id: r.get("group_id"),
                hash: conf_h.clone(),
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r.get("updated_at"),
                deleted_at: r.get("deleted_at"),
                owner_type: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Group,
            items,
        })
    }

    pub async fn build_targeted_topic_manifest(
        pool: &SqlitePool,
        owners: &[String],
    ) -> Result<SyncManifest, String> {
        if owners.is_empty() {
            return Ok(SyncManifest {
                data_type: SyncDataType::Topic,
                items: Vec::new(),
            });
        }

        let expected_owners = owners.iter().cloned().collect::<HashSet<_>>();
        if expected_owners.len() != owners.len() || expected_owners.iter().any(|id| id.is_empty()) {
            return Err("Targeted topic manifest contains empty or duplicate owner ids".into());
        }

        let mut items = Vec::new();
        let mut seen_topics = HashSet::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let placeholders = owner_chunk
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, deleted_at
                 FROM topics WHERE owner_id IN ({})",
                placeholders
            );
            let mut query = sqlx::query(&query_str);
            for owner_id in owner_chunk {
                query = query.bind(owner_id);
            }
            for row in query.fetch_all(pool).await.map_err(|e| e.to_string())? {
                let id: String = row.get("topic_id");
                if id == "default" {
                    continue;
                }
                if !seen_topics.insert(id.clone()) {
                    return Err(format!(
                        "Targeted topic manifest returned duplicate topic {id}"
                    ));
                }
                let config_hash: String = row.get("config_hash");
                let content_hash: String = row.get("content_hash");
                items.push(EntityState {
                    id,
                    hash: config_hash.clone(),
                    config_hash: Some(config_hash),
                    content_hash: Some(content_hash),
                    ts: row.get("updated_at"),
                    deleted_at: row.get("deleted_at"),
                    owner_type: row.get("owner_type"),
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
            let owner_type: String = r.get("owner_type");
            let owner_id: String = r.get("owner_id");
            items.push(EntityState {
                id: format!("{}:{}", owner_type, owner_id),
                hash: r.get("avatar_hash"),
                config_hash: None,
                content_hash: None,
                ts: r.get("updated_at"),
                deleted_at: r.get("deleted_at"),
                owner_type: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Avatar,
            items,
        })
    }

    pub async fn build_phase1_manifests(pool: &SqlitePool) -> Result<Vec<SyncManifest>, String> {
        let mut manifests = Vec::new();
        manifests.push(Self::build_agent_manifest(pool).await?);
        manifests.push(Self::build_group_manifest(pool).await?);
        manifests.push(Self::build_avatar_manifest(pool).await?);
        Ok(manifests)
    }
}

#[cfg(test)]
mod tests {
    use super::Phase1Metadata;

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
             INSERT INTO avatars VALUES ('agent-a', 'agent', 'hash', 10, 9);",
        )
        .execute(&pool)
        .await
        .expect("create avatar fixture");

        let manifest = Phase1Metadata::build_avatar_manifest(&pool)
            .await
            .expect("build avatar manifest");
        assert_eq!(manifest.items.len(), 1);
        assert_eq!(manifest.items[0].id, "agent:agent-a");
        assert_eq!(manifest.items[0].deleted_at, Some(9));
    }
}
