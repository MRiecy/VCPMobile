use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    #[serde(alias = "senderName")]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isThinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,

    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    #[serde(rename = "isGroupMessage", skip_serializing_if = "Option::is_none")]
    pub is_group_message: Option<bool>,
    #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,

    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "internalPath", default)]
    pub internal_path: String,
    #[serde(rename = "extractedText", skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(rename = "imageFrames", skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

fn process_dir(dir: &Path, errors: &mut Vec<String>) {
    if dir.is_dir() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    process_dir(&path, errors);
                } else if path.file_name().unwrap_or_default() == "history.json" {
                    let content = fs::read_to_string(&path).unwrap_or_default();
                    if let Ok(json_arr) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                        for (i, msg_val) in json_arr.into_iter().enumerate() {
                            let mut msg_val_mut = msg_val.clone();
                            // simulate PullExecutor manipulation
                            if let Some(obj) = msg_val_mut.as_object_mut() {
                                if let Some(attachments) =
                                    obj.get_mut("attachments").and_then(|a| a.as_array_mut())
                                {
                                    for att in attachments {
                                        if let Some(att_obj) = att.as_object_mut() {
                                            // simulate the JS DTO transformation since the file on disk doesn't have it flattened yet
                                            // The JS side does:
                                            // type: att.type, name: att.name, size: att.size, hash: att._fileManagerData.hash
                                            // So let's flatten it here to match what Rust receives.
                                            if let Some(data) =
                                                att_obj.get("_fileManagerData").cloned()
                                            {
                                                if let Some(data_obj) = data.as_object() {
                                                    if let Some(hash) = data_obj.get("hash") {
                                                        att_obj.insert(
                                                            "hash".to_string(),
                                                            hash.clone(),
                                                        );
                                                    }
                                                    if let Some(extracted) =
                                                        data_obj.get("extractedText")
                                                    {
                                                        att_obj.insert(
                                                            "extractedText".to_string(),
                                                            extracted.clone(),
                                                        );
                                                    }
                                                    if let Some(frames) =
                                                        data_obj.get("imageFrames")
                                                    {
                                                        att_obj.insert(
                                                            "imageFrames".to_string(),
                                                            frames.clone(),
                                                        );
                                                    }
                                                    if let Some(created) = data_obj.get("createdAt")
                                                    {
                                                        att_obj.insert(
                                                            "createdAt".to_string(),
                                                            created.clone(),
                                                        );
                                                    }
                                                }
                                            }
                                            att_obj.remove("_fileManagerData");
                                        }
                                    }
                                }
                                obj.remove("avatarUrl");
                                obj.remove("avatarColor");
                            }

                            match serde_json::from_value::<ChatMessage>(msg_val_mut.clone()) {
                                Ok(_) => {}
                                Err(e) => {
                                    errors.push(format!(
                                        "File: {}, Msg Index: {}, Error: {}",
                                        path.display(),
                                        i,
                                        e
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let mut errors = Vec::new();
    process_dir(Path::new("G:\\VCPChat\\AppData\\UserData"), &mut errors);
    if errors.is_empty() {
        println!("All messages parsed successfully!");
    } else {
        println!("Found {} errors.", errors.len());
        for e in errors.iter().take(10) {
            println!("{}", e);
        }
    }
}
