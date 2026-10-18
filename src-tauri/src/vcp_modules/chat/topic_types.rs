use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerKey {
    pub owner_type: String,
    pub owner_id: String,
}

impl OwnerKey {
    pub fn new(owner_type: impl Into<String>, owner_id: impl Into<String>) -> Self {
        Self {
            owner_type: owner_type.into(),
            owner_id: owner_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicKey {
    pub owner_type: String,
    pub owner_id: String,
    pub topic_id: String,
}

impl TopicKey {
    pub fn new(
        owner_type: impl Into<String>,
        owner_id: impl Into<String>,
        topic_id: impl Into<String>,
    ) -> Self {
        Self {
            owner_type: owner_type.into(),
            owner_id: owner_id.into(),
            topic_id: topic_id.into(),
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self.owner_type.as_str(), "agent" | "group")
            && !self.owner_id.is_empty()
            && !self.topic_id.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageKey {
    pub topic: TopicKey,
    pub msg_id: String,
}

impl MessageKey {
    pub fn new(topic: TopicKey, msg_id: impl Into<String>) -> Self {
        Self {
            topic,
            msg_id: msg_id.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicActivityDto {
    pub msg_count: i32,
    pub updated_at: i64,
}

pub fn resolve_topic_activity_updated_at(
    topic_updated_at: i64,
    last_message_updated_at: i64,
    created_at: i64,
) -> i64 {
    let topic_activity = if topic_updated_at > 0 {
        topic_updated_at
    } else {
        created_at
    };
    if last_message_updated_at > 0 {
        topic_activity.max(last_message_updated_at)
    } else {
        topic_activity
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
    #[serde(rename = "ownerId")]
    pub owner_id: String,
    #[serde(rename = "ownerType")]
    pub owner_type: String,
}
