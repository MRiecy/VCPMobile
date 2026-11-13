use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::group_types::{
    default_group_mode, default_tag_match_mode, deserialize_member_tags, GroupConfig, MemberTags,
};
use serde::{Deserialize, Serialize};

fn deserialize_required_sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().all(|byte| !byte.is_ascii_uppercase())
    {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "expected a lowercase 64-character SHA-256",
        ))
    }
}

fn deserialize_sync_updated_at<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    if (0..=(1_i64 << 53) - 1).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "expected a non-negative safe integer updatedAt",
        ))
    }
}

/// =================================================================
/// vcp_modules/sync_dto.rs - 双端同步标准契约 (The Shared Truth)
/// =================================================================
/// 接收端以手机 DTO 投影已知字段，忽略对端新增字段；必填字段仍由 Serde 校验。
/// 可选字段缺失时沿用手机强类型默认；显式 null 保留 Option 语义。
/// 兼容字段的容忍是单向的：桌面到手机可缺省，手机到桌面仍构造其所需投影。
/// 不因手机没有独立业务消费者而删除兼容字段，也不为真实业务身份补造默认值。
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
    /// 桌面兼容原值，不驱动手机 chatMode；缺失采用既有默认，回推仍携带。
    #[serde(default = "default_group_mode")]
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
    #[serde(
        default = "default_tag_match_mode",
        skip_serializing_if = "Option::is_none"
    )]
    pub tag_match_mode: Option<String>,
    /// 桌面兼容时间；缺失使用配置默认 0，不表示已恢复远端真实创建时间。
    #[serde(default)]
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
    pub locked: bool,
    pub unread: bool,
    pub owner_id: String,
    #[serde(
        rename = "configHash",
        deserialize_with = "deserialize_required_sha256"
    )]
    pub config_hash: String,
    #[serde(rename = "updatedAt", deserialize_with = "deserialize_sync_updated_at")]
    pub updated_at: i64,
}

/// Group Topic 同步 DTO (无 locked/unread)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GroupTopicSyncDTO {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub owner_id: String,
    #[serde(
        rename = "configHash",
        deserialize_with = "deserialize_required_sha256"
    )]
    pub config_hash: String,
    #[serde(rename = "updatedAt", deserialize_with = "deserialize_sync_updated_at")]
    pub updated_at: i64,
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
    /// 保留桌面兼容投影；接收可缺失，上传无本机帧元数据时明确输出 null。
    #[serde(default)]
    pub image_frames: Option<Vec<String>>,
    /// 接收可缺失；上传从本机附件表读取，不能承诺与远端原始时间原样往返。
    #[serde(default)]
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
    #[serde(rename = "agentId", default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// 以下三项接收可缺省，上传按权威 TopicKey 重建；不替代发言者 agentId。
    /// 单聊 groupId 明确输出 null，不能用空字符串伪造群组身份。
    #[serde(rename = "groupId", default)]
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
    #[serde(
        rename = "contentHash",
        deserialize_with = "deserialize_required_sha256"
    )]
    pub content_hash: String,
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
            is_thinking: None,
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
            content_hash: Some(dto.content_hash),
            shell: None,
        }
    }
}
