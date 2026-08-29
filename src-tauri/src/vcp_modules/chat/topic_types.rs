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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_topic_deserializes_camel_case_fields_and_defaults_missing_values() {
        let topic: Topic = serde_json::from_value(json!({
            "id": "t1",
            "name": "Topic A",
            "createdAt": 123,
            "ownerId": "agent1",
            "ownerType": "agent"
        }))
        .unwrap();

        assert_eq!(topic.id, "t1");
        assert_eq!(topic.name, "Topic A");
        assert_eq!(topic.created_at, 123);
        assert!(!topic.locked);
        assert!(!topic.unread);
        assert_eq!(topic.unread_count, 0);
        assert_eq!(topic.msg_count, 0);
        assert_eq!(topic.owner_id, "agent1");
        assert_eq!(topic.owner_type, "agent");
    }

    #[test]
    fn test_topic_serializes_using_frontend_field_names() {
        let topic = Topic {
            id: "t1".to_string(),
            name: "Topic A".to_string(),
            created_at: 123,
            locked: true,
            unread: true,
            unread_count: 2,
            msg_count: 3,
            owner_id: "agent1".to_string(),
            owner_type: "agent".to_string(),
        };

        let value = serde_json::to_value(&topic).unwrap();
        assert_eq!(value["createdAt"], 123);
        assert_eq!(value["unreadCount"], 2);
        assert_eq!(value["msgCount"], 3);
        assert_eq!(value["ownerId"], "agent1");
        assert_eq!(value["ownerType"], "agent");

        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("created_at"));
        assert!(!obj.contains_key("unread_count"));
        assert!(!obj.contains_key("msg_count"));
        assert!(!obj.contains_key("owner_id"));
        assert!(!obj.contains_key("owner_type"));
    }

    #[test]
    fn topic_activity_uses_live_message_then_topic_then_creation_fallback() {
        assert_eq!(resolve_topic_activity_updated_at(200, 300, 100), 300);
        assert_eq!(resolve_topic_activity_updated_at(400, 300, 100), 400);
        assert_eq!(resolve_topic_activity_updated_at(200, 0, 100), 200);
        assert_eq!(resolve_topic_activity_updated_at(0, 0, 100), 100);
    }
}
