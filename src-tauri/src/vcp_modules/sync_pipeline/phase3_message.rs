use crate::vcp_modules::sync_pipeline::phase2_topic::Phase2Topic;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_dto::{UserMessageSyncDTO, AgentMessageSyncDTO, GroupMessageSyncDTO};
use crate::vcp_modules::sync_utils::query_avatar_color;
use sqlx::SqlitePool;
use sqlx::Row;
use tokio::sync::mpsc;

pub struct Phase3Message;

impl Phase3Message {
    pub async fn get_all_active_topic_ids(pool: &SqlitePool) -> Result<Vec<String>, String> {
        Phase2Topic::get_all_topic_ids(pool).await
    }

    pub async fn build_message_manifest(pool: &SqlitePool, topic_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let rows = sqlx::query(
            "SELECT m.msg_id, m.content, m.updated_at, GROUP_CONCAT(a.hash) as att_hashes
             FROM messages m
             LEFT JOIN message_attachments ma ON m.msg_id = ma.msg_id
             LEFT JOIN attachments a ON ma.hash = a.hash
             WHERE m.topic_id = ? AND m.deleted_at IS NULL
             GROUP BY m.msg_id"
        )
        .bind(topic_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut manifest = Vec::new();
        for r in rows {
            let msg_id: String = r.get("msg_id");
            let content: String = r.get("content");
            let updated_at: i64 = r.get("updated_at");
            
            let att_hashes: Option<String> = r.get("att_hashes");
            let att_hash_vec: Vec<String> = att_hashes
                .map(|s| s.split(',').map(|h| h.to_string()).collect())
                .unwrap_or_default();

            let content_hash = HashAggregator::compute_message_fingerprint(&content, &att_hash_vec);

            manifest.push(serde_json::json!({
                "msgId": msg_id,
                "contentHash": content_hash,
                "updatedAt": updated_at
            }));
        }

        Ok(manifest)
    }

    pub async fn request_desktop_manifest(ws_tx: &mpsc::UnboundedSender<serde_json::Value>, topic_id: &str) {
        let msg = serde_json::json!({
            "type": "GET_MESSAGE_MANIFEST",
            "topicId": topic_id
        });
        let _ = ws_tx.send(msg);
    }

    pub async fn build_message_dtos(
        pool: &SqlitePool,
        messages: &[ChatMessage],
        owner_type: &str,
    ) -> Vec<serde_json::Value> {
        let mut results = Vec::new();

        for msg in messages {
            let msg_value = if msg.role == "user" {
                let dto = UserMessageSyncDTO::from(msg);
                serde_json::to_value(dto).ok()
            } else if owner_type == "group" {
                let avatar_color = query_avatar_color(pool, &msg.agent_id.clone().unwrap_or_default()).await;
                let dto = GroupMessageSyncDTO::from_message(msg, avatar_color);
                serde_json::to_value(dto).ok()
            } else {
                let avatar_color = query_avatar_color(pool, &msg.agent_id.clone().unwrap_or_default()).await;
                let dto = AgentMessageSyncDTO::from_message(msg, avatar_color);
                serde_json::to_value(dto).ok()
            };

            if let Some(v) = msg_value {
                results.push(v);
            }
        }

        results
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
             WHERE m.topic_id = ? AND m.deleted_at IS NULL"
        )
        .bind(topic_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut local_map = std::collections::HashMap::new();
        for r in rows {
            let id: String = r.get("msg_id");
            let entry = local_map.entry(id).or_insert((
                r.get("content"),
                r.get("updated_at"),
                Vec::new(),
            ));
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
