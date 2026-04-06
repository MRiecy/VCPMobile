use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_service;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicFingerprint {
    pub topic_id: String,
    pub revision: i64,
    pub updated_at: i64,
    pub msg_count: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicDelta {
    pub added: Vec<ChatMessage>,
    pub updated: Vec<ChatMessage>,
    pub deleted_ids: Vec<String>,
    pub order_changed: bool,
    pub sync_skipped: bool,
}

pub async fn get_topic_fingerprint_internal(
    app_handle: &AppHandle,
    _item_id: &str,
    topic_id: &str,
) -> Result<TopicFingerprint, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let row_res = sqlx::query(
        "SELECT revision, updated_at, msg_count 
         FROM topics 
         WHERE topic_id = ?"
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    if let Some(row) = row_res {
        use sqlx::Row;
        Ok(TopicFingerprint {
            topic_id: topic_id.to_string(),
            revision: row.get("revision"),
            updated_at: row.get("updated_at"),
            msg_count: row.get("msg_count"),
        })
    } else {
        Ok(TopicFingerprint {
            topic_id: topic_id.to_string(),
            revision: 0,
            updated_at: 0,
            msg_count: 0,
        })
    }
}

pub async fn get_topic_delta_internal(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
    current_history: Vec<ChatMessage>,
    fingerprint: Option<TopicFingerprint>,
) -> Result<TopicDelta, String> {
    // 1. 指纹快速路径 (Fingerprint Fast-path)
    if let Some(fp) = fingerprint {
        let current_fp = get_topic_fingerprint_internal(app_handle, item_id, topic_id).await?;
        if current_fp.revision == fp.revision && current_fp.msg_count == current_history.len() as i32 {
            println!(
                "[VCPCore] Sync skipped for {}: revision matches.",
                topic_id
            );
            return Ok(TopicDelta {
                added: vec![],
                updated: vec![],
                deleted_ids: vec![],
                order_changed: false,
                sync_skipped: true,
            });
        }
    }

    // 3. 读取数据库 中的全量历史记录并应用比对
    let mut new_history = message_service::load_chat_history_internal(app_handle, item_id, topic_id, None, None).await?;

    // 4. 构建索引以便快速比对
    let old_map: HashMap<String, ChatMessage> = current_history
        .iter()
        .map(|m| (m.id.clone(), m.clone()))
        .collect();

    let mut added = Vec::new();
    let mut updated = Vec::new();
    let mut deleted_ids = Vec::new();
    let mut new_ids_set = HashSet::new();
    let new_ids_seq: Vec<String> = new_history.iter().map(|m| m.id.clone()).collect();

    // 5. 找出新增和修改的消息
    for new_msg in new_history.iter_mut() {
        new_ids_set.insert(new_msg.id.clone());

        match old_map.get(&new_msg.id) {
            Some(old_msg) => {
                if old_msg.content != new_msg.content || old_msg.role != new_msg.role {
                    updated.push(new_msg.clone());
                }
            }
            None => {
                added.push(new_msg.clone());
            }
        }
    }

    // 6. 找出已删除的消息
    for id in old_map.keys() {
        if !new_ids_set.contains(id) {
            deleted_ids.push(id.clone());
        }
    }

    let old_ids_still_present: Vec<String> = current_history
        .iter()
        .map(|m| m.id.clone())
        .filter(|id| new_ids_set.contains(id))
        .collect();

    let new_ids_already_present: Vec<String> = new_ids_seq
        .iter()
        .filter(|id| old_map.contains_key(*id))
        .cloned()
        .collect();

    let order_changed = old_ids_still_present != new_ids_already_present;

    Ok(TopicDelta {
        added,
        updated,
        deleted_ids,
        order_changed,
        sync_skipped: false,
    })
}
