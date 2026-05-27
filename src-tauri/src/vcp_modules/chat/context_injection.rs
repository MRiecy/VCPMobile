use sqlx::{Pool, Sqlite};
use chrono::{Local, TimeZone};
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use tauri::State;
use crate::vcp_modules::db_manager::DbState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TarvenRule {
    pub id: String,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub icon: Option<String>,
}

pub struct InjectionContext<'a> {
    pub pool: &'a Pool<Sqlite>,
    #[allow(dead_code)]
    pub agent_name: &'a str,
    pub topic_id: &'a str,
}

#[async_trait]
pub trait ContextInjector: Send + Sync {
    async fn inject(&self, ctx: &InjectionContext<'_>, current_system_prompt: &mut String) -> Result<(), String>;
}

// 1. 基础环境与时间注入器
pub struct BaseEnvironmentInjector;

#[async_trait]
impl ContextInjector for BaseEnvironmentInjector {
    async fn inject(&self, ctx: &InjectionContext<'_>, current_system_prompt: &mut String) -> Result<(), String> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S %Z");
        let mut prepend = format!("当前系统时间: {}\n运行环境: VCP Mobile (Android 移动端)\n", now);

        // 从 topics 表中获取当前 topic 创建时间
        if let Ok(Some(row)) = sqlx::query("SELECT created_at FROM topics WHERE topic_id = ?")
            .bind(ctx.topic_id)
            .fetch_optional(ctx.pool)
            .await
        {
            let created_at: i64 = sqlx::Row::get(&row, "created_at");
            if let Some(dt) = Local.timestamp_millis_opt(created_at).single() {
                prepend.push_str(&format!("当前话题创建于: {}\n", dt.format("%Y-%m-%d %H:%M:%S %Z")));
            }
        }

        prepend.push_str("\n---\n\n");
        current_system_prompt.insert_str(0, &prepend);
        Ok(())
    }
}

// 2. VCPChatTarven 自定义注入器
pub struct TarvenInjector;

#[async_trait]
impl ContextInjector for TarvenInjector {
    async fn inject(&self, ctx: &InjectionContext<'_>, current_system_prompt: &mut String) -> Result<(), String> {
        // 从 settings 表读取 key = 'tarven_rules'
        let row_opt = sqlx::query("SELECT value FROM settings WHERE key = 'tarven_rules'")
            .fetch_optional(ctx.pool)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = row_opt {
            let value_str: String = sqlx::Row::get(&row, "value");
            if let Ok(rules) = serde_json::from_str::<Vec<TarvenRule>>(&value_str) {
                let active_rules: Vec<String> = rules
                    .into_iter()
                    .filter(|r| r.enabled)
                    .map(|r| format!("[规则名: {}]\n{}", r.name, r.content))
                    .collect();

                if !active_rules.is_empty() {
                    let mut tarven_prepend = String::from("VCPChatTarven 自定义上下文注入开启：\n");
                    for rule_content in active_rules {
                        tarven_prepend.push_str(&format!("{}\n", rule_content));
                    }
                    tarven_prepend.push_str("\n---\n\n");
                    current_system_prompt.insert_str(0, &tarven_prepend);
                }
            }
        }
        Ok(())
    }
}

pub async fn build_injected_system_prompt(
    pool: &Pool<Sqlite>,
    topic_id: &str,
    agent_name: &str,
    mut base_prompt: String,
) -> Result<String, String> {
    let ctx = InjectionContext { pool, agent_name, topic_id };
    
    // 执行注入管道
    let injectors: Vec<Box<dyn ContextInjector>> = vec![
        Box::new(BaseEnvironmentInjector),
        Box::new(TarvenInjector),
    ];

    for injector in injectors {
        injector.inject(&ctx, &mut base_prompt).await?;
    }

    // 支持 {{AgentName}} 占位符替换
    let final_prompt = base_prompt.replace("{{AgentName}}", agent_name);
    Ok(final_prompt)
}

// ==========================================
// Tauri Commands
// ==========================================

#[tauri::command]
pub async fn get_tarven_rules(
    db_state: State<'_, DbState>,
) -> Result<Vec<TarvenRule>, String> {
    let row_opt = sqlx::query("SELECT value FROM settings WHERE key = 'tarven_rules'")
        .fetch_optional(&db_state.pool)
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    if let Some(row) = row_opt {
        let value_str: String = sqlx::Row::get(&row, "value");
        serde_json::from_str::<Vec<TarvenRule>>(&value_str)
            .map_err(|e| format!("Failed to parse rules: {}", e))
    } else {
        Ok(Vec::new())
    }
}

#[tauri::command]
pub async fn save_tarven_rules(
    db_state: State<'_, DbState>,
    rules: Vec<TarvenRule>,
) -> Result<(), String> {
    let rules_str = serde_json::to_string(&rules)
        .map_err(|e| format!("Serialization failed: {}", e))?;
    
    let now = Local::now().timestamp_millis();

    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) 
         VALUES ('tarven_rules', ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at"
    )
    .bind(rules_str)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("Database update error: {}", e))?;

    Ok(())
}
