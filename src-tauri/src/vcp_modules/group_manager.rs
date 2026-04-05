// GroupManager: 处理群组(Agent Group)配置与生命周期的核心模块 (IPC 层)
// 源 JS 逻辑参考: ../VCPChat/modules/groupchat.js
// 职责: 作为 Tauri 命令入口，将请求转发给底层的 Service 和 Repository。

use tauri::AppHandle;

pub use crate::vcp_modules::group_cache_coordinator::GroupManagerState;
pub use crate::vcp_modules::group_config_repository_fs::{
    create_group_directory_structure, read_group_config as read_group_config_fs,
    resolve_group_avatar_path, write_group_config as write_group_config_fs, GroupConfig,
};
use crate::vcp_modules::topic_list_manager::Topic;

// --- Tauri Commands ---

#[tauri::command]
pub async fn get_groups(
    state: tauri::State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    Ok(state.get_all_groups())
}

#[tauri::command]
pub async fn read_group_config(
    app: AppHandle,
    state: tauri::State<'_, GroupManagerState>,
    group_id: String,
) -> Result<GroupConfig, String> {
    if let Some(mut config) = state.get_group(&group_id) {
        resolve_group_avatar_path(&app, &mut config);
        return Ok(config);
    }

    // 缓存未命中，尝试磁盘读取
    let config = read_group_config_fs(&app, &group_id)?;

    state.insert_group(config.clone());
    Ok(config)
}

#[tauri::command]
pub async fn create_group(
    app_handle: AppHandle,
    state: tauri::State<'_, GroupManagerState>,
    name: String,
) -> Result<GroupConfig, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    // ID 生成逻辑对齐桌面端: 名称过滤 + 时间戳
    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let group_id = format!("____{}_{}", base_id, timestamp); // 手机端强制 ____ 前缀标识群组

    let default_topic_id = format!("group_topic_{}", timestamp);

    // 1. 创建目录结构
    create_group_directory_structure(&app_handle, &group_id, &default_topic_id)?;

    let default_topic = Topic {
        id: default_topic_id.clone(),
        name: "主要群聊".to_string(),
        created_at: timestamp,
        locked: false,
        unread: false,
        unread_count: 0,
        msg_count: 0,
    };

    let config = GroupConfig {
        id: group_id.clone(),
        name: name.clone(),
        avatar: None,
        avatar_calculated_color: None,
        members: vec![],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({})),
        group_prompt: Some("".to_string()),
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
        use_unified_model: false,
        unified_model: None,
        created_at: timestamp,
        topics: vec![default_topic.clone()],
        tag_match_mode: Some("strict".to_string()),
        extra: serde_json::Map::new(),
    };

    // 2. 写入 config.json
    write_group_config_fs(&app_handle, &config)?;

    // 3. 更新缓存
    state.insert_group(config.clone());

    Ok(config)
}
