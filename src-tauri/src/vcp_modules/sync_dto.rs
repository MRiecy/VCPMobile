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
    pub temperature: f32,
    pub context_token_limit: i32,
    pub max_output_tokens: i32,

    // 模型高级参数
    #[serde(rename = "top_p", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(rename = "top_k", skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
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
            top_p: config.top_p,
            top_k: config.top_k,
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
        }
    }
}

/// 话题同步数据传输对象
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub locked: bool,
    pub unread: bool,
    pub owner_id: String,
    pub owner_type: String,
}

impl From<&Topic> for TopicSyncDTO {
    fn from(topic: &Topic) -> Self {
        Self {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            locked: topic.locked,
            unread: topic.unread,
            owner_id: topic.owner_id.clone(),
            owner_type: if topic.owner_type.is_empty() {
                "agent".to_string()
            } else {
                topic.owner_type.clone()
            },
        }
    }
}

/// 附件解析数据同步 DTO (跨端共享的计算/解析结果)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileManagerDataSyncDTO {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
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

    // 附件解析元数据
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

/// 消息同步 DTO (双端共识契约)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    pub timestamp: u64,

    // 扩展元数据 (仅包含“真理”，不包含本地路径等推导字段)
    #[serde(rename = "isThinking", skip_serializing_if = "Option::is_none")]
    pub is_thinking: Option<bool>,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "isGroupMessage", skip_serializing_if = "Option::is_none")]
    pub is_group_message: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
}

impl From<&ChatMessage> for MessageSyncDTO {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            is_thinking: msg.is_thinking,
            agent_id: msg.agent_id.clone(),
            group_id: msg.group_id.clone(),
            is_group_message: msg.is_group_message,
            attachments: msg
                .attachments
                .as_ref()
                .map(|atts| atts.iter().map(AttachmentSyncDTO::from).collect()),
        }
    }
}
