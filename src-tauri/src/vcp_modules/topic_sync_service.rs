use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::history_repository_fs;
use crate::vcp_modules::path_topology_service::resolve_history_path;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicFingerprint {
    pub topic_id: String,
    pub mtime: u64,
    pub size: u64,
    pub msg_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicDelta {
    pub added: Vec<ChatMessage>,
    pub updated: Vec<ChatMessage>,
    pub deleted_ids: Vec<String>,
    pub order_changed: bool,
    pub sync_skipped: bool,
}

pub fn get_topic_fingerprint_internal(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
) -> Result<TopicFingerprint, String> {
    let history_path = resolve_history_path(app_handle, item_id, topic_id);

    if !history_path.exists() {
        return Ok(TopicFingerprint {
            topic_id: topic_id.to_string(),
            mtime: 0,
            size: 0,
            msg_count: 0,
        });
    }

    let metadata = fs::metadata(&history_path).map_err(|e| e.to_string())?;
    let mtime = metadata
        .modified()
        .unwrap_or(SystemTime::now())
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 为了获取消息总数，我们仍然需要读取文件，但这里的目的是为了极速对比。
    // 在 VCP 架构中，影子数据库 topic_index 其实已经存了 msg_count，
    // 我们优先从文件系统获取基础元数据。
    let history = history_repository_fs::read_history(&history_path)?;

    Ok(TopicFingerprint {
        topic_id: topic_id.to_string(),
        mtime,
        size: metadata.len(),
        msg_count: history.len(),
    })
}

pub fn get_topic_delta_internal(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
    current_history: Vec<ChatMessage>,
    fingerprint: Option<TopicFingerprint>,
) -> Result<TopicDelta, String> {
    let history_path = resolve_history_path(app_handle, item_id, topic_id);

    // 1. 指纹快速路径 (Fingerprint Fast-path)
    // 如果前端传了指纹，且与磁盘元数据一致，直接跳过全量比对
    if let Some(fp) = fingerprint {
        if history_path.exists() {
            let metadata = fs::metadata(&history_path).map_err(|e| e.to_string())?;
            let current_mtime = metadata
                .modified()
                .unwrap_or(SystemTime::now())
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if fp.mtime == current_mtime
                && fp.size == metadata.len()
                && fp.msg_count == current_history.len()
            {
                println!(
                    "[VCPCore] Sync skipped for {}: fingerprint matches.",
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
    }

    // 2. 如果文件不存在，则视为所有当前消息已被删除
    if !history_path.exists() {
        return Ok(TopicDelta {
            added: vec![],
            updated: vec![],
            deleted_ids: current_history.into_iter().map(|m| m.id).collect(),
            order_changed: false,
            sync_skipped: false,
        });
    }

    // 3. 读取磁盘上的最新历史记录并应用预处理 (与 load_chat_history 逻辑对齐)
    let mut new_history = history_repository_fs::read_history(&history_path)?;

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
                // 内容或角色发生变化视为更新。
                if old_msg.content != new_msg.content || old_msg.role != new_msg.role {
                    updated.push(new_msg.clone());
                }
            }
            None => {
                // 新增消息直接返回原始内容。
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
