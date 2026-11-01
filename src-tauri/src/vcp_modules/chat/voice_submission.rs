//! A voice turn enters the same chat owners as a typed message. Group admission
//! waits for the interrupted turn to release its owner before any
//! persistence or model request starts. The call owner claims each stable origin
//! once; a database upsert is not a substitute for that admission.

use crate::vcp_modules::agent_chat_application_service::{
    process_prepared_agent_chat_message, AgentChatPayload, PreparedAgentRequest,
};
use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::chat::voice_service::VoiceReplyPolicy;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_chat_application_service::{
    process_prepared_group_chat_message, GroupChatParams,
};
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::infra::vcp_http_endpoint::ChatRequestPurpose;
use crate::vcp_modules::message_service::{self, MessageWriteResultDto};
use crate::vcp_modules::settings_manager::{read_config, SettingsState};
use crate::vcp_modules::topic_types::TopicKey;
use crate::vcp_modules::vcp_client::{
    freeze_chat_connection, ActiveGroupTurnLease, ActiveGroupTurns, ActiveRequests,
};
use serde_json::{json, Value};
use std::sync::Arc;
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
    reply_policy: Arc<VoiceReplyPolicy>,
}

/// Register only in memory. Admission observes the voice cancellation token;
/// dropping an unaccepted preparation releases its owner without persistence.
/// Output binding can call the Rust narration consumer, so call this outside
/// the voice call's state lock.
pub(crate) async fn prepare_voice_submission(
    app: &AppHandle,
    target: TopicKey,
    origin: String,
    text: &str,
    cancellation: CancellationToken,
    reply_policy: Arc<VoiceReplyPolicy>,
) -> Result<PreparedVoiceSubmission, String> {
    let message = ChatMessage {
        id: origin,
        role: "user".into(),
        content: text.to_owned(),
        timestamp: chrono::Utc::now().timestamp_millis().max(0) as u64,
        ..Default::default()
    };
    let owner = if target.owner_type == "group" {
        let turn =
            acquire_voice_group_turn(&app.state::<ActiveGroupTurns>(), &target, &cancellation)
                .await?;
        turn.set_origin_user_message(&message.id)?;
        turn.bind_optional_output(app, None)?;
        PreparedChatOwner::Group(turn)
    } else {
        let request = PreparedAgentRequest::acquire(
            app,
            &app.state::<ActiveRequests>(),
            &target.owner_id,
            &target.topic_id,
            &message.id,
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
        reply_policy,
    })
}

async fn acquire_voice_group_turn(
    turns: &ActiveGroupTurns,
    target: &TopicKey,
    cancellation: &CancellationToken,
) -> Result<ActiveGroupTurnLease, String> {
    if cancellation.is_cancelled() {
        return Err("语音提交已取消。".into());
    }
    // Speech already requested a stop. Repeat once at submission in case
    // another input started a turn meanwhile; never remove a live owner's lease.
    turns.cancel(target);
    loop {
        if cancellation.is_cancelled() {
            return Err("语音提交已取消。".into());
        }
        // A new group turn has its own hard-stop lifetime. A later typed
        // input may join it independently of this voice origin's token.
        let released = match turns.try_acquire_or_wait(target.clone()) {
            Ok(turn) => return Ok(turn),
            Err(released) => released,
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err("语音提交已取消。".into()),
            _ = released => {}
        }
    }
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
        let Self {
            app,
            target,
            mut message,
            cancellation,
            owner,
            reply_policy,
        } = self;
        if cancellation.is_cancelled() {
            return Ok(json!({ "status": "cancelled" }));
        }
        let settings = read_config(app.clone(), app.state::<SettingsState>()).await?;
        message.name = Some(settings.identity.user_name.clone());
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
                    Some(reply_policy),
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
                    Some(reply_policy),
                    Some(cancellation),
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

    #[tokio::test(start_paused = true)]
    async fn group_admission_waits_for_release_and_keeps_the_new_turn_independent() {
        let turns = ActiveGroupTurns::default();
        let target = TopicKey::new("group", "group", "topic");
        let old = turns.try_acquire(target.clone()).unwrap();
        let cancellation = CancellationToken::new();
        let mut admission = Box::pin(acquire_voice_group_turn(&turns, &target, &cancellation));
        assert!(futures_util::poll!(&mut admission).is_pending());
        assert!(old.is_cancelled());
        assert!(turns.try_acquire(target.clone()).is_err());

        tokio::time::advance(std::time::Duration::from_secs(30)).await;
        assert!(futures_util::poll!(&mut admission).is_pending());

        drop(old);
        let std::task::Poll::Ready(Ok(next)) = futures_util::poll!(&mut admission) else {
            panic!("release must resume admission without advancing the clock");
        };
        drop(admission);
        cancellation.cancel();
        assert!(
            !next.is_cancelled(),
            "the group and voice origin have separate lifetimes"
        );
        assert!(turns.try_acquire(target.clone()).is_err());
        drop(next);
        assert!(turns.try_acquire(target).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn group_admission_follows_a_competing_owner_without_cancelling_it() {
        let turns = ActiveGroupTurns::default();
        let target = TopicKey::new("group", "group", "topic");
        let old = turns.try_acquire(target.clone()).unwrap();
        let cancellation = CancellationToken::new();
        let mut admission = Box::pin(acquire_voice_group_turn(&turns, &target, &cancellation));
        assert!(futures_util::poll!(&mut admission).is_pending());
        drop(old);
        let competing = turns.try_acquire(target.clone()).unwrap();
        assert!(futures_util::poll!(&mut admission).is_pending());
        assert!(!competing.is_cancelled());
        drop(competing);
        assert!(matches!(
            futures_util::poll!(&mut admission),
            std::task::Poll::Ready(Ok(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_group_admission_does_not_acquire_after_release() {
        let turns = ActiveGroupTurns::default();
        let target = TopicKey::new("group", "group", "topic");
        let old = turns.try_acquire(target.clone()).unwrap();
        let cancellation = CancellationToken::new();
        let mut admission = Box::pin(acquire_voice_group_turn(&turns, &target, &cancellation));
        assert!(futures_util::poll!(&mut admission).is_pending());
        cancellation.cancel();
        drop(old);
        assert!(admission.await.is_err());
        assert!(turns.try_acquire(target).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn group_voice_replaces_a_cancelled_jev_session_instead_of_joining_it() {
        let turns = ActiveGroupTurns::default();
        let target = TopicKey::new("group", "group", "topic");
        let old = turns.try_acquire(target.clone()).unwrap();
        let old_session = old.start_session("user_message", false).unwrap();
        let cancellation = CancellationToken::new();
        let mut admission = Box::pin(acquire_voice_group_turn(&turns, &target, &cancellation));
        assert!(futures_util::poll!(&mut admission).is_pending());
        assert!(old_session.cancellation.is_cancelled());
        assert!(old_session.reserve().is_err());

        drop(old);
        let next = admission.await.unwrap();
        let next_session = next.start_session("user_message", false).unwrap();
        assert_ne!(next_session.turn_id, old_session.turn_id);
        assert!(next_session.reserve().is_ok());
    }

    #[tokio::test]
    async fn already_cancelled_voice_admission_leaves_the_current_group_alone() {
        let turns = ActiveGroupTurns::default();
        let target = TopicKey::new("group", "group", "topic");
        let old = turns.try_acquire(target.clone()).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(acquire_voice_group_turn(&turns, &target, &cancellation)
            .await
            .is_err());
        assert!(!old.is_cancelled());
    }

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
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().app.app_directories_override =
            Some(tauri::utils::config::AppDirectoriesOverride::Root(
                "test-fixtures/voice-submission".into(),
            ));
        let app = tauri::test::mock_builder().build(context).unwrap();
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
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    async fn interrupted_agent_reply_persists_partial_text_and_returns_the_transport_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (app, target, message) = fixture().await;
        let db = app.state::<DbState>();
        for migration in [
            include_str!("../../../migrations/0102_stream_stop_intent.sql"),
            include_str!("../../../migrations/0105_agent_voice_config.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&db.pool).await.unwrap();
        }
        app.manage(AgentConfigState::new());
        app.manage(ActiveRequests::default());
        app.manage(crate::vcp_modules::infra::lifecycle_state::LifecycleState::new());
        let request = PreparedAgentRequest::acquire(
            app.handle(),
            &app.state::<ActiveRequests>(),
            &target.owner_id,
            &target.topic_id,
            &message.id,
            None,
            CancellationToken::new(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert!(socket.read(&mut [0; 4096]).await.unwrap() > 0);
            let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"partial reply\"}}]}\n\n";
            // Close without the last HTTP chunk, after sending a complete SSE frame.
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{frame}\r\n",
                frame.len(),
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let policy = VoiceReplyPolicy::from_settings(&Default::default());
        let error = process_prepared_agent_chat_message(
            app.handle().clone(),
            app.state(),
            app.state(),
            request,
            AgentChatPayload {
                agent_id: target.owner_id,
                topic_id: target.topic_id,
                user_message: message,
            },
            crate::vcp_modules::vcp_client::ChatConnectionSnapshot {
                endpoint_url,
                api_key: String::new(),
            },
            None,
            true,
            Some(Arc::new(policy)),
        )
        .await
        .expect_err("a persisted error must reach the voice call owner");
        assert!(error.starts_with("SSE stream read failed:"), "{error}");
        let (content, finish_reason): (String, String) =
            sqlx::query_as("SELECT content, finish_reason FROM messages WHERE role = 'assistant'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(finish_reason, "error");
        assert_eq!(content, format!("partial reply\n\n> VCP流式错误: {error}"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_before_persistence_writes_nothing() {
        let (app, target, mut message) = fixture().await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let may_generate = persist_voice_message(
            app.handle(),
            &target,
            &mut message,
            &cancellation,
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
            app.handle(),
            &target,
            &mut message,
            &cancellation,
            |message, result| committed = Some((message, result)),
        ));
        // Enter the real append operation while its writer is occupied. Once
        // started it must reach its transaction result despite a stop signal.
        assert!(futures_util::poll!(&mut append).is_pending());
        cancellation.cancel();
        blocker.commit().await.unwrap();
        assert!(!append.await.unwrap());
        let (reported, result) = committed.expect("commit callback is not cancelled");
        let row: (String, String, i64) =
            sqlx::query_as("SELECT msg_id, content, timestamp FROM messages WHERE role = 'user'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            row,
            (reported.id, reported.content, reported.timestamp as i64)
        );
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
            app.handle(),
            &target,
            &mut message,
            &cancellation,
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
