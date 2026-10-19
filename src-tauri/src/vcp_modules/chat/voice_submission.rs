//! A voice turn enters the same chat owners as a typed message. Admission is
//! synchronous so the call owner can retain the exact cancellation token before
//! any persistence or model request starts. The call owner claims each stable
//! origin once; a database upsert is not a substitute for that admission.

use crate::vcp_modules::agent_chat_application_service::{
    process_prepared_agent_chat_message, AgentChatPayload, PreparedAgentRequest,
};
use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_chat_application_service::{
    process_prepared_group_chat_message, GroupChatParams,
};
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::message_service::{self, MessageWriteResultDto};
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use crate::vcp_modules::topic_types::TopicKey;
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, ActiveGroupTurnLease, ActiveGroupTurns, ActiveRequests,
    ChatRequestPurpose,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

enum PreparedChatOwner {
    Agent(PreparedAgentRequest),
    Group(ActiveGroupTurnLease),
}

pub(crate) struct PreparedVoiceSubmission {
    app: AppHandle,
    target: TopicKey,
    message: ChatMessage,
    cancellation: CancellationToken,
    owner: PreparedChatOwner,
}

/// Register only in memory. A token cancelled before registration stays
/// cancelled in the real owner; dropping an unaccepted preparation releases it.
/// Output binding can call the Rust narration consumer, so call this outside
/// the voice call's state lock.
pub(crate) fn prepare_voice_submission(
    app: &AppHandle,
    target: TopicKey,
    origin: String,
    text: String,
    cancellation: CancellationToken,
) -> Result<PreparedVoiceSubmission, String> {
    if !target.is_valid() || origin.is_empty() || text.trim().is_empty() {
        return Err("语音消息的会话、标识或正文无效。".into());
    }
    let message = ChatMessage {
        id: origin,
        role: "user".into(),
        content: text.trim().to_string(),
        timestamp: chrono::Utc::now().timestamp_millis().max(0) as u64,
        ..Default::default()
    };
    let owner = if target.owner_type == "group" {
        let turn = app.state::<ActiveGroupTurns>().try_acquire_with_cancellation(
            target.clone(),
            cancellation.clone(),
        )?;
        turn.set_origin_user_message(&message.id)?;
        turn.bind_optional_output(app, None)?;
        PreparedChatOwner::Group(turn)
    } else {
        let request = PreparedAgentRequest::acquire(
            app,
            &app.state::<ActiveRequests>(),
            &AgentChatPayload {
                agent_id: target.owner_id.clone(),
                topic_id: target.topic_id.clone(),
                user_message: message.clone(),
            },
            None,
            cancellation.clone(),
        )?;
        PreparedChatOwner::Agent(request)
    };
    Ok(PreparedVoiceSubmission {
        app: app.clone(),
        target,
        message,
        cancellation,
        owner,
    })
}

impl PreparedVoiceSubmission {
    /// Consume an admitted turn once. The caller must await this future through
    /// persistence; cancellation signals the token, never aborts the transaction.
    /// The callback reports committed user content even if a simultaneous stop
    /// prevents the model request, or later model preparation fails.
    pub(crate) async fn execute(
        self,
        on_persisted: impl FnOnce(ChatMessage, MessageWriteResultDto),
    ) -> Result<Value, String> {
        let Self { app, target, mut message, cancellation, owner } = self;
        if cancellation.is_cancelled() {
            return Ok(json!({ "status": "cancelled" }));
        }
        let settings = read_settings(app.clone(), app.state::<SettingsState>()).await?;
        message.name = Some(settings.user_name.clone());
        if !persist_voice_message(&app, &target, &mut message, &cancellation, on_persisted).await? {
            return Ok(json!({ "status": "cancelled" }));
        }
        let connection = freeze_chat_connection(&settings, ChatRequestPurpose::Interactive)?;
        match owner {
            PreparedChatOwner::Agent(request) => {
                process_prepared_agent_chat_message(
                    app.clone(),
                    app.state::<AgentConfigState>(),
                    app.state::<DbState>(),
                    request,
                    AgentChatPayload {
                        agent_id: target.owner_id,
                        topic_id: target.topic_id,
                        user_message: message,
                    },
                    connection,
                    None,
                    false,
                )
                .await
            }
            PreparedChatOwner::Group(turn) => {
                process_prepared_group_chat_message(
                    app.clone(),
                    app.state::<GroupManagerState>(),
                    app.state::<AgentConfigState>(),
                    app.state::<DbState>(),
                    app.state::<ActiveRequests>(),
                    turn,
                    GroupChatParams {
                        group_id: target.owner_id,
                        topic_id: target.topic_id,
                        user_message: message,
                        connection,
                        stream_channel: None,
                    },
                    false,
                )
                .await
            }
        }
    }
}

async fn persist_voice_message<R: Runtime>(
    app: &AppHandle<R>,
    target: &TopicKey,
    message: &mut ChatMessage,
    cancellation: &CancellationToken,
    on_persisted: impl FnOnce(ChatMessage, MessageWriteResultDto),
) -> Result<bool, String> {
    if cancellation.is_cancelled() {
        return Ok(false);
    }
    let db = app.state::<DbState>();
    // Once admitted to append, await the actual commit result. Selecting the
    // cancellation token against database IO could lose knowledge of a commit.
    let persisted = message_service::append_single_message(
        app.clone(),
        &db.pool,
        &target.owner_id,
        &target.owner_type,
        target.topic_id.clone(),
        message.clone(),
    )
    .await?;
    message.blocks = Some(json!(&persisted.blocks));
    on_persisted(message.clone(), persisted);
    Ok(!cancellation.is_cancelled())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tauri::App<tauri::test::MockRuntime>, TopicKey, ChatMessage) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO agents (owner_type, agent_id, name, model, updated_at)
             VALUES ('agent', 'voice-agent', 'Agent', 'test', 1);
             INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at)
             VALUES ('voice-topic', 'agent', 'voice-agent', 'Voice', 1, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(DbState::new(pool, std::path::PathBuf::new()));
        let target = TopicKey::new("agent", "voice-agent", "voice-topic");
        let message = ChatMessage {
            id: "stable-voice-origin".into(),
            role: "user".into(),
            name: Some("用户".into()),
            content: "已经录好的语音".into(),
            timestamp: 100,
            ..Default::default()
        };
        (app, target, message)
    }

    #[tokio::test]
    async fn cancellation_before_persistence_writes_nothing() {
        let (app, target, mut message) = fixture().await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let may_generate = persist_voice_message(
            app.handle(), &target, &mut message, &cancellation,
            |_, _| panic!("cancelled admission must not report a commit"),
        )
        .await
        .unwrap();
        assert!(!may_generate);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn cancellation_during_append_reports_the_real_commit_and_prevents_generation() {
        let (app, target, mut message) = fixture().await;
        let db = app.state::<DbState>();
        let blocker = db.write_transaction("test.voice.blocker").await.unwrap();
        let cancellation = CancellationToken::new();
        let mut committed = None;
        let mut append = Box::pin(persist_voice_message(
            app.handle(), &target, &mut message, &cancellation,
            |message, result| committed = Some((message, result)),
        ));
        // Enter the real append operation while its writer is occupied. Once
        // started it must reach its transaction result despite a stop signal.
        assert!(futures_util::poll!(&mut append).is_pending());
        cancellation.cancel();
        blocker.commit().await.unwrap();
        assert!(!append.await.unwrap());
        let (reported, result) = committed.expect("commit callback is not cancelled");
        let row: (String, String, i64) = sqlx::query_as(
            "SELECT msg_id, content, timestamp FROM messages WHERE role = 'user'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(row, (reported.id, reported.content, reported.timestamp as i64));
        assert_eq!(reported.blocks, Some(json!(&result.blocks)));
        let mut connection = db.pool.acquire().await.unwrap();
        let activity = crate::vcp_modules::sync_hash::HashAggregator::load_topic_activity(
            &mut connection,
            &target,
        )
        .await
        .unwrap();
        assert_eq!(result.topic_updated_at, activity.updated_at);
        assert_eq!(activity.msg_count, 1);
    }

    #[tokio::test]
    async fn cancellation_after_commit_callback_still_prevents_generation() {
        let (app, target, mut message) = fixture().await;
        let cancellation = CancellationToken::new();
        let may_generate = persist_voice_message(
            app.handle(), &target, &mut message, &cancellation,
            |_, _| cancellation.cancel(),
        )
        .await
        .unwrap();
        assert!(!may_generate);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role = 'user'")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
