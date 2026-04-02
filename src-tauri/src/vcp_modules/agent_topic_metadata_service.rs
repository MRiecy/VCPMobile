use crate::vcp_modules::agent_config_repository_fs::TopicInfo;
use sqlx::{Pool, Sqlite};

/// AgentTopicMetadataService: 负责将 Agent 的话题元数据同步到数据库索引表
pub struct AgentTopicMetadataService;

impl AgentTopicMetadataService {
    /// 将话题列表同步到 `topic_index` 数据库表
    /// 注意：这里只做插入或更新基本信息，不覆盖 msg_count 等动态数据
    pub async fn sync_topics_to_db(
        pool: &Pool<Sqlite>,
        agent_id: &str,
        topics: &[TopicInfo],
    ) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        for topic in topics {
            sqlx::query(
                "INSERT INTO topic_index (topic_id, agent_id, title, mtime, locked, unread, unread_count)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(topic_id) DO UPDATE SET
                    title=excluded.title,
                    mtime=excluded.mtime"
            )
            .bind(&topic.id)
            .bind(agent_id)
            .bind(&topic.name)
            .bind(topic.created_at)
            .bind(false)
            .bind(false)
            .bind(0)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}
