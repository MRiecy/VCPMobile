// agent_handlers.rs: 处理智能体相关的强类型指令
// 对齐原 agentHandlers.js 的业务逻辑，并注入移动端感知

use crate::vcp_modules::agent_service::{
    read_agent_config, AgentConfigState,
};
use crate::vcp_modules::agent_types::{AgentConfig, TopicInfo};
use serde::Deserialize;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Deserialize)]
pub struct AvatarPayload {
    #[allow(dead_code)]
    pub name: String,
    pub r#type: String, // mime type
    pub buffer: Vec<u8>,
}

/// 保存 Agent 头像
/// 逻辑对齐: agentHandlers.js -> save-avatar
/// 增加业务逻辑: 自动清理旧格式、同步到集中式目录
#[tauri::command]
pub async fn save_agent_avatar(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    agent_id: String,
    avatar: AvatarPayload,
) -> Result<String, String> {
    let mut agent_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    agent_dir.push("Agents");
    agent_dir.push(&agent_id);

    if !agent_dir.exists() {
        fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;
    }

    // 确定后缀名
    let ext = match avatar.r#type.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    };

    // 1. 删除旧头像 (清理多种可能的格式)
    let extensions = ["png", "jpg", "gif", "webp"];
    for e in extensions {
        let old_path = agent_dir.join(format!("avatar.{}", e));
        if old_path.exists() {
            let _ = fs::remove_file(old_path);
        }
    }

    // 2. 写入新头像
    let new_avatar_path = agent_dir.join(format!("avatar.{}", ext));
    fs::write(&new_avatar_path, &avatar.buffer).map_err(|e| e.to_string())?;

    // 3. 集中式存储 (avatarimage 目录)
    // 这是为了方便前端通过统一路径访问或进行备份
    let app_data = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    let avatar_image_dir = app_data.join("avatarimage");
    if !avatar_image_dir.exists() {
        let _ = fs::create_dir_all(&avatar_image_dir);
    }

    // 获取 Agent 名称用于集中命名
    let config = read_agent_config(
        app_handle.clone(),
        agent_state,
        agent_id.clone(),
        Some(true),
    )
    .await?;
    let centralized_name = if config.name.is_empty() {
        &agent_id
    } else {
        &config.name
    };

    // 清理集中式目录里的旧名
    for e in extensions {
        let old_centralized = avatar_image_dir.join(format!("{}.{}", centralized_name, e));
        if old_centralized.exists() {
            let _ = fs::remove_file(old_centralized);
        }
    }
    let centralized_path = avatar_image_dir.join(format!("{}.{}", centralized_name, ext));
    fs::write(&centralized_path, &avatar.buffer).map_err(|e| e.to_string())?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    Ok(format!(
        "{}?t={}",
        new_avatar_path.to_string_lossy(),
        timestamp
    ))
}

/// 创建 Agent
/// 逻辑对齐: agentHandlers.js -> create-agent
/// 包含初始配置生成与话题目录初始化
#[tauri::command]
pub async fn create_agent(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    name: String,
    initial_config: Option<serde_json::Value>,
) -> Result<AgentConfig, String> {
    // ID 生成逻辑: 过滤特殊字符 + 时间戳
    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let agent_id = format!("{}_{}", base_id, timestamp);

    let default_topic_id = format!("topic_{}", timestamp);

    let config = if let Some(init) = initial_config {
        let mut c: AgentConfig = serde_json::from_value(init).map_err(|e| e.to_string())?;
        c.id = agent_id.clone();
        c.name = name;
        c
    } else {
        AgentConfig {
            id: agent_id.clone(),
            name: name.clone(),
            system_prompt: format!("你是 {}。", name),
            model: "gemini-2.0-flash".to_string(),
            temperature: 0.7,
            context_token_limit: 1000000,
            max_output_tokens: 60000,
            top_p: None,
            top_k: None,
            stream_output: true,
            tts_voice_primary: None,
            tts_regex_primary: None,
            tts_voice_secondary: None,
            tts_regex_secondary: None,
            tts_speed: 1.0,
            avatar_border_color: None,
            name_text_color: None,
            custom_css: None,
            card_css: None,
            chat_css: None,
            disable_custom_colors: true,
            use_theme_colors_in_chat: true,
            ui_collapse_states: None,
            strip_regexes: vec![],
            avatar_url: None,
            avatar_calculated_color: None,
            topics: vec![TopicInfo {
                id: default_topic_id.clone(),
                name: "主要对话".to_string(),
                created_at: timestamp,
                extra_fields: serde_json::Map::new(),
            }],
            extra: serde_json::Map::new(),
        }
    };
    // 初始化默认话题目录：Agent/Group 统一落在 UserData/data 聚合层，而非 Agents 配置目录
    let topic_dir = crate::vcp_modules::storage_paths::resolve_topic_dir(
        &app_handle,
        &agent_id,
        &default_topic_id,
    );
    fs::create_dir_all(&topic_dir).map_err(|e| e.to_string())?;

    log::info!(
        "[AgentHandlers] Creating agent '{}' with config at Agents/{}/config.json and topic history at {:?}",
        agent_id,
        agent_id,
        topic_dir
    );

    // 初始化默认话题的 history.json (内容为 [])
    let mut history_path = topic_dir.clone();
    history_path.push("history.json");
    fs::write(history_path, "[]").map_err(|e| e.to_string())?;

    // 写入配置 (原子操作)
    crate::vcp_modules::agent_service::save_agent_config(app_handle, state, config.clone()).await?;

    Ok(config)
}

/// 删除 Agent
/// 逻辑对齐: agentHandlers.js -> delete-agent
/// 彻底清理配置目录与数据目录
#[tauri::command]
pub async fn delete_agent(
    app_handle: AppHandle,
    _state: State<'_, AgentConfigState>,
    agent_id: String,
) -> Result<bool, String> {
    let mut agent_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    agent_dir.push("Agents");
    agent_dir.push(&agent_id);

    if agent_dir.exists() {
        fs::remove_dir_all(agent_dir).map_err(|e| e.to_string())?;
    }

    Ok(true)
}
