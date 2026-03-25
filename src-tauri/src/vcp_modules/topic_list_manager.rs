use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_manager::{resolve_history_path, resolve_topic_dir};
use serde::{Deserialize, Serialize};
use std::fs;

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone, Default)]
pub struct Topic {
    #[sqlx(rename = "topic_id")]
    #[serde(default)]
    pub id: String,
    #[sqlx(rename = "title")]
    #[serde(alias = "title")]
    #[serde(default)]
    pub name: String,
    #[sqlx(rename = "mtime")]
    #[serde(rename = "createdAt")]
    #[serde(default)]
    pub created_at: i64,
    #[serde(default = "default_true")]
    pub locked: bool,
    #[serde(default)]
    pub unread: bool,
    #[sqlx(rename = "unread_count")]
    #[serde(rename = "unreadCount")]
    #[serde(default)]
    pub unread_count: i32,
    #[sqlx(rename = "last_msg_preview")]
    #[serde(default)]
    pub last_msg_preview: Option<String>,
    #[sqlx(rename = "msg_count")]
    #[serde(rename = "messageCount")]
    #[serde(default)]
    pub msg_count: i32,
}

#[tauri::command]
pub async fn get_topics(
    db_state: tauri::State<'_, DbState>,
    item_id: String,
) -> Result<Vec<Topic>, String> {
    let topics = sqlx::query_as::<_, Topic>(
        "SELECT topic_id, title, mtime, locked, unread, unread_count, last_msg_preview, msg_count FROM topic_index WHERE agent_id = ? ORDER BY mtime DESC"
    )
    .bind(&item_id)
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(topics)
}

#[tauri::command]
pub async fn create_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    name: String,
) -> Result<Topic, String> {
    let id = format!("topic_{}", uuid::Uuid::new_v4());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let topic = Topic {
        id: id.clone(),
        name: name.clone(),
        created_at: now,
        locked: false,
        unread: false,
        unread_count: 0,
        last_msg_preview: None,
        msg_count: 0,
    };

    // 1. 写入数据库索引
    sqlx::query(
        "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(&item_id)
    .bind(&name)
    .bind(now)
    .bind(false)
    .bind(false)
    .bind(0)
    .execute(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    // 2. 创建目录
    let topic_dir = resolve_topic_dir(&app_handle, &item_id, &id);
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;

    // 初始化 history.json (内容为 [])
    let history_path = topic_dir.join("history.json");
    fs::write(history_path, "[]").map_err(|e| e.to_string())?;

    // 3. 更新父级配置 (config.json) 中的 topics 数组 (Unshift 逻辑)
    // 逻辑对齐: chatHandlers.js -> create-new-topic-for-agent
    if item_id.starts_with("____") || item_id.starts_with("___N_P_") {
        // 处理群组
        let group_state = app_handle.state::<crate::vcp_modules::group_manager::GroupManagerState>();
        let mut config = crate::vcp_modules::group_manager::read_group_config(
            app_handle.clone(),
            group_state.clone(),
            item_id.clone(),
        ).await?;
        config.topics.insert(0, topic.clone());
        
        // 写回磁盘
        let config_path = crate::vcp_modules::group_manager::get_groups_base_path(&app_handle)
            .join(&item_id)
            .join("config.json");
        let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        fs::write(config_path, content).map_err(|e| e.to_string())?;
        // 更新缓存
        group_state.caches.insert(item_id, config);
    } else {
        // 处理 Agent
        let agent_state = app_handle.state::<crate::vcp_modules::agent_config_manager::AgentConfigState>();
        let mut config = crate::vcp_modules::agent_config_manager::read_agent_config(
            app_handle.clone(),
            agent_state.clone(),
            item_id.clone(),
            Some(false),
        ).await?;
        
        // TopicInfo 在 agent_config_manager 中定义，结构略有不同但兼容
        use crate::vcp_modules::agent_config_manager::TopicInfo;
        let info = TopicInfo {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            extra_fields: serde_json::Map::new(),
        };
        config.topics.insert(0, info);
        crate::vcp_modules::agent_config_manager::write_agent_config(app_handle.clone(), agent_state, item_id, config).await?;
    }

    let mut config_path = topic_dir.clone();
    config_path.push("config.json");
    let content = serde_json::to_string_pretty(&topic).map_err(|e| e.to_string())?;
    fs::write(config_path, content).map_err(|e| e.to_string())?;

    Ok(topic)
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
) -> Result<(), String> {
    // 1. 从数据库删除
    sqlx::query("DELETE FROM topic_index WHERE topic_id = ?")
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 删除磁盘文件
    let topic_dir = resolve_topic_dir(&app_handle, &item_id, &topic_id);

    if topic_dir.exists() {
        fs::remove_dir_all(topic_dir).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn update_topic_title(
    db_state: tauri::State<'_, DbState>,
    _item_id: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    sqlx::query("UPDATE topic_index SET title = ?, mtime = ? WHERE topic_id = ?")
        .bind(&title)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

use crate::vcp_modules::app_settings_manager::{read_app_settings, AppSettingsState};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn summarize_topic<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, AppSettingsState>,
    item_id: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    let settings = read_app_settings(app_handle.clone(), settings_state).await?;
    if settings.vcp_server_url.is_empty() || settings.vcp_api_key.is_empty() {
        return Err("VCP settings are missing".to_string());
    }

    // 1. 获取历史记录 (最近4条)
    let history_path = resolve_history_path(&app_handle, &item_id, &topic_id);
    if !history_path.exists() {
        return Err("History not found".to_string());
    }

    let content = fs::read_to_string(&history_path).map_err(|e| e.to_string())?;
    let history: Vec<Value> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    if history.len() < 2 {
        return Err("Not enough messages to summarize".to_string());
    }

    let filtered_history: Vec<_> = history.iter().filter(|m| m["role"] != "system").collect();

    let recent_msgs: Vec<_> = filtered_history.iter().rev().take(4).rev().collect();

    let mut recent_content = String::new();
    for msg in recent_msgs {
        let role_name = if msg["role"] == "user" {
            settings.user_name.as_str()
        } else {
            agent_name.as_str()
        };
        let content_str = msg["content"].as_str().unwrap_or("");
        recent_content.push_str(&format!("{}: {}\n", role_name, content_str));
    }

    // 2. 构造 Prompt (对齐桌面端)
    let summary_prompt = format!(
        "[待总结聊天记录: {}]\n请根据以上对话内容，仅返回一个简洁的话题标题。要求：1. 标题长度控制在10个汉字以内。2. 标题本身不能包含任何标点符号、数字编号或任何非标题文字。3. 直接给出标题文字，不要添加任何解释或前缀。",
        recent_content
    );

    // 3. 调用 AI (强制使用 gemini-2.5-flash + 0.7)
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let model = settings
        .topic_summary_model
        .unwrap_or_else(|| "gemini-2.5-flash".to_string());
    let temp = settings.topic_summary_model_temperature.unwrap_or(0.7);

    let response = client
        .post(&settings.vcp_server_url)
        .header("Authorization", format!("Bearer {}", settings.vcp_api_key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "messages": [{"role": "user", "content": summary_prompt}],
            "model": model,
            "temperature": temp,
            "max_tokens": 4000,
            "stream": false
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        return Err(format!("AI request failed: {}", response.status()));
    }

    let res_json: Value = response.json().await.map_err(|e| e.to_string())?;
    let raw_title = res_json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();

    // 4. 清洗标题 (对齐桌面端 logic)
    let clean_title = clean_summarized_title(raw_title);

    if clean_title.is_empty() {
        return Err("AI failed to generate a valid title".to_string());
    }

    Ok(clean_title)
}

fn clean_summarized_title(raw: &str) -> String {
    // 提取第一行
    let first_line = raw.lines().next().unwrap_or("").trim();

    // 移除标点符号、前缀
    let mut cleaned = first_line
        .replace(|c: char| !c.is_alphanumeric() && !c.is_whitespace(), "")
        .replace("标题", "")
        .replace("总结", "")
        .replace("Topic", "")
        .replace(":", "")
        .replace("：", "")
        .trim()
        .to_string();

    // 移除所有空格
    cleaned = cleaned.replace(char::is_whitespace, "");

    // 截断到12个字符
    let char_count = cleaned.chars().count();
    if char_count > 12 {
        cleaned.chars().take(12).collect()
    } else {
        cleaned
    }
}

async fn update_topic_in_main_config<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
    item_id: &str,
    topic_id: &str,
    update_fn: impl Fn(&mut Value),
) -> Result<(), String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let group_config = config_dir
        .join("AgentGroups")
        .join(item_id)
        .join("config.json");
    let agent_config = config_dir.join("agents").join(item_id).join("config.json");

    let target_path = if group_config.exists() {
        group_config
    } else if agent_config.exists() {
        agent_config
    } else {
        return Err(format!("Config not found for item: {}", item_id));
    };

    let content = fs::read_to_string(&target_path).map_err(|e| e.to_string())?;
    let mut json: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    let mut updated = false;
    if let Some(topics) = json.get_mut("topics").and_then(|v| v.as_array_mut()) {
        if let Some(topic) = topics
            .iter_mut()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(topic_id))
        {
            update_fn(topic);
            updated = true;
        }
    }

    if updated {
        let new_content = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
        let temp_path = target_path.with_extension("tmp");
        fs::write(&temp_path, new_content).map_err(|e| e.to_string())?;
        fs::rename(&temp_path, &target_path).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn toggle_topic_lock(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    // 1. 更新数据库
    sqlx::query("UPDATE topic_index SET locked = ?, mtime = ? WHERE topic_id = ?")
        .bind(locked)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 原子的更新主 config.json
    update_topic_in_main_config(&app_handle, &item_id, &topic_id, |topic| {
        topic["locked"] = serde_json::Value::Bool(locked);
    })
    .await?;

    Ok(())
}

#[tauri::command]
pub async fn set_topic_unread(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, DbState>,
    item_id: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    // 1. 更新数据库
    sqlx::query("UPDATE topic_index SET unread = ?, mtime = ? WHERE topic_id = ?")
        .bind(unread)
        .bind(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        )
        .bind(&topic_id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 原子的更新主 config.json
    update_topic_in_main_config(&app_handle, &item_id, &topic_id, |topic| {
        topic["unread"] = serde_json::Value::Bool(unread);
    })
    .await?;

    Ok(())
}
