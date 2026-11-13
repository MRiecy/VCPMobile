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
use std::io::{self, Write};

pub const SYNC_TOMBSTONE_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// =================================================================
/// vcp_modules/sync_types.rs - 分布式 LWW+Hash 同步协议的核心数据结构
/// =================================================================
/// 计算 JSON 的确定性 SHA-256 Hash
pub fn compute_deterministic_hash(data: &serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    let result = {
        let mut writer = Sha256Writer(&mut hasher);
        write_stable_json(data, &mut writer)
    };
    if result.is_err() {
        return String::new();
    }
    crate::vcp_modules::infra::utils::finalize_sha256_hex(hasher)
}

/// 计算一组哈希的聚合哈希 (Merkle Root)
/// 规则：调用方先将实体身份绑定进叶子，再排序叶子哈希并计算总 Hash
pub fn compute_merkle_root(mut hashes: Vec<String>) -> String {
    if hashes.is_empty() {
        return "".to_string();
    }
    hashes.sort_unstable();
    let mut hasher = Sha256::new();
    for h in hashes {
        hasher.update(h.as_bytes());
    }
    crate::vcp_modules::infra::utils::finalize_sha256_hex(hasher)
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

struct Sha256Writer<'a>(&'a mut Sha256);

impl Write for Sha256Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_json_string<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    serde_json::to_writer(writer, value).map_err(io::Error::other)
}

fn write_stable_json<W: Write>(value: &serde_json::Value, writer: &mut W) -> io::Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            writer.write_all(b"{")?;
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    writer.write_all(b",")?;
                }
                write_json_string(writer, key)?;
                writer.write_all(b":")?;
                let child = map.get(key).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "canonical JSON key disappeared")
                })?;
                write_stable_json(child, writer)?;
            }
            writer.write_all(b"}")
        }
        serde_json::Value::Array(arr) => {
            writer.write_all(b"[")?;
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    writer.write_all(b",")?;
                }
                write_stable_json(v, writer)?;
            }
            writer.write_all(b"]")
        }
        serde_json::Value::String(value) => write_json_string(writer, value),
        serde_json::Value::Number(value) => write!(writer, "{value}"),
        serde_json::Value::Bool(true) => writer.write_all(b"true"),
        serde_json::Value::Bool(false) => writer.write_all(b"false"),
        serde_json::Value::Null => writer.write_all(b"null"),
    }
}

#[cfg(test)]
fn stable_stringify(value: &serde_json::Value) -> String {
    let mut bytes = Vec::new();
    if write_stable_json(value, &mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8(bytes).unwrap_or_default()
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

/// 由 EntityPullResult 的外层身份选择 DTO，不能按字段形状猜测类型。
#[derive(Debug)]
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

#[derive(Debug)]
pub enum EntityPullResult {
    Owner {
        owner_type: OwnerType,
        owner_id: String,
        ok: bool,
        data: Option<EntityPullData>,
        error: Option<WireSyncError>,
    },
    Topic {
        owner_type: OwnerType,
        owner_id: String,
        topic_id: String,
        ok: bool,
        data: Option<EntityPullData>,
        error: Option<WireSyncError>,
    },
}

impl<'de> Deserialize<'de> for EntityPullResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 先读取身份，再按身份解析 data。AgentTopic 的字段包含 GroupTopic 的字段，
        // 忽略新增字段后，untagged 会误选变体并掩盖真正缺失的字段。
        #[derive(Deserialize)]
        #[serde(tag = "entityType", rename_all = "lowercase")]
        enum WireResult {
            Owner {
                #[serde(rename = "ownerType")]
                owner_type: OwnerType,
                #[serde(rename = "ownerId")]
                owner_id: String,
                ok: bool,
                data: Option<serde_json::Value>,
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
                data: Option<serde_json::Value>,
                error: Option<WireSyncError>,
            },
        }

        match WireResult::deserialize(deserializer)? {
            WireResult::Owner {
                owner_type,
                owner_id,
                ok,
                data,
                error,
            } => {
                let data = data
                    .map(|value| match owner_type {
                        OwnerType::Agent => {
                            serde_json::from_value(value).map(EntityPullData::Agent)
                        }
                        OwnerType::Group => {
                            serde_json::from_value(value).map(EntityPullData::Group)
                        }
                    })
                    .transpose()
                    .map_err(|error| {
                        D::Error::custom(format!(
                            "Entity pull owner/{owner_type}/{owner_id} data: {error}"
                        ))
                    })?;
                Ok(Self::Owner {
                    owner_type,
                    owner_id,
                    ok,
                    data,
                    error,
                })
            }
            WireResult::Topic {
                owner_type,
                owner_id,
                topic_id,
                ok,
                data,
                error,
            } => {
                let data = data
                    .map(|value| match owner_type {
                        OwnerType::Agent => {
                            serde_json::from_value(value).map(EntityPullData::AgentTopic)
                        }
                        OwnerType::Group => {
                            serde_json::from_value(value).map(EntityPullData::GroupTopic)
                        }
                    })
                    .transpose()
                    .map_err(|error| {
                        D::Error::custom(format!(
                            "Entity pull topic/{owner_type}/{owner_id}/{topic_id} data: {error}"
                        ))
                    })?;
                Ok(Self::Topic {
                    owner_type,
                    owner_id,
                    topic_id,
                    ok,
                    data,
                    error,
                })
            }
        }
    }
}

#[derive(Debug, Deserialize)]
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
    pub push_message_ids: Option<Vec<String>>,
    #[serde(default)]
    pub push_deleted_messages: Option<Vec<MessageDeleteDecision>>,
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

    fn entity_pull_cases() -> Vec<(serde_json::Value, &'static [&'static str])> {
        let hash = "a".repeat(64);
        vec![
            (
                serde_json::json!({
                    "entityType": "owner", "ownerType": "agent", "ownerId": "agent-1", "ok": true,
                    "data": {
                        "name": "Agent", "systemPrompt": "Prompt", "model": "model",
                        "temperature": 0.7, "contextTokenLimit": 8192,
                        "maxOutputTokens": 2048, "streamOutput": true
                    }
                }),
                &[
                    "name",
                    "systemPrompt",
                    "model",
                    "temperature",
                    "contextTokenLimit",
                    "maxOutputTokens",
                    "streamOutput",
                ],
            ),
            (
                serde_json::json!({
                    "entityType": "owner", "ownerType": "group", "ownerId": "group-1", "ok": true,
                    "data": {
                        "name": "Group", "members": ["agent-1"], "mode": "sequential",
                        "memberTags": {}, "groupPrompt": "", "invitePrompt": "Invite",
                        "useUnifiedModel": false, "unifiedModel": "", "tagMatchMode": "strict",
                        "createdAt": 1
                    }
                }),
                &["name", "members", "useUnifiedModel"],
            ),
            (
                serde_json::json!({
                    "entityType": "topic", "ownerType": "agent", "ownerId": "agent-1",
                    "topicId": "agent-topic", "ok": true,
                    "data": {
                        "id": "agent-topic", "ownerId": "agent-1", "name": "Topic",
                        "createdAt": 1, "locked": true, "unread": false,
                        "configHash": hash, "updatedAt": 2
                    }
                }),
                &[
                    "id",
                    "ownerId",
                    "name",
                    "createdAt",
                    "locked",
                    "unread",
                    "configHash",
                    "updatedAt",
                ],
            ),
            (
                serde_json::json!({
                    "entityType": "topic", "ownerType": "group", "ownerId": "group-1",
                    "topicId": "group-topic", "ok": true,
                    "data": {
                        "id": "group-topic", "ownerId": "group-1", "name": "Topic",
                        "createdAt": 1, "configHash": hash, "updatedAt": 2
                    }
                }),
                &[
                    "id",
                    "ownerId",
                    "name",
                    "createdAt",
                    "configHash",
                    "updatedAt",
                ],
            ),
        ]
    }

    #[test]
    fn entity_pull_batch_accepts_desktop_extensions_and_preserves_dto_identity() {
        let cases = entity_pull_cases();
        let mut items = vec![cases[0].0.clone(); 9];
        items.extend(cases.iter().skip(1).map(|(item, _)| item.clone()));
        for (index, item) in items.iter_mut().enumerate() {
            item["traceId"] = serde_json::json!("desktop-trace");
            item["data"]["futureField"] = serde_json::json!({"nested": [1, null, "extra"]});
            if index < 9 {
                item["ownerId"] = serde_json::json!(format!("agent-{index}"));
            }
        }
        items[9]["data"]
            .as_object_mut()
            .unwrap()
            .remove("futureField");
        items[9]["data"]["enableContextMessageWindow"] = serde_json::json!(true);
        items[9]["data"]["contextMessageWindowSize"] = serde_json::json!(100);
        // 即使字段形状满足 AgentTopic，也必须按 ownerType=group 解析。
        items[11]["data"]["locked"] = serde_json::json!(true);
        items[11]["data"]["unread"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&serde_json::json!({
            "results": items, "serverRevision": 2
        }))
        .expect("serialize batch");
        let response: EntityPullResponse = serde_json::from_slice(&bytes).expect("extended batch");
        assert_eq!(response.results.len(), 12);
        for (index, result) in response.results.iter().enumerate() {
            let (projection, expected) = match result {
                EntityPullResult::Owner {
                    data: Some(EntityPullData::Agent(dto)),
                    ..
                } if index < 9 => (serde_json::to_value(dto).unwrap(), &cases[0].0["data"]),
                EntityPullResult::Owner {
                    data: Some(EntityPullData::Group(dto)),
                    ..
                } if index == 9 => (serde_json::to_value(dto).unwrap(), &cases[1].0["data"]),
                EntityPullResult::Topic {
                    data: Some(EntityPullData::AgentTopic(dto)),
                    ..
                } if index == 10 => (serde_json::to_value(dto).unwrap(), &cases[2].0["data"]),
                EntityPullResult::Topic {
                    data: Some(EntityPullData::GroupTopic(dto)),
                    ..
                } if index == 11 => (serde_json::to_value(dto).unwrap(), &cases[3].0["data"]),
                _ => panic!("wrong DTO at batch index {index}: {result:?}"),
            };
            assert_eq!(&projection, expected);
        }
    }

    #[test]
    fn entity_pull_rejects_each_missing_or_null_required_dto_field_with_context() {
        for (item, required) in entity_pull_cases() {
            for field in required {
                for use_null in [false, true] {
                    let mut invalid = item.clone();
                    if use_null {
                        invalid["data"][field] = serde_json::Value::Null;
                    } else {
                        invalid["data"].as_object_mut().unwrap().remove(*field);
                    }
                    let bytes =
                        serde_json::to_vec(&serde_json::json!({"results": [invalid]})).unwrap();
                    let error = serde_json::from_slice::<EntityPullResponse>(&bytes)
                        .unwrap_err()
                        .to_string();
                    assert!(error.contains(item["ownerId"].as_str().unwrap()), "{error}");
                    if use_null {
                        assert!(error.contains("invalid type"), "{field}: {error}");
                    } else {
                        assert!(
                            error.contains(&format!("missing field `{field}`")),
                            "{error}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn entity_pull_uses_typed_group_defaults_without_hashing_desktop_extensions() {
        use crate::vcp_modules::sync_hash::HashAggregator;

        let mut item = entity_pull_cases()[1].0.clone();
        for field in [
            "memberTags",
            "groupPrompt",
            "invitePrompt",
            "unifiedModel",
            "tagMatchMode",
        ] {
            item["data"].as_object_mut().unwrap().remove(field);
        }
        let EntityPullResult::Owner {
            data: Some(EntityPullData::Group(dto)),
            ..
        } = serde_json::from_value(item.clone()).expect("optional fields may be absent")
        else {
            panic!("expected group");
        };
        assert!(dto.member_tags.is_none());
        assert!(dto.group_prompt.is_none());
        assert!(dto.invite_prompt.is_none());
        assert!(dto.unified_model.is_none());
        assert_eq!(dto.tag_match_mode.as_deref(), Some("strict"));
        item["data"]["enableContextMessageWindow"] = serde_json::json!(true);
        item["data"]["contextMessageWindowSize"] = serde_json::json!(200);
        let EntityPullResult::Owner {
            data: Some(EntityPullData::Group(extended)),
            ..
        } = serde_json::from_value(item).expect("desktop-only fields are ignored")
        else {
            panic!("expected group");
        };
        assert_eq!(
            HashAggregator::compute_group_config_hash(&dto),
            HashAggregator::compute_group_config_hash(&extended)
        );
    }

    #[test]
    fn entity_pull_group_defaults_do_not_replace_explicit_values_or_invalid_types() {
        for (value, expected) in [
            (serde_json::Value::Null, None),
            (serde_json::json!(""), Some("")),
            (serde_json::json!("natural"), Some("natural")),
        ] {
            let mut item = entity_pull_cases()[1].0.clone();
            item["data"]["tagMatchMode"] = value;
            let EntityPullResult::Owner {
                data: Some(EntityPullData::Group(dto)),
                ..
            } = serde_json::from_value(item).expect("explicit optional value")
            else {
                panic!("expected group");
            };
            assert_eq!(dto.tag_match_mode.as_deref(), expected);
        }
        for value in [
            serde_json::json!(false),
            serde_json::json!(1),
            serde_json::json!([]),
        ] {
            let mut item = entity_pull_cases()[1].0.clone();
            item["data"]["tagMatchMode"] = value;
            assert!(serde_json::from_value::<EntityPullResult>(item).is_err());
        }
    }

    #[test]
    fn group_desktop_compatibility_fields_may_be_omitted_but_remain_in_uploads() {
        let mut value = entity_pull_cases()[1].0["data"].clone();
        value.as_object_mut().unwrap().remove("mode");
        value.as_object_mut().unwrap().remove("createdAt");
        let dto: GroupSyncDTO =
            serde_json::from_value(value.clone()).expect("optional desktop fields");
        let outgoing = serde_json::to_value(dto).expect("desktop projection");
        assert_eq!(outgoing["mode"], "sequential");
        assert_eq!(outgoing["createdAt"], 0);

        value["mode"] = serde_json::json!("desktop-mode");
        value["createdAt"] = serde_json::json!(123);
        let dto: GroupSyncDTO =
            serde_json::from_value(value.clone()).expect("preserve desktop values");
        assert_eq!(dto.mode, "desktop-mode");
        assert_eq!(dto.created_at, 123);
        for field in ["mode", "createdAt"] {
            let mut invalid = value.clone();
            invalid[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<GroupSyncDTO>(invalid).is_err());
        }
    }

    #[test]
    fn message_and_attachment_compatibility_fields_may_be_omitted_on_pull() {
        let dto: crate::vcp_modules::sync_dto::MessageSyncDTO = serde_json::from_value(
            serde_json::json!({
                "id": "message", "role": "user", "content": "body",
                "timestamp": 100, "updatedAt": 100, "contentHash": "a".repeat(64),
                "attachments": [{"type": "text/plain", "name": "file.txt", "size": 1, "hash": "b".repeat(64)}]
            }),
        ).expect("receive without desktop compatibility metadata");
        assert!(dto.group_id.is_none());
        assert!(dto.topic_id.is_none());
        assert!(dto.is_group_message.is_none());
        let attachment = &dto.attachments.as_ref().unwrap()[0];
        assert!(attachment.image_frames.is_none());
        assert!(attachment.created_at.is_none());
    }

    #[test]
    fn entity_pull_keeps_identity_and_known_value_validation() {
        for (item, _) in entity_pull_cases() {
            for field in ["entityType", "ownerType", "ownerId", "ok"] {
                let mut invalid = item.clone();
                invalid.as_object_mut().unwrap().remove(field);
                assert!(
                    serde_json::from_value::<EntityPullResult>(invalid).is_err(),
                    "{field}"
                );
            }
            for field in ["entityType", "ownerType"] {
                let mut invalid = item.clone();
                invalid[field] = serde_json::json!("future-kind");
                assert!(
                    serde_json::from_value::<EntityPullResult>(invalid).is_err(),
                    "{field}"
                );
            }
            let mut invalid = item;
            invalid["data"]["name"] = serde_json::json!(123);
            assert!(serde_json::from_value::<EntityPullResult>(invalid).is_err());
        }
        for (index, field, value) in [
            (1, "memberTags", serde_json::json!({"": "tag"})),
            (1, "memberTags", serde_json::json!({"agent-1": ["tag"]})),
            (2, "configHash", serde_json::json!("not-sha256")),
            (3, "updatedAt", serde_json::json!(-1)),
            (3, "updatedAt", serde_json::json!(1_i64 << 53)),
        ] {
            let mut invalid = entity_pull_cases()[index].0.clone();
            invalid["data"][field] = value;
            assert!(
                serde_json::from_value::<EntityPullResult>(invalid).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn canonical_json_escapes_dynamic_keys_and_is_injective_for_member_tags() {
        let injected = serde_json::json!({
            "memberTags": { "a\":\"x\",\"b": "y" }
        });
        let ordinary = serde_json::json!({
            "memberTags": { "a": "x", "b": "y" }
        });

        assert_eq!(
            stable_stringify(&injected),
            r#"{"memberTags":{"a\":\"x\",\"b":"y"}}"#
        );
        assert_ne!(stable_stringify(&injected), stable_stringify(&ordinary));
        assert_ne!(
            compute_deterministic_hash(&injected),
            compute_deterministic_hash(&ordinary)
        );
    }

    #[test]
    fn canonical_hash_matches_the_shared_cross_language_vectors() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/message_canonical_contract.json"))
                .expect("parse canonical hash fixture");
        for case in fixture["canonicalHashCases"]
            .as_array()
            .expect("canonical hash cases")
        {
            let value = &case["value"];
            assert_eq!(
                stable_stringify(value),
                case["expectedCanonical"].as_str().expect("canonical bytes"),
                "case {}",
                case["name"].as_str().unwrap_or("unnamed")
            );
            assert_eq!(
                compute_deterministic_hash(value),
                case["expectedHash"].as_str().expect("canonical hash"),
                "case {}",
                case["name"].as_str().unwrap_or("unnamed")
            );
        }
    }
}
