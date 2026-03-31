use crate::vcp_modules::agent_config_manager::RegexRule;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_watcher::{signal_internal_save, WatcherState};
use crate::vcp_modules::group_manager::resolve_history_path;
use dashmap::DashMap;
use fancy_regex::Regex;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

lazy_static! {
    /// 正则表达式编译缓存: find_pattern -> Compiled Regex
    static ref REGEX_CACHE: DashMap<String, Regex> = DashMap::new();
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    #[serde(rename = "extractedText")]
    pub extracted_text: Option<String>,
    #[serde(default)]
    #[serde(rename = "thumbnailPath")]
    pub thumbnail_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    #[serde(alias = "senderName")]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isThinking")]
    #[serde(alias = "thinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    /// 捕获所有其他未定义的字段
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

// --- 正则处理核心逻辑 (从 chatManager.js 权力下沉) ---

/// 对话深度计算逻辑 (对齐 JS 逻辑)
/// 在 VCP 中，从最新消息往回算
#[allow(dead_code)]
pub fn calculate_depth(history_len: usize, current_index: usize) -> i32 {
    if current_index >= history_len {
        return -1;
    }
    (history_len - 1 - current_index) as i32
}

/// 执行正则转换 (基于影子数据库索引)
pub async fn apply_regex_rules(
    db: &State<'_, DbState>,
    agent_id: &str,
    text: &str,
    scope: &str, // "frontend" 或 "context"
    role: &str,
    depth: i32,
) -> Result<String, String> {
    // 1. 从影子数据库加载该智能体的所有正则规则 (高性能索引)
    let rules =
        sqlx::query_as::<_, RegexRule>("SELECT * FROM agent_regex_rules WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(&db.pool)
            .await
            .map_err(|e: sqlx::Error| e.to_string())?;

    let mut processed_text = text.to_string();

    for rule in rules {
        // 2. 检查作用域对齐
        let should_apply_to_scope = (scope == "context" && rule.apply_to_context)
            || (scope == "frontend" && rule.apply_to_frontend);

        if !should_apply_to_scope {
            continue;
        }

        // 3. 检查角色对齐
        if !rule.apply_to_roles.contains(&role.to_string()) {
            continue;
        }

        // 4. 检查深度对齐 (-1 表示无限制)
        let min_depth_ok = rule.min_depth == -1 || depth >= rule.min_depth;
        let max_depth_ok = rule.max_depth == -1 || depth <= rule.max_depth;

        if !min_depth_ok || !max_depth_ok {
            continue;
        }

        // 5. 执行替换逻辑 (带编译缓存)
        let regex = match REGEX_CACHE.get(&rule.find_pattern) {
            Some(r) => r.clone(),
            None => {
                let r = Regex::new(&rule.find_pattern)
                    .map_err(|e| format!("Invalid regex {}: {}", rule.find_pattern, e))?;
                REGEX_CACHE.insert(rule.find_pattern.clone(), r.clone());
                r
            }
        };

        processed_text = regex
            .replace_all(&processed_text, rule.replace_with.as_str())
            .to_string();
    }

    Ok(processed_text)
}

#[tauri::command]
pub async fn process_regex_for_message(
    db_state: State<'_, DbState>,
    agent_id: String,
    content: String,
    scope: String,
    role: String,
    depth: i32,
) -> Result<String, String> {
    apply_regex_rules(&db_state, &agent_id, &content, &scope, &role, depth).await
}

// --- 历史记录存取逻辑 ---

#[tauri::command]
pub async fn load_chat_history(
    app_handle: AppHandle,
    item_id: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);

    if !history_path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&history_path).map_err(|e| e.to_string())?;
    let full_history: Vec<ChatMessage> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;

    let total_len = full_history.len();
    let end = total_len.saturating_sub(offset.unwrap_or(0));
    let start = end.saturating_sub(limit.unwrap_or(total_len));

    let mut history: Vec<ChatMessage> = full_history
        .into_iter()
        .skip(start)
        .take(end - start)
        .collect();

    // 动态替换桌面端的绝对路径为手机端的绝对路径
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let config_dir_str = config_dir.to_string_lossy().replace("\\", "/");

    for msg in &mut history {
        // 1. 修复附件路径 (Path Rebasing)
        if let Some(attachments) = &mut msg.attachments {
            for att in attachments {
                if let Some(hash) = &att.hash {
                    if let Some(real_path) =
                        crate::vcp_modules::file_manager::resolve_attachment_path(
                            &app_handle,
                            hash,
                            &att.name,
                        )
                    {
                        let new_src = format!("file://{}", real_path.replace("\\", "/"));
                        if att.src != new_src {
                            println!("[VCPCore] Rebasing attachment: {} -> {}", att.src, new_src);
                            att.src = new_src;
                        }

                        // 同时校准缩略图路径
                        let thumb_path = std::path::PathBuf::from(&real_path);
                        if let Some(parent) = thumb_path.parent() {
                            let mut t = parent.to_path_buf();
                            t.push("thumbnails");
                            t.push(format!("{}_thumb.webp", hash));
                            if t.exists() {
                                att.thumbnail_path = Some(format!(
                                    "file://{}",
                                    t.to_string_lossy().replace("\\", "/")
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 2. 替换 extra 里的 avatarUrl (原有逻辑)
        if let Some(avatar_url) = msg.extra.get_mut("avatarUrl") {
            let mut new_url = None;
            if let Some(url_str_raw) = avatar_url.as_str() {
                // 1. 去除 file:// 前缀并统一斜杠
                let url_str = url_str_raw.trim_start_matches("file://").replace("\\", "/");

                // 2. 处理桌面端 Agents 路径
                if url_str.contains("AppData/Agents") {
                    let parts: Vec<&str> = url_str.split('/').collect();
                    if let Some(agent_idx) = parts.iter().position(|&r| r == "Agents") {
                        if parts.len() > agent_idx + 1 {
                            let relative_path = parts[agent_idx + 1..].join("/");
                            new_url = Some(format!("{}/agents/{}", config_dir_str, relative_path));
                        }
                    }
                }
                // 3. 处理桌面端 AgentGroups 路径
                else if url_str.contains("AppData/AgentGroups") {
                    let parts: Vec<&str> = url_str.split('/').collect();
                    if let Some(group_idx) = parts.iter().position(|&r| r == "AgentGroups") {
                        if parts.len() > group_idx + 1 {
                            let relative_path = parts[group_idx + 1..].join("/");
                            new_url =
                                Some(format!("{}/AgentGroups/{}", config_dir_str, relative_path));
                        }
                    }
                }
                // 4. 兼容旧版 VChat 格式: /chat_api/avatar/agent/...
                else if url_str.starts_with("/chat_api/") || url_str.starts_with("/avatar/") {
                    let mut found_path = None;
                    let extensions = ["png", "jpg", "jpeg", "webp", "gif"];

                    if let Some(agent_name) = &msg.name {
                        let mut avatarimage_dir = config_dir.clone();
                        avatarimage_dir.push("avatarimage");
                        for ext in extensions {
                            let possible_path =
                                avatarimage_dir.join(format!("{}.{}", agent_name, ext));
                            if possible_path.exists() {
                                found_path =
                                    Some(possible_path.to_string_lossy().replace("\\", "/"));
                                break;
                            }
                        }
                    }

                    if found_path.is_none() {
                        let mut agent_dir = config_dir.clone();
                        agent_dir.push("agents");
                        agent_dir.push(&item_id);
                        for ext in extensions {
                            let possible_path = agent_dir.join(format!("avatar.{}", ext));
                            if possible_path.exists() {
                                found_path =
                                    Some(possible_path.to_string_lossy().replace("\\", "/"));
                                break;
                            }
                        }
                    }

                    if let Some(path) = found_path {
                        new_url = Some(path);
                    }
                }
            }
            if let Some(path) = new_url {
                *avatar_url = serde_json::Value::String(path);
            }
        }
    }

    Ok(history)
}

#[tauri::command]
pub async fn save_chat_history(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    watcher_state: State<'_, WatcherState>,
    item_id: String,
    topic_id: String,
    history: Vec<ChatMessage>,
) -> Result<(), String> {
    // 1. 发射内部保存信号，防止 Watcher 触发回环
    signal_internal_save(watcher_state);

    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);
    let history_dir = history_path.parent().unwrap();

    fs::create_dir_all(history_dir).map_err(|e| e.to_string())?;

    // 2. 原子写入物理文件
    let content = serde_json::to_string_pretty(&history).map_err(|e| e.to_string())?;
    fs::write(&history_path, content).map_err(|e| e.to_string())?;

    // 3. 同步更新影子数据库索引 (Shadow DB Sync)
    let msg_count = history.len() as i32;
    let last_msg_preview = history.last().map(|m| {
        let mut preview = m.content.chars().take(100).collect::<String>();
        if m.content.chars().count() > 100 {
            preview.push_str("...");
        }
        preview
    });

    // "智能计数判断"
    let mut smart_unread_count = 0;
    let non_system_msgs: Vec<_> = history.iter().filter(|m| m.role != "system").collect();
    if non_system_msgs.len() == 1 && non_system_msgs[0].role == "assistant" {
        smart_unread_count = 1;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query(
        "UPDATE topic_index SET 
            msg_count = ?, 
            last_msg_preview = ?, 
            mtime = ?,
            unread_count = ?
         WHERE topic_id = ?",
    )
    .bind(msg_count)
    .bind(last_msg_preview)
    .bind(now)
    .bind(smart_unread_count)
    .bind(&topic_id)
    .execute(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// 线程安全地向历史记录追加单条消息 (用于并行群聊断点存盘)
lazy_static! {
    /// 历史记录写入锁，防止并行追加时的竞态
    static ref HISTORY_WRITE_LOCKS: DashMap<String, Arc<tokio::sync::Mutex<()>>> = DashMap::new();
}

/// 线程安全地向历史记录追加单条消息 (用于并行群聊断点存盘)
pub async fn append_single_message(
    app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    watcher_state: Option<&WatcherState>,
    item_id: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    let lock_key = format!("{}_{}", item_id, topic_id);
    let lock = HISTORY_WRITE_LOCKS
        .entry(lock_key.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();

    let _guard = lock.lock().await;

    // 1. 加载当前全量历史
    let mut history =
        load_chat_history(app_handle.clone(), item_id.clone(), topic_id.clone(), None, None).await?;

    // 检查是否已存在 (防止重复追加)
    if history.iter().any(|m| m.id == message.id) {
        return Ok(());
    }

    history.push(message);

    if let Some(state) = watcher_state {
        state.last_internal_save_time.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);
    let history_dir = history_path.parent().unwrap();
    fs::create_dir_all(history_dir).map_err(|e| e.to_string())?;

    let content = serde_json::to_string_pretty(&history).map_err(|e| e.to_string())?;
    fs::write(&history_path, content).map_err(|e| e.to_string())?;

    let msg_count = history.len() as i32;
    let last_msg_preview = history.last().map(|m| {
        let mut preview = m.content.chars().take(100).collect::<String>();
        if m.content.chars().count() > 100 {
            preview.push_str("...");
        }
        preview
    });

    let mut smart_unread_count = 0;
    let non_system_msgs: Vec<_> = history.iter().filter(|m| m.role != "system").collect();
    if non_system_msgs.len() == 1 && non_system_msgs[0].role == "assistant" {
        smart_unread_count = 1;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    sqlx::query(
        "UPDATE topic_index SET 
            msg_count = ?, 
            last_msg_preview = ?, 
            mtime = ?,
            unread_count = ?
         WHERE topic_id = ?",
    )
    .bind(msg_count)
    .bind(last_msg_preview)
    .bind(now)
    .bind(smart_unread_count)
    .bind(&topic_id)
    .execute(db_pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

// --- 增量同步逻辑 (Delta Sync) ---

// --- 指纹与同步优化 ---

#[derive(Debug, Serialize, Deserialize)]
pub struct TopicFingerprint {
    pub topic_id: String,
    pub mtime: u64,
    pub size: u64,
    pub msg_count: usize,
}

#[tauri::command]
pub async fn get_topic_fingerprint(
    app_handle: AppHandle,
    item_id: String,
    topic_id: String,
) -> Result<TopicFingerprint, String> {
    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);

    if !history_path.exists() {
        return Ok(TopicFingerprint {
            topic_id,
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
    let content = fs::read_to_string(&history_path).map_err(|e| e.to_string())?;
    let history: Vec<serde_json::Value> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;

    Ok(TopicFingerprint {
        topic_id,
        mtime,
        size: metadata.len(),
        msg_count: history.len(),
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TopicDelta {
    pub added: Vec<ChatMessage>,
    pub updated: Vec<ChatMessage>,
    pub deleted_ids: Vec<String>,
    pub order_changed: bool,
    pub sync_skipped: bool,
}

/// 对比内存中的历史记录与磁盘文件，计算增量更新 (Delta)
#[tauri::command]
pub async fn get_topic_delta(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    item_id: String,
    topic_id: String,
    current_history: Vec<ChatMessage>,
    fingerprint: Option<TopicFingerprint>,
) -> Result<TopicDelta, String> {
    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);

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
    // 注意：这里我们复用 load_chat_history 的逻辑，但为了性能，我们手动处理
    let content = fs::read_to_string(&history_path).map_err(|e| e.to_string())?;
    let mut new_history: Vec<ChatMessage> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;

    // 对新消息进行必要的 Rebasing 和清洗，确保比对时的一致性
    // (由于 get_topic_delta 通常用于同步，这里可以简化，但必须保证 ID 匹配)

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
    let _history_len = new_history.len();
    for (_idx, new_msg) in new_history.iter_mut().enumerate() {
        new_ids_set.insert(new_msg.id.clone());

        match old_map.get(&new_msg.id) {
            Some(old_msg) => {
                // 内容或角色发生变化视为更新。
                if old_msg.content != new_msg.content || old_msg.role != new_msg.role {
                    updated.push(new_msg.clone());
                }
            }
            None => {
                // 正则预处理已回退，新增消息直接返回原始内容。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_chat_message() {
        let json = r#"
        {
            "role": "user",
            "name": "MRiecy",
            "content": "Hello",
            "attachments": [],
            "timestamp": 1772866899762,
            "id": "msg_1772866899758_user_14oeher",
            "extra_field": 123
        }"#;

        let msg: Result<ChatMessage, _> = serde_json::from_str(json);
        assert!(msg.is_ok(), "Failed to parse: {:?}", msg.err());
    }

    #[test]
    fn test_deserialize_actual_file() {
        let path = "G:/VCPChat/AppData/UserData/____1765271785553/topics/group_topic_1772859234535/history.json";
        let s = std::fs::read_to_string(path).unwrap();
        let r: Result<Vec<ChatMessage>, _> = serde_json::from_str(&s);
        assert!(r.is_ok(), "Failed to parse: {:?}", r.err());
    }
}
