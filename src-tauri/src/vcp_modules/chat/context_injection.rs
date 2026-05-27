use sqlx::{Pool, Sqlite};
use chrono::{Local, TimeZone};
use serde::{Serialize, Deserialize};
use tauri::State;
use crate::vcp_modules::db_manager::DbState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TarvenRule {
    pub id: String,
    pub name: String,
    pub rule_type: String, // 'system_suffix' | 'user_suffix' | 'context_inject'
    pub is_enabled: bool,
    pub content: String,
    pub scope: String,     // 'global' | 'agent' | 'group'
    pub wrap: bool,
    
    // context_inject 专用
    pub role: Option<String>, // 'user' | 'assistant'
    pub depth: Option<i32>,
    
    // system_suffix / user_suffix 专用
    pub position: Option<String>, // 'prepend' | 'append'
    
    pub sort_order: i32,
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

pub async fn fetch_active_rules(
    pool: &Pool<Sqlite>,
    scope: &str,
) -> Result<Vec<TarvenRule>, String> {
    let rows = sqlx::query(
        "SELECT id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order 
         FROM tarven_rules 
         WHERE is_enabled = 1 AND (scope = 'global' OR scope = ?)
         ORDER BY sort_order ASC"
    )
    .bind(scope)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to fetch active rules: {}", e))?;

    let mut rules = Vec::new();
    for row in rows {
        use sqlx::Row;
        rules.push(TarvenRule {
            id: row.get("id"),
            name: row.get("name"),
            rule_type: row.get("rule_type"),
            is_enabled: row.get::<i32, _>("is_enabled") != 0,
            content: row.get("content"),
            scope: row.get("scope"),
            wrap: row.get::<i32, _>("wrap") != 0,
            role: row.get("role"),
            depth: row.get("depth"),
            position: row.get("position"),
            sort_order: row.get("sort_order"),
        });
    }
    Ok(rules)
}

async fn inject_base_environment(
    pool: &Pool<Sqlite>,
    topic_id: &str,
    system_prompt: &mut String,
) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z");
    let mut prepend = format!("当前系统时间: {}\n运行环境: VCP Mobile (Android 移动端)\n", now);

    if let Ok(Some(row)) = sqlx::query("SELECT created_at FROM topics WHERE topic_id = ?")
        .bind(topic_id)
        .fetch_optional(pool)
        .await
    {
        use sqlx::Row;
        let created_at: i64 = row.get("created_at");
        if let Some(dt) = Local.timestamp_millis_opt(created_at).single() {
            prepend.push_str(&format!("当前话题创建于: {}\n", dt.format("%Y-%m-%d %H:%M:%S %Z")));
        }
    }

    prepend.push_str("\n---\n\n");
    system_prompt.insert_str(0, &prepend);
}

// 核心流水线：将 VCP 待发送消息列表进行就地拦截与多方位注入
pub async fn apply_tarven_pipeline(
    pool: &Pool<Sqlite>,
    topic_id: &str,
    agent_name: &str,
    scope: &str, // 'agent' | 'group'
    messages: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    // 1. 获取所有激活的规则
    let rules = fetch_active_rules(pool, scope).await?;

    // 2. 处理 System Prompt 注入
    let system_index = messages.iter().position(|m| m["role"].as_str() == Some("system"));
    
    let mut system_content = if let Some(idx) = system_index {
        messages[idx]["content"].as_str().unwrap_or("").to_string()
    } else {
        "".to_string()
    };

    // 注入基础环境真理
    inject_base_environment(pool, topic_id, &mut system_content).await;

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

    if !user_rules.is_empty() {
        if let Some(user_idx) = messages.iter().rposition(|m| m["role"].as_str() == Some("user")) {
            let mut user_content = messages[user_idx]["content"].as_str().unwrap_or("").to_string();
            
            let mut user_prepend_parts = Vec::new();
            let mut user_append_parts = Vec::new();

            for rule in user_rules {
                let rendered = render_rule_content(rule);
                if rule.position.as_deref() == Some("prepend") {
                    user_prepend_parts.push(rendered);
                } else {
                    user_append_parts.push(rendered);
                }
            }

            if !user_prepend_parts.is_empty() {
                let prepend_str = user_prepend_parts.join("\n\n");
                if !user_content.is_empty() {
                    user_content = format!("{}\n\n{}", prepend_str, user_content);
                } else {
                    user_content = prepend_str;
                }
            }

            if !user_append_parts.is_empty() {
                let append_str = user_append_parts.join("\n\n");
                if !user_content.is_empty() {
                    user_content = format!("{}\n\n{}", user_content, append_str);
                } else {
                    user_content = append_str;
                }
            }

            messages[user_idx]["content"] = serde_json::Value::String(user_content);
        }
    }

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
pub async fn get_tarven_rules(
    db_state: State<'_, DbState>,
) -> Result<Vec<TarvenRule>, String> {
    let rows = sqlx::query(
        "SELECT id, name, rule_type, is_enabled, content, scope, wrap, role, depth, position, sort_order 
         FROM tarven_rules 
         ORDER BY sort_order ASC"
    )
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;

    let mut rules = Vec::new();
    for row in rows {
        use sqlx::Row;
        rules.push(TarvenRule {
            id: row.get("id"),
            name: row.get("name"),
            rule_type: row.get("rule_type"),
            is_enabled: row.get::<i32, _>("is_enabled") != 0,
            content: row.get("content"),
            scope: row.get("scope"),
            wrap: row.get::<i32, _>("wrap") != 0,
            role: row.get("role"),
            depth: row.get("depth"),
            position: row.get("position"),
            sort_order: row.get("sort_order"),
        });
    }
    Ok(rules)
}

#[tauri::command]
pub async fn save_tarven_rule(
    db_state: State<'_, DbState>,
    rule: TarvenRule,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    let is_enabled_int = if rule.is_enabled { 1 } else { 0 };
    let wrap_int = if rule.wrap { 1 } else { 0 };

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
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("Failed to save rule: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn delete_tarven_rule(
    db_state: State<'_, DbState>,
    id: String,
) -> Result<(), String> {
    sqlx::query("DELETE FROM tarven_rules WHERE id = ?")
        .bind(id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| format!("Failed to delete rule: {}", e))?;
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

    sqlx::query("UPDATE tarven_rules SET is_enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled_int)
        .bind(now)
        .bind(id)
        .execute(&db_state.pool)
        .await
        .map_err(|e| format!("Failed to toggle rule: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn reorder_rules(
    db_state: State<'_, DbState>,
    rule_ids: Vec<String>,
) -> Result<(), String> {
    let now = Local::now().timestamp_millis();
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;

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
    rules: Vec<TarvenRule>,
    mock_messages: Option<Vec<serde_json::Value>>,
) -> Result<Vec<serde_json::Value>, String> {
    let mut messages = mock_messages.unwrap_or_else(|| {
        vec![
            serde_json::json!({ "role": "system", "content": "你是一个智能助手。" }),
            serde_json::json!({ "role": "user", "content": "你好，请问你是？" }),
            serde_json::json!({ "role": "assistant", "content": "我是你的 AI 助手，有什么可以帮你的吗？" }),
            serde_json::json!({ "role": "user", "content": "帮我写一首关于秋天的诗。" }),
        ]
    });

    // 1. 处理 System Prompt
    let system_index = messages.iter().position(|m| m["role"].as_str() == Some("system"));
    let mut system_content = if let Some(idx) = system_index {
        messages[idx]["content"].as_str().unwrap_or("").to_string()
    } else {
        "".to_string()
    };

    // 模拟环境注入
    let mock_now = Local::now().format("%Y-%m-%d %H:%M:%S %Z");
    let prepend = format!("当前系统时间: {}\n运行环境: VCP Mobile (Android 移动端)\n当前话题创建于: {}\n\n---\n\n", mock_now, mock_now);
    system_content.insert_str(0, &prepend);

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

    system_content = system_content.replace("{{AgentName}}", "秋水智能体").replace("{{VCPChatAgentName}}", "秋水智能体");

    if let Some(idx) = system_index {
        messages[idx]["content"] = serde_json::Value::String(system_content);
    } else {
        messages.insert(0, serde_json::json!({ "role": "system", "content": system_content }));
    }

    // 2. 处理 User Suffix
    let user_rules: Vec<&TarvenRule> = rules
        .iter()
        .filter(|r| r.rule_type == "user_suffix" && r.is_enabled)
        .collect();

    if !user_rules.is_empty() {
        if let Some(user_idx) = messages.iter().rposition(|m| m["role"].as_str() == Some("user")) {
            let mut user_content = messages[user_idx]["content"].as_str().unwrap_or("").to_string();
            
            let mut user_prepend_parts = Vec::new();
            let mut user_append_parts = Vec::new();

            for rule in user_rules {
                let rendered = render_rule_content(rule);
                if rule.position.as_deref() == Some("prepend") {
                    user_prepend_parts.push(rendered);
                } else {
                    user_append_parts.push(rendered);
                }
            }

            if !user_prepend_parts.is_empty() {
                let prepend_str = user_prepend_parts.join("\n\n");
                if !user_content.is_empty() {
                    user_content = format!("{}\n\n{}", prepend_str, user_content);
                } else {
                    user_content = prepend_str;
                }
            }

            if !user_append_parts.is_empty() {
                let append_str = user_append_parts.join("\n\n");
                if !user_content.is_empty() {
                    user_content = format!("{}\n\n{}", user_content, append_str);
                } else {
                    user_content = append_str;
                }
            }

            messages[user_idx]["content"] = serde_json::Value::String(user_content);
        }
    }

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
