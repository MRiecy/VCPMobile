use crate::vcp_modules::agent_types::RegexRule;
use crate::vcp_modules::db_manager::DbState;
use dashmap::DashMap;
use fancy_regex::Regex;
use lazy_static::lazy_static;
use tauri::State;

lazy_static! {
    /// 正则表达式编译缓存: find_pattern -> Compiled Regex
    static ref REGEX_CACHE: DashMap<String, Regex> = DashMap::new();
}

/// 执行正则转换 (基于影子数据库索引)
pub async fn apply_regex_rules(
    db: &State<'_, DbState>,
    agent_id: &str,
    text: &str,
    scope: &str, // "frontend" 或 "context"
    role: &str,
    depth: i32,
) -> Result<String, String> {
    // 1. 从影子数据库加载该智能体的所有正则规则 (高性能索引)
    let rules =
        sqlx::query_as::<_, RegexRule>("SELECT * FROM agent_regex_rules WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(&db.pool)
            .await
            .map_err(|e: sqlx::Error| e.to_string())?;

    let mut processed_text = text.to_string();

    for rule in rules {
        // 2. 检查作用域对齐
        let should_apply_to_scope = (scope == "context" && rule.apply_to_context)
            || (scope == "frontend" && rule.apply_to_frontend);

        if !should_apply_to_scope {
            continue;
        }

        // 3. 检查角色对齐
        if !rule.apply_to_roles.contains(&role.to_string()) {
            continue;
        }

        // 4. 检查深度对齐 (-1 表示无限制)
        let min_depth_ok = rule.min_depth == -1 || depth >= rule.min_depth;
        let max_depth_ok = rule.max_depth == -1 || depth <= rule.max_depth;

        if !min_depth_ok || !max_depth_ok {
            continue;
        }

        // 5. 执行替换逻辑 (带编译缓存)
        let regex = match REGEX_CACHE.get(&rule.find_pattern) {
            Some(r) => r.clone(),
            None => {
                let r = Regex::new(&rule.find_pattern)
                    .map_err(|e| format!("Invalid regex {}: {}", rule.find_pattern, e))?;
                REGEX_CACHE.insert(rule.find_pattern.clone(), r.clone());
                r
            }
        };

        processed_text = regex
            .replace_all(&processed_text, rule.replace_with.as_str())
            .to_string();
    }

    Ok(processed_text)
}
