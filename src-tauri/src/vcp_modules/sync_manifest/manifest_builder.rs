use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use sqlx::Row;
use sqlx::SqlitePool;

pub struct ManifestBuilder;

impl ManifestBuilder {
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

        let placeholders = owners.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let query_str = format!(
            "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, deleted_at 
             FROM topics WHERE owner_id IN ({})",
            placeholders
        );

        let mut q = sqlx::query(&query_str);
        for owner_id in owners {
            q = q.bind(owner_id);
        }

        let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r.get("topic_id");
            if id == "default" {
                continue;
            }
            let conf_h: String = r.get("config_hash");
            let cont_h: String = r.get("content_hash");

            items.push(EntityState {
                id,
                hash: conf_h.clone(),
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r.get("updated_at"),
                deleted_at: r.get("deleted_at"),
                owner_type: r.get("owner_type"),
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Topic,
            items,
        })
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT owner_id, owner_type, avatar_hash, updated_at 
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
                deleted_at: None,
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
