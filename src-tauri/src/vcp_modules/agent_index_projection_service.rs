use sqlx::{Sqlite, Transaction};

/// 将 Agent 的基本信息同步到 agent_index 影子表
pub async fn sync_agent_index(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    name: &str,
    mtime: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO agent_index (agent_id, name, mtime) VALUES (?, ?, ?)
         ON CONFLICT(agent_id) DO UPDATE SET name=excluded.name, mtime=excluded.mtime",
    )
    .bind(agent_id)
    .bind(name)
    .bind(mtime)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}
