use crate::vcp_modules::topic_types::TopicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    match owner_type {
        "agent" | "group" => !owner_id.is_empty(),
        "user" => owner_id == "user_avatar",
        _ => false,
    }
}

pub fn parse_topic_key(value: &serde_json::Value, field: &str) -> Result<TopicKey, String> {
    let key: TopicKey = serde_json::from_value(value.clone())
        .map_err(|error| format!("{field} requires ownerType, ownerId and topicId: {error}"))?;
    if !key.is_valid() {
        return Err(format!("{field} contains an invalid topic identity"));
    }
    Ok(key)
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

/// 同步数据的实体类型
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncDataType {
    Agent,
    Group,
    Avatar,
    Topic,
    Message,
}

impl fmt::Display for SyncDataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncDataType::Agent => write!(f, "agent"),
            SyncDataType::Group => write!(f, "group"),
            SyncDataType::Avatar => write!(f, "avatar"),
            SyncDataType::Topic => write!(f, "topic"),
            SyncDataType::Message => write!(f, "message"),
        }
    }
}

/// 核心状态向量 (State Vector / Fingerprint)
/// 极简设计，只包含标识、内容指纹和绝对时间戳，用于阶段一的指纹广播
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntityState {
    /// 实体的唯一标识 (agent_id, group_id, 或 avatar 对应的 owner_id)
    pub id: String,
    /// 状态指纹 (SHA-256 Hash，代表内容的本质)
    /// 在 V2 协议中，Agent/Group 优先使用 config_hash 和 content_hash
    pub hash: String,
    /// 配置内容指纹 (V2 优化)
    #[serde(rename = "configHash", skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
    /// 内容聚合指纹 (V2 优化，代表旗下话题/消息是否有变动)
    #[serde(rename = "contentHash", skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// 绝对时间戳 / 逻辑时钟 (LWW 裁决标准)
    pub ts: i64,
    /// 软删除时间戳 (可选，用于双向删除同步)
    #[serde(rename = "deletedAt", skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    /// 所有者类型 (仅用于 topic 类型，区分 agent_topic 和 group_topic)
    #[serde(rename = "ownerType", skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,
    /// 所有者 ID（仅用于 topic，和 ownerType 共同构成无歧义身份）
    #[serde(rename = "ownerId", skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}

/// 阶段一：同步清单 (Manifest)
/// 手机端发送给电脑端，或者电脑端发送给手机端的全量/增量清单
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncManifest {
    pub data_type: SyncDataType,
    pub items: Vec<EntityState>,
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
    fn test_sync_data_type_display_and_serde_are_lowercase() {
        assert_eq!(SyncDataType::Agent.to_string(), "agent");
        assert_eq!(SyncDataType::Group.to_string(), "group");
        assert_eq!(SyncDataType::Avatar.to_string(), "avatar");
        assert_eq!(SyncDataType::Topic.to_string(), "topic");
        assert_eq!(SyncDataType::Message.to_string(), "message");

        let encoded = serde_json::to_string(&SyncDataType::Message).unwrap();
        assert_eq!(encoded, r#""message""#);
        let decoded: SyncDataType = serde_json::from_str(r#""topic""#).unwrap();
        assert_eq!(decoded, SyncDataType::Topic);
    }
}
