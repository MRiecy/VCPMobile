// AgentConfigManager: 处理智能体(Agent)配置的核心模块 (Facade 层)
// 职责: 作为应用层 Facade，协调文件系统存储 (AgentConfigRepositoryFS) 与数据库索引同步 (Projections)。

use crate::vcp_modules::agent_config_repository_fs::{
    self, AgentConfig as FsAgentConfig, AgentConfigState as FsAgentConfigState,
    RegexRule as FsRegexRule, TopicInfo as FsTopicInfo,
};
use crate::vcp_modules::agent_index_projection_service;
use crate::vcp_modules::agent_regex_projection_service;
use crate::vcp_modules::agent_topic_metadata_service::AgentTopicMetadataService;
use crate::vcp_modules::db_manager::DbState;
use tauri::{AppHandle, Manager, State};

// Re-export structs for compatibility with other modules
pub type TopicInfo = FsTopicInfo;
pub type RegexRule = FsRegexRule;
// pub type UiCollapseStates = FsUiCollapseStates;
pub type AgentConfig = FsAgentConfig;
pub type AgentConfigState = FsAgentConfigState;

#[tauri::command]
pub async fn read_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    agent_config_repository_fs::read_agent_config_fs(
        &app_handle,
        &state,
        &agent_id,
        allow_default.unwrap_or(false),
    )
    .await
}

#[tauri::command]
pub async fn save_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent: AgentConfig,
) -> Result<bool, String> {
    let agent_id = if agent.id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    } else {
        agent.id.clone()
    };

    // 获取锁，确保串行写入
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    internal_write_agent_config(&app_handle, &state, &agent_id, &agent).await
}

#[tauri::command]
pub async fn write_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    config: AgentConfig,
) -> Result<bool, String> {
    // 获取锁，确保串行写入
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    internal_write_agent_config(&app_handle, &state, &agent_id, &config).await
}

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    agent_config_repository_fs::get_all_agents_fs(&app_handle, &state).await
}

#[tauri::command]
pub async fn update_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    updates: serde_json::Value,
) -> Result<AgentConfig, String> {
    // 获取锁，确保整个“读取-修改-写入”过程是原子的
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;

    // 1. 读取当前配置
    let config =
        agent_config_repository_fs::read_agent_config_fs(&app_handle, &state, &agent_id, false)
            .await?;

    // 2. 合并更新字段
    let mut config_val = serde_json::to_value(&config).map_err(|e| e.to_string())?;

    if let Some(updates_obj) = updates.as_object() {
        if let Some(config_obj) = config_val.as_object_mut() {
            for (k, v) in updates_obj {
                config_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let new_config: AgentConfig = serde_json::from_value(config_val).map_err(|e| e.to_string())?;

    // 3. 持久化
    internal_write_agent_config(&app_handle, &state, &agent_id, &new_config).await?;

    Ok(new_config)
}

/// 内部使用的写入逻辑，不包含锁（调用者必须持有锁）
/// 协调文件写入与多个投影服务的同步
async fn internal_write_agent_config(
    app_handle: &AppHandle,
    state: &AgentConfigState,
    agent_id: &str,
    config: &AgentConfig,
) -> Result<bool, String> {
    // 1. 写入文件系统
    let mtime =
        agent_config_repository_fs::write_agent_config_fs(app_handle, state, agent_id, config)
            .await?;

    // 2. 同步到数据库投影
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    // 使用事务保证投影同步的原子性
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 2a. 同步 Agent Index
    agent_index_projection_service::sync_agent_index(&mut tx, agent_id, &config.name, mtime as i64)
        .await?;

    // 2b. 同步正则规则 (内部会开启自己的事务，或者我们可以重构它以接受 tx)
    // 目前 sync_regex_rules_to_db 内部开启了事务，为了保持简单先这样调用
    // 但为了更好的原子性，理想情况是共享 tx。
    // 考虑到 sync_regex_rules_to_db 已经实现了删除+插入的逻辑，我们先提交当前的 tx 再调用它，或者重构它。
    tx.commit().await.map_err(|e| e.to_string())?;

    agent_regex_projection_service::sync_regex_rules_to_db(pool, agent_id, &config.strip_regexes)
        .await?;

    // 2c. 同步话题元数据
    AgentTopicMetadataService::sync_topics_to_db(pool, agent_id, &config.topics).await?;

    Ok(true)
}

/// 重建整个影子数据库索引 (通常在全量同步后调用)
#[tauri::command]
pub async fn rebuild_db_index(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<usize, String> {
    let agents = agent_config_repository_fs::get_all_agents_fs(&app_handle, &state).await?;
    let mut count = 0;

    for config in agents {
        let agent_id = config.id.clone();
        let mutex = state.acquire_lock(&agent_id).await;
        let _lock = mutex.lock().await;

        if internal_write_agent_config(&app_handle, &state, &agent_id, &config)
            .await
            .is_ok()
        {
            count += 1;
        }
    }

    Ok(count)
}
