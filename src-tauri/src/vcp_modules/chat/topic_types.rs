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
    #[serde(rename = "ownerId", default)]
    pub owner_id: String,
    #[serde(rename = "ownerType", default)]
    pub owner_type: String,
}
