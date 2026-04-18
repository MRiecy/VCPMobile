use sqlx::Row;
use sqlx::SqlitePool;

pub struct Phase2Topic;

impl Phase2Topic {
    pub async fn get_all_topic_ids(pool: &SqlitePool) -> Result<Vec<String>, String> {
        let rows = sqlx::query("SELECT topic_id FROM topics WHERE deleted_at IS NULL")
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(rows.iter().map(|r| r.get("topic_id")).collect())
    }
}
