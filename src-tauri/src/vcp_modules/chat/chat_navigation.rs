use super::chat_focus::{
    DeleteScope, FocusSnapshot, FocusTarget, CALL_IN_PROGRESS, DELETING, STALE,
};
use super::topic_service::{TopicSortMode, TOPIC_LIST_QUERY};
use super::voice_service::{self, VoiceCallState};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::OwnerKey;
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};
use tauri::{AppHandle, Runtime, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedView {
    document_id: u64,
    focus: FocusSnapshot,
}

#[derive(Serialize)]
pub struct RegisteredIntent {
    focus: FocusSnapshot,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NavigationResult {
    Accepted { focus: FocusSnapshot },
    CallInProgress,
    Stale,
    Missing,
}

impl NavigationResult {
    fn from_focus(result: Result<FocusSnapshot, String>) -> Result<Self, String> {
        match result {
            Ok(focus) => Ok(Self::Accepted { focus }),
            Err(error) => Self::from_admission(error),
        }
    }

    fn from_admission(error: String) -> Result<Self, String> {
        match error.as_str() {
            STALE => Ok(Self::Stale),
            CALL_IN_PROGRESS => Ok(Self::CallInProgress),
            DELETING => Ok(Self::Stale),
            _ => Err(error),
        }
    }
}

#[tauri::command]
pub fn connect_chat_view<R: Runtime>(
    app: AppHandle<R>,
    webview: tauri::Webview<R>,
    calls: State<'_, VoiceCallState>,
) -> Result<ConnectedView, String> {
    let (document_id, focus) = calls.attach_chat_document()?;
    // Check after issuing the inert ticket: a replaced native host cannot
    // receive a ticket newer than the host that already superseded it.
    if !crate::vcp_modules::vcp_client::is_current_webview(&app, &webview) {
        return Err(STALE.into());
    }
    Ok(ConnectedView { document_id, focus })
}

#[tauri::command]
pub fn register_chat_navigation_intent(
    calls: State<'_, VoiceCallState>,
    document_id: u64,
    sequence: u64,
) -> Result<Option<RegisteredIntent>, String> {
    match calls.begin_navigation(document_id, sequence) {
        Ok(focus) => Ok(Some(RegisteredIntent { focus })),
        Err(error) if error == STALE => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct Selection<'a> {
    anchor_id: Option<&'a str>,
    select_recent: bool,
    remembered_topic_id: Option<&'a str>,
    sort_mode: TopicSortMode,
    restoring: bool,
}

async fn live_topic(
    connection: &mut SqliteConnection,
    owner: &OwnerKey,
    topic_id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM topics WHERE owner_type=? AND owner_id=? AND topic_id=? AND deleted_at IS NULL)",
    )
    .bind(&owner.owner_type)
    .bind(&owner.owner_id)
    .bind(topic_id)
    .fetch_one(connection)
    .await
}

/// Selection uses one SQLite read snapshot. Its result is only a candidate until
/// the call owner rechecks the document, intent and deletion scope at commit.
async fn resolve_target(
    pool: &SqlitePool,
    scope: Option<&DeleteScope>,
    selection: &Selection<'_>,
) -> Result<Option<FocusTarget>, sqlx::Error> {
    let Some(scope) = scope else {
        return Ok(selection
            .anchor_id
            .is_none()
            .then_some(FocusTarget::Welcome));
    };
    let (owner, requested_topic) = match scope {
        DeleteScope::Owner(owner) => (owner.clone(), None),
        DeleteScope::Topic(topic) => (
            OwnerKey::new(&topic.owner_type, &topic.owner_id),
            Some(topic.topic_id.as_str()),
        ),
    };
    let mut tx = pool.begin().await?;
    let owner_query = match owner.owner_type.as_str() {
        "agent" => "SELECT EXISTS (SELECT 1 FROM agents WHERE owner_type='agent' AND agent_id=? AND deleted_at IS NULL)",
        "group" => "SELECT EXISTS (SELECT 1 FROM groups WHERE owner_type='group' AND group_id=? AND deleted_at IS NULL)",
        _ => return Ok(selection.restoring.then_some(FocusTarget::Welcome)),
    };
    let available: bool = sqlx::query_scalar(owner_query)
        .bind(&owner.owner_id)
        .fetch_one(&mut *tx)
        .await?;
    if !available {
        return Ok(selection.restoring.then_some(FocusTarget::Welcome));
    }
    let mut topic_id = None;
    if let Some(requested) = requested_topic {
        if live_topic(&mut tx, &owner, requested).await? {
            topic_id = Some(requested.to_owned());
        } else if !selection.restoring {
            return Ok(None);
        }
    } else if selection.select_recent {
        if let Some(remembered) = selection.remembered_topic_id {
            if live_topic(&mut tx, &owner, remembered).await? {
                topic_id = Some(remembered.to_owned());
            }
        }
        if topic_id.is_none() {
            let query = format!("{TOPIC_LIST_QUERY} LIMIT 1");
            topic_id = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
                .bind(&owner.owner_id)
                .bind(&owner.owner_type)
                .bind(selection.sort_mode.query_code())
                .fetch_optional(&mut *tx)
                .await?;
        }
    }
    if let Some(anchor) = selection.anchor_id {
        let Some(topic) = &topic_id else {
            return Ok(None);
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM messages WHERE owner_type=? AND owner_id=? AND topic_id=? AND msg_id=? AND deleted_at IS NULL)",
        )
        .bind(&owner.owner_type)
        .bind(&owner.owner_id)
        .bind(topic)
        .bind(anchor)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Ok(None);
        }
    }
    tx.commit().await?;
    Ok(Some(match topic_id {
        Some(topic_id) => FocusTarget::Topic {
            owner_type: owner.owner_type,
            owner_id: owner.owner_id,
            topic_id,
        },
        None => FocusTarget::Owner {
            owner_type: owner.owner_type,
            owner_id: owner.owner_id,
        },
    }))
}

async fn navigate(
    pool: &SqlitePool,
    calls: &VoiceCallState,
    document_id: u64,
    sequence: u64,
    target: FocusTarget,
    selection: Selection<'_>,
) -> Result<NavigationResult, String> {
    let scope = target.scope();
    if let Err(error) = calls.validate_navigation(document_id, sequence, scope.clone()) {
        return NavigationResult::from_admission(error);
    }
    let candidate = resolve_target(pool, scope.as_ref(), &selection).await;
    let target = match candidate {
        Ok(Some(target)) => target,
        other => {
            if let Err(error) = calls.validate_navigation(document_id, sequence, scope) {
                return NavigationResult::from_admission(error);
            }
            return match other {
                Err(error) => Err(error.to_string()),
                _ => Ok(NavigationResult::Missing),
            };
        }
    };
    NavigationResult::from_focus(calls.commit_navigation(
        document_id,
        sequence,
        target,
        selection.anchor_id.map(str::to_owned),
    ))
}

#[tauri::command]
pub async fn restore_chat_view(
    db: State<'_, DbState>,
    calls: State<'_, VoiceCallState>,
    document_id: u64,
    hint: Option<FocusTarget>,
) -> Result<NavigationResult, String> {
    restore(&db.pool, &calls, document_id, hint.unwrap_or_default()).await
}

async fn restore(
    pool: &SqlitePool,
    calls: &VoiceCallState,
    document_id: u64,
    hint: FocusTarget,
) -> Result<NavigationResult, String> {
    let scope = hint.scope();
    match calls.begin_restore(document_id, scope.clone()) {
        Ok(focus) if focus.focus_revision != 0 => return Ok(NavigationResult::Accepted { focus }),
        Err(error) if error == DELETING => return restore_welcome(calls, document_id),
        Err(error) => return NavigationResult::from_admission(error),
        _ => {}
    }
    let candidate = resolve_target(
        pool,
        scope.as_ref(),
        &Selection {
            restoring: true,
            ..Selection::default()
        },
    )
    .await;
    let target = match candidate {
        Ok(target) => target.unwrap_or_default(),
        Err(error) => {
            return match calls.document_focus(document_id) {
                Ok(focus) if focus.focus_revision != 0 => Ok(NavigationResult::Accepted { focus }),
                Err(error) => NavigationResult::from_admission(error),
                _ => Err(error.to_string()),
            }
        }
    };
    match calls.commit_restore(document_id, target) {
        // A committed deletion can invalidate this hint between reading and
        // applying it. Establish a baseline without overriding a newer winner.
        Err(error) if error == STALE || error == DELETING => restore_welcome(calls, document_id),
        result => NavigationResult::from_focus(result),
    }
}

fn restore_welcome(calls: &VoiceCallState, document_id: u64) -> Result<NavigationResult, String> {
    match calls.begin_restore(document_id, None) {
        Ok(focus) if focus.focus_revision != 0 => Ok(NavigationResult::Accepted { focus }),
        Ok(_) => {
            NavigationResult::from_focus(calls.commit_restore(document_id, FocusTarget::Welcome))
        }
        Err(error) => NavigationResult::from_admission(error),
    }
}

#[tauri::command]
pub async fn request_chat_navigation(
    db: State<'_, DbState>,
    calls: State<'_, VoiceCallState>,
    document_id: u64,
    sequence: u64,
    target: FocusTarget,
    anchor_id: Option<String>,
    select_recent: Option<bool>,
    remembered_topic_id: Option<String>,
    sort_mode: Option<TopicSortMode>,
) -> Result<NavigationResult, String> {
    if sequence == 0 {
        return Ok(NavigationResult::Stale);
    }
    navigate(
        &db.pool,
        &calls,
        document_id,
        sequence,
        target,
        Selection {
            anchor_id: anchor_id.as_deref(),
            select_recent: select_recent.unwrap_or_default(),
            remembered_topic_id: remembered_topic_id.as_deref(),
            sort_mode: sort_mode.unwrap_or_default(),
            restoring: false,
        },
    )
    .await
}

/// The tombstone is already committed. Report cleanup errors separately, so a
/// native audio teardown failure cannot turn a successful deletion into a retry.
pub(crate) async fn observe_target_deletion<R: Runtime>(app: &AppHandle<R>, scope: &DeleteScope) {
    if let Err(error) = voice_service::target_deleted(app, scope).await {
        log::error!("[Chat] Cleanup after target deletion failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    #[test]
    fn a_replaced_webview_cannot_acquire_a_later_document_ticket() {
        let previous_app = tauri::test::mock_app();
        let app = tauri::test::mock_app();
        let view = |app: &tauri::App<tauri::test::MockRuntime>| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .build()
            .unwrap()
        };
        let old = view(&previous_app);
        let current = view(&app);
        app.manage(VoiceCallState::default());
        let calls = app.state::<VoiceCallState>();
        let active = connect_chat_view(
            app.handle().clone(),
            current.as_ref().clone(),
            calls.clone(),
        )
        .unwrap();
        calls.begin_navigation(active.document_id, 1).unwrap();
        assert_eq!(
            connect_chat_view(app.handle().clone(), old.as_ref().clone(), calls.clone())
                .err()
                .as_deref(),
            Some(STALE)
        );
        assert!(calls.begin_navigation(active.document_id, 2).is_ok());
    }

    async fn pool() -> SqlitePool {
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
            "INSERT INTO agents(owner_type,agent_id,name,model,updated_at) VALUES('agent','a','A','m',1);
             INSERT INTO groups(owner_type,group_id,name,updated_at) VALUES('group','a','G',1);
             INSERT INTO topics(owner_type,owner_id,topic_id,title,created_at,updated_at) VALUES
               ('agent','a','first','First',1,4),('agent','a','second','Second',2,3),
               ('group','a','first','First group',1,1);
             INSERT INTO messages(owner_type,owner_id,topic_id,msg_id,role,content,timestamp,created_at,updated_at) VALUES
               ('group','a','first','anchor','user','group only',1,1,1),
               ('agent','a','second','newer-anchor','user','accepted anchor',1,1,1);",
        ).execute(&pool).await.unwrap();
        pool
    }

    fn topic(id: &str) -> FocusTarget {
        FocusTarget::Topic {
            owner_type: "agent".into(),
            owner_id: "a".into(),
            topic_id: id.into(),
        }
    }

    #[tokio::test]
    async fn rejected_anchor_keeps_the_previous_focus_and_excludes_superseded_intents() {
        let db = pool().await;
        let calls = VoiceCallState::default();
        let (doc, _) = calls.attach_chat_document().unwrap();
        calls.begin_navigation(doc, 1).unwrap();
        let NavigationResult::Accepted { focus } =
            navigate(&db, &calls, doc, 1, topic("second"), Selection::default())
                .await
                .unwrap()
        else {
            panic!("initial topic was not accepted");
        };
        calls.begin_navigation(doc, 2).unwrap();
        calls.begin_navigation(doc, 3).unwrap();
        let anchor = Selection {
            anchor_id: Some("anchor"),
            ..Selection::default()
        };
        assert!(matches!(
            navigate(&db, &calls, doc, 3, topic("first"), anchor)
                .await
                .unwrap(),
            NavigationResult::Missing
        ));
        assert!(matches!(
            navigate(&db, &calls, doc, 2, topic("first"), Selection::default())
                .await
                .unwrap(),
            NavigationResult::Stale
        ));
        assert_eq!(calls.document_focus(doc).unwrap(), focus);
    }

    #[tokio::test]
    async fn selecting_an_owner_honors_the_remembered_live_topic_and_existing_sort_order() {
        let db = pool().await;
        let owner = DeleteScope::Owner(OwnerKey::new("agent", "a"));
        let mut selected = Selection {
            select_recent: true,
            remembered_topic_id: Some("gone"),
            ..Selection::default()
        };
        assert_eq!(
            resolve_target(&db, Some(&owner), &selected).await.unwrap(),
            Some(topic("second"))
        );
        selected.sort_mode = TopicSortMode::Updated;
        assert_eq!(
            resolve_target(&db, Some(&owner), &selected).await.unwrap(),
            Some(topic("first"))
        );
        selected.remembered_topic_id = Some("second");
        assert_eq!(
            resolve_target(&db, Some(&owner), &selected).await.unwrap(),
            Some(topic("second"))
        );
    }

    #[tokio::test]
    async fn cold_start_drops_only_the_missing_part_of_the_device_preference() {
        let db = pool().await;
        let cold = Selection {
            restoring: true,
            ..Selection::default()
        };
        let scope = topic("missing").scope().unwrap();
        let owner = FocusTarget::Owner {
            owner_type: "agent".into(),
            owner_id: "a".into(),
        };
        assert_eq!(
            resolve_target(&db, Some(&scope), &cold).await.unwrap(),
            Some(owner)
        );
        sqlx::query("UPDATE agents SET deleted_at=7")
            .execute(&db)
            .await
            .unwrap();
        assert_eq!(
            resolve_target(&db, Some(&scope), &cold).await.unwrap(),
            Some(FocusTarget::Welcome)
        );
    }

    #[tokio::test]
    async fn cold_restore_and_an_explicit_selection_can_finish_in_either_order() {
        let db = pool().await;
        let calls = VoiceCallState::default();
        let (document, _) = calls.attach_chat_document().unwrap();
        calls.begin_navigation(document, 1).unwrap();
        let NavigationResult::Accepted { focus } = restore(&db, &calls, document, topic("first"))
            .await
            .unwrap()
        else {
            panic!("a pending user choice must still have a baseline");
        };
        assert_eq!(focus.target, topic("first"));
        assert!(matches!(
            navigate(
                &db,
                &calls,
                document,
                1,
                topic("second"),
                Selection::default()
            )
            .await
            .unwrap(),
            NavigationResult::Accepted { .. }
        ));
        assert_eq!(
            calls.document_focus(document).unwrap().target,
            topic("second")
        );

        let calls = VoiceCallState::default();
        let (document, _) = calls.attach_chat_document().unwrap();
        let db_in_use = db.acquire().await.unwrap();
        let restore_hint = restore(&db, &calls, document, topic("first"));
        tokio::pin!(restore_hint);
        assert!(futures_util::poll!(&mut restore_hint).is_pending());
        calls.begin_navigation(document, 1).unwrap();
        let accepted = calls
            .commit_navigation(document, 1, topic("second"), Some("newer-anchor".into()))
            .unwrap();
        drop(db_in_use);
        let NavigationResult::Accepted { focus } = restore_hint.await.unwrap() else {
            panic!("restore must return the already accepted user choice");
        };
        assert_eq!(focus, accepted);
    }
}
