use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use sqlx::Row;
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

        // 级联将该 Agent 下的所有话题标记为逻辑删除
        sqlx::query("UPDATE topics SET deleted_at = ? WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL")
            .bind(now)
            .bind(agent_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联将该 Agent 下所有话题的所有消息标记为逻辑删除
        sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id IN (SELECT topic_id FROM topics WHERE owner_id = ? AND owner_type = 'agent') AND deleted_at IS NULL")
            .bind(now)
            .bind(agent_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联清除该 Agent 下的所有活跃生成，杜绝已删除消息复活
        sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = 'agent'")
            .bind(agent_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut tx = db.pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_agent_hash(&mut tx, agent_id).await?;
        tx.commit().await.map_err(|e| e.to_string())?;

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

        // 级联将该 Group 下的所有话题标记为逻辑删除
        sqlx::query("UPDATE topics SET deleted_at = ? WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL")
            .bind(now)
            .bind(group_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联将该 Group 下所有话题的所有消息标记为逻辑删除
        sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id IN (SELECT topic_id FROM topics WHERE owner_id = ? AND owner_type = 'group') AND deleted_at IS NULL")
            .bind(now)
            .bind(group_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联清除该 Group 下的所有活跃生成，杜绝已删除消息复活
        sqlx::query("DELETE FROM active_generations WHERE owner_id = ? AND owner_type = 'group'")
            .bind(group_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        let mut tx = db.pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_group_hash(&mut tx, group_id).await?;
        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let now = chrono::Utc::now().timestamp_millis();

        let parent_row = sqlx::query("SELECT owner_id, owner_type FROM topics WHERE topic_id = ?")
            .bind(topic_id)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        sqlx::query("UPDATE topics SET deleted_at = ? WHERE topic_id = ?")
            .bind(now)
            .bind(topic_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联将该话题下的所有消息标记为逻辑删除
        sqlx::query("UPDATE messages SET deleted_at = ? WHERE topic_id = ? AND deleted_at IS NULL")
            .bind(now)
            .bind(topic_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        // 级联清除活跃生成注册表，杜绝已删除消息复活
        sqlx::query("DELETE FROM active_generations WHERE topic_id = ?")
            .bind(topic_id)
            .execute(&db.pool)
            .await
            .map_err(|e| e.to_string())?;

        if let Some(row) = parent_row {
            let owner_id: String = row.get("owner_id");
            let owner_type: String = row.get("owner_type");

            let mut tx = db.pool.begin().await.map_err(|e| e.to_string())?;
            if owner_type == "agent" {
                let _ = HashAggregator::bubble_agent_hash(&mut tx, &owner_id).await;
            } else if owner_type == "group" {
                let _ = HashAggregator::bubble_group_hash(&mut tx, &owner_id).await;
            }
            let _ = tx.commit().await;
        }

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

        Ok(())
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;

        // 1. 物理强清除已删除超过安全期（30天）的消息的预渲染缓存
        let render_cache =
            sqlx::query("DELETE FROM render_cache WHERE (topic_id, msg_id) IN (SELECT topic_id, msg_id FROM messages WHERE deleted_at IS NOT NULL AND deleted_at < ?)")
                .bind(threshold)
                .execute(&db.pool)
                .await
                .map_err(|e| e.to_string())?;

        // 2. 仅清空已删除超过安全期（30天）的消息的正文内容，保留消息的主键、角色与墓碑时间戳（防止多端同步幽灵复活，并释放大文本空间）
        let messages =
            sqlx::query("UPDATE messages SET content = '[已清空]' WHERE deleted_at IS NOT NULL AND deleted_at < ? AND content != '[已清空]'")
                .bind(threshold)
                .execute(&db.pool)
                .await
                .map_err(|e| e.to_string())?;

        log::info!(
            "[DeleteExecutor] Completed safety-period cleanup (older than {} days): cleared_messages_content={}, deleted_render_caches={}",
            days,
            messages.rows_affected(),
            render_cache.rows_affected()
        );

        Ok(())
    }
}
