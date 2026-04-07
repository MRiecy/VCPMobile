use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Topic {
    pub id: String,
    pub name: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: i64,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(rename = "unreadCount", default)]
    pub unread_count: i32,
    #[serde(rename = "msgCount", default)]
    pub msg_count: i32,
    /// 捕获并保留所有额外的动态字段
    #[serde(flatten)]
    pub extra_fields: serde_json::Map<String, serde_json::Value>,
}
