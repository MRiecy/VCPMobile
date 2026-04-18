use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

pub struct PullExecutor;

impl PullExecutor {
    pub async fn pull_agent<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent",
            http_url, agent_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull agent failed: {}", res.status()));
        }

        let dto: AgentSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::UpsertAgent {
                id: agent_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_group<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group",
            http_url, group_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull group failed: {}", res.status()));
        }

        let dto: GroupSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::UpsertGroup {
                id: group_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_avatar<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-avatar?id={}",
            http_url, owner_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull avatar failed: {}", res.status()));
        }

        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::UpsertAvatar {
                owner_type: owner_type.to_string(),
                owner_id: owner_id.to_string(),
                bytes: bytes.to_vec(),
            })
            .await;

        Ok(())
    }

    pub async fn pull_agent_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=agent_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            // Topic not found on desktop, skip silently
            return Ok(());
        }

        if !res.status().is_success() {
            return Err(format!("Pull agent_topic failed: {}", res.status()));
        }

        let dto: AgentTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::UpsertAgentTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_group_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/mobile-sync/download-entity?id={}&type=group_topic",
            http_url, topic_id
        );
        let res = client
            .get(&url)
            .header("x-sync-token", sync_token)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if res.status() == reqwest::StatusCode::NOT_FOUND {
            // Topic not found on desktop, skip silently
            return Ok(());
        }

        if !res.status().is_success() {
            return Err(format!("Pull group_topic failed: {}", res.status()));
        }

        let dto: GroupTopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        write_queue
            .submit(DbWriteTask::UpsertGroupTopic {
                topic_id: topic_id.to_string(),
                dto,
            })
            .await;

        Ok(())
    }

    pub async fn pull_messages(
        app: &AppHandle,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        msg_ids: &[String],
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();

        // 尝试获取 topic 信息，如果不存在则使用默认值
        // 消息数据会在 topic 后续同步时被正确关联
        let topic_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let (owner_id, owner_type) = match topic_row {
            Some(r) => (r.get("owner_id"), r.get("owner_type")),
            None => {
                // Topic 还未同步，使用占位值，后续 topic 同步时会更新
                println!("[PullExecutor] Topic {} not yet available, messages will be linked later", topic_id);
                ("pending_owner".to_string(), "agent".to_string())
            }
        };

        let url = format!("{}/api/mobile-sync/download-messages", http_url);
        let res = client
            .post(&url)
            .header("x-sync-token", sync_token)
            .json(&serde_json::json!({ "topicId": topic_id, "msgIds": msg_ids }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Pull messages failed: {}", res.status()));
        }

        let messages: Vec<serde_json::Value> = res.json().await.map_err(|e| e.to_string())?;
        let mut parsed_messages = Vec::new();

        for mut m_val in messages {
            if let Some(obj) = m_val.as_object_mut() {
                if let Some(attachments) = obj.get_mut("attachments").and_then(|a| a.as_array_mut())
                {
                    for att in attachments {
                        if let Some(att_obj) = att.as_object_mut() {
                            if let Some(hash) = att_obj.get("hash").and_then(|h| h.as_str()) {
                                if !hash.is_empty() {
                                    let existing_path: Option<String> = sqlx::query_scalar(
                                        "SELECT internal_path FROM attachments WHERE hash = ?",
                                    )
                                    .bind(hash)
                                    .fetch_optional(&db.pool)
                                    .await
                                    .ok()
                                    .flatten();

                                    if let Some(path) = existing_path {
                                        att_obj
                                            .entry("internalPath".to_string())
                                            .or_insert(serde_json::json!(path));
                                        att_obj.entry("src".to_string()).or_insert(
                                            serde_json::json!(format!("file://{}", path)),
                                        );
                                    } else {
                                        let default_path = format!("file://attachments/{}", hash);
                                        att_obj.entry("internalPath".to_string()).or_insert(
                                            serde_json::json!(
                                                default_path.trim_start_matches("file://")
                                            ),
                                        );
                                        att_obj
                                            .entry("src".to_string())
                                            .or_insert(serde_json::json!(default_path));
                                    }
                                }
                            }
                            att_obj
                                .entry("status".to_string())
                                .or_insert(serde_json::json!("ready"));
                        }
                    }
                }
                obj.remove("avatarUrl");
                obj.remove("avatarColor");
            }

            if let Ok(msg) =
                serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(m_val)
            {
                parsed_messages.push(msg);
            }
        }

        write_queue
            .submit(DbWriteTask::UpsertMessages {
                topic_id: topic_id.to_string(),
                owner_id,
                owner_type,
                messages: parsed_messages,
            })
            .await;

        Ok(())
    }
}
