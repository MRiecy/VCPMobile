use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::file_manager::resolve_attachment_cas_file;
use crate::vcp_modules::message_repository::{
    compile_and_serialize_render_async, deserialize_render_async, resolve_message_updated_at,
    serialize_render_async, write_render_cache_cas, MessageRepository, RENDERER_SCHEMA_VERSION,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::Row;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

// =================================================================
// vcp_modules/message_service.rs - 消息业务逻辑中心 (含附件对齐)
// =================================================================

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

    let query_str = if limit.is_some() && before_timestamp.is_some() {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, r.content_hash AS cache_content_hash, r.renderer_schema_version, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
           AND (m.timestamp < ? OR (m.timestamp = ? AND m.msg_id < ?))
         ORDER BY m.timestamp DESC, m.msg_id DESC
         LIMIT ?"
    } else if limit.is_some() {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, r.content_hash AS cache_content_hash, r.renderer_schema_version, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
         ORDER BY m.timestamp DESC, m.msg_id DESC
         LIMIT ?"
    } else {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, r.content_hash AS cache_content_hash, r.renderer_schema_version, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
         ORDER BY m.timestamp DESC, m.msg_id DESC"
    };

    let mut q = sqlx::query(query_str)
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
    // 收集所有 msg_id，用于批量查询附件
    let mut msg_ids = Vec::new();
    for row in &rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        msg_ids.push(msg_id);
    }

    // 批量查询所有附件（利用 message_attachments 索引表）
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
        let mut q = sqlx::query(&att_query)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let att_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for ar in att_rows {
            let msg_id: String = ar.get("msg_id");
            let hash: String = ar.get("hash");
            let mime_type: String = ar.get("mime_type");
            let internal_path: String = ar.get("internal_path");
            let display_name: String = ar.get("display_name");
            let size_i64: i64 = ar.get("size");
            let created_at_i64: i64 = ar.get("created_at");
            let mut extracted_text: Option<String> = ar.get("extracted_text");

            // ⚡ 极度优雅的消息-附件解耦调用：将物理文件判定、异步持久化完全委托给 file_manager
            if include_extracted_text && extracted_text.is_none() {
                extracted_text = crate::vcp_modules::infra::file_manager::ensure_extracted_text(
                    pool,
                    &hash,
                    &internal_path,
                    &mime_type,
                )
                .await;
            }

            att_map.entry(msg_id).or_default().push(Attachment {
                r#type: mime_type,
                src: ar.get("src"),
                name: display_name,
                size: size_i64 as u64,
                hash: Some(hash),
                status: Some(ar.get("status")),
                attachment_order: Some(ar.get("attachment_order")),
                internal_path,
                extracted_text,
                image_frames: ar
                    .get::<Option<String>, _>("image_frames")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                thumbnail_path: ar.get("thumbnail_path"),
                created_at: Some(created_at_i64 as u64),
            });
        }
    }

    // 预计算外壳属性所需的全局数据（避免调用 get_agents 触发昂贵的多余 topics 联表查询）
    let agents = match sqlx::query(
        "SELECT a.agent_id, a.name, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.owner_type = 'agent' AND a.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                use sqlx::Row;
                crate::vcp_modules::agent_types::AgentConfig {
                    id: row.get("agent_id"),
                    name: row.get("name"),
                    avatar_calculated_color: row.get("dominant_color"),
                    system_prompt: String::new(),
                    mobile_system_prompt: String::new(),
                    model: String::new(),
                    temperature: 0.0,
                    context_token_limit: 0,
                    max_output_tokens: 0,
                    stream_output: false,
                    use_temperature: false,
                    topics: vec![],
                }
            })
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };

    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), app_handle.state())
            .await
            .ok();
    let user_name = settings
        .map(|s| s.user_name)
        .unwrap_or_else(|| "User".to_string());

    let user_avatar_color: Option<String> = sqlx::query_scalar(
        "SELECT dominant_color FROM avatars
         WHERE owner_type = 'user' AND owner_id = 'user_avatar' AND deleted_at IS NULL",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

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

                let pool_c = pool.clone();
                let cache_key = MessageKey::new(key.clone(), msg_id.clone());
                let observed_hash = content_hash_raw.clone();
                tokio::spawn(async move {
                    let _ =
                        write_render_cache_cas(&pool_c, &cache_key, &observed_hash, &serialized)
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
            updated_at: None,
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

    const ROW_SELECT: &str =
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) as name, m.agent_id, m.content, m.timestamp, m.is_group_message, m.group_id, m.finish_reason, r.render_content, r.content_hash AS cache_content_hash, r.renderer_schema_version, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON a.owner_type = 'agent' AND m.agent_id = a.agent_id";

    let mut rows: Vec<sqlx::sqlite::SqliteRow> = Vec::new();

    // 前向窗口（早于锚点，含锚点本身）
    let before_rows = sqlx::query(&format!(
        "{} WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
           AND (m.timestamp < ? OR (m.timestamp = ? AND m.msg_id <= ?))
         ORDER BY m.timestamp DESC, m.msg_id DESC LIMIT ?",
        ROW_SELECT
    ))
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
        let after_rows = sqlx::query(&format!(
            "{} WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.deleted_at IS NULL
               AND (m.timestamp > ? OR (m.timestamp = ? AND m.msg_id > ?))
             ORDER BY m.timestamp ASC, m.msg_id ASC LIMIT ?",
            ROW_SELECT
        ))
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

/// 为 Agent 和 Group 组装大模型上下文提供专用的轻量历史查询。
/// 只查询消息纯文本和附件（在需要时提取文本），完全跳过 render_content 反序列化和 UI shell 预计算。
pub async fn load_chat_text_history_for_context(
    app_handle: &AppHandle,
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
        let mut q = sqlx::query(&att_query)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in &msg_ids {
            q = q.bind(id);
        }
        let att_rows = q.fetch_all(pool).await.map_err(|e| e.to_string())?;

        for ar in att_rows {
            let msg_id: String = ar.get("msg_id");
            let hash: String = ar.get("hash");
            let mime_type: String = ar.get("mime_type");
            let internal_path: String = ar.get("internal_path");
            let display_name: String = ar.get("display_name");
            let size_i64: i64 = ar.get("size");
            let created_at_i64: i64 = ar.get("created_at");
            let mut extracted_text: Option<String> = ar.get("extracted_text");

            if include_extracted_text && extracted_text.is_none() {
                extracted_text = crate::vcp_modules::infra::file_manager::ensure_extracted_text(
                    pool,
                    &hash,
                    &internal_path,
                    &mime_type,
                )
                .await;
            }

            att_map.entry(msg_id).or_default().push(Attachment {
                r#type: mime_type,
                src: ar.get("src"),
                name: display_name,
                size: size_i64 as u64,
                hash: Some(hash),
                status: Some(ar.get("status")),
                attachment_order: Some(ar.get("attachment_order")),
                internal_path,
                extracted_text,
                image_frames: ar
                    .get::<Option<String>, _>("image_frames")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                thumbnail_path: ar.get("thumbnail_path"),
                created_at: Some(created_at_i64 as u64),
            });
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
async fn normalize_attachments_from_local_cas<R: tauri::Runtime>(
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
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
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
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;

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
) -> Result<Vec<ContentBlock>, String> {
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
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &key, &render_bytes, false).await?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(blocks)
}

#[tauri::command]
pub async fn fetch_raw_message_content(
    app_handle: tauri::AppHandle,
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
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
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
            if !write_render_cache_cas(pool, &key, &observed_hash, &serialized).await? {
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
) -> Result<Vec<ContentBlock>, String> {
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
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    MessageRepository::upsert_message(&mut tx, &message, &key, &render_bytes, skip_bubble).await?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(blocks)
}

pub struct MessageDeletionResult {
    pub deleted_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub deleted_at: i64,
    pub msg_count: i32,
}

pub async fn delete_messages(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    msg_ids: Vec<String>,
    deleted_at: Option<i64>,
) -> Result<MessageDeletionResult, String> {
    if msg_ids.is_empty() {
        let msg_count: i32 = sqlx::query_scalar(
            "SELECT msg_count FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(db_pool)
        .await
        .map_err(|e| e.to_string())?;
        return Ok(MessageDeletionResult {
            deleted_ids: Vec::new(),
            active_ids: Vec::new(),
            deleted_at: deleted_at.unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
            msg_count,
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
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
    let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let select_deleted_ids = format!(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({placeholders})"
    );
    let mut deleted_query = sqlx::query_scalar(&select_deleted_ids)
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
    let mut active_query = sqlx::query_scalar(&select_active_ids)
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
    let mut q = sqlx::query(&delete_query)
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
    let mut q_cache = sqlx::query(&delete_cache_query)
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
    let mut q_attachments = sqlx::query(&delete_attachments_query)
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
    let mut q_active = sqlx::query(&delete_active_gen_query)
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
    let msg_count = HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(MessageDeletionResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count,
    })
}

/// Applies Desktop message tombstones as one Topic-local durable mutation. Aggregate count/root
/// repair is intentionally owned by SyncFinalizer so a batch of tombstones does not rescan the
/// same Topic once per message.
pub(crate) async fn apply_sync_message_tombstones(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    tombstones: &[(String, i64)],
) -> Result<Vec<String>, String> {
    const MAX_SAFE_JSON_INTEGER: i64 = (1_i64 << 53) - 1;
    if tombstones.is_empty() {
        return Ok(Vec::new());
    }
    if tombstones.len() > 10_000
        || tombstones.iter().any(|(id, deleted_at)| {
            id.is_empty() || !(0..=MAX_SAFE_JSON_INTEGER).contains(deleted_at)
        })
        || tombstones
            .iter()
            .map(|(id, _)| id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != tombstones.len()
    {
        return Err(
            "Sync message tombstones require 1..=10000 unique ids and safe deletedAt values"
                .to_string(),
        );
    }

    let mut tx = db_pool.begin().await.map_err(|error| error.to_string())?;
    let placeholders = vec!["?"; tombstones.len()].join(", ");
    let live_sql = format!(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({placeholders})"
    );
    let mut live_query = sqlx::query_scalar(&live_sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for (id, _) in tombstones {
        live_query = live_query.bind(id);
    }
    let deleted_ids: Vec<String> = live_query
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    if deleted_ids.is_empty() {
        tx.commit().await.map_err(|error| error.to_string())?;
        return Ok(Vec::new());
    }

    let active_sql = format!(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({})",
        vec!["?"; deleted_ids.len()].join(", ")
    );
    let mut active_query = sqlx::query_scalar(&active_sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in &deleted_ids {
        active_query = active_query.bind(id);
    }
    let active_ids = active_query
        .fetch_all(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;

    let tombstone_times = tombstones
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    let values = deleted_ids
        .iter()
        .map(|_| "(?, ?)")
        .collect::<Vec<_>>()
        .join(", ");
    let update_sql = format!(
        "WITH incoming(msg_id, deleted_at) AS (VALUES {values})
         UPDATE messages
         SET deleted_at = (
             SELECT incoming.deleted_at FROM incoming WHERE incoming.msg_id = messages.msg_id
         )
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
           AND msg_id IN (SELECT msg_id FROM incoming)"
    );
    let mut update = sqlx::query(&update_sql);
    for id in &deleted_ids {
        update = update.bind(id).bind(
            tombstone_times
                .get(id)
                .copied()
                .ok_or_else(|| format!("Missing Desktop tombstone time for message {id}"))?,
        );
    }
    let updated = update
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    if updated.rows_affected() != deleted_ids.len() as u64 {
        return Err(format!(
            "Sync message tombstones changed {} rows, expected {} for topic {}",
            updated.rows_affected(),
            deleted_ids.len(),
            key.topic_id
        ));
    }

    for table in ["render_cache", "message_attachments", "active_generations"] {
        let delete_sql = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({})",
            vec!["?"; deleted_ids.len()].join(", ")
        );
        let mut delete = sqlx::query(&delete_sql)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in &deleted_ids {
            delete = delete.bind(id);
        }
        delete
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
    }

    // FTS is removed by after_messages_logical_delete in this same transaction.
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(active_ids)
}

pub async fn truncate_history_after_message(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    anchor_message_id: &str,
) -> Result<MessageDeletionResult, String> {
    let mut tx = db_pool.begin().await.map_err(|e| e.to_string())?;
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
    let msg_count = HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(MessageDeletionResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count,
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

pub async fn finalize_stream_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
) -> Result<(), String> {
    finalize_stream_message_inner(
        app_handle,
        pool,
        message_key,
        full_content,
        is_aborted,
        finish_reason,
        stream_channel,
        agent_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finalize_stream_message_inner<R: tauri::Runtime>(
    _app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
) -> Result<(), String> {
    let owner_id = message_key.topic.owner_id.as_str();
    let owner_type = message_key.topic.owner_type.as_str();
    let topic_id = message_key.topic.topic_id.as_str();
    let message_id = message_key.msg_id.as_str();
    let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;

    let mut final_content = full_content;
    if is_aborted {
        final_content.push_str("\n\n> VCP流式错误: 请求已中止");
    }

    let is_group = owner_type == "group";

    let final_agent_id = if is_group {
        agent_id
    } else {
        Some(owner_id.to_string())
    };

    let mut agent_name = None;
    if let Some(ref aid) = final_agent_id {
        if let Ok(Some(row)) =
            sqlx::query("SELECT name FROM agents WHERE owner_type = 'agent' AND agent_id = ?")
                .bind(aid)
                .fetch_optional(pool)
                .await
        {
            use sqlx::Row;
            agent_name = Some(row.get::<String, _>("name"));
        }
    }

    let terminal_reason = finish_reason.unwrap_or_else(|| "completed".to_string());
    let context = if owner_id.is_empty() || topic_id.is_empty() {
        None
    } else if is_group {
        Some(serde_json::json!({
            "ownerId": owner_id,
            "ownerType": owner_type,
            "groupId": owner_id,
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
    let (end_blocks, end_timestamp) = if owner_id.is_empty() || topic_id.is_empty() {
        (None, final_ts)
    } else {
        match commit_stream_message(
            pool,
            message_key,
            &final_content,
            final_ts,
            &terminal_reason,
            final_agent_id.as_deref(),
            agent_name.as_deref(),
        )
        .await
        {
            Ok((blocks, start_timestamp)) => (Some(blocks), start_timestamp),
            Err(error) => {
                if let Some(chan) = &stream_channel {
                    let event = crate::vcp_modules::vcp_client::StreamEvent::error(
                        message_id.to_string(),
                        context.clone(),
                        format!("终态保存失败: {}", error),
                    );
                    let _ = chan.send(event);
                }
                return Err(error);
            }
        }
    };

    if let Some(chan) = stream_channel {
        let event = crate::vcp_modules::vcp_client::StreamEvent::end(
            message_id.to_string(),
            context,
            Some(terminal_reason),
            end_blocks,
            Some(end_timestamp),
        );
        let _ = chan.send(event);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn commit_stream_message(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_key: &MessageKey,
    final_content: &str,
    final_ts: u64,
    finish_reason: &str,
    agent_id: Option<&str>,
    agent_name: Option<&str>,
) -> Result<(Vec<ContentBlock>, u64), String> {
    let key = &message_key.topic;
    let owner_id = key.owner_id.as_str();
    let owner_type = key.owner_type.as_str();
    let topic_id = key.topic_id.as_str();
    let message_id = message_key.msg_id.as_str();
    let (blocks, render_bytes) =
        compile_and_serialize_render_async(final_content.to_string()).await?;
    let is_group = owner_type == "group";
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    let start_timestamp: i64 = sqlx::query_scalar(
        "SELECT m.timestamp FROM messages m
         JOIN active_generations ag
           ON ag.owner_type = m.owner_type AND ag.owner_id = m.owner_id
          AND ag.topic_id = m.topic_id AND ag.msg_id = m.msg_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ? AND m.msg_id = ?
           AND m.finish_reason IS NULL AND m.deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| {
        format!(
            "Generation {} is not pending for {} {} topic {}",
            message_id, owner_type, owner_id, topic_id
        )
    })?;
    let fingerprint_timestamp = u64::try_from(start_timestamp)
        .map_err(|_| format!("Message {message_id} has a negative timestamp"))?;
    let content_hash = HashAggregator::compute_message_fingerprint(
        message_id,
        "assistant",
        agent_name,
        final_content,
        fingerprint_timestamp,
        agent_id,
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

    HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok((blocks, start_timestamp as u64))
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
) -> Result<(), String> {
    use crate::vcp_modules::db_manager::DbState;
    use tauri::Manager;
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    delete_message_attachment_in_pool(pool, &key.topic, &key.msg_id, attachment_order, &hash, now)
        .await
}

async fn delete_message_attachment_in_pool(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    message_id: &str,
    attachment_order: i32,
    hash: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
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
        tx.commit().await.map_err(|e| e.to_string())?;
        return Ok(());
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

    HashAggregator::bubble_from_topic(&mut tx, key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod stream_lifecycle_tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

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
    async fn deleting_attachment_updates_message_version_and_bubbles() {
        let pool = test_pool().await;
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
        delete_message_attachment_in_pool(
            &pool,
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
        let key = message_key("message-1");
        begin_stream_message(&pool, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        let topic_updated_after_begin: i64 =
            sqlx::query_scalar("SELECT updated_at FROM topics WHERE topic_id = 'topic-1'")
                .fetch_one(&pool)
                .await
                .expect("topic time after begin");
        assert_eq!(topic_updated_after_begin, 1);

        assert!(
            begin_stream_message(&pool, &key, Some("agent-1"), Some("Agent"),)
                .await
                .is_err()
        );

        let skeleton: (i64, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT timestamp, name, agent_id FROM messages WHERE msg_id = 'message-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("skeleton identity");

        let (_, terminal_timestamp) = commit_stream_message(
            &pool,
            &key,
            "terminal body",
            2,
            "completed",
            Some("agent-1"),
            Some("Agent"),
        )
        .await
        .expect("finalize generation");
        assert_eq!(terminal_timestamp, skeleton.0 as u64);
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
        assert!(commit_stream_message(
            &pool,
            &key,
            "late body",
            3,
            "completed",
            Some("agent-1"),
            Some("Agent"),
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn begin_generation_cannot_cross_a_topic_tombstone() {
        let pool = test_pool().await;
        sqlx::query("UPDATE topics SET deleted_at = 7 WHERE topic_id = 'topic-1'")
            .execute(&pool)
            .await
            .expect("tombstone topic");

        let error = begin_stream_message(
            &pool,
            &message_key("message-after-delete"),
            Some("agent-1"),
            Some("Agent"),
        )
        .await
        .expect_err("atomic skeleton insert must observe the tombstone");
        assert!(error.contains("live agent"));
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
        let key = message_key("message-2");
        begin_stream_message(&pool, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");
        sqlx::query("DELETE FROM render_cache WHERE msg_id = 'message-2'")
            .execute(&pool)
            .await
            .expect("remove cache fixture");

        assert!(commit_stream_message(
            &pool,
            &key,
            "must roll back",
            2,
            "completed",
            Some("agent-1"),
            Some("Agent"),
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
    async fn tombstoned_generation_rejects_late_finalization() {
        let pool = test_pool().await;
        let key = message_key("message-deleted");
        begin_stream_message(&pool, &key, Some("agent-1"), Some("Agent"))
            .await
            .expect("begin generation");

        delete_messages(
            &pool,
            &topic_key(),
            vec!["message-deleted".to_string()],
            None,
        )
        .await
        .expect("delete generation");
        assert!(commit_stream_message(
            &pool,
            &key,
            "late terminal body",
            999,
            "completed",
            Some("agent-1"),
            Some("Agent"),
        )
        .await
        .is_err());

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
    async fn render_cache_rejects_stale_hash_and_invalid_identity() {
        let pool = test_pool().await;
        let key = message_key("message-cache");
        begin_stream_message(&pool, &key, Some("agent-1"), Some("Agent"))
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
            !write_render_cache_cas(&pool, &key, &old_hash, &stale_bytes,)
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
}
