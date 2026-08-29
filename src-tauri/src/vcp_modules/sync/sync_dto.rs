use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::group_types::{deserialize_member_tags, GroupConfig, MemberTags};
use crate::vcp_modules::topic_types::Topic;
use serde::{Deserialize, Serialize};

/// =================================================================
/// vcp_modules/sync_dto.rs - 双端同步标准契约 (The Shared Truth)
/// =================================================================
/// 智能体同步数据传输对象
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupSyncDTO {
    pub name: String,
    pub members: Vec<String>,
    pub mode: String,
    #[serde(
        default,
        deserialize_with = "deserialize_member_tags",
        skip_serializing_if = "Option::is_none"
    )]
    pub member_tags: Option<MemberTags>,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    pub extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

/// 消息持久同步 DTO：Push/Pull 共用同一份 canonical 契约。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    #[serde(
        rename = "isThinking",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_thinking: Option<bool>,
    #[serde(rename = "agentId", default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    #[serde(
        rename = "isGroupMessage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_group_message: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
    #[serde(rename = "contentHash", skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl From<MessageSyncDTO> for crate::vcp_modules::chat_manager::ChatMessage {
    fn from(dto: MessageSyncDTO) -> Self {
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
                        status: None,
                        attachment_order: None,
                        internal_path: "".to_string(),
                        extracted_text: a.extracted_text,
                        image_frames: a.image_frames,
                        thumbnail_path: None,
                        created_at: a.created_at,
                    })
                    .collect()
            }),
            // blocks/render_cache 是本地状态；下游依设置可选预渲染，否则由加载路径懒生成。
            blocks: None,
            content_hash: dto.content_hash,
            shell: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::chat_manager::ChatMessage;
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
    fn group_sync_dto_requires_string_member_tags_with_non_empty_keys() {
        let valid: GroupSyncDTO = serde_json::from_value(json!({
            "name": "Group",
            "members": ["agent-1"],
            "mode": "naturerandom",
            "memberTags": { "agent-1": "猫娘, 科学", "历史成员": "" },
            "useUnifiedModel": false,
            "createdAt": 1
        }))
        .expect("string memberTags map");
        assert_eq!(
            valid
                .member_tags
                .as_ref()
                .and_then(|tags| tags.get("agent-1"))
                .map(String::as_str),
            Some("猫娘, 科学")
        );

        for invalid_tags in [json!({ "agent-1": ["猫娘"] }), json!({ "": "猫娘" })] {
            let result = serde_json::from_value::<GroupSyncDTO>(json!({
                "name": "Group",
                "members": ["agent-1"],
                "mode": "naturerandom",
                "memberTags": invalid_tags,
                "useUnifiedModel": false,
                "createdAt": 1
            }));
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_message_sync_dto_into_chat_message_maps_attachments_and_defaults_local_paths() {
        let dto = MessageSyncDTO {
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
                extracted_text: Some("extracted".to_string()),
                image_frames: None,
                created_at: Some(200),
            }]),
            content_hash: Some("content-hash".to_string()),
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
