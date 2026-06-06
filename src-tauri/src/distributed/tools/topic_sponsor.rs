// distributed/tools/topic_sponsor.rs
// [OneShot] TopicSponsor — lets distributed agents create, inspect, and reply to local topics.

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::Row;
use tauri::{AppHandle, Manager};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::{InvocationCommand, ToolManifest};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_repository::{
    ContentCompressor, MessageRenderCompiler, MessageRepository,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::SyncDataType;

pub struct TopicSponsorTool;

#[derive(Debug, Clone)]
struct AgentInfo {
    id: String,
    name: String,
}

#[async_trait]
impl OneShotTool for TopicSponsorTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "TopicSponsor".to_string(),
            display_name: "AI主动创建话题".to_string(),
            description: "允许 Agent 在 VCPMobile 本机创建、查询和回复话题。".to_string(),
            placeholder: None,
            invocation_commands: vec![InvocationCommand {
                command_identifier: "TopicSponsor".to_string(),
                description: "在 VCPMobile 本机话题库中执行话题操作。\n\
参数:\n\
- command (字符串, 必需): CreateTopic, ReadUnlockedTopics, CheckNewTopics, CheckUnreadMessages, ReplyToTopic, CheckTopicOwnership, ListUnlockedTopics, ReadTopicContent\n\
- maid (字符串, 必需): 目标或发起请求的智能体名称\n\
- topic_name (字符串, CreateTopic 必需): 新话题名称\n\
- initial_message (字符串, CreateTopic 必需): 第一条 assistant 消息\n\
- topic_id/topicId/TopicId (字符串): 指定话题 ID\n\
- message (字符串, ReplyToTopic 必需): 回复内容\n\
- sender_name (字符串, ReplyToTopic 必需): 回复者名称\n\
- caller_name (字符串, CheckTopicOwnership 必需): 调用者名称\n\
- include_read (布尔, ReadUnlockedTopics 可选): 是否包含已读话题\n\
- days (整数, CheckNewTopics 可选): 检查最近几天\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」TopicSponsor「末」\n\
command:「始」CreateTopic「末」\n\
maid:「始」HANNA「末」\n\
topic_name:「始」一个新想法「末」\n\
initial_message:「始」主人，我突然想到，我们可以一起写一个故事！「末」\n\
<<<[END_TOOL_REQUEST]>>>"
                    .to_string(),
                example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」TopicSponsor「末」\ncommand:「始」ReplyToTopic「末」\nmaid:「始」HANNA「末」\ntopic_id:「始」topic_1234567890「末」\nmessage:「始」这是一条回复消息「末」\nsender_name:「始」HANNA「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
            }],
            web_socket_push: None,
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let command = get_string_arg(&args, "command")
            .ok_or_else(|| "请求中缺少 'command' 参数。".to_string())?;
        let maid_name =
            get_string_arg(&args, "maid").ok_or_else(|| "请求中缺少 'maid' 参数。".to_string())?;

        let db_state = app
            .try_state::<DbState>()
            .ok_or_else(|| "数据库尚未初始化，TopicSponsor 暂不可用。".to_string())?;
        let pool = &db_state.pool;

        let agent = find_agent_info(pool, &maid_name).await?;
        match command.as_str() {
            "CreateTopic" => handle_create_topic(app, pool, &agent, &args).await,
            "ReadUnlockedTopics" => handle_read_unlocked_topics(pool, &agent, &args).await,
            "CheckNewTopics" => handle_check_new_topics(pool, &agent, &args).await,
            "CheckUnreadMessages" => handle_check_unread_messages(pool, &agent).await,
            "ReplyToTopic" => handle_reply_to_topic(app, pool, &agent, &args).await,
            "CheckTopicOwnership" => handle_check_topic_ownership(pool, &agent, &args).await,
            "ListUnlockedTopics" => handle_list_unlocked_topics(pool, &agent).await,
            "ReadTopicContent" => handle_read_topic_content(pool, &agent, &args).await,
            other => Err(format!("未知的命令: {}", other)),
        }
    }
}

async fn handle_create_topic(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_name = get_string_arg(args, "topic_name")
        .ok_or_else(|| "请求中缺少 'topic_name' 参数。".to_string())?;
    let initial_message = get_string_arg(args, "initial_message")
        .ok_or_else(|| "请求中缺少 'initial_message' 参数。".to_string())?;

    let now = crate::vcp_modules::infra::utils::now_millis();
    let topic_id = format!("topic_{}_{}", now, uuid::Uuid::new_v4().simple());
    let msg_id = format!("msg_{}_assistant_{}", now, uuid::Uuid::new_v4().simple());

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO topics (
            topic_id, owner_id, owner_type, title, created_at, updated_at,
            msg_count, locked, unread, unread_count
         ) VALUES (?, ?, 'agent', ?, ?, ?, 0, 0, 1, 1)",
    )
    .bind(&topic_id)
    .bind(&agent.id)
    .bind(&topic_name)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let chat_message = ChatMessage {
        id: msg_id.clone(),
        role: "assistant".to_string(),
        name: Some(agent.name.clone()),
        content: initial_message.clone(),
        timestamp: now as u64,
        is_thinking: Some(false),
        agent_id: Some(agent.id.clone()),
        group_id: None,
        topic_id: Some(topic_id.clone()),
        is_group_message: Some(false),
        finish_reason: Some("completed".to_string()),
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    };
    let blocks = MessageRenderCompiler::compile(&initial_message);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    MessageRepository::upsert_message(&mut tx, &chat_message, &topic_id, &render_bytes, true)
        .await?;

    sqlx::query("UPDATE topics SET msg_count = 1 WHERE topic_id = ?")
        .bind(&topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    HashAggregator::bubble_from_topic(&mut tx, &topic_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    notify_topic_change(app, pool, &topic_id, now).await?;

    Ok(json!({
        "status": "success",
        "result": {
            "message": format!("成功创建了新的话题：{}", topic_name),
            "topic_id": topic_id,
            "topic_name": topic_name,
            "agent_name": agent.name,
            "agent_id": agent.id,
            "initial_message": initial_message,
            "message_id": msg_id
        }
    }))
}

async fn handle_read_unlocked_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let include_read = get_bool_arg(args, "include_read").unwrap_or(false);
    let mut topics = load_topics(pool, &agent.id, Some(false), None).await?;
    if !include_read {
        topics.retain(|topic| topic.unread);
    }

    let mut topics_with_messages = Vec::new();
    for topic in topics {
        let messages = load_messages(pool, &topic.id).await?;
        topics_with_messages.push(json!({
            "topic_id": topic.id,
            "topic_name": topic.name,
            "locked": topic.locked,
            "unread": topic.unread,
            "created_at": topic.created_at,
            "message_count": messages.len(),
            "messages": messages
        }));
    }

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "topics": topics_with_messages,
            "total_topics": topics_with_messages.len()
        }
    }))
}

async fn handle_check_new_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let days = get_i64_arg(args, "days").unwrap_or(3).max(1);
    let cutoff = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;
    let topics = load_topics(pool, &agent.id, Some(false), Some(cutoff)).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let mapped: Vec<Value> = topics
        .iter()
        .map(|topic| {
            json!({
                "topic_id": topic.id,
                "topic_name": topic.name,
                "created_at": topic.created_at,
                "age_hours": (now - topic.created_at) as f64 / 3_600_000.0,
                "locked": topic.locked
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "has_new_topics": !mapped.is_empty(),
            "new_topics_count": mapped.len(),
            "topics": mapped
        }
    }))
}

async fn handle_check_unread_messages(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
) -> Result<Value, String> {
    let topics = load_topics(pool, &agent.id, None, None)
        .await?
        .into_iter()
        .filter(|topic| topic.unread)
        .collect::<Vec<_>>();
    let mut unread_topics = Vec::new();
    for topic in topics {
        let last_message_time = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT timestamp FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp DESC, msg_id DESC LIMIT 1",
        )
        .bind(&topic.id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten()
        .unwrap_or(topic.created_at);

        unread_topics.push(json!({
            "topic_id": topic.id,
            "topic_name": topic.name,
            "locked": topic.locked,
            "unread": topic.unread,
            "last_message_time": last_message_time
        }));
    }

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "has_unread": !unread_topics.is_empty(),
            "unread_topics": unread_topics
        }
    }))
}

async fn handle_reply_to_topic(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let message =
        get_string_arg(args, "message").ok_or_else(|| "请求中缺少 'message' 参数。".to_string())?;
    let sender_name = get_string_arg(args, "sender_name").unwrap_or_else(|| agent.name.clone());

    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    if topic.locked && !topic.unread {
        return Err(format!(
            "话题 {} 已锁定且未标记为未读，无法添加回复。",
            topic_id
        ));
    }

    let now = crate::vcp_modules::infra::utils::now_millis();
    let chat_message = ChatMessage {
        id: format!("msg_{}_assistant_{}", now, uuid::Uuid::new_v4().simple()),
        role: "assistant".to_string(),
        name: Some(sender_name.clone()),
        content: message,
        timestamp: now as u64,
        is_thinking: Some(false),
        agent_id: Some(agent.id.clone()),
        group_id: None,
        topic_id: Some(topic_id.clone()),
        is_group_message: Some(false),
        finish_reason: Some("completed".to_string()),
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    };

    crate::vcp_modules::message_service::append_single_message(
        app.clone(),
        pool,
        &agent.id,
        "agent",
        topic_id.clone(),
        chat_message.clone(),
    )
    .await?;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE topics
         SET unread = 1, unread_count = unread_count + 1, updated_at = ?
         WHERE topic_id = ?",
    )
    .bind(now)
    .bind(&topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic(&mut tx, &topic_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    notify_topic_change(app, pool, &topic_id, now).await?;

    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    Ok(json!({
        "status": "success",
        "result": {
            "message": format!("成功在 {} 的话题 \"{}\" 中添加回复。", agent.name, topic.name),
            "topic_id": topic_id,
            "topic_name": topic.name,
            "sender": sender_name,
            "message_id": chat_message.id,
            "timestamp": now,
            "agent_name": agent.name,
            "agent_id": agent.id
        }
    }))
}

async fn handle_check_topic_ownership(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let caller_name = get_string_arg(args, "caller_name")
        .ok_or_else(|| "请求中缺少 'caller_name' 参数。".to_string())?;
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    let creator_name = first_message_name(pool, &topic_id)
        .await?
        .unwrap_or_else(|| "unknown".to_string());

    Ok(json!({
        "status": "success",
        "result": {
            "is_owner": creator_name == caller_name,
            "creator_name": creator_name,
            "topic_name": topic.name
        }
    }))
}

async fn handle_list_unlocked_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
) -> Result<Value, String> {
    let topics = load_topics(pool, &agent.id, Some(false), None).await?;
    let mapped: Vec<Value> = topics
        .iter()
        .map(|topic| {
            json!({
                "topic_id": topic.id,
                "topic_name": topic.name,
                "locked": topic.locked,
                "unread": topic.unread,
                "created_at": topic.created_at,
                "message_count": topic.msg_count
            })
        })
        .collect();

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "has_unlocked_topics": !mapped.is_empty(),
            "unlocked_topics_count": mapped.len(),
            "topics": mapped
        }
    }))
}

async fn handle_read_topic_content(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    args: &Value,
) -> Result<Value, String> {
    let topic_id =
        get_topic_id_arg(args).ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
    let topic = find_topic(pool, &agent.id, &topic_id).await?;
    let messages = load_messages(pool, &topic_id).await?;

    Ok(json!({
        "status": "success",
        "result": {
            "agent_name": agent.name,
            "agent_id": agent.id,
            "topic_id": topic.id,
            "topic_name": topic.name,
            "topic_info": {
                "locked": topic.locked,
                "unread": topic.unread,
                "created_at": topic.created_at
            },
            "message_count": messages.len(),
            "messages": messages
        }
    }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TopicRow {
    id: String,
    name: String,
    created_at: i64,
    locked: bool,
    unread: bool,
    unread_count: i32,
    msg_count: i32,
    owner_id: String,
    owner_type: String,
}

async fn load_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
    locked: Option<bool>,
    created_after: Option<i64>,
) -> Result<Vec<TopicRow>, String> {
    let mut sql = "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL"
        .to_string();
    if locked.is_some() {
        sql.push_str(" AND locked = ?");
    }
    if created_after.is_some() {
        sql.push_str(" AND created_at > ?");
    }
    sql.push_str(" ORDER BY updated_at DESC, created_at DESC");

    let mut query = sqlx::query(&sql).bind(agent_id);
    if let Some(locked) = locked {
        query = query.bind(if locked { 1 } else { 0 });
    }
    if let Some(created_after) = created_after {
        query = query.bind(created_after);
    }

    let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| TopicRow {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            unread: row.get::<i32, _>("unread") != 0,
            msg_count: row.get("msg_count"),
            unread_count: row.try_get("unread_count").unwrap_or(0),
            owner_id: agent_id.to_string(),
            owner_type: "agent".to_string(),
        })
        .collect())
}

async fn find_topic(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
    topic_id: &str,
) -> Result<TopicRow, String> {
    let row = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         LIMIT 1",
    )
    .bind(agent_id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("话题 {} 不存在。", topic_id))?;

    Ok(TopicRow {
        id: row.get("topic_id"),
        name: row.get("title"),
        created_at: row.get("created_at"),
        locked: row.get::<i32, _>("locked") != 0,
        unread: row.get::<i32, _>("unread") != 0,
        unread_count: row.get("unread_count"),
        msg_count: row.get("msg_count"),
        owner_id: agent_id.to_string(),
        owner_type: "agent".to_string(),
    })
}

async fn load_messages(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT msg_id, role, name, content, timestamp, agent_id, finish_reason
         FROM messages
         WHERE topic_id = ? AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC",
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    rows.into_iter()
        .map(|row| {
            let content_bytes: Vec<u8> = row.get("content");
            let content = ContentCompressor::decompress(&content_bytes)?;
            Ok(json!({
                "role": row.get::<String, _>("role"),
                "name": row.get::<Option<String>, _>("name"),
                "content": content,
                "timestamp": row.get::<i64, _>("timestamp"),
                "id": row.get::<String, _>("msg_id"),
                "agentId": row.get::<Option<String>, _>("agent_id"),
                "finishReason": row.get::<Option<String>, _>("finish_reason")
            }))
        })
        .collect()
}

async fn first_message_name(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
) -> Result<Option<String>, String> {
    sqlx::query_scalar(
        "SELECT name FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp ASC, msg_id ASC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())
}

async fn find_agent_info(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    maid_name: &str,
) -> Result<AgentInfo, String> {
    let row = sqlx::query(
        "SELECT agent_id, name
         FROM agents
         WHERE deleted_at IS NULL AND name LIKE ?
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(format!("%{}%", maid_name))
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("未找到名为 \"{}\" 的 Agent。", maid_name))?;

    Ok(AgentInfo {
        id: row.get("agent_id"),
        name: row.get("name"),
    })
}

async fn notify_topic_change(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    ts: i64,
) -> Result<(), String> {
    if let Some(sync_state) = app.try_state::<SyncState>() {
        let row = sqlx::query("SELECT config_hash FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row {
            let hash: String = row.get("config_hash");
            let _ = sync_state.ws_sender.send(SyncCommand::NotifyLocalChange {
                data_type: SyncDataType::Topic,
                id: topic_id.to_string(),
                hash,
                ts,
            });
        }
    }
    Ok(())
}

fn get_string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn get_topic_id_arg(args: &Value) -> Option<String> {
    ["topic_id", "topicId", "TopicId"]
        .iter()
        .find_map(|key| get_string_arg(args, key))
}

fn get_bool_arg(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => value.trim().parse::<bool>().ok(),
        _ => None,
    }
}

fn get_i64_arg(args: &Value, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_sponsor_manifest_exposes_create_topic() {
        let manifest = TopicSponsorTool.manifest();
        assert_eq!(manifest.name, "TopicSponsor");
        assert!(manifest
            .invocation_commands
            .iter()
            .any(|command| command.description.contains("CreateTopic")));
    }

    #[test]
    fn parses_legacy_topic_id_aliases() {
        assert_eq!(
            get_topic_id_arg(&json!({ "TopicId": "topic_1" })).as_deref(),
            Some("topic_1")
        );
    }
}
