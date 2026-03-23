use std::fs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isThinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

fn main() {
    let base_path = "G:/VCPChat/AppData/UserData";
    let entries = fs::read_dir(base_path).unwrap();
    let mut success_count = 0;
    let mut fail_count = 0;

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            let topics_dir = path.join("topics");
            if topics_dir.exists() {
                for topic_entry in fs::read_dir(topics_dir).unwrap() {
                    let topic_entry = topic_entry.unwrap();
                    let history_path = topic_entry.path().join("history.json");
                    if history_path.exists() {
                        let content = fs::read_to_string(&history_path).unwrap();
                        let res: Result<Vec<ChatMessage>, _> = serde_json::from_str(&content);
                        if res.is_err() {
                            println!("FAILED: {:?} - Error: {}", history_path, res.err().unwrap());
                            fail_count += 1;
                        } else {
                            success_count += 1;
                        }
                    }
                }
            }
        }
    }
    println!("Done. Success: {}, Failed: {}", success_count, fail_count);
}
