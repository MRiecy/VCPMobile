use crate::vcp_modules::db_manager::DbState;
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

impl DeleteExecutor {
    pub async fn soft_delete_agent<R: Runtime>(
        app: &AppHandle<R>,
        agent_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE agents SET deleted_at = ? WHERE agent_id = ?")
            .bind(now)
            .bind(agent_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!("[DeleteExecutor] Soft deleted Agent: {}", agent_id);
        Ok(())
    }

    pub async fn soft_delete_group<R: Runtime>(
        app: &AppHandle<R>,
        group_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE groups SET deleted_at = ? WHERE group_id = ?")
            .bind(now)
            .bind(group_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!("[DeleteExecutor] Soft deleted Group: {}", group_id);
        Ok(())
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE topics SET deleted_at = ? WHERE topic_id = ?")
            .bind(now)
            .bind(topic_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!("[DeleteExecutor] Soft deleted Topic: {}", topic_id);
        Ok(())
    }

    pub async fn soft_delete_message<R: Runtime>(
        app: &AppHandle<R>,
        msg_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE messages SET deleted_at = ? WHERE msg_id = ?")
            .bind(now)
            .bind(msg_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!("[DeleteExecutor] Soft deleted Message: {}", msg_id);
        Ok(())
    }

    pub async fn soft_delete_avatar<R: Runtime>(
        app: &AppHandle<R>,
        owner_type: &str,
        owner_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        sqlx::query("UPDATE avatars SET deleted_at = ? WHERE owner_type = ? AND owner_id = ?")
            .bind(now)
            .bind(owner_type)
            .bind(owner_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!("[DeleteExecutor] Soft deleted Avatar: {}:{}", owner_type, owner_id);
        Ok(())
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;

        let agents = sqlx::query("DELETE FROM agents WHERE deleted_at IS NOT NULL AND deleted_at < ?")
            .bind(threshold)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let groups = sqlx::query("DELETE FROM groups WHERE deleted_at IS NOT NULL AND deleted_at < ?")
            .bind(threshold)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let topics = sqlx::query("DELETE FROM topics WHERE deleted_at IS NOT NULL AND deleted_at < ?")
            .bind(threshold)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let messages = sqlx::query("DELETE FROM messages WHERE deleted_at IS NOT NULL AND deleted_at < ?")
            .bind(threshold)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        println!(
            "[DeleteExecutor] Cleaned up old records: agents={}, groups={}, topics={}, messages={}",
            agents.rows_affected(),
            groups.rows_affected(),
            topics.rows_affected(),
            messages.rows_affected()
        );

        Ok(())
    }
}
