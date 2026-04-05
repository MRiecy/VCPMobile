use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};

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
    #[sqlx(rename = "updated_at")]
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
    #[sqlx(rename = "msg_count")]
    #[serde(rename = "messageCount")]
    #[serde(default)]
    pub msg_count: i32,
}

/// 获取指定 Agent 或 Group 的话题列表
pub async fn get_topics_by_item_id(
    db_state: &DbState,
    item_id: &str,
) -> Result<Vec<Topic>, String> {
    let topics = sqlx::query_as::<_, Topic>(
        "SELECT topic_id, title, updated_at, locked, unread, unread_count, msg_count FROM topic_state WHERE item_id = ? ORDER BY updated_at DESC"
    )
    .bind(item_id)
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(topics)
}
