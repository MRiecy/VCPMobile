use crate::vcp_modules::group_config_repository_fs::GroupConfig;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// GroupManager 的全局状态，负责运行时缓存管理
pub struct GroupManagerState {
    /// 配置缓存: group_id -> GroupConfig
    pub caches: DashMap<String, GroupConfig>,
    /// 任务队列锁: group_id -> Mutex
    #[allow(dead_code)]
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    /// 获取群组配置 (优先从缓存读取)
    pub fn get_group(&self, group_id: &str) -> Option<GroupConfig> {
        self.caches.get(group_id).map(|c| c.clone())
    }

    /// 插入或更新群组配置到缓存
    pub fn insert_group(&self, config: GroupConfig) {
        self.caches.insert(config.id.clone(), config);
    }

    /// 获取所有缓存的群组配置
    pub fn get_all_groups(&self) -> Vec<GroupConfig> {
        self.caches
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}
