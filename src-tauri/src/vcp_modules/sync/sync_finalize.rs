use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use crate::vcp_modules::sync_pipeline::SyncPipeline;
use crate::vcp_modules::sync_service::emit_sync_log;
use sqlx::Row;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;

pub struct SyncFinalizer;

impl SyncFinalizer {
    pub async fn execute(
        app_handle: &AppHandle,
        db: &DbState,
        write_queue: &DbWriteQueue,
        pipeline: &SyncPipeline,
        logger: &Arc<Mutex<SyncLogger>>,
        modified_topics: HashSet<String>,
    ) -> Result<(), String> {
        // 1. 强制落盘数据库写队列
        write_queue.flush().await;

        // 2. 全局 Hash 冒泡
        if !modified_topics.is_empty() {
            let start_instant = std::time::Instant::now();
            println!(
                "[SyncFinalizer] Finalizing {} modified topics (recalculating hashes)...",
                modified_topics.len()
            );
            emit_sync_log(
                app_handle,
                "info",
                &format!("正在校验 {} 个话题的一致性...", modified_topics.len()),
            );

            // [批量优化 Phase 1] 一次性批量预读取所有受影响话题的元数据到内存中，消灭循环内 N+1 读
            struct TopicBubbleMeta {
                owner_id: String,
                owner_type: String,
                title: String,
                created_at: i64,
                locked: bool,
                unread: bool,
            }

            let mut meta_map = std::collections::HashMap::new();
            let placeholders = modified_topics
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let query_sql = format!(
                "SELECT topic_id, owner_id, owner_type, title, created_at, locked, unread FROM topics WHERE topic_id IN ({})",
                placeholders
            );
            let mut q = sqlx::query(&query_sql);
            for tid in &modified_topics {
                q = q.bind(tid);
            }

            if let Ok(rows) = q.fetch_all(&db.pool).await {
                for row in rows {
                    let tid: String = row.get("topic_id");
                    meta_map.insert(
                        tid,
                        TopicBubbleMeta {
                            owner_id: row.get("owner_id"),
                            owner_type: row.get("owner_type"),
                            title: row.get("title"),
                            created_at: row.get("created_at"),
                            locked: row.get::<i64, _>("locked") != 0,
                            unread: row.get::<i64, _>("unread") != 0,
                        },
                    );
                }
            }

            let mut bubbled_topics = 0usize;

            if let Ok(mut tx) = db.pool.begin().await {
                // 1. [Batch Optimization] 一条 SQL 更新所有受影响话题的消息计数和时间戳
                let placeholders = modified_topics
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "UPDATE topics SET
                        msg_count = (SELECT COUNT(*) FROM messages WHERE messages.topic_id = topics.topic_id AND deleted_at IS NULL),
                        updated_at = ?
                     WHERE topic_id IN ({})",
                    placeholders
                );
                let mut query = sqlx::query(&sql).bind(chrono::Utc::now().timestamp_millis());
                for tid in &modified_topics {
                    query = query.bind(tid);
                }
                let _ = query.execute(&mut *tx).await;

                // 2. 逐话题计算指纹并向上冒泡（使用传参版接口，彻底避免折返 SELECT）
                let mut affected_agents: HashSet<String> = HashSet::new();
                let mut affected_groups: HashSet<String> = HashSet::new();

                for tid in &modified_topics {
                    if let Some(meta) = meta_map.get(tid) {
                        if let Err(e) = HashAggregator::bubble_topic_hash_with_meta(
                            &mut tx,
                            tid,
                            &meta.owner_type,
                            &meta.title,
                            meta.created_at,
                            meta.locked,
                            meta.unread,
                        )
                        .await
                        {
                            println!(
                                "[SyncFinalizer] bubble_topic_hash_with_meta failed for {}: {}",
                                tid, e
                            );
                            if let Ok(mut l) = logger.lock() {
                                l.log(
                                    LogLevel::Error,
                                    "finalize",
                                    &format!("Bubble topic hash failed for {}: {}", tid, e),
                                );
                            }
                        } else {
                            bubbled_topics += 1;
                        }

                        // 直接从内存提取 owner 归属，杜绝 N+1 读
                        if meta.owner_type == "agent" {
                            affected_agents.insert(meta.owner_id.clone());
                        } else if meta.owner_type == "group" {
                            affected_groups.insert(meta.owner_id.clone());
                        }
                    }
                }

                let agent_count = affected_agents.len();
                let group_count = affected_groups.len();

                for aid in affected_agents {
                    let _ = HashAggregator::bubble_agent_hash(&mut tx, &aid).await;
                }
                for gid in affected_groups {
                    let _ = HashAggregator::bubble_group_hash(&mut tx, &gid).await;
                }

                match tx.commit().await {
                    Ok(_) => {
                        let elapsed = start_instant.elapsed();
                        let success_msg = format!(
                            "[SyncFinalizer] 一致性校验校验成功！耗时: {:?}. 冒泡话题: {}, 级联智能体: {}, 级联群组: {}.",
                            elapsed, bubbled_topics, agent_count, group_count
                        );
                        println!("{}", success_msg);
                        emit_sync_log(app_handle, "success", &success_msg);
                    }
                    Err(e) => {
                        let err_msg = format!("[SyncFinalizer] Transaction commit failed: {}", e);
                        println!("{}", err_msg);
                        emit_sync_log(app_handle, "error", &err_msg);
                        return Err(err_msg);
                    }
                }
            }
        }

        // 3. 推进 Pipeline 状态
        let _ = pipeline.on_messages_done().await;

        Ok(())
    }
}
