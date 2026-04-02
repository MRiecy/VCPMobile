use crate::vcp_modules::agent_config_repository_fs::RegexRule;
use sqlx::{Pool, Sqlite};

/// 同步智能体的正则规则到数据库 (Shadow DB)
///
/// 该函数会先删除该智能体在 `agent_regex_rules` 表中的所有旧规则，
/// 然后插入当前配置中的所有新规则。
pub async fn sync_regex_rules_to_db(
    pool: &Pool<Sqlite>,
    agent_id: &str,
    rules: &[RegexRule],
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    // 1. 删除旧规则
    sqlx::query("DELETE FROM agent_regex_rules WHERE agent_id = ?")
        .bind(agent_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // 2. 插入新规则
    for rule in rules {
        let roles_json =
            serde_json::to_string(&rule.apply_to_roles).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            "INSERT INTO agent_regex_rules (
                rule_id, agent_id, title, find_pattern, replace_with, 
                apply_to_roles, apply_to_frontend, apply_to_context, min_depth, max_depth
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&rule.id)
        .bind(agent_id)
        .bind(&rule.title)
        .bind(&rule.find_pattern)
        .bind(&rule.replace_with)
        .bind(roles_json)
        .bind(rule.apply_to_frontend)
        .bind(rule.apply_to_context)
        .bind(rule.min_depth)
        .bind(rule.max_depth)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok(())
}
