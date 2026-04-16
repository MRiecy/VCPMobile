use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use sqlx::SqlitePool;
use sqlx::Row;

pub struct Phase2Topic;

impl Phase2Topic {
    pub async fn build_topic_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, content_hash, updated_at, owner_id, owner_type 
             FROM topics 
             WHERE deleted_at IS NULL"
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r.get("topic_id");
            let owner_type: String = r.get("owner_type");
            
            let hash = if owner_type == "group" {
                let dto = GroupTopicSyncDTO {
                    id: id.clone(),
                    name: r.get("title"),
                    created_at: r.get("created_at"),
                    owner_id: r.get("owner_id"),
                };
                HashAggregator::aggregate_topic_manifest_hash(
                    &HashAggregator::compute_group_topic_metadata_hash(&dto),
                    r.get("content_hash"),
                )
            } else {
                let dto = AgentTopicSyncDTO {
                    id: id.clone(),
                    name: r.get("title"),
                    created_at: r.get("created_at"),
                    locked: r.get::<i64, _>("locked") != 0,
                    unread: r.get::<i64, _>("unread") != 0,
                    owner_id: r.get("owner_id"),
                };
                HashAggregator::aggregate_topic_manifest_hash(
                    &HashAggregator::compute_agent_topic_metadata_hash(&dto),
                    r.get("content_hash"),
                )
            };

            items.push(EntityState {
                id,
                hash,
                ts: r.get("updated_at"),
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Topic,
            items,
        })
    }

    pub async fn get_topic_ids_by_owner(pool: &SqlitePool, owner_id: &str, owner_type: &str) -> Result<Vec<String>, String> {
        let rows = sqlx::query(
            "SELECT topic_id FROM topics WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL"
        )
        .bind(owner_id)
        .bind(owner_type)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(rows.iter().map(|r| r.get("topic_id")).collect())
    }

    pub async fn get_all_topic_ids(pool: &SqlitePool) -> Result<Vec<String>, String> {
        let rows = sqlx::query("SELECT topic_id FROM topics WHERE deleted_at IS NULL")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(rows.iter().map(|r| r.get("topic_id")).collect())
    }
}
