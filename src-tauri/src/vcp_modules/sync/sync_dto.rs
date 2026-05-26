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

impl From<&Attachment> for AttachmentSyncDTO {
    fn from(att: &Attachment) -> Self {
        Self {
            r#type: att.r#type.clone(),
            name: att.name.clone(),
            size: att.size,
            hash: att.hash.clone().unwrap_or_default(),
            status: att.status.clone(),
            extracted_text: att.extracted_text.clone(),
            image_frames: att.image_frames.clone(),
            created_at: att.created_at,
        }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl From<&ChatMessage> for UserMessageSyncDTO {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            attachments: msg
                .attachments
                .as_ref()
                .map(|atts| atts.iter().map(AttachmentSyncDTO::from).collect()),
            content_hash: msg.content_hash.clone(),
        }
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
    pub fn from_message(msg: &ChatMessage, avatar_color: String) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
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
    pub fn from_message(msg: &ChatMessage, avatar_color: String) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
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
