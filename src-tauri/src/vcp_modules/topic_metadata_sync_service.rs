use crate::vcp_modules::agent_service::{
    read_agent_config, save_agent_config as write_agent_config, AgentConfigState,
};
use crate::vcp_modules::agent_types::TopicInfo;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::storage_paths::{
    get_groups_base_path, is_group_item, resolve_topic_dir,
};
use crate::vcp_modules::topic_list_manager::Topic;
use serde_json::{Map, Value};
use std::fs;
use tauri::{AppHandle, Manager};

/// 同步新创建的话题到主配置和话题目录
pub async fn sync_new_topic(
    app_handle: &AppHandle,
    item_id: &str,
    topic: &Topic,
) -> Result<(), String> {
    // 1. 更新父级配置 (config.json) 中的 topics 数组 (Unshift 逻辑)
    if is_group_item(app_handle, item_id) {
        // 处理群组
        let group_state = app_handle.state::<GroupManagerState>();
        let mut config =
            read_group_config(app_handle.clone(), group_state.clone(), item_id.to_string()).await?;
        config.topics.insert(0, topic.clone());

        // 写回磁盘
        let config_path = get_groups_base_path(app_handle)
            .join(item_id)
            .join("config.json");
        let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        fs::write(config_path, content).map_err(|e| e.to_string())?;
        // 更新缓存
        group_state.caches.insert(item_id.to_string(), config);
    } else {
        // 处理 Agent
        let agent_state = app_handle.state::<AgentConfigState>();
        let mut config = read_agent_config(
            app_handle.clone(),
            agent_state.clone(),
            item_id.to_string(),
            Some(false),
        )
        .await?;

        let info = TopicInfo {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            extra_fields: Map::new(),
        };
        config.topics.insert(0, info);
        write_agent_config(app_handle.clone(), agent_state, config).await?;
    }

    Ok(())
}

/// 更新主配置和话题目录中话题的元数据
pub async fn update_topic_metadata(
    app_handle: &AppHandle,
    item_id: &str,
    topic_id: &str,
    update_fn: impl Fn(&mut Value),
) -> Result<(), String> {
    if is_group_item(app_handle, item_id) {
        // 处理群组
        let group_state = app_handle.state::<GroupManagerState>();
        let mut config =
            read_group_config(app_handle.clone(), group_state.clone(), item_id.to_string()).await?;

        if let Some(topic) = config.topics.iter_mut().find(|t| t.id == topic_id) {
            let mut topic_json = serde_json::to_value(&topic).map_err(|e| e.to_string())?;
            update_fn(&mut topic_json);
            *topic = serde_json::from_value(topic_json).map_err(|e| e.to_string())?;

            // 写回磁盘
            let config_path = get_groups_base_path(app_handle)
                .join(item_id)
                .join("config.json");
            let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
            fs::write(config_path, content).map_err(|e| e.to_string())?;
            // 更新缓存
            group_state.caches.insert(item_id.to_string(), config);
        }
    } else {
        // 处理 Agent
        let agent_state = app_handle.state::<AgentConfigState>();
        let mut config = read_agent_config(
            app_handle.clone(),
            agent_state.clone(),
            item_id.to_string(),
            Some(false),
        )
        .await?;

        if let Some(topic) = config.topics.iter_mut().find(|t| t.id == topic_id) {
            let mut topic_json = serde_json::to_value(&topic).map_err(|e| e.to_string())?;
            update_fn(&mut topic_json);
            *topic = serde_json::from_value(topic_json).map_err(|e| e.to_string())?;

            write_agent_config(app_handle.clone(), agent_state, config)
                .await?;
        }
    }

    // 同时尝试更新话题目录下的 config.json (如果存在)，保持兼容性
    let topic_dir_config = resolve_topic_dir(app_handle, item_id, topic_id).join("config.json");
    if let Ok(content) = fs::read_to_string(&topic_dir_config) {
        if let Ok(mut json) = serde_json::from_str::<Value>(&content) {
            update_fn(&mut json);
            if let Ok(new_content) = serde_json::to_string_pretty(&json) {
                let _ = fs::write(&topic_dir_config, new_content);
            }
        }
    }

    Ok(())
}
