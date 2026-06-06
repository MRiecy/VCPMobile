// distributed/tools/topic_memo.rs
// [OneShot] TopicMemo — exposes local agent topics to the distributed server.

use async_trait::async_trait;
use serde_json::{json, Value};
use sqlx::Row;
use tauri::{AppHandle, Manager};

use crate::distributed::tool_registry::OneShotTool;
use crate::distributed::types::{InvocationCommand, ToolManifest};
use crate::vcp_modules::context_sanitizer;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::settings_manager::{self, SettingsState};

pub struct TopicMemoTool;

#[derive(Debug, Clone)]
struct AgentInfo {
    id: String,
    name: String,
}

#[derive(Debug, Clone)]
struct TopicInfo {
    id: String,
    title: String,
    created_at: i64,
    locked: bool,
    msg_count: i32,
}

#[derive(Debug, Clone)]
struct MessageInfo {
    role: String,
    name: Option<String>,
    content: String,
}

#[async_trait]
impl OneShotTool for TopicMemoTool {
    fn manifest(&self) -> ToolManifest {
        ToolManifest {
            name: "TopicMemo".to_string(),
            display_name: "话题回忆插件".to_string(),
            description:
                "获取 VCPMobile 本机智能体的话题列表和完整聊天记录，实现 AI 的话题级回忆功能。"
                    .to_string(),
            placeholder: None,
            invocation_commands: vec![
                InvocationCommand {
                    command_identifier: "TopicMemo".to_string(),
                    description: "列出指定智能体的话题，或根据话题 ID 读取完整聊天记录。\n\
参数:\n\
- command (字符串, 可选): ListTopics 或 GetTopicContent，默认 ListTopics\n\
- maid (字符串, 必需): 智能体名称或名称片段\n\
- topic_id/topicId/TopicId (字符串, GetTopicContent 必需): 话题 ID\n\
调用格式:\n\
<<<[TOOL_REQUEST]>>>\n\
tool_name:「始」TopicMemo「末」\n\
command:「始」ListTopics「末」\n\
maid:「始」小克「末」\n\
<<<[END_TOOL_REQUEST]>>>"
                        .to_string(),
                    example: "<<<[TOOL_REQUEST]>>>\ntool_name:「始」TopicMemo「末」\ncommand:「始」GetTopicContent「末」\nmaid:「始」小克「末」\ntopic_id:「始」topic_1766100505346「末」\n<<<[END_TOOL_REQUEST]>>>".to_string(),
                },
            ],
            web_socket_push: None,
        }
    }

    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String> {
        let command = get_string_arg(&args, "command").unwrap_or_else(|| "ListTopics".to_string());
        let maid_name =
            get_string_arg(&args, "maid").ok_or_else(|| "请求中缺少 'maid' 参数。".to_string())?;

        let db_state = app
            .try_state::<DbState>()
            .ok_or_else(|| "数据库尚未初始化，TopicMemo 暂不可用。".to_string())?;
        let pool = &db_state.pool;

        let agent = find_agent_info(pool, &maid_name).await?;
        let result = match command.as_str() {
            "ListTopics" => list_topics(pool, &agent).await?,
            "GetTopicContent" => {
                let topic_id = get_topic_id_arg(&args)
                    .ok_or_else(|| "请求中缺少 'topic_id' 参数。".to_string())?;
                let user_name = find_user_name(app).await;
                get_topic_content(pool, &agent, &topic_id, &user_name).await?
            }
            other => {
                return Err(format!(
                    "未知的指令: {}，支持的指令: ListTopics, GetTopicContent",
                    other
                ));
            }
        };

        Ok(json!({
            "status": "success",
            "result": result
        }))
    }
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
    .ok_or_else(|| format!("未找到名为 \"{}\" 的智能体。", maid_name))?;

    Ok(AgentInfo {
        id: row.get("agent_id"),
        name: row.get("name"),
    })
}

async fn find_user_name(app: &AppHandle) -> String {
    if let Some(settings_state) = app.try_state::<SettingsState>() {
        if let Ok(settings) = settings_manager::read_settings(app.clone(), settings_state).await {
            if !settings.user_name.trim().is_empty() {
                return settings.user_name;
            }
        }
    }
    "用户".to_string()
}

async fn load_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
) -> Result<Vec<TopicInfo>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC, created_at DESC",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(|row| TopicInfo {
            id: row.get("topic_id"),
            title: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            msg_count: row.get("msg_count"),
        })
        .collect())
}

async fn list_topics(pool: &sqlx::Pool<sqlx::Sqlite>, agent: &AgentInfo) -> Result<String, String> {
    let topics = load_topics(pool, &agent.id).await?;

    if topics.is_empty() {
        return Ok(format!("[TopicMemo] {} 暂无任何话题记录。", agent.name));
    }

    let mut result = format!("## {} 的话题列表\n\n", agent.name);
    result.push_str(&format!("共 {} 个话题：\n\n", topics.len()));

    for (index, topic) in topics.iter().enumerate() {
        let locked_tag = if topic.locked { " 🔒" } else { "" };
        result.push_str(&format!(
            "{}. **{}**{}\n",
            index + 1,
            topic.title,
            locked_tag
        ));
        result.push_str(&format!("   - ID: `{}`\n", topic.id));
        result.push_str(&format!(
            "   - 创建时间: {}\n",
            format_timestamp(topic.created_at)
        ));
        result.push_str(&format!("   - 消息数量: {} 条\n\n", topic.msg_count));
    }

    Ok(result)
}

async fn get_topic_content(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent: &AgentInfo,
    topic_id: &str,
    user_name: &str,
) -> Result<String, String> {
    let topic_row = sqlx::query(
        "SELECT topic_id, title, created_at, locked, msg_count
         FROM topics
         WHERE owner_type = 'agent'
           AND owner_id = ?
           AND topic_id = ?
           AND deleted_at IS NULL
         LIMIT 1",
    )
    .bind(&agent.id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| {
        format!(
            "未找到 ID 为 \"{}\" 的话题。可用的话题ID请先使用 ListTopics 指令查询。",
            topic_id
        )
    })?;

    let topic = TopicInfo {
        id: topic_row.get("topic_id"),
        title: topic_row.get("title"),
        created_at: topic_row.get("created_at"),
        locked: topic_row.get::<i32, _>("locked") != 0,
        msg_count: topic_row.get("msg_count"),
    };

    let message_rows = sqlx::query(
        "SELECT role, name, content
         FROM messages
         WHERE topic_id = ?
           AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC",
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let messages: Vec<MessageInfo> = message_rows
        .into_iter()
        .map(|row| {
            let content_bytes: Vec<u8> = row.get("content");
            let content = ContentCompressor::decompress(&content_bytes).unwrap_or_default();
            MessageInfo {
                role: row.get("role"),
                name: row.get("name"),
                content,
            }
        })
        .collect();

    if messages.is_empty() {
        return Ok(format!("## 话题：{}\n\n该话题暂无聊天记录。", topic.title));
    }

    let mut result = format!("## 话题：{}\n", topic.title);
    result.push_str(&format!(
        "创建时间：{}\n",
        format_timestamp(topic.created_at)
    ));
    result.push_str(&format!("消息数量：{} 条\n\n", messages.len()));
    result.push_str("---\n\n");

    for message in messages {
        let speaker_name = speaker_name(&message, user_name, &agent.name);
        let clean_content = clean_message_content(&message.content);
        if !clean_content.is_empty() {
            result.push_str(&format!("**{}**: {}\n\n", speaker_name, clean_content));
        }
    }

    Ok(result)
}

fn speaker_name(message: &MessageInfo, user_name: &str, agent_name: &str) -> String {
    if message.role == "user" {
        user_name.to_string()
    } else {
        message
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(agent_name)
            .to_string()
    }
}

fn clean_message_content(content: &str) -> String {
    let without_executable_blocks = strip_executable_html_blocks(content);
    let cleaned_source = without_executable_blocks.as_deref().unwrap_or(content);

    if !context_sanitizer::contains_html(cleaned_source) {
        return cleaned_source.trim().to_string();
    }

    context_sanitizer::html_to_vcp_markdown(cleaned_source, false)
        .replace(['\r', '\t'], " ")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_executable_html_blocks(content: &str) -> Option<String> {
    let lower_content = content.to_ascii_lowercase();
    if !(lower_content.contains("<script") || lower_content.contains("<style")) {
        return None;
    }

    let mut output = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(start) = find_script_or_style_start(rest) {
        output.push_str(&rest[..start]);

        let tag_end = match rest[start..].find('>') {
            Some(end) => start + end + 1,
            None => return Some(output),
        };
        let tag = if rest[start..].starts_with("<script") {
            "script"
        } else {
            "style"
        };
        let close_tag = format!("</{}>", tag);
        let lower_after_open = rest[tag_end..].to_ascii_lowercase();

        if let Some(close_start) = lower_after_open.find(&close_tag) {
            let close_end = tag_end + close_start + close_tag.len();
            rest = &rest[close_end..];
        } else {
            return Some(output);
        }
    }

    output.push_str(rest);
    Some(output)
}

fn find_script_or_style_start(input: &str) -> Option<usize> {
    let lower = input.to_ascii_lowercase();
    match (lower.find("<script"), lower.find("<style")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn format_timestamp(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y/%m/%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| timestamp_ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_topic_id_aliases() {
        assert_eq!(
            get_topic_id_arg(&json!({ "topicId": "topic_1" })),
            Some("topic_1".to_string())
        );
        assert_eq!(
            get_topic_id_arg(&json!({ "TopicId": "topic_2" })),
            Some("topic_2".to_string())
        );
        assert_eq!(
            get_topic_id_arg(&json!({ "topic_id": "topic_3" })),
            Some("topic_3".to_string())
        );
    }

    #[test]
    fn cleans_html_message_content() {
        let cleaned =
            clean_message_content("<p>Hello <strong>Topic</strong></p><script>x</script>");
        assert!(cleaned.contains("Hello **Topic**"));
        assert!(!cleaned.contains("<p>"));
        assert!(!cleaned.contains('x'));

        let upper = clean_message_content("<STYLE>.a{}</STYLE><p>Ok</p>");
        assert_eq!(upper, "Ok");
    }
}
