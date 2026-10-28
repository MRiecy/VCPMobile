use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::TopicKey;
use chrono::{Local, TimeZone};
use log::warn;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Pool, Row, Sqlite};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TarvenRule {
    pub id: String,
    pub name: String,
    pub rule_type: String, // 'system_suffix' | 'user_suffix' | 'context_inject'
    pub is_enabled: bool,
    pub content: String,
    pub scope: String, // 'global' | 'agent' | 'group'
    pub wrap: bool,

    // context_inject专用
    pub role: Option<String>, // 'user' | 'assistant'
    pub depth: Option<i32>,

    // system_suffix / user_suffix 专用
    pub position: Option<String>, // 'prepend' | 'append'

    pub sort_order: i32,
}

impl Default for TarvenRule {
    fn default() -> Self {
        Self {
            id: String::new(), name: String::new(), rule_type: "system_suffix".into(),
            is_enabled: true, content: String::new(), scope: "global".into(), wrap: true,
            role: Some("user".into()), depth: Some(0), position: Some("append".into()), sort_order: 0,
        }
    }
}

impl TarvenRule {
    fn defaults_for_id(id: &str) -> Self {
        let mut rule = Self { id: id.into(), ..Self::default() };
        let preset = match id {
            "system_meta_injection" => Some(("系统元数据注入", true,
                "包含当前系统时间、运行环境及话题创建时间元数据注入系统提示词。")),
            "time_anchoring_v2" => Some(("消息时间线感知 V2", false,
                "为上下文中每条消息注入伪系统发送时间戳，使大模型具备精确的时间线感知，防止其对物理时间产生幻觉。")),
            _ => None,
        };
        if let Some((name, enabled, content)) = preset {
            rule.name = name.into();
            rule.rule_type = id.into();
            rule.is_enabled = enabled;
            rule.content = content.into();
            rule.wrap = false;
            rule.role = None;
            rule.depth = None;
            rule.position = None;
            rule.sort_order = -100;
        }
        rule
    }

    fn normalize(&mut self) {
        let defaults = Self::defaults_for_id(&self.id);
        if !matches!(self.rule_type.as_str(), "system_suffix" | "user_suffix" | "context_inject" | "system_meta_injection" | "time_anchoring_v2") {
            self.rule_type = defaults.rule_type;
        }
        if !matches!(self.scope.as_str(), "global" | "agent" | "group") { self.scope = defaults.scope; }
        if !matches!(self.role.as_deref(), None | Some("user" | "assistant")) { self.role = defaults.role; }
        if !matches!(self.position.as_deref(), None | Some("prepend" | "append")) { self.position = defaults.position; }
        if self.depth.is_some_and(|depth| depth < 0) { self.depth = defaults.depth; }
    }
}

fn stored_rule_field<T>(row: &SqliteRow, key: &str, default: T, changed: &mut bool) -> Result<T, String>
where T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite> {
    match row.try_get(key) {
        Ok(value) => Ok(value),
        Err(sqlx::Error::ColumnDecode { .. }) => { *changed = true; Ok(default) }
        Err(error) => Err(error.to_string()),
    }
}

fn decode_rule(row: &SqliteRow) -> Result<(TarvenRule, bool), String> {
    let id: String = row.try_get("id").map_err(|error| error.to_string())?;
    if id.is_empty() { return Err("注入规则缺少标识，无法恢复配置".into()); }
    let defaults = TarvenRule::defaults_for_id(&id);
    let mut changed = false;
    let mut boolean = |key, default| -> Result<bool, String> {
        let value: i64 = stored_rule_field(row, key, i64::from(default), &mut changed)?;
        match value { 0 => Ok(false), 1 => Ok(true), _ => { changed = true; Ok(default) } }
    };
    let is_enabled = boolean("is_enabled", defaults.is_enabled)?;
    let wrap = boolean("wrap", defaults.wrap)?;
    let mut rule = TarvenRule {
        id, is_enabled, wrap,
        name: stored_rule_field(row, "name", defaults.name, &mut changed)?,
        rule_type: stored_rule_field(row, "rule_type", defaults.rule_type, &mut changed)?,
        content: stored_rule_field(row, "content", defaults.content, &mut changed)?,
        scope: stored_rule_field(row, "scope", defaults.scope, &mut changed)?,
        role: stored_rule_field(row, "role", defaults.role, &mut changed)?,
        depth: stored_rule_field(row, "depth", defaults.depth, &mut changed)?,
        position: stored_rule_field(row, "position", defaults.position, &mut changed)?,
        sort_order: stored_rule_field(row, "sort_order", defaults.sort_order, &mut changed)?,
    };
    let before = rule.clone();
    rule.normalize();
    changed |= rule != before;
    Ok((rule, changed))
}

const RULE_QUERY: &str = "SELECT id,name,rule_type,is_enabled,content,scope,wrap,role,depth,position,sort_order FROM tarven_rules ORDER BY sort_order ASC";

async fn read_tarven_rules(db: &DbState) -> Result<Vec<TarvenRule>, String> {
    let rows = sqlx::query(RULE_QUERY).fetch_all(&db.pool).await.map_err(|error| error.to_string())?;
    let decoded = rows.iter().map(decode_rule).collect::<Result<Vec<_>, _>>()?;
    if !decoded.iter().any(|(_, changed)| *changed) {
        return Ok(decoded.into_iter().map(|(rule, _)| rule).collect());
    }
    let db = db.clone();
    tokio::spawn(async move {
        let mut tx = db.write_transaction("context.rule.normalize").await?;
        let rows = sqlx::query(RULE_QUERY).fetch_all(&mut *tx).await.map_err(|error| error.to_string())?;
        let mut rules = Vec::with_capacity(rows.len());
        for row in rows {
            let (rule, changed) = decode_rule(&row)?;
            if changed {
                sqlx::query("UPDATE tarven_rules SET name=?,rule_type=?,is_enabled=?,content=?,scope=?,wrap=?,role=?,depth=?,position=?,sort_order=? WHERE id=?")
                    .bind(&rule.name).bind(&rule.rule_type).bind(rule.is_enabled).bind(&rule.content)
                    .bind(&rule.scope).bind(rule.wrap).bind(&rule.role).bind(rule.depth).bind(&rule.position)
                    .bind(rule.sort_order).bind(&rule.id).execute(&mut *tx).await.map_err(|error| error.to_string())?;
            }
            rules.push(rule);
        }
        tx.commit().await.map_err(|error| error.to_string())?;
        rules.sort_by_key(|rule| rule.sort_order);
        Ok(rules)
    }).await.map_err(|error| format!("注入规则修复任务失败：{error}"))?
}

// ---------------------------------------------------------
// 注入逻辑内部引擎
// ---------------------------------------------------------

fn render_rule_content(rule: &TarvenRule) -> String {
    if rule.wrap {
        format!(
            "<vcp_injection description=\"由 VCPMobile 注入\">\n{}\n</vcp_injection>",
            rule.content
        )
    } else {
        rule.content.clone()
    }
}

fn message_text_content_mut(message: &mut serde_json::Value) -> Option<&mut String> {
    match message.get_mut("content")? {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Array(parts) => parts.iter_mut().find_map(|part| {
            if part.get("type").and_then(serde_json::Value::as_str) != Some("text") {
                return None;
            }
            match part.get_mut("text") {
                Some(serde_json::Value::String(text)) => Some(text),
                _ => None,
            }
        }),
        _ => None,
    }
}

fn apply_user_suffix_rules(messages: &mut [serde_json::Value], rules: &[&TarvenRule]) {
    if rules.is_empty() {
        return;
    }
    let Some(user_idx) = messages
        .iter()
        .rposition(|message| message["role"].as_str() == Some("user"))
    else {
        return;
    };

    let mut prepend_parts = Vec::new();
    let mut append_parts = Vec::new();
    for rule in rules {
        let rendered = render_rule_content(rule);
        if rule.position.as_deref() == Some("prepend") {
            prepend_parts.push(rendered);
        } else {
            append_parts.push(rendered);
        }
    }

    let Some(user_content) = message_text_content_mut(&mut messages[user_idx]) else {
        return;
    };
    if !prepend_parts.is_empty() {
        let prepend = prepend_parts.join("\n\n");
        *user_content = if user_content.is_empty() {
            prepend
        } else {
            format!("{}\n\n{}", prepend, user_content)
        };
    }
    if !append_parts.is_empty() {
        let append = append_parts.join("\n\n");
        *user_content = if user_content.is_empty() {
            append
        } else {
            format!("{}\n\n{}", user_content, append)
        };
    }
}

pub async fn fetch_active_rules(
    db: &DbState,
    scope: &str,
) -> Result<Vec<TarvenRule>, String> {
    Ok(read_tarven_rules(db).await?.into_iter()
        .filter(|rule| rule.is_enabled && (rule.scope == "global" || rule.scope == scope)).collect())
}

fn format_system_metadata(now_str: &str, created_at_str: Option<&str>, system_prompt: &mut String) {
    let mut metadata = format!(
        "<system_metadata>\n\
         - 当前系统时间: {}\n\
         - 运行环境: VCPMobile v{} (Android 移动端)\n",
        now_str,
        env!("CARGO_PKG_VERSION")
    );

    if let Some(created_at) = created_at_str {
        metadata.push_str(&format!("- 当前话题创建于: {}\n", created_at));
    }

    metadata.push_str("</system_metadata>\n\n");

    let original_prompt = std::mem::take(system_prompt);
    *system_prompt = format!("{}{}", metadata, original_prompt);
}

async fn inject_base_environment(
    pool: &Pool<Sqlite>,
    topic: &TopicKey,
    system_prompt: &mut String,
) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string();
    let mut created_at_str = None;

    match sqlx::query(
        "SELECT created_at FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&topic.owner_type)
    .bind(&topic.owner_id)
    .bind(&topic.topic_id)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(row)) => {
            use sqlx::Row;
            let created_at: i64 = row.get("created_at");
            if let Some(dt) = Local.timestamp_millis_opt(created_at).single() {
                created_at_str = Some(dt.format("%Y-%m-%d %H:%M:%S %Z").to_string());
            }
        }
        Ok(None) => {}
        Err(e) => {
            warn!(
                "Failed to fetch topic created_at for topic_id {}: {:?}",
                topic.topic_id, e
            );
        }
    }

    format_system_metadata(&now, created_at_str.as_deref(), system_prompt);
}

// 核心流水线：将 VCP 待发送消息列表进行就地拦截与多方位注入
pub async fn apply_tarven_pipeline(
    pool: &Pool<Sqlite>,
    topic: &TopicKey,
    agent_name: &str,
    rules: &[TarvenRule],
    messages: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    // 2. 处理 System Prompt 注入
    let system_index = messages
        .iter()
        .position(|m| m["role"].as_str() == Some("system"));

    let mut system_content = if let Some(idx) = system_index {
        messages[idx]["content"].as_str().unwrap_or("").to_string()
    } else {
        "".to_string()
    };

    // 检查是否启用了系统环境元数据注入
    if rules.iter().any(|r| r.id == "system_meta_injection") {
        // 注入基础环境真理
        inject_base_environment(pool, topic, &mut system_content).await;
    }

    // 过滤 system_suffix 规则并按位置拼接
    let system_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "system_suffix")
        .collect();

    let mut system_prepend_parts = Vec::new();
    let mut system_append_parts = Vec::new();

    for rule in system_rules {
        let rendered = render_rule_content(rule);
        if rule.position.as_deref() == Some("prepend") {
            system_prepend_parts.push(rendered);
        } else {
            system_append_parts.push(rendered);
        }
    }

    if !system_prepend_parts.is_empty() {
        let prepend_str = system_prepend_parts.join("\n\n");
        if !system_content.is_empty() {
            system_content = format!("{}\n\n{}", prepend_str, system_content);
        } else {
            system_content = prepend_str;
        }
    }

    if !system_append_parts.is_empty() {
        let append_str = system_append_parts.join("\n\n");
        if !system_content.is_empty() {
            system_content = format!("{}\n\n{}", system_content, append_str);
        } else {
            system_content = append_str;
        }
    }

    // 替换占位符
    system_content = system_content
        .replace("{{AgentName}}", agent_name)
        .replace("{{VCPChatAgentName}}", agent_name);

    // 回写或插入首部 system 消息
    if let Some(idx) = system_index {
        messages[idx]["content"] = serde_json::Value::String(system_content);
    } else if !system_content.is_empty() {
        messages.insert(
            0,
            serde_json::json!({
                "role": "system",
                "content": system_content
            }),
        );
    }

    // 3. 处理 User Suffix 注入（追加到最新一轮用户输入文本中，仅在大模型上下文生效，不写历史记录表）
    let user_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "user_suffix")
        .collect();

    apply_user_suffix_rules(messages, &user_rules);

    // 4. 处理 Context Inject 上下文独立节点插入
    let context_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "context_inject")
        .collect();

    if !context_rules.is_empty() {
        let mut system_msgs = Vec::new();
        let mut non_system_msgs = Vec::new();

        for msg in messages.drain(..) {
            if msg["role"].as_str() == Some("system") {
                system_msgs.push(msg);
            } else {
                non_system_msgs.push(msg);
            }
        }

        // 根据 depth 从大到小排列，确保 insert 的 index 不会因前面元素的插入而错位
        let mut sorted_context_rules = context_rules;
        sorted_context_rules.sort_by(|a, b| {
            let depth_b = b.depth.unwrap_or(0);
            let depth_a = a.depth.unwrap_or(0);
            depth_b.cmp(&depth_a)
        });

        for rule in sorted_context_rules {
            let role = rule.role.as_deref().unwrap_or("user");
            let depth = rule.depth.unwrap_or(0) as usize;
            let insert_index = if non_system_msgs.len() > depth {
                non_system_msgs.len() - depth
            } else {
                0
            };

            let virtual_msg = serde_json::json!({
                "role": role,
                "content": render_rule_content(rule),
                "__tavernInjected": true
            });

            non_system_msgs.insert(insert_index, virtual_msg);
        }

        // 重组
        messages.extend(system_msgs);
        messages.extend(non_system_msgs);
    }

    Ok(())
}

// ---------------------------------------------------------
// Tauri Commands
// ---------------------------------------------------------

#[tauri::command]
pub async fn get_tarven_rules(db_state: State<'_, DbState>) -> Result<Vec<TarvenRule>, String> {
    read_tarven_rules(&db_state).await
}

#[tauri::command]
pub async fn save_tarven_rule(
    db_state: State<'_, DbState>,
    mut rule: TarvenRule,
) -> Result<(), String> {
    rule.normalize();
    let now = Local::now().timestamp_millis();
    let is_enabled_int = if rule.is_enabled { 1 } else { 0 };
    let wrap_int = if rule.wrap { 1 } else { 0 };

    let mut tx = db_state.write_transaction("context.rule.save").await?;
    sqlx::query(
        "INSERT INTO tarven_rules (id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order, created_at, updated_at) 
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET 
            name = excluded.name,
            rule_type = excluded.rule_type,
            is_enabled = excluded.is_enabled,
            content = excluded.content,
            scope = excluded.scope,
            wrap = excluded.wrap,
            role = excluded.role,
            depth = excluded.depth,
            position = excluded.position,
            sort_order = excluded.sort_order,
            updated_at = excluded.updated_at"
    )
    .bind(rule.id)
    .bind(rule.name)
    .bind(rule.rule_type)
    .bind(is_enabled_int)
    .bind(rule.content)
    .bind(rule.scope)
    .bind(wrap_int)
    .bind(rule.role)
    .bind(rule.depth)
    .bind(rule.position)
    .bind(rule.sort_order)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("Failed to save rule: {}", e))?;
    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn delete_tarven_rule(db_state: State<'_, DbState>, id: String) -> Result<(), String> {
    if id == "system_meta_injection" || id == "time_anchoring_v2" {
        return Err("系统内置高级注入规则禁止被删除".to_string());
    }

    let mut tx = db_state.write_transaction("context.rule.delete").await?;
    sqlx::query("DELETE FROM tarven_rules WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("Failed to delete rule: {}", e))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn toggle_rule_enabled(
    db_state: State<'_, DbState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let enabled_int = if enabled { 1 } else { 0 };
    let now = Local::now().timestamp_millis();

    let mut tx = db_state.write_transaction("context.rule.toggle").await?;
    sqlx::query("UPDATE tarven_rules SET is_enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled_int)
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("Failed to toggle rule: {}", e))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn reorder_rules(
    db_state: State<'_, DbState>,
    rule_ids: Vec<String>,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    let mut tx = db_state.write_transaction("context.rule.reorder").await?;

    for (index, id) in rule_ids.iter().enumerate() {
        sqlx::query("UPDATE tarven_rules SET sort_order = ?, updated_at = ? WHERE id = ?")
            .bind(index as i32)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to update sort order for {}: {}", id, e))?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn preview_tarven_injection(
    mut rules: Vec<TarvenRule>,
    mock_messages: Option<Vec<serde_json::Value>>,
) -> Result<Vec<serde_json::Value>, String> {
    for rule in &mut rules { rule.normalize(); }
    let mut messages = mock_messages.unwrap_or_else(|| {
        vec![
            serde_json::json!({ "role": "system", "content": "你是一个智能助手。" }),
            serde_json::json!({ "role": "user", "content": "你好，请问你是？" }),
            serde_json::json!({ "role": "assistant", "content": "我是你的 AI 助手，有什么可以帮你的吗？" }),
            serde_json::json!({ "role": "user", "content": "帮我写一首关于秋天的诗。" }),
        ]
    });

    // 1. 处理 System Prompt
    let system_index = messages
        .iter()
        .position(|m| m["role"].as_str() == Some("system"));
    let mut system_content = if let Some(idx) = system_index {
        messages[idx]["content"].as_str().unwrap_or("").to_string()
    } else {
        "".to_string()
    };

    // 检查是否启用了系统环境元数据注入
    if rules
        .iter()
        .any(|r| r.id == "system_meta_injection" && r.is_enabled)
    {
        // 模拟环境注入
        let mock_now = Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string();
        format_system_metadata(&mock_now, Some(&mock_now), &mut system_content);
    }

    // 过滤 system_suffix 规则并按位置拼接
    let system_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "system_suffix" && r.is_enabled)
        .collect();

    let mut system_prepend_parts = Vec::new();
    let mut system_append_parts = Vec::new();

    for rule in system_rules {
        let rendered = render_rule_content(rule);
        if rule.position.as_deref() == Some("prepend") {
            system_prepend_parts.push(rendered);
        } else {
            system_append_parts.push(rendered);
        }
    }

    if !system_prepend_parts.is_empty() {
        let prepend_str = system_prepend_parts.join("\n\n");
        if !system_content.is_empty() {
            system_content = format!("{}\n\n{}", prepend_str, system_content);
        } else {
            system_content = prepend_str;
        }
    }

    if !system_append_parts.is_empty() {
        let append_str = system_append_parts.join("\n\n");
        if !system_content.is_empty() {
            system_content = format!("{}\n\n{}", system_content, append_str);
        } else {
            system_content = append_str;
        }
    }

    system_content = system_content
        .replace("{{AgentName}}", "秋水智能体")
        .replace("{{VCPChatAgentName}}", "秋水智能体");

    if let Some(idx) = system_index {
        messages[idx]["content"] = serde_json::Value::String(system_content);
    } else {
        messages.insert(
            0,
            serde_json::json!({ "role": "system", "content": system_content }),
        );
    }

    // 2. 处理 User Suffix
    let user_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "user_suffix" && r.is_enabled)
        .collect();

    apply_user_suffix_rules(&mut messages, &user_rules);

    // 3. 处理 Context Inject
    let context_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "context_inject" && r.is_enabled)
        .collect();

    if !context_rules.is_empty() {
        let mut system_msgs = Vec::new();
        let mut non_system_msgs = Vec::new();

        for msg in messages.drain(..) {
            if msg["role"].as_str() == Some("system") {
                system_msgs.push(msg);
            } else {
                non_system_msgs.push(msg);
            }
        }

        let mut sorted_context_rules = context_rules;
        sorted_context_rules.sort_by(|a, b| {
            let depth_b = b.depth.unwrap_or(0);
            let depth_a = a.depth.unwrap_or(0);
            depth_b.cmp(&depth_a)
        });

        for rule in sorted_context_rules {
            let role = rule.role.as_deref().unwrap_or("user");
            let depth = rule.depth.unwrap_or(0) as usize;
            let insert_index = if non_system_msgs.len() > depth {
                non_system_msgs.len() - depth
            } else {
                0
            };

            let virtual_msg = serde_json::json!({
                "role": role,
                "content": render_rule_content(rule),
                "__tavernInjected": true
            });

            non_system_msgs.insert(insert_index, virtual_msg);
        }

        messages.extend(system_msgs);
        messages.extend(non_system_msgs);
    }

    Ok(messages)
}

pub async fn sync_system_preset_rules(pool: &Pool<Sqlite>) -> Result<(), String> {
    let now = chrono::Local::now().timestamp_millis();
    for id in ["system_meta_injection", "time_anchoring_v2"] {
        let preset = TarvenRule::defaults_for_id(id);
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tarven_rules WHERE id = ?)")
                .bind(id)
                .fetch_one(pool)
                .await
                .map_err(|e| format!("Failed to query tarven_rules existence: {}", e))?;

        if exists {
            // 已存在，只热覆盖更新内容、名称、规则类型等，但不篡改用户的 is_enabled 状态
            sqlx::query(
                "UPDATE tarven_rules 
                 SET name = ?, rule_type = ?, content = ?, updated_at = ? 
                 WHERE id = ?",
            )
            .bind(&preset.name)
            .bind(&preset.rule_type)
            .bind(&preset.content)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| format!("Failed to update system preset rule: {}", e))?;
        } else {
            // 不存在，执行完整插入
            sqlx::query(
                "INSERT INTO tarven_rules (id, name, rule_type, is_enabled, content, scope, wrap, sort_order, created_at, updated_at) 
                 VALUES (?, ?, ?, ?, ?, 'global', 0, -100, ?, ?)"
            )
            .bind(id)
            .bind(&preset.name)
            .bind(&preset.rule_type)
            .bind(preset.is_enabled)
            .bind(&preset.content)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .map_err(|e| format!("Failed to insert system preset rule: {}", e))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod default_recovery_tests {
    use super::*;

    async fn database() -> DbState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool).await.unwrap();
        DbState::new(pool, std::path::PathBuf::new())
    }

    #[tokio::test]
    async fn rules_restore_only_invalid_fields_and_persist_before_filtering() {
        let db = database().await;
        sqlx::query("INSERT INTO tarven_rules(id,name,rule_type,is_enabled,content,scope,wrap,role,depth,position,sort_order,created_at,updated_at) VALUES ('bad',x'ff','context_inject','true','keep text','bad-scope',2,'system',-3,'before',7,12,34),('kept','Kept','user_suffix',0,'','agent',0,NULL,NULL,NULL,-4,12,35)")
            .execute(&db.pool).await.unwrap();
        let rules = read_tarven_rules(&db).await.unwrap();
        let bad = rules.iter().find(|rule| rule.id == "bad").unwrap();
        let defaults = TarvenRule::default();
        assert_eq!(bad.name, defaults.name);
        assert_eq!(bad.rule_type, "context_inject");
        assert_eq!(bad.content, "keep text");
        assert_eq!(bad.is_enabled, defaults.is_enabled);
        assert_eq!(bad.wrap, defaults.wrap);
        assert_eq!(bad.scope, defaults.scope);
        assert_eq!(bad.role, defaults.role);
        assert_eq!(bad.depth, defaults.depth);
        assert_eq!(bad.position, defaults.position);
        assert_eq!(bad.sort_order, 7);
        let kept = rules.iter().find(|rule| rule.id == "kept").unwrap();
        assert!(!kept.is_enabled);
        assert!(!kept.wrap);
        assert_eq!(kept.content, "");
        assert_eq!((kept.role.clone(), kept.depth, kept.position.clone()), (None, None, None));
        let stored: (String, i64, i64) = sqlx::query_as("SELECT typeof(is_enabled),depth,updated_at FROM tarven_rules WHERE id='bad'")
            .fetch_one(&db.pool).await.unwrap();
        assert_eq!(stored, ("integer".into(), 0, 34));
        assert_eq!(read_tarven_rules(&db).await.unwrap(), rules);
        let active = fetch_active_rules(&db, "group").await.unwrap();
        assert_eq!(active.iter().map(|rule| rule.id.as_str()).collect::<Vec<_>>(), vec!["bad"]);
    }

    #[tokio::test]
    async fn builtin_rules_use_their_own_defaults_and_keep_valid_user_toggles() {
        let db = database().await;
        sync_system_preset_rules(&db.pool).await.unwrap();
        sqlx::query("UPDATE tarven_rules SET is_enabled=x'ff'").execute(&db.pool).await.unwrap();
        // Existence checking must not decode a possibly damaged toggle at boot.
        sync_system_preset_rules(&db.pool).await.unwrap();
        let rules = read_tarven_rules(&db).await.unwrap();
        let system = rules.iter().find(|rule| rule.id == "system_meta_injection").unwrap();
        let time = rules.iter().find(|rule| rule.id == "time_anchoring_v2").unwrap();
        assert!(system.is_enabled);
        assert!(!time.is_enabled);
        sqlx::query("UPDATE tarven_rules SET is_enabled=CASE id WHEN 'time_anchoring_v2' THEN 1 ELSE 0 END").execute(&db.pool).await.unwrap();
        sync_system_preset_rules(&db.pool).await.unwrap();
        let rules = read_tarven_rules(&db).await.unwrap();
        assert!(!rules.iter().find(|rule| rule.id == "system_meta_injection").unwrap().is_enabled);
        assert!(rules.iter().find(|rule| rule.id == "time_anchoring_v2").unwrap().is_enabled);
    }

    #[tokio::test]
    async fn rule_write_failures_are_errors_and_identity_is_not_defaulted() {
        let db = database().await;
        sync_system_preset_rules(&db.pool).await.unwrap();
        sqlx::query("UPDATE tarven_rules SET is_enabled='broken'").execute(&db.pool).await.unwrap();
        sqlx::raw_sql("CREATE TRIGGER reject_rule_repair BEFORE UPDATE ON tarven_rules BEGIN SELECT RAISE(ABORT,'write failed'); END;")
            .execute(&db.pool).await.unwrap();
        assert!(read_tarven_rules(&db).await.unwrap_err().contains("write failed"));
        sqlx::query("DROP TRIGGER reject_rule_repair").execute(&db.pool).await.unwrap();
        sqlx::query("UPDATE tarven_rules SET id=NULL WHERE id='time_anchoring_v2'").execute(&db.pool).await.unwrap();
        assert!(read_tarven_rules(&db).await.is_err());
    }

    #[tokio::test]
    async fn preview_uses_default_role_and_depth_without_limiting_valid_large_depths() {
        let rule = TarvenRule {
            id: "context".into(), rule_type: "context_inject".into(), content: "inserted".into(),
            role: Some("invalid".into()), depth: Some(-1), wrap: false, ..TarvenRule::default()
        };
        let messages = preview_tarven_injection(vec![rule.clone()], Some(vec![
            serde_json::json!({"role":"user","content":"original"})
        ])).await.unwrap();
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert_eq!(messages.last().unwrap()["content"], "inserted");
        let mut deep = TarvenRule { depth: Some(100), ..rule };
        deep.normalize();
        assert_eq!(deep.depth, Some(100));
    }
}
