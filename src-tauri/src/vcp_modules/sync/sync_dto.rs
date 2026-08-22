use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::topic_types::Topic;
use serde::{Deserialize, Serialize};

/// =================================================================
/// vcp_modules/sync_dto.rs - 双端同步标准契约 (The Shared Truth)
/// =================================================================
/// 智能体同步数据传输对象
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSyncDTO {
    pub name: String,
    pub system_prompt: String,
    pub model: String,
    pub temperature: f64,
    pub context_token_limit: i32,
    pub max_output_tokens: i32,
    pub stream_output: bool,
}

impl From<&AgentConfig> for AgentSyncDTO {
    fn from(config: &AgentConfig) -> Self {
        Self {
            name: config.name.clone(),
            system_prompt: config.system_prompt.clone(),
            model: config.model.clone(),
            temperature: config.temperature,
            context_token_limit: config.context_token_limit,
            max_output_tokens: config.max_output_tokens,
            stream_output: config.stream_output,
        }
    }
}

/// 群组同步数据传输对象
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupSyncDTO {
    pub name: String,
    pub members: Vec<String>,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_tags: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_prompt: Option<String>,
    pub use_unified_model: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unified_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_match_mode: Option<String>,
    pub created_at: i64,
}

impl From<&GroupConfig> for GroupSyncDTO {
    fn from(config: &GroupConfig) -> Self {
        Self {
            name: config.name.clone(),
            members: config.members.clone(),
            mode: config.mode.clone(),
            member_tags: config.member_tags.clone(),
            group_prompt: config.group_prompt.clone(),
            invite_prompt: config.invite_prompt.clone(),
            use_unified_model: config.use_unified_model,
            unified_model: config.unified_model.clone(),
            tag_match_mode: config.tag_match_mode.clone(),
            created_at: config.created_at,
        }
    }
}

/// Agent Topic 同步 DTO (包含 locked/unread)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    #[serde(default = "default_locked")]
    pub locked: bool,
    #[serde(default = "default_unread")]
    pub unread: bool,
    pub owner_id: String,
}

fn default_locked() -> bool {
    true
}
fn default_unread() -> bool {
    false
}

impl From<&Topic> for AgentTopicSyncDTO {
    fn from(topic: &Topic) -> Self {
        Self {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            locked: topic.locked,
            unread: topic.unread,
            owner_id: topic.owner_id.clone(),
        }
    }
}

/// Group Topic 同步 DTO (无 locked/unread)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupTopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub owner_id: String,
}

impl From<&Topic> for GroupTopicSyncDTO {
    fn from(topic: &Topic) -> Self {
        Self {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            owner_id: topic.owner_id.clone(),
        }
    }
}

/// 附件同步 DTO
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentSyncDTO {
    pub r#type: String,
    pub name: String,
    pub size: u64,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

impl TryFrom<&Attachment> for AttachmentSyncDTO {
    type Error = String;

    fn try_from(att: &Attachment) -> Result<Self, Self::Error> {
        let hash = att
            .hash
            .as_deref()
            .map(str::to_ascii_lowercase)
            .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
            .ok_or_else(|| {
                format!(
                    "Attachment {} requires a valid SHA-256 content hash",
                    att.name
                )
            })?;
        Ok(Self {
            r#type: att.r#type.clone(),
            name: att.name.clone(),
            size: att.size,
            hash,
            status: att.status.clone(),
            extracted_text: att.extracted_text.clone(),
            image_frames: att.image_frames.clone(),
            created_at: att.created_at,
        })
    }
}

/// User 消息同步 DTO (包含 attachments)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserMessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl TryFrom<&ChatMessage> for UserMessageSyncDTO {
    type Error = String;

    fn try_from(msg: &ChatMessage) -> Result<Self, Self::Error> {
        if msg.id.is_empty() || msg.role.is_empty() {
            return Err("User message requires non-empty id and role".to_string());
        }
        Ok(Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            updated_at: msg.updated_at.unwrap_or(msg.timestamp),
            attachments: msg
                .attachments
                .as_ref()
                .map(|attachments| {
                    attachments
                        .iter()
                        .map(AttachmentSyncDTO::try_from)
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?,
            content_hash: msg.content_hash.clone(),
        })
    }
}

/// Agent 消息同步 DTO (包含 agentId, avatarColor)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "isThinking", default)]
    pub is_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(rename = "avatarColor")]
    pub avatar_color: String,
    #[serde(rename = "contentHash", skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl AgentMessageSyncDTO {
    #[allow(dead_code)] // The bounded push path builds this DTO by moving owned fields.
    pub fn from_message(msg: &ChatMessage, avatar_color: String) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            updated_at: msg.updated_at.unwrap_or(msg.timestamp),
            agent_id: msg.agent_id.clone().unwrap_or_default(),
            is_thinking: msg.is_thinking,
            finish_reason: msg.finish_reason.clone(),
            avatar_color,
            content_hash: msg.content_hash.clone(),
        }
    }
}

/// Group 消息同步 DTO (包含 agentId, groupId, topicId, avatarColor)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupMessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "groupId")]
    pub group_id: String,
    #[serde(rename = "topicId")]
    pub topic_id: String,
    #[serde(rename = "isGroupMessage")]
    pub is_group_message: bool,
    #[serde(rename = "avatarColor")]
    pub avatar_color: String,
    #[serde(rename = "contentHash", skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl GroupMessageSyncDTO {
    #[allow(dead_code)] // The bounded push path builds this DTO by moving owned fields.
    pub fn from_message(msg: &ChatMessage, avatar_color: String) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            updated_at: msg.updated_at.unwrap_or(msg.timestamp),
            agent_id: msg.agent_id.clone().unwrap_or_default(),
            group_id: msg.group_id.clone().unwrap_or_default(),
            topic_id: msg.topic_id.clone().unwrap_or_default(),
            is_group_message: true,
            avatar_color,
            content_hash: msg.content_hash.clone(),
        }
    }
}

/// ⚡ 捍卫 sync_dto.rs 的至高威严：专门用于同步下载消息的平铺标准网络契约
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessagePullSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(default)]
    pub is_thinking: Option<bool>,
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", default)]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", default)]
    pub topic_id: Option<String>,
    #[serde(rename = "isGroupMessage", default)]
    pub is_group_message: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
    #[serde(rename = "contentHash", skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(rename = "avatarColor", skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
}

impl From<MessagePullSyncDTO> for crate::vcp_modules::chat_manager::ChatMessage {
    fn from(dto: MessagePullSyncDTO) -> Self {
        Self {
            id: dto.id,
            role: dto.role,
            name: dto.name,
            content: dto.content,
            timestamp: dto.timestamp,
            updated_at: Some(dto.updated_at),
            is_thinking: dto.is_thinking,
            agent_id: dto.agent_id,
            group_id: dto.group_id,
            topic_id: dto.topic_id,
            is_group_message: dto.is_group_message,
            finish_reason: dto.finish_reason,
            attachments: dto.attachments.map(|atts| {
                atts.into_iter()
                    .map(|a| crate::vcp_modules::chat_manager::Attachment {
                        r#type: a.r#type,
                        src: "".to_string(), // 在下游的 process_topic_messages 里会被 path_map 自动填充
                        name: a.name,
                        size: a.size,
                        hash: Some(a.hash),
                        status: a.status,
                        internal_path: "".to_string(),
                        extracted_text: a.extracted_text,
                        image_frames: a.image_frames,
                        thumbnail_path: None,
                        created_at: a.created_at,
                    })
                    .collect()
            }),
            blocks: None, // ⚡ 同步下载阶段不再执行耗时预渲染，直接设为 None，由 Lazy Render 闭环接管！
            content_hash: dto.content_hash,
            shell: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
    use crate::vcp_modules::topic_types::Topic;
    use serde_json::json;

    #[test]
    fn test_agent_topic_sync_dto_deserialization_defaults_locked_and_unread() {
        let dto: AgentTopicSyncDTO = serde_json::from_value(json!({
            "id": "topic-1",
            "name": "Topic",
            "createdAt": 123,
            "ownerId": "agent-1"
        }))
        .unwrap();

        assert!(dto.locked);
        assert!(!dto.unread);
    }

    #[test]
    fn test_agent_and_group_topic_dto_from_topic_preserve_contract_fields() {
        let topic = Topic {
            id: "topic-1".to_string(),
            name: "Topic".to_string(),
            created_at: 123,
            locked: false,
            unread: true,
            unread_count: 2,
            msg_count: 3,
            owner_id: "owner-1".to_string(),
            owner_type: "agent".to_string(),
        };

        let agent_dto = AgentTopicSyncDTO::from(&topic);
        assert_eq!(agent_dto.id, "topic-1");
        assert_eq!(agent_dto.name, "Topic");
        assert_eq!(agent_dto.created_at, 123);
        assert!(!agent_dto.locked);
        assert!(agent_dto.unread);
        assert_eq!(agent_dto.owner_id, "owner-1");

        let group_dto = GroupTopicSyncDTO::from(&topic);
        assert_eq!(group_dto.id, "topic-1");
        assert_eq!(group_dto.name, "Topic");
        assert_eq!(group_dto.created_at, 123);
        assert_eq!(group_dto.owner_id, "owner-1");
    }

    #[test]
    fn test_attachment_sync_dto_from_attachment_preserves_sync_fields_only() {
        let attachment = Attachment {
            r#type: "image".to_string(),
            src: "/local/path.png".to_string(),
            name: "path.png".to_string(),
            size: 42,
            hash: Some("A".repeat(64)),
            status: Some("ready".to_string()),
            internal_path: "internal/path.png".to_string(),
            extracted_text: Some("text".to_string()),
            image_frames: Some(vec!["frame-1".to_string()]),
            thumbnail_path: Some("thumb.png".to_string()),
            created_at: Some(100),
        };

        let dto = AttachmentSyncDTO::try_from(&attachment).expect("valid attachment DTO");
        assert_eq!(dto.r#type, "image");
        assert_eq!(dto.name, "path.png");
        assert_eq!(dto.size, 42);
        assert_eq!(dto.hash, "a".repeat(64));
        assert_eq!(dto.status.as_deref(), Some("ready"));
        assert_eq!(dto.extracted_text.as_deref(), Some("text"));
        assert_eq!(dto.image_frames.as_ref().unwrap()[0], "frame-1");
        assert_eq!(dto.created_at, Some(100));

        let json = serde_json::to_value(&dto).unwrap();
        let obj = json.as_object().unwrap();
        assert!(!obj.contains_key("src"));
        assert!(!obj.contains_key("internalPath"));
        assert!(!obj.contains_key("thumbnailPath"));
    }

    #[test]
    fn attachment_sync_dto_rejects_missing_or_invalid_hash() {
        let mut attachment = Attachment {
            r#type: "file".to_string(),
            src: String::new(),
            name: "missing.bin".to_string(),
            size: 1,
            hash: None,
            status: None,
            internal_path: String::new(),
            extracted_text: None,
            image_frames: None,
            thumbnail_path: None,
            created_at: None,
        };
        assert!(AttachmentSyncDTO::try_from(&attachment).is_err());
        attachment.hash = Some("not-a-sha256".to_string());
        assert!(AttachmentSyncDTO::try_from(&attachment).is_err());
    }

    #[test]
    fn test_message_pull_sync_dto_into_chat_message_maps_attachments_and_defaults_local_paths() {
        let dto = MessagePullSyncDTO {
            id: "msg-1".to_string(),
            role: "user".to_string(),
            name: Some("User".to_string()),
            content: "hello".to_string(),
            timestamp: 123,
            updated_at: 124,
            is_thinking: Some(false),
            agent_id: Some("agent-1".to_string()),
            group_id: Some("group-1".to_string()),
            topic_id: Some("topic-1".to_string()),
            is_group_message: Some(true),
            finish_reason: Some("stop".to_string()),
            attachments: Some(vec![AttachmentSyncDTO {
                r#type: "file".to_string(),
                name: "a.txt".to_string(),
                size: 10,
                hash: "hash-a".to_string(),
                status: Some("ready".to_string()),
                extracted_text: Some("extracted".to_string()),
                image_frames: None,
                created_at: Some(200),
            }]),
            content_hash: Some("content-hash".to_string()),
            avatar_color: Some("#fff".to_string()),
        };

        let msg = ChatMessage::from(dto);
        assert_eq!(msg.updated_at, Some(124));
        assert_eq!(msg.id, "msg-1");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.name.as_deref(), Some("User"));
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.timestamp, 123);
        assert_eq!(msg.agent_id.as_deref(), Some("agent-1"));
        assert_eq!(msg.group_id.as_deref(), Some("group-1"));
        assert_eq!(msg.topic_id.as_deref(), Some("topic-1"));
        assert_eq!(msg.is_group_message, Some(true));
        assert_eq!(msg.content_hash.as_deref(), Some("content-hash"));
        assert!(msg.blocks.is_none());
        assert!(msg.shell.is_none());

        let attachment = &msg.attachments.as_ref().unwrap()[0];
        assert_eq!(attachment.r#type, "file");
        assert_eq!(attachment.name, "a.txt");
        assert_eq!(attachment.hash.as_deref(), Some("hash-a"));
        assert_eq!(attachment.src, "");
        assert_eq!(attachment.internal_path, "");
        assert!(attachment.thumbnail_path.is_none());
    }
}
