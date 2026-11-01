use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::resolve_attachment_cas_file;
use crate::vcp_modules::message_repository::{
    compile_and_serialize_render_async, deserialize_render_async, resolve_message_updated_at,
    serialize_render_async, write_render_cache_cas, MessageRepository, RENDERER_SCHEMA_VERSION,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::SYNC_TOMBSTONE_HASH;
use crate::vcp_modules::topic_types::{MessageKey, TopicActivityDto, TopicKey};
use serde::Serialize;
use sqlx::Row;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

pub(super) const ROW_SELECT: &str =
    "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.updated_at, m.is_group_message, m.group_id, m.finish_reason, r.render_content, r.content_hash AS cache_content_hash, r.renderer_schema_version, m.content_hash
     FROM messages m
     LEFT JOIN render_cache r
       ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
      AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
     LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id";

// =================================================================
// vcp_modules/message_service.rs - 消息业务逻辑中心 (含附件对齐)
// =================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageWriteResultDto {
    pub blocks: Vec<ContentBlock>,
    pub topic_updated_at: i64,
}

fn project_shell_agent(
    row: sqlx::sqlite::SqliteRow,
) -> Result<crate::vcp_modules::agent_types::AgentConfig, String> {
    use crate::vcp_modules::agent_types::AgentConfig;
    let name = match row.try_get::<Option<String>, _>("name") {
        Ok(Some(name)) => name,
        Ok(None) | Err(sqlx::Error::ColumnDecode { .. }) => AgentConfig::default().name,
        Err(error) => return Err(error.to_string()),
    };
    let avatar_calculated_color = match row.try_get("dominant_color") {
        Ok(color) => color,
        Err(sqlx::Error::ColumnDecode { .. }) => None,
        Err(error) => return Err(error.to_string()),
    };
    // Only identity and presentation fields are consumed by precompute_shell;
    // these placeholders are never saved or used as an Agent configuration.
    Ok(AgentConfig {
        id: row.try_get("agent_id").map_err(|error| error.to_string())?,
        name,
        avatar_calculated_color,
        system_prompt: String::new(),
        mobile_system_prompt: String::new(),
        model: String::new(),
        temperature: 0.0,
        context_token_limit: 0,
        max_output_tokens: 0,
        stream_output: false,
        use_temperature: false,
        voice: Default::default(),
        topics: vec![],
    })
}

async fn project_history_attachment(
    app: &AppHandle<impl tauri::Runtime>,
    db: &DbState,
    row: &sqlx::sqlite::SqliteRow,
    include_extracted_text: bool,
) -> Attachment {
    let mut attachment = Attachment {
        r#type: row.get("mime_type"),
        src: row.get("src"),
        name: row.get("display_name"),
        size: row.get::<i64, _>("size") as u64,
        hash: Some(row.get("hash")),
        status: Some(row.get("status")),
        attachment_order: Some(row.get("attachment_order")),
        internal_path: row.get("internal_path"),
        extracted_text: row.get("extracted_text"),
        image_frames: row
            .get::<Option<String>, _>("image_frames")
            .and_then(|frames| serde_json::from_str(&frames).ok()),
        thumbnail_path: row.get("thumbnail_path"),
        created_at: Some(row.get::<i64, _>("created_at") as u64),
    };
    crate::vcp_modules::file_manager::refresh_attachment_paths(app, &mut attachment).await;
    if include_extracted_text
        && attachment.extracted_text.is_none()
        && attachment.status.as_deref() == Some("ready")
    {
        if let Some(hash) = attachment.hash.as_deref() {
            attachment.extracted_text = crate::vcp_modules::file_manager::ensure_extracted_text(
                db,
                hash,
                &attachment.internal_path,
                &attachment.r#type,
            )
            .await;
        }
    }
    attachment
}

#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history_internal(
    _app_handle: &AppHandle,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    before_timestamp: Option<i64>,
    before_message_id: Option<&str>,
    include_content: bool,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = _app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let key = TopicKey::new(owner_type, owner_id, topic_id);

    let owner_matches: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM topics
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owner_matches == 0 {
        return Err("topic does not belong to the selected owner".to_string());
    }

    let offset = offset.unwrap_or(0);
    if before_timestamp.is_some() != before_message_id.is_some() {
        return Err("history cursor requires both beforeTimestamp and beforeMessageId".to_string());
    }
    if offset > 0 && before_timestamp.is_none() {
        return Err(
            "offset history pagination is no longer supported; use a keyset cursor".to_string(),
        );
    }

    let mut query = format!(
        "{ROW_SELECT} WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL"
    );
    if limit.is_some() && before_timestamp.is_some() {
        query.push_str(" AND (m.timestamp < ? OR (m.timestamp = ? AND m.msg_id < ?))");
    }
    query.push_str(" ORDER BY m.timestamp DESC, m.msg_id DESC");
    if limit.is_some() {
        query.push_str(" LIMIT ?");
    }
    let mut q = sqlx::query(sqlx::AssertSqlSafe(query))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    if let Some(l) = limit {
        if let (Some(before_ts), Some(before_id)) = (before_timestamp, before_message_id) {
            q = q
                .bind(before_ts)
                .bind(before_ts)
                .bind(before_id)
                .bind(l as i64);
        } else {
            q = q.bind(l as i64);
        }
    }
    let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

    let mut history = convert_history_rows(
        _app_handle,
        pool,
        &key,
        rows,
        include_content,
        include_extracted_text,
    )
    .await?;

    history.reverse();
    Ok(history)
}

/// 将历史查询结果行转换为 ChatMessage 列表（保持输入行序，调用方负责排序/反转）。
/// 从 load_chat_history_internal 抽出的共享逻辑，供锚点加载 API 复用。
async fn convert_history_rows(
    app_handle: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    rows: Vec<sqlx::sqlite::SqliteRow>,
    include_content: bool,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    let ids: Vec<&str> = rows.iter().map(|row| row.get("msg_id")).collect();
    let attachments = history_attachment_rows(pool, key, &ids, include_extracted_text).await?;
    project_history_rows(
        app_handle,
        pool,
        key,
        rows,
        attachments,
        include_content,
        include_extracted_text,
    )
    .await
}

pub(super) async fn history_attachment_rows<'e>(
    executor: impl sqlx::SqliteExecutor<'e>,
    key: &TopicKey,
    msg_ids: &[&str],
    include_extracted_text: bool,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    if msg_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let extracted_text_column = if include_extracted_text {
        "a.extracted_text"
    } else {
        "NULL"
    };
    let att_query = format!(
            "SELECT a.hash, a.mime_type, a.size, a.internal_path, {} as extracted_text, a.image_frames, a.thumbnail_path, a.created_at,
                    ma.msg_id, ma.attachment_order, ma.display_name, ma.src, ma.status
             FROM message_attachments ma
             JOIN attachments a ON ma.hash = a.hash
             WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
               AND ma.msg_id IN ({})
             ORDER BY ma.msg_id, ma.attachment_order ASC",
            extracted_text_column, placeholders
        );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(att_query))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for &id in msg_ids {
        q = q.bind(id.to_owned());
    }
    q.fetch_all(executor).await.map_err(|e| e.to_string())
}

pub(super) async fn project_history_rows(
    app_handle: &AppHandle<impl tauri::Runtime>,
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    rows: Vec<sqlx::sqlite::SqliteRow>,
    attachment_rows: Vec<sqlx::sqlite::SqliteRow>,
    include_content: bool,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let db_state = app_handle.state::<DbState>();
    let mut att_map: std::collections::HashMap<String, Vec<Attachment>> =
        std::collections::HashMap::new();
    for ar in attachment_rows {
        let msg_id: String = ar.get("msg_id");
        let attachment =
            project_history_attachment(app_handle, &db_state, &ar, include_extracted_text).await;
        att_map.entry(msg_id).or_default().push(attachment);
    }

    // 预计算外壳属性所需的全局数据（避免调用 get_agents 触发昂贵的多余 topics 联表查询）
    let agents = sqlx::query(
        "SELECT a.agent_id, a.name, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.owner_type = 'agent' AND a.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?
    .into_iter().map(project_shell_agent).collect::<Result<Vec<_>, _>>()?;

    let settings =
        crate::vcp_modules::settings_manager::read_config(app_handle.clone(), app_handle.state())
            .await?;
    let user_name = settings.identity.user_name.clone();

    let user_avatar_color: Option<String> = match sqlx::query_scalar::<_, Option<String>>(
        "SELECT dominant_color FROM avatars
         WHERE owner_type = 'user' AND owner_id = 'user_avatar' AND deleted_at IS NULL",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(color) => color.flatten(),
        Err(sqlx::Error::ColumnDecode { .. }) => None,
        Err(error) => return Err(error.to_string()),
    };

    let mut history = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let role: String = row.get("role");
        let name: Option<String> = row.get("name");

        let content: String = row.get("content");
        let content_hash_raw: String = row.get("content_hash");
        let cached_blocks = decode_valid_render_cache(
            row.get("render_content"),
            row.get("cache_content_hash"),
            row.get("renderer_schema_version"),
            &content_hash_raw,
        )
        .await;

        // 缓存只有在 source hash、renderer schema 和压缩载荷均有效时才命中。
        let (blocks, content) = if let Some(blocks) = cached_blocks {
            let content = if include_content {
                content
            } else {
                String::new()
            };
            (Some(blocks), content)
        } else {
            // 未命中：直接用明文 content → 编译 blocks → 异步回写 cache
            let decompressed = content.clone();
            if decompressed.is_empty() {
                (None, String::new())
            } else {
                let (compiled, serialized) =
                    compile_and_serialize_render_async(decompressed.clone()).await?;
                let blocks_json = serde_json::to_value(&compiled).ok();

                let db_c = app_handle.state::<DbState>().inner().clone();
                let cache_key = MessageKey::new(key.clone(), msg_id.clone());
                let observed_hash = content_hash_raw.clone();
                tokio::spawn(async move {
                    let _ = write_render_cache_cas(&db_c, &cache_key, &observed_hash, &serialized)
                        .await;
                });

                let content = if include_content {
                    decompressed
                } else {
                    String::new()
                };
                (blocks_json, content)
            }
        };

        let content_hash = if content_hash_raw.is_empty() {
            None
        } else {
            Some(content_hash_raw)
        };

        let timestamp: i64 = row.get("timestamp");
        let is_thinking: Option<bool> = Some(false);

        let attachments = att_map.remove(&msg_id);

        let mut message = ChatMessage {
            id: msg_id,
            role,
            name,
            content,
            timestamp: timestamp as u64,
            updated_at: u64::try_from(row.get::<i64, _>("updated_at")).ok(),
            is_thinking,
            agent_id: row.get("agent_id"),
            group_id: row.get("group_id"),
            topic_id: Some(key.topic_id.clone()),
            is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
            finish_reason: row.get("finish_reason"),
            attachments,
            blocks,
            shell: None,
            content_hash,
        };

        message.shell = Some(crate::vcp_modules::pre_renderer::precompute_shell(
            &message,
            &agents,
            &user_name,
            user_avatar_color.as_deref(),
        ));
        history.push(message);
    }

    Ok(history)
}

/// 锚点加载：以指定消息为中心，取前 before_n 条 + 锚点 + 后 after_n 条，
/// 按 (timestamp, msg_id) 升序返回。为全局搜索结果跳转定位服务。
pub async fn load_chat_history_around_internal(
    app_handle: &AppHandle,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    anchor_msg_id: &str,
    before_n: usize,
    after_n: usize,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let key = TopicKey::new(owner_type, owner_id, topic_id);

    let owner_matches: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM topics
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owner_matches == 0 {
        return Err("topic does not belong to the selected owner".to_string());
    }

    let anchor_ts: Option<i64> = sqlx::query_scalar(
        "SELECT timestamp FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let Some(anchor_ts) = anchor_ts else {
        return Ok(Vec::new());
    };

    let mut rows: Vec<sqlx::sqlite::SqliteRow> = Vec::new();

    // 前向窗口（早于锚点，含锚点本身）
    let before_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{} WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
           AND (m.timestamp < ? OR (m.timestamp = ? AND m.msg_id <= ?))
         ORDER BY m.timestamp DESC, m.msg_id DESC LIMIT ?",
        ROW_SELECT
    )))
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_ts)
    .bind(anchor_ts)
    .bind(anchor_msg_id)
    .bind(before_n as i64 + 1)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    rows.extend(before_rows);

    // 后向窗口（晚于锚点）
    if after_n > 0 {
        let after_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{} WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
               AND (m.timestamp > ? OR (m.timestamp = ? AND m.msg_id > ?))
             ORDER BY m.timestamp ASC, m.msg_id ASC LIMIT ?",
            ROW_SELECT
        )))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_ts)
        .bind(anchor_ts)
        .bind(anchor_msg_id)
        .bind(after_n as i64)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.extend(after_rows);
    }

    // 统一按 (timestamp, msg_id) 升序排序后转换
    rows.sort_by(|a, b| {
        use sqlx::Row;
        let ta: i64 = a.get("timestamp");
        let tb: i64 = b.get("timestamp");
        let ia: std::string::String = a.get("msg_id");
        let ib: std::string::String = b.get("msg_id");
        (ta, ia).cmp(&(tb, ib))
    });

    convert_history_rows(app_handle, pool, &key, rows, false, false).await
}

/// Plain committed text for decision requests. The SQL byte preflight excludes
/// oversized older messages before their strings cross into Rust. It retains a
/// contiguous newest suffix; callers account for JSON escaping within the same
/// budget before constructing the final request. No attachment/cache IO occurs.
pub(crate) async fn load_chat_plain_text_history_for_context(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    limit: usize,
    byte_budget: usize,
) -> Result<Vec<ChatMessage>, String> {
    let limit = i64::try_from(limit).map_err(|error| error.to_string())?;
    let byte_budget = i64::try_from(byte_budget).map_err(|error| error.to_string())?;
    let rows = sqlx::query(
        "WITH candidates AS MATERIALIZED (
            SELECT m.rowid AS message_rowid, m.timestamp, m.msg_id,
                   length(CAST(m.msg_id AS BLOB)) + length(CAST(m.role AS BLOB))
                   + length(CAST(COALESCE(m.name, a.name, CASE WHEN m.role = 'user' THEN '用户' ELSE 'AI' END) AS BLOB))
                   + COALESCE(length(CAST(m.agent_id AS BLOB)), 0)
                   + length(CAST(m.content AS BLOB)) AS raw_bytes
            FROM messages m
            LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
            WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
              AND NOT EXISTS (
                SELECT 1 FROM active_generations ag
                WHERE ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
                  AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
              )
            ORDER BY m.timestamp DESC, m.msg_id DESC LIMIT ?
         ), budgeted AS (
            SELECT message_rowid,
                   SUM(raw_bytes) OVER (
                     ORDER BY timestamp DESC, msg_id DESC ROWS UNBOUNDED PRECEDING
                   ) AS used_bytes
            FROM candidates
         )
         SELECT m.msg_id, m.role, COALESCE(m.name, a.name) AS name,
                m.agent_id, m.content, m.timestamp
         FROM budgeted b
         JOIN messages m ON m.rowid = b.message_rowid
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE b.used_bytes <= ?
         ORDER BY m.timestamp ASC, m.msg_id ASC",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(limit)
    .bind(byte_budget)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    rows.into_iter()
        .map(|row| {
            Ok(ChatMessage {
                id: row.try_get("msg_id").map_err(|error| error.to_string())?,
                role: row.try_get("role").map_err(|error| error.to_string())?,
                name: row.try_get("name").map_err(|error| error.to_string())?,
                agent_id: row.try_get("agent_id").map_err(|error| error.to_string())?,
                content: row.try_get("content").map_err(|error| error.to_string())?,
                timestamp: u64::try_from(
                    row.try_get::<i64, _>("timestamp")
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?,
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod plain_text_history_tests {
    use super::*;

    #[tokio::test]
    async fn text_budget_keeps_a_complete_contiguous_suffix_without_attachment_tables() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Intentionally no attachments or render_cache tables: this projection
        // must depend only on committed message text and speaker identity.
        sqlx::raw_sql(
            "CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                role TEXT, name TEXT, agent_id TEXT, content TEXT, timestamp INTEGER, deleted_at INTEGER
             );
             CREATE TABLE agents (owner_type TEXT, agent_id TEXT, name TEXT);
             CREATE TABLE active_generations (owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT);
             INSERT INTO agents VALUES ('agent', 'a', '成员');
             INSERT INTO active_generations VALUES ('group','g','t','active');",
        ).execute(&pool).await.unwrap();
        for (id, text, timestamp, deleted) in [
            ("old", "older".into(), 1, None),
            ("large", "中".repeat(100), 2, None),
            ("new-a", "甲".into(), 3, None),
            ("new-z", "乙".into(), 3, None),
            ("active", "unfinished".into(), 4, None),
            ("deleted", "deleted".into(), 5, Some(9)),
        ] {
            sqlx::query(
                "INSERT INTO messages VALUES ('group','g','t',?,'assistant',NULL,'a',?,?,?)",
            )
            .bind(id)
            .bind(text)
            .bind(timestamp)
            .bind(deleted)
            .execute(&pool)
            .await
            .unwrap();
        }
        let key = TopicKey::new("group", "g", "t");
        let history = load_chat_plain_text_history_for_context(&pool, &key, 200, 70)
            .await
            .unwrap();
        assert_eq!(
            history
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["new-a", "new-z"]
        );
        assert_eq!(history[0].content, "甲");
        assert_eq!(history[0].name.as_deref(), Some("成员"));
        assert!(history
            .iter()
            .all(|message| message.attachments.is_none() && message.blocks.is_none()));
        let limited = load_chat_plain_text_history_for_context(&pool, &key, 1, 70)
            .await
            .unwrap();
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, "new-z");
        assert!(
            load_chat_plain_text_history_for_context(&pool, &key, 200, 1)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// 为 Agent 和 Group 组装大模型上下文提供专用的轻量历史查询。
/// 只查询消息纯文本和附件（在需要时提取文本），完全跳过 render_content 反序列化和 UI shell 预计算。
pub async fn load_chat_text_history_for_context(
    app_handle: &AppHandle<impl tauri::Runtime>,
    key: &TopicKey,
    limit: Option<usize>,
    offset: Option<usize>,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;

    let offset = offset.unwrap_or(0);

    // 彻底剥离了对 render_cache 联表查询，仅拉取核心文本和配置字段
    let query_str = if limit.is_some() {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, m.content_hash 
         FROM messages m
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
           AND NOT EXISTS (
             SELECT 1 FROM active_generations ag
             WHERE ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
               AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
           )
         ORDER BY m.timestamp DESC, m.msg_id DESC
         LIMIT ? OFFSET ?"
    } else {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, m.content_hash 
         FROM messages m
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
           AND NOT EXISTS (
             SELECT 1 FROM active_generations ag
             WHERE ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
               AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
           )
         ORDER BY m.timestamp DESC, m.msg_id DESC"
    };

    let mut q = sqlx::query(query_str)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    if let Some(l) = limit {
        q = q.bind(l as i64);
        q = q.bind(offset as i64);
    }
    let rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

    // 收集所有 msg_id，用于查询附件
    let mut msg_ids = Vec::new();
    for row in &rows {
        let msg_id: String = row.get("msg_id");
        msg_ids.push(msg_id);
    }

    let mut att_map: std::collections::HashMap<String, Vec<Attachment>> =
        std::collections::HashMap::new();
    if !msg_ids.is_empty() {
        let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let extracted_text_column = if include_extracted_text {
            "a.extracted_text"
        } else {
            "NULL"
        };
        let att_query = format!(
            "SELECT a.hash, a.mime_type, a.size, a.internal_path, {} as extracted_text, a.image_frames, a.thumbnail_path, a.created_at,
                    ma.msg_id, ma.attachment_order, ma.display_name, ma.src, ma.status
             FROM message_attachments ma
             JOIN attachments a ON ma.hash = a.hash
             WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
               AND ma.msg_id IN ({})
             ORDER BY ma.msg_id, ma.attachment_order ASC",
            extracted_text_column, placeholders
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(att_query))
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let att_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for ar in att_rows {
            let msg_id: String = ar.get("msg_id");
            let attachment =
                project_history_attachment(app_handle, &db_state, &ar, include_extracted_text)
                    .await;
            att_map.entry(msg_id).or_default().push(attachment);
        }
    }

    let mut history = Vec::new();
    for row in rows {
        let msg_id: String = row.get("msg_id");
        let role: String = row.get("role");
        let name: Option<String> = row.get("name");

        let content: String = row.get("content");

        let content_hash_raw: String = row.get("content_hash");
        let content_hash = if content_hash_raw.is_empty() {
            None
        } else {
            Some(content_hash_raw)
        };

        let timestamp: i64 = row.get("timestamp");
        let attachments = att_map.remove(&msg_id);

        let message = ChatMessage {
            id: msg_id,
            role,
            name,
            content,
            timestamp: timestamp as u64,
            updated_at: None,
            is_thinking: Some(false),
            agent_id: row.get("agent_id"),
            group_id: row.get("group_id"),
            topic_id: Some(key.topic_id.clone()),
            is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
            finish_reason: row.get("finish_reason"),
            attachments,
            blocks: None, // 彻底不加载和反序列化渲染 cache 块
            shell: None,  // 彻底不预计算 UI 头像、边框背景等外壳属性
            content_hash,
        };
        history.push(message);
    }

    history.reverse();
    Ok(history)
}

/// 以 Mobile 本地 CAS 为唯一文件真理源，禁止消息写入隐式下载桌面附件。
pub(crate) async fn normalize_attachments_from_local_cas<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    message: &mut ChatMessage,
) -> Result<(), String> {
    let attachments = match &mut message.attachments {
        Some(atts) => atts,
        None => return Ok(()),
    };

    for att in attachments {
        let hash = att
            .hash
            .as_deref()
            .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
            .ok_or_else(|| format!("Attachment {} requires a valid SHA-256 hash", att.name))?
            .to_ascii_lowercase();
        att.hash = Some(hash.clone());

        match resolve_attachment_cas_file(app, pool, &hash).await {
            Ok(local) => {
                let local_path = local.path.to_string_lossy().into_owned();
                att.r#type = local.mime_type;
                att.size = local.size_bytes;
                att.src = format!("file://{local_path}");
                att.internal_path = local_path;
                att.status = Some("ready".to_string());
            }
            Err(_) if att.status.as_deref() == Some("desktop_only") => {
                att.src.clear();
                att.internal_path.clear();
            }
            Err(error) => {
                return Err(format!(
                    "Attachment {} is not available in the Mobile CAS: {error}",
                    att.name
                ));
            }
        }
    }
    Ok(())
}

pub async fn begin_stream_message(
    db_state: &DbState,
    message_key: &MessageKey,
    agent_id: Option<&str>,
    agent_name: Option<&str>,
) -> Result<(), String> {
    let key = &message_key.topic;
    let message_id = &message_key.msg_id;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let (_, render_bytes) = compile_and_serialize_render_async(String::new()).await?;
    let fingerprint_timestamp = u64::try_from(now)
        .map_err(|_| "Current timestamp cannot be represented as u64".to_string())?;
    let content_hash = HashAggregator::compute_message_fingerprint(
        message_id,
        "assistant",
        agent_name,
        "",
        fingerprint_timestamp,
        agent_id,
        &[],
    );
    let is_group = key.owner_type == "group";
    let mut tx = db_state.write_transaction("message.stream.begin").await?;

    let inserted = sqlx::query(
        "INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content, timestamp,
            is_group_message, group_id, finish_reason, content_hash, created_at, updated_at
         ) SELECT ?, ?, ?, ?, 'assistant', ?, ?, '', ?, ?, ?, NULL, ?, ?, ?
           WHERE EXISTS (
             SELECT 1 FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
           )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(agent_name)
    .bind(agent_id)
    .bind(now)
    .bind(is_group)
    .bind(if is_group {
        Some(key.owner_id.as_str())
    } else {
        None
    })
    .bind(&content_hash)
    .bind(now)
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if inserted.rows_affected() != 1 {
        tx.commit().await.map_err(|e| e.to_string())?;
        return Err(format!(
            "Topic {} does not belong to live {} {}",
            key.topic_id, key.owner_type, key.owner_id
        ));
    }

    sqlx::query(
        "INSERT INTO render_cache (
            owner_type, owner_id, topic_id, msg_id, render_content, content_hash,
            renderer_schema_version, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(render_bytes)
    .bind(&content_hash)
    .bind(RENDERER_SCHEMA_VERSION)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    HashAggregator::bubble_from_topic(&mut tx, key).await?;

    sqlx::query(
        "INSERT INTO active_generations (owner_type, owner_id, topic_id, msg_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn append_single_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
) -> Result<MessageWriteResultDto, String> {
    normalize_attachments_from_local_cas(&app_handle, db_pool, &mut message).await?;

    let (blocks, render_bytes): (Vec<ContentBlock>, Vec<u8>) =
        if let Some(blocks_val) = &message.blocks {
            let blocks: Vec<ContentBlock> =
                serde_json::from_value(blocks_val.clone()).map_err(|e| e.to_string())?;
            let render_bytes = serialize_render_async(blocks.clone()).await?;
            (blocks, render_bytes)
        } else {
            compile_and_serialize_render_async(message.content.clone()).await?
        };

    let key = TopicKey::new(owner_type, owner_id, &topic_id);
    let db_state = app_handle.state::<DbState>();
    let mut tx = db_state.write_transaction("message.append").await?;
    let activity =
        MessageRepository::upsert_message(&mut tx, &message, &key, &render_bytes, false).await?;
    let activity = match activity {
        Some(activity) => activity,
        None => HashAggregator::load_topic_activity(&mut tx, &key).await?,
    };

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(MessageWriteResultDto {
        blocks,
        topic_updated_at: activity.updated_at,
    })
}

/// Editing a sent message and discarding its following history are one commit.
/// Attachment validation and compilation happen before any history is changed.
pub async fn edit_resend_message<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    db_state: &DbState,
    key: &TopicKey,
    mut message: ChatMessage,
) -> Result<(MessageWriteResultDto, MessageDeletionResult), String> {
    normalize_attachments_from_local_cas(app_handle, &db_state.pool, &mut message).await?;
    let (blocks, render_bytes) =
        compile_and_serialize_render_async(message.content.clone()).await?;
    let mut tx = db_state.write_transaction("message.edit_resend").await?;
    let mut deletion = truncate_history_after_message_in_tx(&mut tx, key, &message.id).await?;
    let activity =
        MessageRepository::upsert_message(&mut tx, &message, key, &render_bytes, false).await?;
    if let Some(activity) = activity {
        deletion.msg_count = activity.msg_count;
        deletion.topic_updated_at = activity.updated_at;
    }
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok((
        MessageWriteResultDto {
            blocks,
            topic_updated_at: deletion.topic_updated_at,
        },
        deletion,
    ))
}

#[tauri::command]
pub async fn fetch_raw_message_content<R: tauri::Runtime>(
    app_handle: tauri::AppHandle<R>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
) -> Result<String, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);

    let row = sqlx::query(
        "SELECT content FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    match row {
        Some(r) => {
            let content: String = r.get(0);
            Ok(content)
        }
        None => Err(format!("Message {} not found", key.msg_id)),
    }
}

#[tauri::command]
pub async fn re_render_message(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    message_id: String,
    topic_id: String,
) -> Result<serde_json::Value, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);

    let row = sqlx::query(
        "SELECT content, content_hash FROM messages \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    match row {
        Some(r) => {
            let decompressed: String = r.get("content");

            let observed_hash: String = r.get("content_hash");
            let (compiled, serialized) = compile_and_serialize_render_async(decompressed).await?;
            if !write_render_cache_cas(&db_state, &key, &observed_hash, &serialized).await? {
                return Err(format!(
                    "Message {} changed while re-rendering; stale cache discarded",
                    key.msg_id
                ));
            }

            serde_json::to_value(&compiled).map_err(|e| e.to_string())
        }
        None => Err(format!(
            "Message {} with topic {} not found",
            key.msg_id, key.topic.topic_id
        )),
    }
}

pub async fn patch_single_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
    skip_bubble: bool,
) -> Result<MessageWriteResultDto, String> {
    normalize_attachments_from_local_cas(&app_handle, db_pool, &mut message).await?;

    // 优先使用传入的 blocks，如果缺失则实时编译
    let (blocks, render_bytes): (Vec<ContentBlock>, Vec<u8>) =
        if let Some(blocks_val) = &message.blocks {
            let blocks: Vec<ContentBlock> =
                serde_json::from_value(blocks_val.clone()).map_err(|e| e.to_string())?;
            let render_bytes = serialize_render_async(blocks.clone()).await?;
            (blocks, render_bytes)
        } else {
            compile_and_serialize_render_async(message.content.clone()).await?
        };

    let key = TopicKey::new(owner_type, owner_id, &topic_id);
    let db_state = app_handle.state::<DbState>();
    let mut tx = db_state.write_transaction("message.patch").await?;
    let activity =
        MessageRepository::upsert_message(&mut tx, &message, &key, &render_bytes, skip_bubble)
            .await?;
    let activity = match activity {
        Some(activity) => activity,
        None => HashAggregator::load_topic_activity(&mut tx, &key).await?,
    };

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(MessageWriteResultDto {
        blocks,
        topic_updated_at: activity.updated_at,
    })
}

pub struct MessageDeletionResult {
    pub deleted_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub deleted_at: i64,
    pub msg_count: i32,
    pub topic_updated_at: i64,
}

pub async fn delete_messages(
    db_state: &DbState,
    key: &TopicKey,
    msg_ids: Vec<String>,
    deleted_at: Option<i64>,
) -> Result<MessageDeletionResult, String> {
    if msg_ids.is_empty() {
        let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
        let activity = HashAggregator::load_topic_activity(&mut tx, key).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok(MessageDeletionResult {
            deleted_ids: Vec::new(),
            active_ids: Vec::new(),
            deleted_at: deleted_at.unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
            msg_count: activity.msg_count,
            topic_updated_at: activity.updated_at,
        });
    }
    if key.topic_id.is_empty()
        || msg_ids.len() > 10_000
        || msg_ids.iter().any(|id| id.is_empty())
        || msg_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != msg_ids.len()
    {
        return Err("Message delete requires a topic and 1..=10000 unique message ids".to_string());
    }
    let mut tx = db_state.write_transaction("message.delete").await?;
    let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let select_deleted_ids = format!(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({placeholders})"
    );
    let mut deleted_query = sqlx::query_scalar(sqlx::AssertSqlSafe(select_deleted_ids))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        deleted_query = deleted_query.bind(id);
    }
    let deleted_ids: Vec<String> = deleted_query
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    let select_active_ids = format!(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})"
    );
    let mut active_query = sqlx::query_scalar(sqlx::AssertSqlSafe(select_active_ids))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        active_query = active_query.bind(id);
    }
    let active_ids = active_query
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let delete_query = format!(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let now = deleted_at.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let mut q = sqlx::query(sqlx::AssertSqlSafe(delete_query))
        .bind(now)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        q = q.bind(id);
    }
    let deleted = q.execute(&mut *tx).await.map_err(|e| e.to_string())?;
    if deleted.rows_affected() != deleted_ids.len() as u64 {
        return Err(format!(
            "Message delete changed {} rows, expected {} for topic {}",
            deleted.rows_affected(),
            deleted_ids.len(),
            key.topic_id
        ));
    }

    // 物理强清除 render_cache 缓存，杜绝幽灵缓存残留
    let delete_cache_query = format!(
        "DELETE FROM render_cache
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let mut q_cache = sqlx::query(sqlx::AssertSqlSafe(delete_cache_query))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        q_cache = q_cache.bind(id);
    }
    q_cache.execute(&mut *tx).await.map_err(|e| e.to_string())?;

    // 物理强清除 message_attachments 关联，防止孤立关联残留
    let delete_attachments_query = format!(
        "DELETE FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let mut q_attachments = sqlx::query(sqlx::AssertSqlSafe(delete_attachments_query))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        q_attachments = q_attachments.bind(id);
    }
    q_attachments
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // 级联清除活跃生成注册表，杜绝已删除消息复活
    let delete_active_gen_query = format!(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id IN ({})",
        msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ")
    );
    let mut q_active = sqlx::query(sqlx::AssertSqlSafe(delete_active_gen_query))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &msg_ids {
        q_active = q_active.bind(id);
    }
    q_active
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // FTS 由 after_messages_logical_delete 触发器在同一事务中清理。
    let activity = HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(MessageDeletionResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count: activity.msg_count,
        topic_updated_at: activity.updated_at,
    })
}

/// Applies Desktop message tombstones as one Topic-local durable mutation. Aggregate count/root
/// repair is intentionally owned by SyncFinalizer so a batch of tombstones does not rescan the
/// same Topic once per message.
pub(crate) async fn apply_sync_message_tombstones(
    db_state: &DbState,
    key: &TopicKey,
    tombstones: &[(String, i64)],
) -> Result<Vec<String>, String> {
    const MAX_SAFE_JSON_INTEGER: i64 = (1_i64 << 53) - 1;
    if tombstones.is_empty() {
        return Ok(Vec::new());
    }
    if tombstones
        .iter()
        .any(|(id, deleted_at)| id.is_empty() || !(0..=MAX_SAFE_JSON_INTEGER).contains(deleted_at))
        || tombstones
            .iter()
            .map(|(id, _)| id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != tombstones.len()
    {
        return Err(
            "Sync message tombstones require unique non-empty ids and safe deletedAt values"
                .to_string(),
        );
    }

    let mut tx = db_state.write_transaction("sync.delete.message").await?;
    let topic_is_live = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    if !topic_is_live {
        tx.commit().await.map_err(|error| error.to_string())?;
        return Ok(Vec::new());
    }

    const TOMBSTONE_CHUNK: usize = 400;
    let mut active_ids = Vec::new();
    for chunk in tombstones.chunks(TOMBSTONE_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let active_sql = format!(
            "SELECT msg_id FROM active_generations
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({placeholders})"
        );
        let mut active_query = sqlx::query_scalar(sqlx::AssertSqlSafe(active_sql))
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for (id, _) in chunk {
            active_query = active_query.bind(id);
        }
        active_ids.extend(
            active_query
                .fetch_all(&mut *tx)
                .await
                .map_err(|error| error.to_string())?,
        );

        let values = chunk
            .iter()
            .map(|_| "(?, ?, ?, ?, 'system', '', ?, ?, ?, ?, ?, ?, ?)")
            .collect::<Vec<_>>()
            .join(", ");
        let upsert_sql = format!(
            "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
                is_group_message, group_id, content_hash, created_at, updated_at, deleted_at
             ) VALUES {values}
             ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                content_hash = excluded.content_hash,
                updated_at = MAX(messages.updated_at, excluded.updated_at),
                deleted_at = CASE
                    WHEN messages.deleted_at IS NULL
                      OR messages.deleted_at < excluded.deleted_at
                    THEN excluded.deleted_at
                    ELSE messages.deleted_at
                END"
        );
        let mut upsert = sqlx::query(sqlx::AssertSqlSafe(upsert_sql));
        for (id, deleted_at) in chunk {
            upsert = upsert
                .bind(&key.owner_type)
                .bind(&key.owner_id)
                .bind(&key.topic_id)
                .bind(id)
                .bind(*deleted_at)
                .bind(i64::from(key.owner_type == "group"))
                .bind((key.owner_type == "group").then_some(key.owner_id.as_str()))
                .bind(SYNC_TOMBSTONE_HASH)
                .bind(*deleted_at)
                .bind(*deleted_at)
                .bind(*deleted_at);
        }
        upsert
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;

        for table in ["render_cache", "message_attachments", "active_generations"] {
            let delete_sql = format!(
                "DELETE FROM {table}
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                   AND msg_id IN ({placeholders})"
            );
            let mut delete = sqlx::query(sqlx::AssertSqlSafe(delete_sql))
                .bind(&key.owner_type)
                .bind(&key.owner_id)
                .bind(&key.topic_id);
            for (id, _) in chunk {
                delete = delete.bind(id);
            }
            delete
                .execute(&mut *tx)
                .await
                .map_err(|error| error.to_string())?;
        }
    }

    // FTS is removed by after_messages_logical_delete in this same transaction.
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(active_ids)
}

pub async fn truncate_history_after_message(
    db_state: &DbState,
    key: &TopicKey,
    anchor_message_id: &str,
) -> Result<MessageDeletionResult, String> {
    let mut tx = db_state.write_transaction("message.truncate").await?;
    let result = truncate_history_after_message_in_tx(&mut tx, key, anchor_message_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(result)
}

async fn truncate_history_after_message_in_tx(
    tx: &mut sqlx::SqliteConnection,
    key: &TopicKey,
    anchor_message_id: &str,
) -> Result<MessageDeletionResult, String> {
    let anchor_timestamp: i64 = sqlx::query_scalar(
        "SELECT timestamp FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_message_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("Anchor message {anchor_message_id} is missing or deleted"))?;

    let active_ids: Vec<String> = sqlx::query_scalar(
        "SELECT ag.msg_id FROM active_generations ag
         JOIN messages m
           ON m.owner_type = ag.owner_type AND m.owner_id = ag.owner_id
          AND m.topic_id = ag.topic_id AND m.msg_id = ag.msg_id
         WHERE ag.owner_type = ? AND ag.owner_id = ? AND ag.topic_id = ?
           AND (m.timestamp > ? OR (m.timestamp = ? AND m.msg_id > ?))",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let deleted_ids: Vec<String> = sqlx::query_scalar(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    // 物理强清除 render_cache，消灭幽灵缓存
    sqlx::query(
        "DELETE FROM render_cache
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN (
             SELECT msg_id FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
           )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN (
             SELECT msg_id FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
           )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN (
             SELECT msg_id FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
           )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let now = chrono::Utc::now().timestamp_millis();
    let deleted = sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
           AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_timestamp)
    .bind(anchor_timestamp)
    .bind(anchor_message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if deleted.rows_affected() != deleted_ids.len() as u64 {
        return Err(format!(
            "History truncation changed {} rows, expected {} for topic {}",
            deleted.rows_affected(),
            deleted_ids.len(),
            key.topic_id
        ));
    }
    // FTS 由 after_messages_logical_delete 触发器在同一事务中清理。
    let activity = HashAggregator::bubble_from_topic(tx, key).await?;
    Ok(MessageDeletionResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count: activity.msg_count,
        topic_updated_at: activity.updated_at,
    })
}

async fn decode_valid_render_cache(
    render_content: Option<Vec<u8>>,
    cache_content_hash: Option<String>,
    renderer_schema_version: Option<i64>,
    message_content_hash: &str,
) -> Option<serde_json::Value> {
    if cache_content_hash.as_deref() != Some(message_content_hash)
        || renderer_schema_version != Some(RENDERER_SCHEMA_VERSION)
    {
        return None;
    }
    let blocks = deserialize_render_async(render_content?).await.ok()?;
    serde_json::to_value(blocks).ok()
}

/// The durable message identity returned to request owners. A removed message
/// has no result and must never be reintroduced into a later group context.
#[derive(Debug)]
pub(crate) struct FinalizedStreamMessage {
    pub(crate) content: String,
    pub(crate) finish_reason: String,
    pub(crate) timestamp: u64,
    pub(crate) topic_updated_at: Option<i64>,
    /// A late error must not count an existing terminal as a new speaker completion.
    pub(crate) newly_committed: bool,
}

#[derive(Debug)]
enum StreamCommit {
    Committed(Vec<ContentBlock>, u64, i64, Option<String>),
    AlreadyFinalized(FinalizedStreamMessage),
    Removed,
}

/// A transport result that still needs its durable transaction. RequestOutput
/// owns the same allocation during a retry; live partials are not cached here.
#[derive(Clone)]
pub(crate) struct PendingStreamTerminal {
    content: String,
    finish_reason: String,
    agent_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_stream_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
) -> Result<Option<FinalizedStreamMessage>, String> {
    let terminal = std::sync::Arc::new(PendingStreamTerminal {
        content: full_content,
        finish_reason: if is_aborted {
            "cancelled_by_user".to_string()
        } else {
            finish_reason.unwrap_or_else(|| "completed".to_string())
        },
        agent_id,
    });
    crate::vcp_modules::vcp_client::retain_request_terminal(
        &app_handle,
        message_key,
        terminal.clone(),
    );
    finalize_stream_message_inner(
        app_handle,
        pool,
        message_key,
        terminal,
        stream_channel,
        false,
    )
    .await
}

/// A completed helper file is recovered history even when SQLite has not yet
/// accepted its terminal transaction. Preserve that origin on the normal end event.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_recovered_stream_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    full_content: String,
    finish_reason: String,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
    agent_id: Option<String>,
) -> Result<Option<FinalizedStreamMessage>, String> {
    finalize_stream_message_inner(
        app_handle,
        pool,
        message_key,
        std::sync::Arc::new(PendingStreamTerminal {
            content: full_content,
            finish_reason,
            agent_id,
        }),
        Some(stream_channel),
        true,
    )
    .await
}

pub(crate) async fn retry_stream_terminal<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    terminal: std::sync::Arc<PendingStreamTerminal>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
) -> Result<Option<FinalizedStreamMessage>, String> {
    finalize_stream_message_inner(
        app_handle,
        pool,
        message_key,
        terminal,
        stream_channel,
        true,
    )
    .await
}

async fn finalize_stream_message_inner<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    _pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    terminal: std::sync::Arc<PendingStreamTerminal>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    replayed: bool,
) -> Result<Option<FinalizedStreamMessage>, String> {
    let owner_id = message_key.topic.owner_id.as_str();
    let owner_type = message_key.topic.owner_type.as_str();
    let topic_id = message_key.topic.topic_id.as_str();
    let message_id = message_key.msg_id.as_str();
    let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;

    let final_content = &terminal.content;

    let is_group = owner_type == "group";

    let final_agent_id = if is_group {
        terminal.agent_id.clone()
    } else {
        Some(owner_id.to_string())
    };

    let terminal_reason = &terminal.finish_reason;
    let mut context = if owner_id.is_empty() || topic_id.is_empty() {
        None
    } else if is_group {
        Some(serde_json::json!({
            "ownerId": owner_id,
            "ownerType": owner_type,
            "groupId": owner_id,
            "agentId": final_agent_id,
            "topicId": topic_id,
            "isGroupMessage": true,
        }))
    } else {
        Some(serde_json::json!({
            "ownerId": owner_id,
            "ownerType": owner_type,
            "agentId": owner_id,
            "topicId": topic_id,
        }))
    };
    if replayed {
        if let Some(context) = context.as_mut() {
            context["replayed"] = serde_json::json!(true);
        }
    }
    let (end_blocks, end_timestamp, topic_updated_at) = if owner_id.is_empty()
        || topic_id.is_empty()
    {
        (None, final_ts, None)
    } else {
        let db_state = app_handle.state::<DbState>();
        match commit_stream_message(
            &db_state,
            message_key,
            final_content,
            final_ts,
            terminal_reason,
        )
        .await
        {
            Ok(StreamCommit::Committed(blocks, start_timestamp, updated_at, _agent_name)) => {
                #[cfg(target_os = "ios")]
                if !replayed && !matches!(terminal_reason.as_str(), "error" | "cancelled_by_user") {
                    // The transaction won the terminal transition, so this reply exists
                    // in the exact history window referenced by its notification.
                    tauri_plugin_vcp_mobile::system::notify_committed_reply(
                        &app_handle,
                        owner_type,
                        owner_id,
                        topic_id,
                        message_id,
                        _agent_name.as_deref().unwrap_or("智能体"),
                    );
                }
                (Some(blocks), start_timestamp, Some(updated_at))
            }
            Ok(StreamCommit::AlreadyFinalized(message)) => {
                crate::vcp_modules::vcp_client::release_committed_helper(&app_handle, message_key)
                    .await;
                // A different durable writer may have settled the row while
                // this consumer was attached. Replay that result through the
                // same output owner; its closed flag suppresses duplicates.
                let mut context = context;
                if let Some(context) = context.as_mut() {
                    context["replayed"] = serde_json::json!(true);
                }
                crate::vcp_modules::vcp_client::deliver_request_event(
                    &app_handle,
                    message_key,
                    stream_channel,
                    crate::vcp_modules::vcp_client::StreamEvent::end(
                        message_id.to_string(),
                        context,
                        Some(message.finish_reason.clone()),
                        Some(message.content.clone()),
                        None,
                        Some(message.timestamp),
                        message.topic_updated_at,
                    ),
                );
                return Ok(Some(message));
            }
            Ok(StreamCommit::Removed) => {
                crate::vcp_modules::vcp_client::discard_request_helper(&app_handle, message_key)
                    .await;
                return Ok(None);
            }
            Err(error) => {
                let mut event = crate::vcp_modules::vcp_client::StreamEvent::error(
                    message_id.to_string(),
                    context.clone(),
                    format!("终态保存失败: {}", error),
                );
                event.content = Some(final_content.clone());
                crate::vcp_modules::vcp_client::deliver_request_event(
                    &app_handle,
                    message_key,
                    stream_channel.clone(),
                    event,
                );
                return Err(error);
            }
        }
    };

    crate::vcp_modules::vcp_client::release_committed_helper(&app_handle, message_key).await;

    {
        let event = crate::vcp_modules::vcp_client::StreamEvent::end(
            message_id.to_string(),
            context,
            Some(terminal_reason.clone()),
            Some(final_content.clone()),
            end_blocks,
            Some(end_timestamp),
            topic_updated_at,
        );
        crate::vcp_modules::vcp_client::deliver_request_event(
            &app_handle,
            message_key,
            stream_channel,
            event,
        );
    }

    let terminal = std::sync::Arc::unwrap_or_clone(terminal);
    Ok(Some(FinalizedStreamMessage {
        content: terminal.content,
        finish_reason: terminal.finish_reason,
        timestamp: end_timestamp,
        topic_updated_at,
        newly_committed: true,
    }))
}

async fn commit_stream_message(
    db_state: &DbState,
    message_key: &MessageKey,
    final_content: &str,
    final_ts: u64,
    finish_reason: &str,
) -> Result<StreamCommit, String> {
    let key = &message_key.topic;
    let owner_id = key.owner_id.as_str();
    let owner_type = key.owner_type.as_str();
    let topic_id = key.topic_id.as_str();
    let message_id = message_key.msg_id.as_str();
    let (blocks, render_bytes) =
        compile_and_serialize_render_async(final_content.to_string()).await?;
    let is_group = owner_type == "group";
    let mut tx = db_state
        .write_transaction("message.stream.finalize")
        .await?;

    let row = sqlx::query(
        "SELECT m.timestamp, m.content, m.name, m.agent_id, m.finish_reason, m.deleted_at,
                ag.msg_id IS NOT NULL AS pending
         FROM messages m
         LEFT JOIN active_generations ag
           ON ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
          AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let Some(row) = row else {
        return Ok(StreamCommit::Removed);
    };
    if row.get::<Option<i64>, _>("deleted_at").is_some() {
        return Ok(StreamCommit::Removed);
    }
    let start_timestamp: i64 = row.get("timestamp");
    let message_name: Option<String> = row.get("name");
    let message_agent_id: Option<String> = row.get("agent_id");
    let fingerprint_timestamp = u64::try_from(start_timestamp)
        .map_err(|_| format!("Message {message_id} has a negative timestamp"))?;
    if let Some(reason) = row.get::<Option<String>, _>("finish_reason") {
        let activity = HashAggregator::load_topic_activity(&mut tx, key).await?;
        return Ok(StreamCommit::AlreadyFinalized(FinalizedStreamMessage {
            content: row.get("content"),
            finish_reason: reason,
            timestamp: fingerprint_timestamp,
            topic_updated_at: Some(activity.updated_at),
            newly_committed: false,
        }));
    }
    if !row.get::<bool, _>("pending") {
        return Err(format!(
            "Generation {} is not pending for {} {} topic {}",
            message_id, owner_type, owner_id, topic_id
        ));
    }
    let content_hash = HashAggregator::compute_message_fingerprint(
        message_id,
        "assistant",
        message_name.as_deref(),
        final_content,
        fingerprint_timestamp,
        message_agent_id.as_deref(),
        &[],
    );

    let updated = sqlx::query(
        "UPDATE messages SET role = 'assistant', content = ?, \
         is_group_message = ?, group_id = ?, finish_reason = ?, \
         content_hash = ?, updated_at = ? \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND finish_reason IS NULL AND deleted_at IS NULL \
         AND EXISTS(SELECT 1 FROM active_generations \
                    WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?)",
    )
    .bind(final_content)
    .bind(is_group)
    .bind(if is_group { Some(owner_id) } else { None })
    .bind(finish_reason)
    .bind(&content_hash)
    .bind(final_ts as i64)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if updated.rows_affected() != 1 {
        return Err(format!(
            "Generation {} is not pending for {} {} topic {}",
            message_id, owner_type, owner_id, topic_id
        ));
    }

    let cache_updated = sqlx::query(
        "UPDATE render_cache SET render_content = ?, content_hash = ?, \
         renderer_schema_version = ?, updated_at = ? \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(render_bytes)
    .bind(&content_hash)
    .bind(RENDERER_SCHEMA_VERSION)
    .bind(final_ts as i64)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if cache_updated.rows_affected() != 1 {
        return Err(format!(
            "Render cache missing for pending generation {}",
            message_id
        ));
    }

    sqlx::query(
        "DELETE FROM messages_fts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(&key.topic_id)
    .bind(final_content)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let deleted = sqlx::query(
        "DELETE FROM active_generations \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if deleted.rows_affected() != 1 {
        return Err(format!(
            "Active generation {} changed during finalization",
            message_id
        ));
    }

    let activity = HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(StreamCommit::Committed(
        blocks,
        start_timestamp as u64,
        activity.updated_at,
        message_name,
    ))
}

#[tauri::command]
pub async fn delete_message_attachment(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
    attachment_order: i32,
    hash: String,
) -> Result<TopicActivityDto, String> {
    use crate::vcp_modules::db_manager::DbState;
    use tauri::Manager;
    let db_state = app_handle.state::<DbState>();
    let now = crate::vcp_modules::infra::utils::now_millis();
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    delete_message_attachment_in_db(
        &db_state,
        &key.topic,
        &key.msg_id,
        attachment_order,
        &hash,
        now,
    )
    .await
}

async fn delete_message_attachment_in_db(
    db_state: &DbState,
    key: &TopicKey,
    message_id: &str,
    attachment_order: i32,
    hash: &str,
    now: i64,
) -> Result<TopicActivityDto, String> {
    let mut tx = db_state
        .write_transaction("message.attachment.delete")
        .await?;
    let deleted = sqlx::query(
        "DELETE FROM message_attachments \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND attachment_order = ? AND hash = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(attachment_order)
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if deleted.rows_affected() == 0 {
        let activity = HashAggregator::load_topic_activity(&mut tx, key).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok(activity);
    }

    let (role, name, content, timestamp, agent_id, old_hash, old_updated_at): (
        String,
        Option<String>,
        String,
        i64,
        Option<String>,
        String,
        i64,
    ) = sqlx::query_as(
        "SELECT role, name, content, timestamp, agent_id, content_hash, updated_at \
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let timestamp = u64::try_from(timestamp)
        .map_err(|_| format!("Message {message_id} has a negative timestamp"))?;
    let attachment_hashes: Vec<String> = sqlx::query_scalar(
        "SELECT hash FROM message_attachments \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let new_hash = HashAggregator::compute_message_fingerprint(
        message_id,
        &role,
        name.as_deref(),
        &content,
        timestamp,
        agent_id.as_deref(),
        &attachment_hashes,
    );
    let updated_at = resolve_message_updated_at(
        None,
        timestamp,
        &new_hash,
        Some((&old_hash, old_updated_at)),
        now,
    )
    .map_err(|error| format!("message {message_id} {error}"))?;
    let updated = sqlx::query(
        "UPDATE messages SET content_hash = ?, updated_at = ? \
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&new_hash)
    .bind(updated_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if updated.rows_affected() != 1 {
        return Err(format!("Message {message_id} is missing or deleted"));
    }

    let activity = HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(activity)
}

#[cfg(test)]
mod stream_lifecycle_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn shell_presentation_defaults_bad_values_without_fabricating_identity() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let broken =
            sqlx::query("SELECT 'agent' AS agent_id,x'ff' AS name,x'ff' AS dominant_color")
                .fetch_one(&pool)
                .await
                .unwrap();
        let agent = project_shell_agent(broken).unwrap();
        assert_eq!(agent.id, "agent");
        assert_eq!(
            agent.name,
            crate::vcp_modules::agent_types::AgentConfig::default().name
        );
        assert_eq!(agent.avatar_calculated_color, None);
        let empty = sqlx::query("SELECT 'agent' AS agent_id,'' AS name,NULL AS dominant_color")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(project_shell_agent(empty).unwrap().name, "");
        let identity =
            sqlx::query("SELECT x'ff' AS agent_id,'Keep' AS name,NULL AS dominant_color")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(project_shell_agent(identity).is_err());
        let missing_column = sqlx::query("SELECT 'agent' AS agent_id,NULL AS dominant_color")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(project_shell_agent(missing_column).is_err());
    }

    fn topic_key() -> TopicKey {
        TopicKey::new("agent", "agent-1", "topic-1")
    }

    fn message_key(message_id: &str) -> MessageKey {
        MessageKey::new(topic_key(), message_id)
    }

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .expect("baseline schema");
        sqlx::raw_sql(include_str!(
            "../../../migrations/0102_stream_stop_intent.sql"
        ))
        .execute(&pool)
        .await
        .expect("stream stop intent schema");
        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at)
             VALUES ('agent', 'agent-1', 'Agent', 'test', 1)",
        )
        .execute(&pool)
        .await
        .expect("agent fixture");
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at) \
             VALUES ('topic-1', 'agent', 'agent-1', 'Topic', 1, 1)",
        )
        .execute(&pool)
        .await
        .expect("topic fixture");
        pool
    }

    #[tokio::test]
    async fn edit_resend_rolls_back_truncation_when_the_message_write_fails() {
        let pool = test_pool().await;
        let app = tauri::test::mock_app();
        app.manage(DbState::new(pool.clone(), std::path::PathBuf::new()));
        let anchor = ChatMessage {
            id: "anchor".into(),
            role: "user".into(),
            content: "original".into(),
            timestamp: 1,
            ..Default::default()
        };
        let following = ChatMessage {
            id: "following".into(),
            role: "assistant".into(),
            content: "following reply".into(),
            timestamp: 2,
            ..Default::default()
        };
        for message in [anchor.clone(), following] {
            append_single_message(
                app.handle().clone(),
                &pool,
                "agent-1",
                "agent",
                "topic-1".into(),
                message,
            )
            .await
            .expect("seed history");
        }
        sqlx::raw_sql(
            "CREATE TRIGGER reject_edited_body BEFORE UPDATE OF content ON messages
             WHEN NEW.msg_id = 'anchor' AND NEW.content = 'edited'
             BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        )
        .execute(&pool)
        .await
        .expect("inject write failure");
        let edited = ChatMessage {
            content: "edited".into(),
            ..anchor
        };
        let db = app.state::<DbState>();
        let error = edit_resend_message(app.handle(), &db, &topic_key(), edited.clone())
            .await
            .err()
            .expect("the edit must fail after attempted truncation");
        assert!(error.contains("injected write failure"));
        let rows: Vec<(String, String, Option<i64>)> =
            sqlx::query_as("SELECT msg_id, content, deleted_at FROM messages ORDER BY timestamp")
                .fetch_all(&pool)
                .await
                .expect("history after rollback");
        assert_eq!(
            rows,
            vec![
                ("anchor".into(), "original".into(), None),
                ("following".into(), "following reply".into(), None),
            ]
        );
        sqlx::query("DROP TRIGGER reject_edited_body")
            .execute(&pool)
            .await
            .expect("remove injected failure");
        let (persisted, deletion) = edit_resend_message(app.handle(), &db, &topic_key(), edited)
            .await
            .expect("retry edit");
        assert!(!persisted.blocks.is_empty());
        assert_eq!(deletion.deleted_ids, vec!["following"]);
        assert_eq!(deletion.msg_count, 1);
        let visible: Vec<(String, String)> =
            sqlx::query_as("SELECT msg_id, content FROM messages WHERE deleted_at IS NULL")
                .fetch_all(&pool)
                .await
                .expect("committed history");
        assert_eq!(visible, vec![("anchor".into(), "edited".into())]);
    }

    #[tokio::test]
    async fn sync_message_tombstones_persist_missing_rows_and_remain_monotonic() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
                content_hash, created_at, updated_at
             ) VALUES (
                'agent', 'agent-1', 'topic-1', 'existing', 'assistant', 'body', 1,
                'live-hash', 1, 1
             );
             INSERT INTO active_generations (owner_type, owner_id, topic_id, msg_id, created_at)
             VALUES ('agent', 'agent-1', 'topic-1', 'existing', 1);
             INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
             VALUES ('existing', 'topic-1', 'body', 'agent', 'agent-1');",
        )
        .execute(&pool)
        .await
        .expect("seed live message");
        let db = DbState::new(pool.clone(), std::path::PathBuf::new());

        let active_ids = apply_sync_message_tombstones(
            &db,
            &topic_key(),
            &[("existing".to_string(), 40), ("missing".to_string(), 50)],
        )
        .await
        .expect("apply mixed tombstones");
        assert_eq!(active_ids, vec!["existing".to_string()]);

        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT msg_id, content, content_hash, deleted_at FROM messages
             WHERE topic_id = 'topic-1' ORDER BY msg_id",
        )
        .fetch_all(&pool)
        .await
        .expect("read message tombstones");
        assert_eq!(
            rows,
            vec![
                (
                    "existing".to_string(),
                    "body".to_string(),
                    SYNC_TOMBSTONE_HASH.to_string(),
                    40,
                ),
                (
                    "missing".to_string(),
                    String::new(),
                    SYNC_TOMBSTONE_HASH.to_string(),
                    50,
                ),
            ]
        );
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_generations WHERE topic_id = 'topic-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("read active generation count");
        let fts_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE topic_id = 'topic-1'")
                .fetch_one(&pool)
                .await
                .expect("read FTS count");
        assert_eq!(active_count, 0);
        assert_eq!(fts_count, 0);

        apply_sync_message_tombstones(
            &db,
            &topic_key(),
            &[("missing".to_string(), 45), ("existing".to_string(), 60)],
        )
        .await
        .expect("repeat tombstones");
        let repeated: Vec<(String, i64)> = sqlx::query_as(
            "SELECT msg_id, deleted_at FROM messages
             WHERE topic_id = 'topic-1' ORDER BY msg_id",
        )
        .fetch_all(&pool)
        .await
        .expect("read monotonic tombstones");
        assert_eq!(
            repeated,
            vec![("existing".to_string(), 60), ("missing".to_string(), 50)]
        );

        assert!(
            begin_stream_message(&db, &message_key("missing"), Some("agent-1"), Some("Agent"),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn deleting_attachment_updates_message_version_and_bubbles() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let hash_a = "a".repeat(64);
        let hash_b = "b".repeat(64);
        let old_hash = HashAggregator::compute_message_fingerprint(
            "message-with-attachments",
            "user",
            None,
            "hello",
            1,
            None,
            &[hash_a.clone(), hash_b.clone()],
        );
        sqlx::query(
            "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
                content_hash, created_at, updated_at
             ) VALUES ('agent', 'agent-1', 'topic-1', 'message-with-attachments',
                       'user', 'hello', 1, ?, 1, 10)",
        )
        .bind(&old_hash)
        .execute(&pool)
        .await
        .expect("message fixture");
        sqlx::query(
            "INSERT INTO message_attachments (
                owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                display_name, created_at
             ) VALUES
                ('agent', 'agent-1', 'topic-1', 'message-with-attachments', ?, 0, 'a', 1),
                ('agent', 'agent-1', 'topic-1', 'message-with-attachments', ?, 1, 'b', 1)",
        )
        .bind(&hash_a)
        .bind(&hash_b)
        .execute(&pool)
        .await
        .expect("attachment relation fixtures");
        let mut tx = pool.begin().await.expect("begin initial hash transaction");
        HashAggregator::bubble_from_topic(&mut tx, &topic_key())
            .await
            .expect("initialize aggregate hashes");
        tx.commit().await.expect("commit initial hashes");
        let before_roots: (String, String) = sqlx::query_as(
            "SELECT t.content_hash, a.content_hash FROM topics t \
             JOIN agents a ON a.agent_id = t.owner_id WHERE t.topic_id = 'topic-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("initial aggregate hashes");

        let topic_key = TopicKey::new("agent", "agent-1", "topic-1");
        delete_message_attachment_in_db(
            &db_state,
            &topic_key,
            "message-with-attachments",
            0,
            &hash_a,
            5,
        )
        .await
        .expect("delete attachment relation");

        let expected_hash = HashAggregator::compute_message_fingerprint(
            "message-with-attachments",
            "user",
            None,
            "hello",
            1,
            None,
            std::slice::from_ref(&hash_b),
        );
        let after_delete: (String, i64, String, String) = sqlx::query_as(
            "SELECT m.content_hash, m.updated_at, t.content_hash, a.content_hash \
             FROM messages m JOIN topics t ON t.topic_id = m.topic_id \
             JOIN agents a ON a.agent_id = t.owner_id \
             WHERE m.topic_id = 'topic-1' AND m.msg_id = 'message-with-attachments'",
        )
        .fetch_one(&pool)
        .await
        .expect("state after attachment deletion");
        assert_eq!(after_delete.0, expected_hash);
        assert_eq!(after_delete.1, 11);
        assert_ne!(after_delete.2, before_roots.0);
        assert_ne!(after_delete.3, before_roots.1);
        let remaining_hashes: Vec<String> = sqlx::query_scalar(
            "SELECT hash FROM message_attachments
             WHERE owner_type = 'agent' AND owner_id = 'agent-1'
               AND topic_id = 'topic-1' AND msg_id = 'message-with-attachments'",
        )
        .fetch_all(&pool)
        .await
        .expect("remaining attachment relations");
        assert_eq!(remaining_hashes, vec![hash_b]);
    }

    #[tokio::test]
    async fn pending_generation_can_only_finalize_once() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-1");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        let topic_updated_after_begin: i64 =
            sqlx::query_scalar("SELECT updated_at FROM topics WHERE topic_id = 'topic-1'")
                .fetch_one(&pool)
                .await
                .expect("topic time after begin");
        assert_eq!(topic_updated_after_begin, 1);

        assert!(
            begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"),)
                .await
                .is_err()
        );

        let skeleton: (i64, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT timestamp, name, agent_id FROM messages WHERE msg_id = 'message-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("skeleton identity");

        let committed = commit_stream_message(
            &db_state,
            &key,
            "terminal body",
            2,
            "completed",
        )
        .await
        .expect("finalize generation");
        let StreamCommit::Committed(
            _,
            terminal_timestamp,
            terminal_topic_updated_at,
            committed_name,
        ) = committed
        else {
            panic!("pending generation must commit");
        };
        assert_eq!(terminal_timestamp, skeleton.0 as u64);
        assert_eq!(terminal_topic_updated_at, 2);
        assert_eq!(committed_name.as_deref(), Some("Agent"));
        let topic_updated_after_finalize: i64 =
            sqlx::query_scalar("SELECT updated_at FROM topics WHERE topic_id = 'topic-1'")
                .fetch_one(&pool)
                .await
                .expect("topic time after finalize");
        assert_eq!(topic_updated_after_finalize, 1);

        let row: (String, Option<String>, i64, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT content, finish_reason, timestamp, name, agent_id \
             FROM messages WHERE msg_id = 'message-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("terminal message");
        assert_eq!(row.0, "terminal body");
        assert_eq!(row.1.as_deref(), Some("completed"));
        assert_eq!(row.2, skeleton.0);
        assert_eq!(row.3, skeleton.1);
        assert_eq!(row.4, skeleton.2);
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_generations WHERE msg_id = 'message-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("active count");
        assert_eq!(active, 0);
        let repeated = commit_stream_message(
            &db_state,
            &key,
            "late body",
            3,
            "completed",
        )
        .await
        .expect("late finalization is idempotent");
        let StreamCommit::AlreadyFinalized(message) = repeated else {
            panic!("late finalization must return the durable message");
        };
        assert_eq!(message.content, "terminal body");
        assert_eq!(message.finish_reason, "completed");
        assert_eq!(message.timestamp, skeleton.0 as u64);
        assert_eq!(message.topic_updated_at, Some(terminal_topic_updated_at));

        let app = tauri::test::mock_app();
        app.manage(db_state);
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let channel = Channel::new(move |body| {
            let _ = events.send(body.deserialize::<serde_json::Value>()?);
            Ok(())
        });
        finalize_stream_message(
            app.handle().clone(),
            &pool,
            &key,
            "late partial".to_string(),
            true,
            None,
            Some(channel),
            Some("agent-1".to_string()),
        )
        .await
        .expect("late consumer receives the durable result");
        let event = received.try_recv().expect("durable end event");
        assert_eq!(event["type"], "end");
        assert_eq!(event["context"]["replayed"], true);
        assert_eq!(event["content"], "terminal body");
        assert_eq!(event["finishReason"], "completed");
        assert_eq!(event["timestamp"], skeleton.0);
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn begin_generation_cannot_cross_a_topic_tombstone() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        sqlx::query("UPDATE topics SET deleted_at = 7 WHERE topic_id = 'topic-1'")
            .execute(&pool)
            .await
            .expect("tombstone topic");

        begin_stream_message(
            &db_state,
            &message_key("message-after-delete"),
            Some("agent-1"),
            Some("Agent"),
        )
        .await
        .expect_err("atomic skeleton insert must observe the tombstone");
        let message_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages WHERE msg_id = 'message-after-delete'",
        )
        .fetch_one(&pool)
        .await
        .expect("message count");
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_generations WHERE msg_id = 'message-after-delete'",
        )
        .fetch_one(&pool)
        .await
        .expect("active count");
        assert_eq!(message_count, 0);
        assert_eq!(active_count, 0);
    }

    #[tokio::test]
    async fn finalization_failure_rolls_back_and_keeps_recovery_record() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-2");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        sqlx::query("DELETE FROM render_cache WHERE msg_id = 'message-2'")
            .execute(&pool)
            .await
            .expect("remove cache fixture");

        assert!(commit_stream_message(
            &db_state,
            &key,
            "must roll back",
            2,
            "completed",
        )
        .await
        .is_err());

        let finish_reason: Option<String> =
            sqlx::query_scalar("SELECT finish_reason FROM messages WHERE msg_id = 'message-2'")
                .fetch_one(&pool)
                .await
                .expect("pending message");
        assert!(finish_reason.is_none());
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_generations WHERE msg_id = 'message-2'",
        )
        .fetch_one(&pool)
        .await
        .expect("active count");
        assert_eq!(active, 1);
    }

    #[tokio::test]
    async fn tombstoned_generation_discards_late_finalization() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-deleted");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");

        delete_messages(
            &db_state,
            &topic_key(),
            vec!["message-deleted".to_string()],
            None,
        )
        .await
        .expect("delete generation");
        let app = tauri::test::mock_app();
        app.manage(db_state);
        let events = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let event_count = events.clone();
        let channel = Channel::new(move |_| {
            event_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        });
        let late = finalize_stream_message(
            app.handle().clone(),
            &pool,
            &key,
            "late terminal body".to_string(),
            false,
            Some("completed".to_string()),
            Some(channel),
            Some("agent-1".to_string()),
        )
        .await
        .expect("deleted generation is settled without an error");
        assert!(late.is_none());
        assert_eq!(events.load(std::sync::atomic::Ordering::Relaxed), 0);

        let deleted_at: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM messages WHERE msg_id = 'message-deleted'")
                .fetch_one(&pool)
                .await
                .expect("deleted message");
        assert!(deleted_at.is_some());
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM active_generations WHERE msg_id = 'message-deleted'",
        )
        .fetch_one(&pool)
        .await
        .expect("active count");
        assert_eq!(active, 0);
    }

    #[tokio::test]
    async fn stopped_generation_preserves_body_and_persists_cancelled_reason() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-stopped");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        let app = tauri::test::mock_app();
        app.manage(db_state);

        let message = finalize_stream_message(
            app.handle().clone(),
            &pool,
            &key,
            "partial reply".to_string(),
            true,
            None,
            None,
            Some("agent-1".to_string()),
        )
        .await
        .expect("commit stopped generation")
        .expect("stopped message remains visible");
        assert_eq!(message.content, "partial reply");
        assert_eq!(message.finish_reason, "cancelled_by_user");
        let persisted: (String, String) = sqlx::query_as(
            "SELECT content, finish_reason FROM messages WHERE msg_id = 'message-stopped'",
        )
        .fetch_one(&pool)
        .await
        .expect("durable stopped message");
        assert_eq!(
            persisted,
            ("partial reply".to_string(), "cancelled_by_user".to_string())
        );
    }

    #[tokio::test]
    async fn live_unfinished_message_without_generation_remains_an_error() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-unowned");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        sqlx::query("DELETE FROM active_generations WHERE msg_id = 'message-unowned'")
            .execute(&pool)
            .await
            .expect("simulate inconsistent pending state");
        assert!(commit_stream_message(
            &db_state,
            &key,
            "unowned body",
            999,
            "completed",
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn jev_cache_recompilation_replaces_old_tool_projection_without_editing_message() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("jev-cache");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        let raw = "<<<[TOOL_REQUEST]>>>\nJEV:「始」使用 'B站获取'「末」\n<<<[END_TOOL_REQUEST]>>>";
        let hash = crate::vcp_modules::sync_hash::HashAggregator::compute_content_hash(raw);
        sqlx::query("UPDATE messages SET content = ?, content_hash = ? WHERE msg_id = 'jev-cache'")
            .bind(raw)
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();
        let old_blocks = vec![ContentBlock::tool_use(
            "Processing...".to_string(),
            "\nJEV:「始」使用 'B站获取'「末」\n".to_string(),
            true,
        )];
        let old_cache = serialize_render_async(old_blocks).await.unwrap();
        assert!(
            decode_valid_render_cache(Some(old_cache), Some(hash.clone()), Some(2), &hash)
                .await
                .is_none()
        );

        let (_, new_cache) = compile_and_serialize_render_async(raw.to_string())
            .await
            .unwrap();
        assert!(write_render_cache_cas(&db_state, &key, &hash, &new_cache)
            .await
            .unwrap());
        let row = sqlx::query("SELECT m.content, m.content_hash, r.render_content, r.renderer_schema_version \
            FROM messages m JOIN render_cache r USING (owner_type, owner_id, topic_id, msg_id) WHERE m.msg_id = 'jev-cache'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String, _>("content"), raw);
        assert_eq!(row.get::<String, _>("content_hash"), hash);
        let decoded = decode_valid_render_cache(
            Some(row.get("render_content")),
            Some(hash.clone()),
            Some(row.get("renderer_schema_version")),
            &hash,
        )
        .await
        .unwrap();
        assert_eq!(decoded[0]["type"], "jev-tool-use");
        assert_eq!(decoded[0]["tool_name"], "B站获取");
        assert_eq!(decoded[0]["content"], "\nJEV:「始」使用 'B站获取'「末」\n");
    }

    #[tokio::test]
    async fn render_cache_rejects_stale_hash_and_invalid_identity() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-cache");
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        let old_hash: String =
            sqlx::query_scalar("SELECT content_hash FROM messages WHERE msg_id = 'message-cache'")
                .fetch_one(&pool)
                .await
                .expect("old hash");
        let old_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT render_content FROM render_cache WHERE msg_id = 'message-cache'",
        )
        .fetch_one(&pool)
        .await
        .expect("old cache");

        sqlx::query(
            "UPDATE messages SET content = 'new', content_hash = 'new-hash' \
             WHERE msg_id = 'message-cache'",
        )
        .execute(&pool)
        .await
        .expect("concurrent edit");
        let (_, stale_bytes) = compile_and_serialize_render_async("stale".to_string())
            .await
            .expect("compile stale");
        assert!(
            !write_render_cache_cas(&db_state, &key, &old_hash, &stale_bytes,)
                .await
                .expect("cache CAS")
        );
        let after_bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT render_content FROM render_cache WHERE msg_id = 'message-cache'",
        )
        .fetch_one(&pool)
        .await
        .expect("cache after CAS");
        assert_eq!(after_bytes, old_bytes);

        assert!(decode_valid_render_cache(
            Some(old_bytes.clone()),
            Some(old_hash.clone()),
            Some(RENDERER_SCHEMA_VERSION),
            &old_hash,
        )
        .await
        .is_some());
        assert!(decode_valid_render_cache(
            Some(old_bytes.clone()),
            Some(old_hash.clone()),
            Some(RENDERER_SCHEMA_VERSION + 1),
            &old_hash,
        )
        .await
        .is_none());
        assert!(decode_valid_render_cache(
            Some(vec![1, 2, 3]),
            Some(old_hash.clone()),
            Some(RENDERER_SCHEMA_VERSION),
            &old_hash,
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn finalizing_after_agent_rename_preserves_message_name_and_fingerprint() {
        let pool = test_pool().await;
        let db_state = DbState::new(pool.clone(), std::path::PathBuf::new());
        let key = message_key("message-rename");

        // 1. 开始生成流：存入初始旧名 "OldAgent"
        begin_stream_message(&db_state, &key, Some("agent-1"), Some("OldAgent"))
            .await
            .expect("begin generation");

        // 2. 模拟生成期间助手被改名：在 agents 表中更新或插入新名字 "NewAgent"
        sqlx::query(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at) \
             VALUES ('agent', 'agent-1', 'NewAgent', 'test-model', 1) \
             ON CONFLICT(owner_type, agent_id) DO UPDATE SET name = 'NewAgent'",
        )
        .execute(&pool)
        .await
        .expect("update agent name to NewAgent");

        // 3. 终态结算
        let app = tauri::test::mock_app();
        app.manage(db_state.clone());
        let finalized = finalize_stream_message(
            app.handle().clone(),
            &pool,
            &key,
            "Final content".to_string(),
            false,
            Some("completed".to_string()),
            None,
            Some("agent-1".to_string()),
        )
        .await
        .expect("finalize stream message");

        assert!(finalized.is_some());

        // 4. 验证数据库中持久化的消息记录：name 保持为流开始时的 "OldAgent"
        let (name, hash, content, timestamp, agent_id): (
            Option<String>,
            String,
            String,
            i64,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT name, content_hash, content, timestamp, agent_id \
             FROM messages WHERE msg_id = 'message-rename'",
        )
        .fetch_one(&pool)
        .await
        .expect("fetch finalized message");

        assert_eq!(name.as_deref(), Some("OldAgent"));

        // 5. 核心断言：content_hash 必须与消息自身 name ("OldAgent") 完全自洽，消除指纹漂移
        let expected_hash = HashAggregator::compute_message_fingerprint(
            "message-rename",
            "assistant",
            Some("OldAgent"),
            &content,
            timestamp as u64,
            agent_id.as_deref(),
            &[],
        );
        assert_eq!(hash, expected_hash);
    }
}
