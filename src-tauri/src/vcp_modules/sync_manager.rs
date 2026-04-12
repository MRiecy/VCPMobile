use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::{EntityState, SyncDataType, SyncManifest};
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO, TopicSyncDTO};
use crate::vcp_modules::agent_service::{self, AgentConfigState};
use crate::vcp_modules::group_service::{self, GroupManagerState};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::Digest;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{mpsc, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use std::path::Path;

/// =================================================================
/// vcp_modules/sync_manager.rs - 手机端同步调度中心 (Pointer-Based)
/// =================================================================

pub struct SyncState {
    pub ws_sender: mpsc::UnboundedSender<SyncCommand>,
}

pub enum SyncCommand {
    NotifyLocalChange { id: String, data_type: SyncDataType, hash: String, ts: i64 },
    StartFullSync,
}

pub fn init_sync_manager(app_handle: AppHandle) -> mpsc::UnboundedSender<SyncCommand> {
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
    let handle_clone = app_handle.clone();

    tauri::async_runtime::spawn(async move {
        let http_client = reqwest::Client::new();
        let semaphore = Arc::new(Semaphore::new(10)); 
        
        loop {
            let (ws_url, base_url) = {
                let settings_state = handle_clone.state::<crate::vcp_modules::settings_manager::SettingsState>();
                match crate::vcp_modules::settings_manager::read_settings(handle_clone.clone(), settings_state).await {
                    Ok(s) => {
                        if s.sync_server_url.is_empty() {
                            tokio::time::sleep(Duration::from_secs(10)).await;
                            continue;
                        }
                        let ws_addr = if let Ok(mut u) = url::Url::parse(&s.sync_server_url) {
                            let scheme = if u.scheme() == "https" { "wss" } else { "ws" };
                            u.set_scheme(scheme).ok();
                            u.set_query(Some(&format!("token={}", s.sync_token)));
                            u.to_string()
                        } else {
                            format!("ws://127.0.0.1:5974?token={}", s.sync_token)
                        };
                        (ws_addr, s.sync_server_url.clone())
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            };

            match connect_async(&ws_url).await {
                Ok((mut ws_stream, _)) => {
                    println!("[SyncManager] WebSocket Connected.");
                    
                    if let Ok(manifests) = generate_initial_manifests(&handle_clone).await {
                        for manifest in manifests {
                            let msg = json!({
                                "type": "SYNC_MANIFEST",
                                "data": manifest.items,
                                "dataType": manifest.data_type
                            });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    if let Ok(topic_ids) = get_all_active_topic_ids(&handle_clone).await {
                        for tid in topic_ids {
                            let msg = json!({ "type": "GET_HISTORY_MANIFEST", "topicId": tid });
                            let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                        }
                    }

                    loop {
                        tokio::select! {
                            Some(cmd) = rx.recv() => {
                                match cmd {
                                    SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
                                        let msg = json!({ "type": "SYNC_ENTITY_UPDATE", "id": id, "dataType": data_type, "hash": hash, "ts": ts });
                                        let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                    },
                                    SyncCommand::StartFullSync => {
                                        if let Ok(manifests) = generate_initial_manifests(&handle_clone).await {
                                            for manifest in manifests {
                                                let _ = ws_stream.send(Message::Text(json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type}).to_string().into())).await;
                                            }
                                        }
                                    }
                                }
                            }
                            Some(Ok(msg)) = ws_stream.next() => {
                                if let Message::Text(text) = msg {
                                    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                                    let h = handle_clone.clone();
                                    let c = http_client.clone();
                                    let base = base_url.clone();
                                    let sem = semaphore.clone();
                                    
                                    match payload["type"].as_str() {
                                        Some("SYNC_DIFF_RESULTS") => {
                                            if let Some(items) = payload["data"].as_array() {
                                                let entity_type = payload["dataType"].as_str().unwrap_or("agent").to_string();
                                                for item in items {
                                                    let id = item["id"].as_str().unwrap_or_default().to_string();
                                                    let action = item["action"].as_str().unwrap_or_default();
                                                    let h_inner = h.clone(); let c_inner = c.clone(); let base_inner = base.clone(); let sem_inner = sem.clone(); let et_inner = entity_type.clone();
                                                    
                                                    if action == "PULL" {
                                                        tauri::async_runtime::spawn(async move {
                                                            let _permit = sem_inner.acquire().await;
                                                            let _ = perform_pull(&h_inner, &c_inner, &base_inner, &id, &et_inner).await;
                                                        });
                                                    } else if action == "PUSH" {
                                                        tauri::async_runtime::spawn(async move {
                                                            let _permit = sem_inner.acquire().await;
                                                            let _ = perform_push(&h_inner, &c_inner, &base_inner, &id, &et_inner).await;
                                                        });
                                                    }
                                                }
                                            }
                                        },
                                        Some("HISTORY_MANIFEST_RESULTS") => {
                                            let topic_id = payload["topicId"].as_str().unwrap_or_default().to_string();
                                            if let Some(remote_msgs) = payload["messages"].as_array() {
                                                let h_inner = h.clone(); let c_inner = c.clone(); let base_inner = base.clone(); let sem_inner = sem.clone(); let msgs_vec = remote_msgs.clone();
                                                tauri::async_runtime::spawn(async move {
                                                    let _permit = sem_inner.acquire().await;
                                                    let _ = perform_history_delta_sync(&h_inner, &c_inner, &base_inner, &topic_id, &msgs_vec).await;
                                                });
                                            }
                                        },
                                        Some("SYNC_ENTITY_UPDATE") => {
                                            let id = payload["id"].as_str().unwrap_or_default().to_string();
                                            let entity_type = payload["dataType"].as_str().unwrap_or("agent").to_string();
                                            if entity_type == "history" {
                                                let msg = json!({ "type": "GET_HISTORY_MANIFEST", "topicId": id });
                                                let _ = ws_stream.send(Message::Text(msg.to_string().into())).await;
                                            } else {
                                                let h_inner = h.clone(); let c_inner = c.clone(); let base_inner = base.clone(); let sem_inner = sem.clone();
                                                tauri::async_runtime::spawn(async move {
                                                    let _permit = sem_inner.acquire().await;
                                                    let _ = perform_pull(&h_inner, &c_inner, &base_inner, &id, &entity_type).await;
                                                });
                                            }
                                        },
                                        _ => {}
                                    }
                                }
                            }
                            else => break,
                        }
                    }
                }
                Err(e) => {
                    println!("[SyncManager] Connection failed: {}. Retrying in 5s...", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });

    tx
}

fn stable_stringify(value: &Value) -> String {
    crate::vcp_modules::sync_types::stable_stringify(value)
}

fn compute_message_fingerprint(content: &str, attachment_hashes: Vec<String>) -> String {
    let mut sorted_hashes = attachment_hashes;
    sorted_hashes.sort();
    let fp_obj = json!({ "content": content, "attachmentHashes": sorted_hashes });
    let mut hasher = md5::Context::new();
    hasher.consume(stable_stringify(&fp_obj).as_bytes());
    format!("{:x}", hasher.compute())
}

async fn generate_initial_manifests(app: &AppHandle) -> Result<Vec<SyncManifest>, String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let mut manifests = Vec::new();

    let mut agent_items = Vec::new();
    let rows = sqlx::query("SELECT agent_id, config_hash, updated_at FROM agents WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        agent_items.push(EntityState { id: r.get("agent_id"), hash: r.get("config_hash"), ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Agent, items: agent_items });

    let mut group_items = Vec::new();
    let rows = sqlx::query("SELECT group_id, config_hash, updated_at FROM groups WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        group_items.push(EntityState { id: r.get("group_id"), hash: r.get("config_hash"), ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Group, items: group_items });

    let mut avatar_items = Vec::new();
    let rows = sqlx::query("SELECT owner_id, owner_type, hash, updated_at FROM avatars")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        let owner_id: String = r.get("owner_id");
        let owner_type: String = r.get("owner_type");
        avatar_items.push(EntityState { 
            id: format!("{}:{}", owner_type, owner_id), 
            hash: r.get("hash"), 
            ts: r.get("updated_at") 
        });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::Avatar, items: avatar_items });

    let mut topic_items = Vec::new();
    let rows = sqlx::query("SELECT topic_id, title, created_at, locked, unread, updated_at, owner_id, owner_type FROM topics WHERE deleted_at IS NULL").fetch_all(pool).await.map_err(|e| e.to_string())?;
    for r in rows {
        use sqlx::Row;
        let id: String = r.get("topic_id");
        let name: String = r.get("title");
        let owner_id: String = r.get("owner_id");
        let owner_type: String = r.get("owner_type");
        let created_at: i64 = r.get("created_at");
        let locked: bool = r.get::<i64, _>("locked") != 0;
        let unread: bool = r.get::<i64, _>("unread") != 0;

        let dto = TopicSyncDTO {
            id: id.clone(),
            name,
            created_at,
            locked: if owner_type == "group" { true } else { locked },
            unread: if owner_type == "group" { false } else { unread },
            owner_id,
            owner_type,
        };
        
        let mut hasher = sha2::Sha256::new();
        sha2::Digest::update(&mut hasher, stable_stringify(&serde_json::to_value(&dto).unwrap_or(Value::Null)).as_bytes());
        topic_items.push(EntityState { id, hash: format!("{:x}", sha2::Digest::finalize(hasher)), ts: r.get("updated_at") });
    }
    manifests.push(SyncManifest { data_type: SyncDataType::History, items: topic_items });

    Ok(manifests)
}

async fn get_all_active_topic_ids(app: &AppHandle) -> Result<Vec<String>, String> {
    let db_state = app.state::<DbState>();
    let rows = sqlx::query("SELECT topic_id FROM topics WHERE deleted_at IS NULL").fetch_all(&db_state.pool).await.map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(|r| { use sqlx::Row; r.get(0) }).collect())
}

async fn perform_pull<R: Runtime>(app: &AppHandle<R>, client: &reqwest::Client, base_url: &str, id: &str, entity_type: &str) -> Result<(), String> {
    let settings_state = app.state::<crate::vcp_modules::settings_manager::SettingsState>();
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), settings_state).await?;
    let url = format!("{}/api/mobile-sync/download-entity?id={}&type={}", base_url, id, entity_type);
    let res = client.get(&url).header("x-sync-token", &settings.sync_token).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() { return Err(format!("Pull failed: {}", res.status())); }

    if entity_type == "agent" {
        let dto: AgentSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        agent_service::apply_sync_update(app, &app.state::<AgentConfigState>(), id, dto).await?;
    } else if entity_type == "group" {
        let dto: GroupSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        group_service::apply_sync_update(app, &app.state::<GroupManagerState>(), id, dto).await?;
    } else if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            let owner_type = parts[0];
            let owner_id = parts[1];
            let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
            let url = format!("{}/api/mobile-sync/download-avatar?id={}", base_url, owner_id);
            let resp = client.get(&url).header("x-sync-token", &settings.sync_token).send().await.map_err(|e| e.to_string())?;
            if resp.status().is_success() {
                let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                let app_data = app.path().app_data_dir().unwrap();
                let avatar_dir = app_data.join("avatars");
                if !avatar_dir.exists() { std::fs::create_dir_all(&avatar_dir).ok(); }
                let ext = "png"; // 简单起见统一 png
                let local_path = avatar_dir.join(format!("{}_{}.{}", owner_type, owner_id, ext));
                std::fs::write(&local_path, &bytes).ok();
                
                // 更新 DB
                let mut hasher = sha2::Sha256::new();
                sha2::Digest::update(&mut hasher, &bytes);
                let hash = format!("{:x}", sha2::Digest::finalize(hasher));
                let now = chrono::Utc::now().timestamp_millis();
                let db_state = app.state::<DbState>();
                sqlx::query("INSERT INTO avatars (owner_id, owner_type, file_path, hash, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(owner_id, owner_type) DO UPDATE SET file_path=excluded.file_path, hash=excluded.hash, updated_at=excluded.updated_at")
                    .bind(owner_id).bind(owner_type).bind(local_path.to_string_lossy().to_string()).bind(hash).bind(now).execute(&db_state.pool).await.ok();
            }
        }
    } else if entity_type == "topic" || entity_type == "history" {
        let dto: TopicSyncDTO = res.json().await.map_err(|e| e.to_string())?;
        let db_state = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(topic_id) DO UPDATE SET title = excluded.title, locked = excluded.locked, unread = excluded.unread, updated_at = excluded.updated_at")
            .bind(id).bind(&dto.name).bind(&dto.owner_id).bind(&dto.owner_type).bind(dto.created_at)
            .bind(if dto.locked { 1 } else { 0 }).bind(if dto.unread { 1 } else { 0 }).bind(now)
            .execute(&db_state.pool).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn perform_push(app: &AppHandle, client: &reqwest::Client, base_url: &str, id: &str, entity_type: &str) -> Result<(), String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;

    let payload = if entity_type == "topic" || entity_type == "history" {
        let row = sqlx::query("SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(id).fetch_one(pool).await.map_err(|e| e.to_string())?;
        use sqlx::Row;
        
        let owner_type: String = row.get("owner_type");
        let dto = TopicSyncDTO {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: if owner_type == "group" { true } else { row.get::<i64, _>("locked") != 0 },
            unread: if owner_type == "group" { false } else { row.get::<i64, _>("unread") != 0 },
            owner_id: row.get("owner_id"),
            owner_type,
        };
        json!({ "id": id, "type": "topic", "data": dto })
    } else if entity_type == "agent" {
        let state = app.state::<AgentConfigState>();
        let config = agent_service::read_agent_config(app.clone(), state.clone(), id.to_string(), None).await?;
        json!({ "id": id, "type": "agent", "data": AgentSyncDTO::from(&config) })
    } else if entity_type == "group" {
        let state = app.state::<GroupManagerState>();
        let config = group_service::read_group_config(app.clone(), state.clone(), id.to_string()).await?;
        json!({ "id": id, "type": "group", "data": GroupSyncDTO::from(&config) })
    } else if entity_type == "avatar" {
        let parts: Vec<&str> = id.split(':').collect();
        if parts.len() == 2 {
            let owner_type = parts[0];
            let owner_id = parts[1];
            let row = sqlx::query("SELECT file_path FROM avatars WHERE owner_id = ? AND owner_type = ?")
                .bind(owner_id).bind(owner_type).fetch_one(pool).await.map_err(|e| e.to_string())?;
            use sqlx::Row;
            let file_path: String = row.get("file_path");
            if let Ok(_bytes) = std::fs::read(&file_path) {
                // 这里我们其实不通过 upload-entity 推送二进制，而是利用 upload-avatar (如果未来有的话)
                // 或者直接通过现有的逻辑触发。简单起见，推送 metadata，让对方主动来 download
                json!({ "id": id, "type": "avatar", "data": {} }) 
            } else { return Ok(()); }
        } else { return Ok(()); }
    } else { return Ok(()); };

    client.post(&format!("{}/api/mobile-sync/upload-entity", base_url)).header("x-sync-token", &settings.sync_token).json(&payload).send().await.map_err(|e| e.to_string())?;
    if entity_type == "history" { perform_history_push(app, client, base_url, id).await?; }
    println!("[SyncManager] Pushed: {} ({})", id, entity_type);
    Ok(())
}

async fn perform_history_push(app: &AppHandle, client: &reqwest::Client, base_url: &str, topic_id: &str) -> Result<(), String> {
    let pool = &app.state::<DbState>().pool;
    let r = sqlx::query("SELECT owner_id FROM topics WHERE topic_id = ?").bind(topic_id).fetch_one(pool).await.map_err(|e| e.to_string())?;
    let owner_id: String = { use sqlx::Row; r.get("owner_id") };
    let history = crate::vcp_modules::message_service::load_chat_history_internal(app, &owner_id, "agent", topic_id, Some(1000), None).await?;
    let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
    let app_data = app.path().app_data_dir().unwrap();

    for msg in &history {
        if let Some(atts) = &msg.attachments {
            for att in atts {
                if let Some(hash) = &att.hash {
                    let ext = Path::new(&att.name).extension().and_then(|s| s.to_str()).unwrap_or("bin");
                    let local_path = app_data.join("attachments").join(format!("{}.{}", hash, ext));
                    if local_path.exists() {
                        if let Ok(bytes) = std::fs::read(&local_path) {
                            let _ = client.post(&format!("{}/api/mobile-sync/upload-attachment?hash={}&name={}", base_url, hash, urlencoding::encode(&att.name))).header("x-sync-token", &settings.sync_token).body(bytes).send().await;
                        }
                    }
                }
            }
        }
    }

    let dto_history: Vec<crate::vcp_modules::sync_dto::MessageSyncDTO> = history.iter().map(crate::vcp_modules::sync_dto::MessageSyncDTO::from).collect();
    client.post(&format!("{}/api/mobile-sync/upload-messages", base_url))
        .header("x-sync-token", &settings.sync_token)
        .json(&json!({ "topicId": topic_id, "messages": dto_history }))
        .send().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn perform_history_delta_sync(app: &AppHandle, client: &reqwest::Client, base_url: &str, topic_id: &str, remote_msgs: &Vec<Value>) -> Result<(), String> {
    let db_state = app.state::<DbState>();
    let pool = &db_state.pool;
    let rows = sqlx::query("SELECT m.msg_id, m.content, m.updated_at, a.hash as att_hash FROM messages m LEFT JOIN message_attachments ma ON m.msg_id = ma.msg_id LEFT JOIN attachments a ON ma.hash = a.hash WHERE m.topic_id = ? AND m.deleted_at IS NULL").bind(topic_id).fetch_all(pool).await.map_err(|e| e.to_string())?;
    let mut local_map = std::collections::HashMap::new();
    for r in rows {
        use sqlx::Row;
        let id: String = r.get("msg_id");
        let entry = local_map.entry(id).or_insert((r.get::<String, _>("content"), r.get::<i64, _>("updated_at"), Vec::new()));
        if let Some(h) = r.get::<Option<String>, _>("att_hash") { entry.2.push(h); }
    }

    let mut to_pull_ids = Vec::new();
    let mut to_push = false;
    let mut remote_ids = std::collections::HashSet::new();

    for rm in remote_msgs {
        let rid = rm["msg_id"].as_str().unwrap_or_default().to_string();
        remote_ids.insert(rid.clone());
        let rhash = rm["content_hash"].as_str().unwrap_or_default();
        let rts = rm["updated_at"].as_i64().unwrap_or(0);
        if let Some((lcontent, lts, latts)) = local_map.get(&rid) {
            if compute_message_fingerprint(lcontent, latts.clone()) != rhash {
                if rts > *lts { to_pull_ids.push(rid); }
                else { to_push = true; }
            }
        } else { to_pull_ids.push(rid); }
    }

    // Check if mobile has messages that desktop doesn't have
    for lid in local_map.keys() {
        if !remote_ids.contains(lid) {
            to_push = true;
            break;
        }
    }

    if to_push { let _ = perform_history_push(app, client, base_url, topic_id).await; }
    if !to_pull_ids.is_empty() {
        let agent_id_row = sqlx::query("SELECT owner_id FROM topics WHERE topic_id = ?").bind(topic_id).fetch_one(pool).await.map_err(|e| e.to_string())?;
        let agent_id: String = { use sqlx::Row; agent_id_row.get("owner_id") };
        let settings = crate::vcp_modules::settings_manager::read_settings(app.clone(), app.state()).await?;
        let res = client.post(&format!("{}/api/mobile-sync/download-messages", base_url)).header("x-sync-token", &settings.sync_token).json(&json!({ "topicId": topic_id, "msgIds": to_pull_ids })).send().await.map_err(|e| e.to_string())?;
        if res.status().is_success() {
            let messages: Vec<Value> = res.json().await.map_err(|e| e.to_string())?;
            for m_val in messages {
                if let Ok(msg) = serde_json::from_value::<crate::vcp_modules::chat_manager::ChatMessage>(m_val) {
                    crate::vcp_modules::message_service::patch_single_message(app.clone(), pool, &agent_id, "agent", topic_id.to_string(), msg).await?;
                }
            }
            let msg_count: i32 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE topic_id = ? AND deleted_at IS NULL").bind(topic_id).fetch_one(pool).await.map_err(|e| e.to_string())?;
            sqlx::query("UPDATE topics SET msg_count = ? WHERE topic_id = ?").bind(msg_count).bind(topic_id).execute(pool).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
