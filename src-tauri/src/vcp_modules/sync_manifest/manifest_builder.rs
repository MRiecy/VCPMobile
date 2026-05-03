use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_hash::HashAggregator;
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
            let h: String = r.get("config_hash");
            items.push(EntityState {
                id: r.get("agent_id"),
                hash: h,
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
            let h: String = r.get("config_hash");
            items.push(EntityState {
                id: r.get("group_id"),
                hash: h,
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

    pub async fn build_topic_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, content_hash, updated_at, owner_id, owner_type, deleted_at 
             FROM topics"
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r.get("topic_id");
            if id == "default" {
                continue;
            }
            let owner_type: String = r.get("owner_type");

            let hash = if owner_type == "group" {
                let dto = GroupTopicSyncDTO {
                    id: id.clone(),
                    name: r.get("title"),
                    created_at: r.get("created_at"),
                    owner_id: r.get("owner_id"),
                };
                HashAggregator::compute_group_topic_metadata_hash(&dto)
            } else {
                let dto = AgentTopicSyncDTO {
                    id: id.clone(),
                    name: r.get("title"),
                    created_at: r.get("created_at"),
                    locked: r.get::<i64, _>("locked") != 0,
                    unread: r.get::<i64, _>("unread") != 0,
                    owner_id: r.get("owner_id"),
                };
                HashAggregator::compute_agent_topic_metadata_hash(&dto)
            };

            items.push(EntityState {
                id,
                hash,
                ts: r.get("updated_at"),
                deleted_at: r.get("deleted_at"),
                owner_type: Some(owner_type),
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
