use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_pipeline::phase2_topic::Phase2Topic;
use sqlx::Row;
use sqlx::SqlitePool;

pub struct Phase3Message;

impl Phase3Message {
    pub async fn get_all_active_topic_ids(pool: &SqlitePool) -> Result<Vec<String>, String> {
        Phase2Topic::get_all_topic_ids(pool).await
    }

    pub async fn compute_local_message_hashes(
        pool: &SqlitePool,
        topic_id: &str,
    ) -> Result<std::collections::HashMap<String, (String, i64, Vec<String>)>, String> {
        let rows = sqlx::query(
            "SELECT m.msg_id, m.content, m.updated_at, a.hash as att_hash 
             FROM messages m 
             LEFT JOIN message_attachments ma ON m.msg_id = ma.msg_id 
             LEFT JOIN attachments a ON ma.hash = a.hash 
             WHERE m.topic_id = ? AND m.deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut local_map = std::collections::HashMap::new();
        for r in rows {
            let id: String = r.get("msg_id");
            let entry =
                local_map
                    .entry(id)
                    .or_insert((r.get("content"), r.get("updated_at"), Vec::new()));
            if let Some(h) = r.get::<Option<String>, _>("att_hash") {
                entry.2.push(h);
            }
        }

        Ok(local_map)
    }

    pub async fn compute_message_diff(
        pool: &SqlitePool,
        topic_id: &str,
        remote_msgs: &[serde_json::Value],
    ) -> Result<(Vec<String>, bool), String> {
        let local_map = Self::compute_local_message_hashes(pool, topic_id).await?;

        let mut to_pull_ids = Vec::new();
        let mut to_push = false;
        let mut remote_ids = std::collections::HashSet::new();

        for rm in remote_msgs {
            let rid = rm["msgId"].as_str().unwrap_or_default().to_string();
            remote_ids.insert(rid.clone());

            if let Some((lcontent, lts, latts)) = local_map.get(&rid) {
                let local_hash = HashAggregator::compute_message_fingerprint(lcontent, latts);
                let remote_hash = rm["contentHash"].as_str().unwrap_or_default();

                if local_hash != remote_hash {
                    let remote_ts = rm["updatedAt"].as_i64().unwrap_or(0);
                    if remote_ts > *lts {
                        to_pull_ids.push(rid);
                    } else {
                        to_push = true;
                    }
                }
            } else {
                to_pull_ids.push(rid);
            }
        }

        for lid in local_map.keys() {
            if !remote_ids.contains(lid) {
                to_push = true;
                break;
            }
        }

        Ok((to_pull_ids, to_push))
    }
}
