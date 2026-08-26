use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_error::WireSyncError;
use crate::vcp_modules::topic_types::{MessageKey, OwnerKey, TopicKey};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

pub const SYNC_TOMBSTONE_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// =================================================================
/// vcp_modules/sync_types.rs - 分布式 LWW+Hash 同步协议的核心数据结构
/// =================================================================
/// 计算 JSON 的确定性 SHA-256 Hash
pub fn compute_deterministic_hash<T: Serialize>(data: &T) -> String {
    if let Ok(val) = serde_json::to_value(data) {
        let json_str = stable_stringify(&val);
        crate::vcp_modules::infra::utils::calculate_sha256(json_str.as_bytes())
    } else {
        "".to_string()
    }
}

/// 计算一组哈希的聚合哈希 (Merkle Root)
/// 规则：调用方先将实体身份绑定进叶子，再排序叶子哈希并计算总 Hash
pub fn compute_merkle_root(mut hashes: Vec<String>) -> String {
    if hashes.is_empty() {
        return "".to_string();
    }
    hashes.sort();
    let mut hasher = Sha256::new();
    for h in hashes {
        hasher.update(h.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Protocol-level avatar identity contract. Agent and group avatars use a
/// non-empty entity id; the only user avatar identity is the fixed singleton.
pub fn is_valid_avatar_owner(owner_type: &str, owner_id: &str) -> bool {
    match AvatarOwnerType::try_from(owner_type) {
        Ok(AvatarOwnerType::Agent | AvatarOwnerType::Group) => !owner_id.is_empty(),
        Ok(AvatarOwnerType::User) => owner_id == "user_avatar",
        Err(()) => false,
    }
}

pub fn stable_stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut res = String::new();
            res.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    res.push(',');
                }
                res.push_str(&format!(
                    "\"{}\":{}",
                    k,
                    stable_stringify(map.get(*k).unwrap())
                ));
            }
            res.push('}');
            res
        }
        serde_json::Value::Array(arr) => {
            let mut res = String::new();
            res.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    res.push(',');
                }
                res.push_str(&stable_stringify(v));
            }
            res.push(']');
            res
        }
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
}

/// Manifest 状态集合类别。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ManifestType {
    Owner,
    Topic,
    Avatar,
}

impl fmt::Display for ManifestType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestType::Owner => write!(f, "owner"),
            ManifestType::Topic => write!(f, "topic"),
            ManifestType::Avatar => write!(f, "avatar"),
        }
    }
}

/// Agent 与 Group 的业务命名空间。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum OwnerType {
    Agent,
    Group,
}

impl OwnerType {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerType::Agent => "agent",
            OwnerType::Group => "group",
        }
    }
}

impl fmt::Display for OwnerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for OwnerType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "agent" => Ok(OwnerType::Agent),
            "group" => Ok(OwnerType::Group),
            _ => Err(()),
        }
    }
}

/// 公共 HTTP Entity 选择器。Owner 与 Topic 始终携带完整身份。
#[derive(Debug, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(tag = "entityType", rename_all = "lowercase")]
pub enum EntitySelector {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
    },
}

impl EntitySelector {
    pub fn owner(owner_type: OwnerType, owner_id: impl Into<String>) -> Self {
        Self::Owner {
            owner_type,
            owner_id: owner_id.into(),
        }
    }

    pub fn topic(key: &TopicKey) -> Result<Self, String> {
        let owner_type = OwnerType::try_from(key.owner_type.as_str())
            .map_err(|_| "Entity topic selector requires agent/group ownerType".to_string())?;
        if key.owner_id.is_empty() || key.topic_id.is_empty() {
            return Err("Entity topic selector requires complete identity".to_string());
        }
        Ok(Self::Topic {
            owner_type,
            owner_id: key.owner_id.clone(),
            topic_id: key.topic_id.clone(),
        })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
            } => format!("owner/{owner_type}/{owner_id}"),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
            } => format!("topic/{owner_type}/{owner_id}/{topic_id}"),
        }
    }

    pub fn topic_id(&self) -> Option<&str> {
        match self {
            Self::Owner { .. } => None,
            Self::Topic { topic_id, .. } => Some(topic_id),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EntityPullRequest {
    pub items: Vec<EntitySelector>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
pub enum EntityPushData {
    Agent(AgentSyncDTO),
    Group(GroupSyncDTO),
    AgentTopic(AgentTopicSyncDTO),
    GroupTopic(GroupTopicSyncDTO),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EntityPullData {
    Agent(AgentSyncDTO),
    Group(GroupSyncDTO),
    GroupTopic(GroupTopicSyncDTO),
    AgentTopic(AgentTopicSyncDTO),
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "entityType", rename_all = "lowercase")]
pub enum EntityPushItem {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        data: EntityPushData,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        data: EntityPushData,
    },
}

impl EntityPushItem {
    pub fn selector(&self) -> EntitySelector {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
                ..
            } => EntitySelector::owner(*owner_type, owner_id),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
                ..
            } => EntitySelector::Topic {
                owner_type: *owner_type,
                owner_id: owner_id.clone(),
                topic_id: topic_id.clone(),
            },
        }
    }

    pub fn is_consistent(&self) -> bool {
        match self {
            Self::Owner {
                owner_type: OwnerType::Agent,
                owner_id,
                data: EntityPushData::Agent(_),
            }
            | Self::Owner {
                owner_type: OwnerType::Group,
                owner_id,
                data: EntityPushData::Group(_),
            } => !owner_id.is_empty(),
            Self::Topic {
                owner_type: OwnerType::Agent,
                owner_id,
                topic_id,
                data: EntityPushData::AgentTopic(_),
            }
            | Self::Topic {
                owner_type: OwnerType::Group,
                owner_id,
                topic_id,
                data: EntityPushData::GroupTopic(_),
            } => !owner_id.is_empty() && !topic_id.is_empty(),
            _ => false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EntityPushRequest {
    pub items: Vec<EntityPushItem>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "entityType", rename_all = "lowercase", deny_unknown_fields)]
pub enum EntityPullResult {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        ok: bool,
        #[serde(default)]
        data: Option<EntityPullData>,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        ok: bool,
        #[serde(default)]
        data: Option<EntityPullData>,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityPullResponse {
    pub results: Vec<EntityPullResult>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "entityType", rename_all = "lowercase", deny_unknown_fields)]
pub enum EntityPushResult {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
}

impl EntityPushResult {
    pub fn into_parts(self) -> (EntitySelector, bool, Option<WireSyncError>) {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
                ok,
                error,
            } => (EntitySelector::owner(owner_type, owner_id), ok, error),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
                ok,
                error,
            } => (
                EntitySelector::Topic {
                    owner_type,
                    owner_id,
                    topic_id,
                },
                ok,
                error,
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityPushResponse {
    pub results: Vec<EntityPushResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarPushResponse {
    pub owner_type: AvatarOwnerType,
    pub owner_id: String,
    pub ok: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MessagePushResponseFrame {
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
    StreamError {
        error: WireSyncError,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePullTopicSelector {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub message_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagePullRequest {
    pub topics: Vec<MessagePullTopicSelector>,
}

/// Avatar 允许的命名空间；`user` 只允许固定的 `user_avatar` 身份。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AvatarOwnerType {
    Agent,
    Group,
    User,
}

impl AvatarOwnerType {
    pub fn as_str(self) -> &'static str {
        match self {
            AvatarOwnerType::Agent => "agent",
            AvatarOwnerType::Group => "group",
            AvatarOwnerType::User => "user",
        }
    }
}

impl fmt::Display for AvatarOwnerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for AvatarOwnerType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "agent" => Ok(AvatarOwnerType::Agent),
            "group" => Ok(AvatarOwnerType::Group),
            "user" => Ok(AvatarOwnerType::User),
            _ => Err(()),
        }
    }
}

/// Mobile 内部使用完整身份承载删除目标，避免可选字段组合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTarget {
    Owner {
        owner_type: OwnerType,
        owner_id: String,
    },
    Topic(TopicKey),
    Avatar {
        owner_type: AvatarOwnerType,
        owner_id: String,
    },
    Message(MessageKey),
}

/// Manifest 仲裁动作。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManifestAction {
    Pull,
    Push,
    PullDelete,
    PushDelete,
    Skip,
}

impl ManifestAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestAction::Pull => "PULL",
            ManifestAction::Push => "PUSH",
            ManifestAction::PullDelete => "PULL_DELETE",
            ManifestAction::PushDelete => "PUSH_DELETE",
            ManifestAction::Skip => "SKIP",
        }
    }

    pub fn is_delete(self) -> bool {
        matches!(
            self,
            ManifestAction::PullDelete | ManifestAction::PushDelete
        )
    }
}

impl fmt::Display for ManifestAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    OwnerMetadata,
    TopicMetadata,
    Messages,
}

impl SyncPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncPhase::OwnerMetadata => "owner_metadata",
            SyncPhase::TopicMetadata => "topic_metadata",
            SyncPhase::Messages => "messages",
        }
    }
}

impl fmt::Display for SyncPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum OwnerManifestState {
    Live(OwnerManifestLive),
    Deleted(OwnerManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestLive {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub config_hash: String,
    pub content_hash: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestDeleted {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum TopicManifestState {
    Live(TopicManifestLive),
    Deleted(TopicManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestLive {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub config_hash: String,
    pub content_hash: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestDeleted {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum AvatarManifestState {
    Live(AvatarManifestLive),
    Deleted(AvatarManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestLive {
    pub owner_type: AvatarOwnerType,
    pub owner_id: String,
    pub binary_hash: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestDeleted {
    pub owner_type: AvatarOwnerType,
    pub owner_id: String,
    pub deleted_at: i64,
}

/// Manifest 的三种条目在类型层分离，墓碑不再携带伪造的 live Hash。
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "manifestType", rename_all = "lowercase")]
pub enum ManifestRequest {
    Owner {
        items: Vec<OwnerManifestState>,
    },
    Topic {
        items: Vec<TopicManifestState>,
        #[serde(rename = "targetedOwners")]
        targeted_owners: Vec<OwnerKey>,
    },
    Avatar {
        items: Vec<AvatarManifestState>,
    },
}

impl ManifestRequest {
    pub fn manifest_type(&self) -> ManifestType {
        match self {
            ManifestRequest::Owner { .. } => ManifestType::Owner,
            ManifestRequest::Topic { .. } => ManifestType::Topic,
            ManifestRequest::Avatar { .. } => ManifestType::Avatar,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestDecision {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    pub deleted_at: Option<i64>,
    #[serde(default)]
    pub content_hash_mismatch: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestDecision {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestDecision {
    pub owner_type: AvatarOwnerType,
    pub owner_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "manifestType", rename_all = "lowercase", deny_unknown_fields)]
pub enum ManifestResultFrame {
    Owner {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        results: Vec<OwnerManifestDecision>,
    },
    Topic {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        results: Vec<TopicManifestDecision>,
    },
    Avatar {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        results: Vec<AvatarManifestDecision>,
    },
}

impl ManifestResultFrame {
    pub fn manifest_type(&self) -> ManifestType {
        match self {
            ManifestResultFrame::Owner { .. } => ManifestType::Owner,
            ManifestResultFrame::Topic { .. } => ManifestType::Topic,
            ManifestResultFrame::Avatar { .. } => ManifestType::Avatar,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ManifestRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    #[serde(flatten)]
    pub manifest: ManifestRequest,
}

impl ManifestRequestFrame {
    pub fn new(manifest: ManifestRequest) -> Self {
        Self {
            frame_type: "SYNC_MANIFEST_REQUEST",
            manifest,
        }
    }
}

fn deserialize_manifest_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_MANIFEST_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_MANIFEST_RESULT"))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicDiffState {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub config_hash: String,
    pub content_hash: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDiffRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    pub topics: Vec<TopicDiffState>,
}

impl TopicDiffRequestFrame {
    pub fn new(topics: Vec<TopicDiffState>) -> Self {
        Self {
            frame_type: "SYNC_TOPIC_DIFF_REQUEST",
            topics,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicDiffResultFrame {
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_topic_diff_result_type"
    )]
    _frame_type: (),
    pub changed_topics: Vec<TopicKey>,
}

fn deserialize_topic_diff_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_TOPIC_DIFF_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_TOPIC_DIFF_RESULT"))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum MessageVersionState {
    Live(MessageLiveState),
    Deleted(MessageDeletedState),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageLiveState {
    pub message_hash: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeletedState {
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffTopicState {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub content_hash: String,
    pub messages: BTreeMap<String, MessageVersionState>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDiffRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    pub topics: Vec<MessageDiffTopicState>,
}

impl MessageDiffRequestFrame {
    pub fn new(topics: Vec<MessageDiffTopicState>) -> Self {
        Self {
            frame_type: "SYNC_MESSAGE_DIFF_REQUEST",
            topics,
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeleteDecision {
    pub msg_id: String,
    pub deleted_at: i64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffDecision {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub ok: bool,
    #[serde(default)]
    pub pull_message_ids: Option<Vec<String>>,
    #[serde(default)]
    pub push_topic: Option<bool>,
    #[serde(default)]
    pub delete_messages: Option<Vec<MessageDeleteDecision>>,
    #[serde(default)]
    pub error: Option<WireSyncError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffResultFrame {
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_message_diff_result_type"
    )]
    _frame_type: (),
    pub results: Vec<MessageDiffDecision>,
}

fn deserialize_message_diff_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_MESSAGE_DIFF_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_MESSAGE_DIFF_RESULT"))
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "targetType", rename_all = "lowercase")]
enum DeleteNotificationTarget {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: String,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
    },
    Avatar {
        #[serde(rename = "ownerType")]
        owner_type: AvatarOwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Message {
        #[serde(rename = "ownerType")]
        owner_type: String,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        #[serde(rename = "msgId")]
        msg_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNotificationFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    #[serde(flatten)]
    target: DeleteNotificationTarget,
    deleted_at: i64,
}

impl DeleteNotificationFrame {
    pub fn new(target: DeleteTarget, deleted_at: i64) -> Self {
        let target = match target {
            DeleteTarget::Owner {
                owner_type,
                owner_id,
            } => DeleteNotificationTarget::Owner {
                owner_type,
                owner_id,
            },
            DeleteTarget::Topic(key) => DeleteNotificationTarget::Topic {
                owner_type: key.owner_type,
                owner_id: key.owner_id,
                topic_id: key.topic_id,
            },
            DeleteTarget::Avatar {
                owner_type,
                owner_id,
            } => DeleteNotificationTarget::Avatar {
                owner_type,
                owner_id,
            },
            DeleteTarget::Message(key) => DeleteNotificationTarget::Message {
                owner_type: key.topic.owner_type,
                owner_id: key.topic.owner_id,
                topic_id: key.topic.topic_id,
                msg_id: key.msg_id,
            },
        };
        Self {
            frame_type: "SYNC_ENTITY_DELETE",
            target,
            deleted_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn avatar_owner_contract_is_closed_and_keeps_the_user_singleton() {
        assert!(is_valid_avatar_owner("agent", "agent-1"));
        assert!(is_valid_avatar_owner("group", "group-1"));
        assert!(is_valid_avatar_owner("user", "user_avatar"));
        assert!(!is_valid_avatar_owner("user", "other"));
        assert!(!is_valid_avatar_owner("agent", ""));
        assert!(!is_valid_avatar_owner("system", "system"));
    }

    #[test]
    fn test_stable_stringify_sorts_object_keys_recursively() {
        let value = json!({
            "z": 1,
            "a": {
                "b": true,
                "a": [3, 2, 1]
            },
            "m": null
        });

        assert_eq!(
            stable_stringify(&value),
            r#"{"a":{"a":[3,2,1],"b":true},"m":null,"z":1}"#
        );
    }

    #[test]
    fn test_compute_deterministic_hash_is_key_order_independent() {
        let left = json!({ "name": "Nova", "config": { "model": "a", "temperature": 0.7 } });
        let right = json!({ "config": { "temperature": 0.7, "model": "a" }, "name": "Nova" });

        assert_eq!(
            compute_deterministic_hash(&left),
            compute_deterministic_hash(&right)
        );
    }

    #[test]
    fn test_compute_merkle_root_is_order_independent_and_empty_safe() {
        let hashes_a = vec!["ccc".to_string(), "aaa".to_string(), "bbb".to_string()];
        let hashes_b = vec!["bbb".to_string(), "ccc".to_string(), "aaa".to_string()];

        assert_eq!(compute_merkle_root(hashes_a), compute_merkle_root(hashes_b));
        assert_eq!(compute_merkle_root(Vec::new()), "");
    }

    #[test]
    fn sync_discriminators_have_one_vocabulary_each() {
        assert_eq!(ManifestType::Owner.to_string(), "owner");
        assert_eq!(OwnerType::Agent.to_string(), "agent");
        assert_eq!(AvatarOwnerType::User.to_string(), "user");
        assert_eq!(ManifestAction::PullDelete.to_string(), "PULL_DELETE");

        let manifest: ManifestType = serde_json::from_str(r#""topic""#).unwrap();
        assert_eq!(manifest, ManifestType::Topic);
    }
}
