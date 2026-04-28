use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// =================================================================
/// vcp_modules/sync_types.rs - 分布式 LWW+Hash 同步协议的核心数据结构
/// =================================================================
/// 计算 JSON 的确定性 SHA-256 Hash
pub fn compute_deterministic_hash<T: Serialize>(data: &T) -> String {
    if let Ok(val) = serde_json::to_value(data) {
        let json_str = stable_stringify(&val);
        let mut hasher = Sha256::new();
        hasher.update(json_str.as_bytes());
        format!("{:x}", hasher.finalize())
    } else {
        "".to_string()
    }
}

/// 计算一组哈希的聚合哈希 (Merkle Root)
/// 规则：将所有哈希按 ID 字典序排列后，连接并计算总 Hash
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
    pub hash: String,
    /// 绝对时间戳 / 逻辑时钟 (LWW 裁决标准)
    pub ts: i64,
    /// 软删除时间戳 (可选，用于双向删除同步)
    #[serde(rename = "deletedAt", skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    /// 所有者类型 (仅用于 topic 类型，区分 agent_topic 和 group_topic)
    #[serde(rename = "ownerType", skip_serializing_if = "Option::is_none")]
    pub owner_type: Option<String>,
}

/// 阶段一：同步清单 (Manifest)
/// 手机端发送给电脑端，或者电脑端发送给手机端的全量/增量清单
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncManifest {
    pub data_type: SyncDataType,
    pub items: Vec<EntityState>,
}
