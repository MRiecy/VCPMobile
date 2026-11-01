use super::{
    chat_focus::Navigation,
    chat_manager::ChatMessage,
    message_service::{history_attachment_rows, project_history_rows, ROW_SELECT},
    topic_types::{resolve_topic_activity_updated_at, TopicKey},
    voice_service::VoiceCallState,
};
use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, FromRow, QueryBuilder, Row, Sqlite, SqliteConnection};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

const CAPACITY: usize = 500;
const BEFORE_ANCHOR: usize = 12;
const AFTER_ANCHOR: usize = 8;

#[derive(Clone, FromRow)]
struct Member {
    id: String,
    timestamp: i64,
    content_hash: String,
    finish_reason: Option<String>,
    updated_at: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct WindowId {
    key: TopicKey,
    focus_revision: u64,
    window_revision: u64,
}

pub(super) struct RetainedWindow {
    id: WindowId,
    members: Vec<Member>,
    pinned: bool,
}

/// Only the accepted window's bounded membership survives a WebView reset. No
/// message body, stream buffer or database transaction lives in this state.
#[derive(Default)]
pub(super) struct WindowReads {
    ticket: u64,
    window: Option<Arc<RetainedWindow>>,
}

pub(super) type StartedRead = (u64, Option<Arc<RetainedWindow>>, Option<String>);

impl Navigation {
    fn window_anchor(&self, document: u64, id: &WindowId) -> Result<Option<String>, String> {
        let focus = self
            .current_document_focus(document)
            .map_err(|_| "stale_window".to_string())?;
        if !focus.target.matches_topic(&id.key)
            || focus.focus_revision != id.focus_revision
            || focus.window_revision != id.window_revision
        {
            return Err("stale_window".into());
        }
        Ok(focus.anchor_id)
    }

    pub(super) fn start_window_read(
        &mut self,
        document: u64,
        id: &WindowId,
    ) -> Result<StartedRead, String> {
        let anchor = self.window_anchor(document, id)?;
        let reads = &mut self.window_reads;
        reads.ticket += 1;
        Ok((
            reads.ticket,
            reads.window.as_ref().filter(|w| w.id == *id).cloned(),
            anchor,
        ))
    }

    pub(super) fn finish_window_read(
        &mut self,
        document: u64,
        ticket: u64,
        window: &Arc<RetainedWindow>,
    ) -> Result<Option<Arc<RetainedWindow>>, String> {
        if self.window_reads.ticket != ticket {
            return Err("stale_window".into());
        }
        self.window_anchor(document, &window.id)?;
        Ok(self.window_reads.window.replace(window.clone()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownWindowRow {
    id: String,
    content_hash: Option<String>,
    finish_reason: Option<String>,
    updated_at: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatWindowProjection {
    member_ids: Vec<String>,
    messages: Vec<ChatMessage>,
    #[serde(rename = "hasMoreOlder")]
    has_older: bool,
    #[serde(rename = "hasMoreNewer")]
    has_newer: bool,
    pinned: bool,
    anchor_removed: bool,
    replacement_anchor_id: Option<String>,
    topic_activity: WindowActivity,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowActivity {
    msg_count: i64,
    updated_at: i64,
    unread: bool,
    unread_count: i64,
}

enum Selection<'a> {
    Latest,
    AtOrBefore(i64, &'a str),
    Before(i64, &'a str),
    After(i64, &'a str),
    From(i64, &'a str),
    Ids(Vec<&'a str>),
}

async fn select_members(
    conn: &mut SqliteConnection,
    key: &TopicKey,
    selection: Selection<'_>,
    limit: usize,
) -> Result<Vec<Member>, sqlx::Error> {
    let mut q = QueryBuilder::<Sqlite>::new(
        "SELECT msg_id AS id, timestamp, content_hash, finish_reason, updated_at FROM messages WHERE owner_type = ",
    );
    q.push_bind(&key.owner_type)
        .push(" AND owner_id = ")
        .push_bind(&key.owner_id)
        .push(" AND topic_id = ")
        .push_bind(&key.topic_id)
        .push(" AND deleted_at IS NULL");
    let ascending = matches!(selection, Selection::After(..));
    let range = match selection {
        Selection::Latest => None,
        Selection::Ids(ids) => {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            q.push(" AND msg_id IN (");
            let mut list = q.separated(",");
            for id in ids {
                list.push_bind(id);
            }
            list.push_unseparated(")");
            None
        }
        Selection::AtOrBefore(ts, id) => Some(("<=", ts, id)),
        Selection::Before(ts, id) => Some(("<", ts, id)),
        Selection::After(ts, id) => Some((">", ts, id)),
        Selection::From(ts, id) => Some((">=", ts, id)),
    };
    if let Some((op, ts, id)) = range {
        q.push(" AND (timestamp, msg_id) ")
            .push(op)
            .push(" (")
            .push_bind(ts)
            .push(",")
            .push_bind(id)
            .push(")");
    }
    q.push(if ascending {
        " ORDER BY timestamp, msg_id"
    } else {
        " ORDER BY timestamp DESC, msg_id DESC"
    })
    .push(" LIMIT ")
    .push_bind(limit as i64);
    let mut members: Vec<Member> = q.build_query_as().fetch_all(conn).await?;
    if !ascending {
        members.reverse();
    }
    Ok(members)
}

struct WindowData {
    members: Vec<Member>,
    pinned: bool,
    has_older: bool,
    has_newer: bool,
    anchor_removed: bool,
    replacement_anchor_id: Option<String>,
    activity: WindowActivity,
}

#[derive(Deserialize)]
pub struct WindowCursor {
    timestamp: i64,
    id: String,
}

#[derive(Clone, Copy)]
enum WindowRead<'a> {
    Open,
    Older { pin: bool, before: &'a WindowCursor },
    Reconcile { follow_latest: bool },
}

async fn select_window(
    conn: &mut SqliteConnection,
    key: &TopicKey,
    previous: Option<&RetainedWindow>,
    anchor_id: Option<&str>,
    read: WindowRead<'_>,
    limit: usize,
) -> Result<WindowData, String> {
    let row = sqlx::query("SELECT msg_count, updated_at, last_message_updated_at, created_at, unread, unread_count FROM topics WHERE owner_type=? AND owner_id=? AND topic_id=? AND deleted_at IS NULL")
        .bind(&key.owner_type).bind(&key.owner_id).bind(&key.topic_id).fetch_optional(&mut *conn)
        .await.map_err(|e| e.to_string())?.ok_or("stale_window")?;
    let activity = WindowActivity {
        msg_count: row.get("msg_count"),
        updated_at: resolve_topic_activity_updated_at(
            row.get("updated_at"),
            row.get("last_message_updated_at"),
            row.get("created_at"),
        ),
        unread: row.get("unread"),
        unread_count: row.get("unread_count"),
    };
    let anchor: Option<(i64, Option<i64>)> = if let Some(id) = anchor_id {
        sqlx::query_as("SELECT timestamp, deleted_at FROM messages WHERE owner_type=? AND owner_id=? AND topic_id=? AND msg_id=?")
            .bind(&key.owner_type).bind(&key.owner_id).bind(&key.topic_id).bind(id)
            .fetch_optional(&mut *conn).await.map_err(|e| e.to_string())?
    } else {
        None
    };
    let anchor_removed = anchor_id.is_some() && !matches!(anchor, Some((_, None)));
    let older = matches!(read, WindowRead::Older { .. });
    let follow_latest = matches!(
        read,
        WindowRead::Reconcile {
            follow_latest: true
        }
    );
    let mut pinned = anchor_id.is_some()
        || previous.is_some_and(|p| p.pinned)
        || matches!(read, WindowRead::Older { pin: true, .. });
    let mut members = if let Some(prev) = previous {
        let existing = select_members(
            conn,
            key,
            Selection::Ids(prev.members.iter().map(|m| m.id.as_str()).collect()),
            CAPACITY,
        )
        .await
        .map_err(|e| e.to_string())?;
        if pinned || older {
            existing
        } else {
            let range = match prev.members.first() {
                Some(first) => Selection::From(first.timestamp, &first.id),
                None => Selection::Latest,
            };
            let mut candidates = select_members(conn, key, range, CAPACITY + 1)
                .await
                .map_err(|e| e.to_string())?;
            if candidates.len() > CAPACITY {
                candidates.remove(0);
                let ids: HashSet<_> = existing.iter().map(|m| m.id.as_str()).collect();
                if !follow_latest || !candidates.iter().any(|m| ids.contains(m.id.as_str())) {
                    pinned = true;
                    candidates = existing;
                }
            }
            candidates
        }
    } else if let Some(id) = anchor_id {
        if let Some((ts, _)) = anchor {
            let mut left =
                select_members(conn, key, Selection::AtOrBefore(ts, id), BEFORE_ANCHOR + 1)
                    .await
                    .map_err(|e| e.to_string())?;
            left.extend(
                select_members(conn, key, Selection::After(ts, id), AFTER_ANCHOR)
                    .await
                    .map_err(|e| e.to_string())?,
            );
            left
        } else {
            Vec::new()
        }
    } else {
        select_members(conn, key, Selection::Latest, limit)
            .await
            .map_err(|e| e.to_string())?
    };
    if let WindowRead::Older { before, .. } = read {
        let timestamp = previous
            .and_then(|w| w.members.iter().find(|m| m.id == before.id))
            .map_or(before.timestamp, |m| m.timestamp);
        let mut page = select_members(conn, key, Selection::Before(timestamp, &before.id), limit)
            .await
            .map_err(|e| e.to_string())?;
        page.append(&mut members);
        page.sort_by(|a, b| (a.timestamp, &a.id).cmp(&(b.timestamp, &b.id)));
        page.dedup_by(|a, b| a.id == b.id);
        if page.len() > CAPACITY {
            page.truncate(CAPACITY);
            pinned = true;
        }
        members = page;
    }
    let anchor_cursor = anchor_id.zip(anchor).map(|(id, (ts, _))| (ts, id));
    let before = members
        .first()
        .or_else(|| previous?.members.first())
        .map(|m| (m.timestamp, m.id.as_str()))
        .or(anchor_cursor);
    let after = members
        .last()
        .or_else(|| previous?.members.last())
        .map(|m| (m.timestamp, m.id.as_str()))
        .or(anchor_cursor);
    let has_older = if let Some((ts, id)) = before {
        !select_members(conn, key, Selection::Before(ts, id), 1)
            .await
            .map_err(|e| e.to_string())?
            .is_empty()
    } else {
        false
    };
    let has_newer = if let Some((ts, id)) = after {
        !select_members(conn, key, Selection::After(ts, id), 1)
            .await
            .map_err(|e| e.to_string())?
            .is_empty()
    } else {
        members.is_empty()
            && !select_members(conn, key, Selection::Latest, 1)
                .await
                .map_err(|e| e.to_string())?
                .is_empty()
    };
    let replacement_anchor_id = if anchor_removed {
        anchor_id.and_then(|id| {
            let ts = anchor.map(|(ts, _)| ts).or_else(|| {
                previous?
                    .members
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| m.timestamp)
            })?;
            members
                .iter()
                .find(|m| (m.timestamp, m.id.as_str()) > (ts, id))
                .or_else(|| members.last())
                .map(|m| m.id.clone())
        })
    } else {
        None
    };
    Ok(WindowData {
        members,
        pinned,
        has_older,
        has_newer,
        anchor_removed,
        replacement_anchor_id,
        activity,
    })
}

async fn changed_rows(
    conn: &mut SqliteConnection,
    key: &TopicKey,
    members: &[Member],
    known: &[KnownWindowRow],
) -> Result<(Vec<SqliteRow>, Vec<SqliteRow>), String> {
    let known: HashMap<_, _> = known.iter().map(|k| (k.id.as_str(), k)).collect();
    let ids: Vec<_> = members
        .iter()
        .filter(|m| {
            !known.get(m.id.as_str()).is_some_and(|k| {
                k.content_hash.as_deref() == Some(m.content_hash.as_str())
                    && k.finish_reason == m.finish_reason
                    && k.updated_at == Some(m.updated_at)
            })
        })
        .map(|m| m.id.as_str())
        .collect();
    if ids.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut q = QueryBuilder::<Sqlite>::new(ROW_SELECT);
    q.push(" WHERE m.owner_type=")
        .push_bind(&key.owner_type)
        .push(" AND m.owner_id=")
        .push_bind(&key.owner_id)
        .push(" AND m.topic_id=")
        .push_bind(&key.topic_id)
        .push(" AND m.deleted_at IS NULL AND m.msg_id IN (");
    let mut list = q.separated(",");
    for &id in &ids {
        list.push_bind(id);
    }
    list.push_unseparated(") ORDER BY m.timestamp,m.msg_id");
    let rows = q
        .build()
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    let attachments = history_attachment_rows(&mut *conn, key, &ids, false).await?;
    Ok((rows, attachments))
}

#[allow(clippy::too_many_arguments)]
async fn window_projection(
    app: AppHandle<impl tauri::Runtime>,
    document_id: u64,
    id: WindowId,
    read: WindowRead<'_>,
    limit: usize,
    known: Vec<KnownWindowRow>,
    observed_ids: Vec<String>,
) -> Result<ChatWindowProjection, String> {
    if limit == 0 || limit > CAPACITY || known.len() > CAPACITY || observed_ids.len() > CAPACITY {
        return Err("invalid chat window size".into());
    }
    let calls = app.state::<VoiceCallState>();
    let (ticket, retained, anchor_id) = calls.start_window_read(document_id, &id)?;
    let db = app.state::<DbState>();
    // Membership, terminal reasons, deletions, rendered rows and attachments
    // come from one SQLite snapshot. Release it before filesystem/render work.
    let mut tx = db.pool.begin().await.map_err(|e| e.to_string())?;
    let previous = retained.as_deref();
    // A current tail can gain live rows between SQLite reads. Keep those
    // observed members when the reader pins that tail by paging upward.
    let observed = if let Some(prev) = previous.filter(|w| !w.pinned && !observed_ids.is_empty()) {
        let retained_ids: HashSet<_> = prev.members.iter().map(|m| m.id.as_str()).collect();
        let new_ids = observed_ids
            .iter()
            .map(String::as_str)
            .filter(|id| !retained_ids.contains(id))
            .collect();
        let extras = select_members(&mut tx, &id.key, Selection::Ids(new_ids), CAPACITY)
            .await
            .map_err(|e| e.to_string())?;
        if extras.is_empty() {
            None
        } else {
            let mut members = prev.members.clone();
            members.extend(extras);
            members.sort_by(|a, b| (a.timestamp, &a.id).cmp(&(b.timestamp, &b.id)));
            members.drain(..members.len().saturating_sub(CAPACITY));
            Some(RetainedWindow {
                id: id.clone(),
                members,
                pinned: false,
            })
        }
    } else {
        None
    };
    let window = select_window(
        &mut tx,
        &id.key,
        observed.as_ref().or(previous),
        anchor_id.as_deref(),
        read,
        limit,
    )
    .await?;
    let (rows, attachments) = changed_rows(&mut tx, &id.key, &window.members, &known).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let messages = project_history_rows(
        &app,
        &db.pool,
        &id.key,
        rows,
        attachments,
        matches!(read, WindowRead::Reconcile { .. }),
        false,
    )
    .await?;
    let member_ids = window.members.iter().map(|m| m.id.clone()).collect();
    // If the whole pinned page was deleted, its former keys still delimit
    // paging on the next read. None of them is returned as a live member.
    let members = if window.pinned && window.members.is_empty() {
        previous.map(|w| w.members.clone()).unwrap_or_default()
    } else {
        window.members
    };
    calls.finish_window_read(
        document_id,
        ticket,
        RetainedWindow {
            id,
            members,
            pinned: window.pinned,
        },
    )?;
    Ok(ChatWindowProjection {
        member_ids,
        messages,
        has_older: window.has_older,
        has_newer: window.has_newer,
        pinned: window.pinned,
        anchor_removed: window.anchor_removed,
        replacement_anchor_id: window.replacement_anchor_id,
        topic_activity: window.activity,
    })
}

#[tauri::command]
pub async fn read_chat_window<R: tauri::Runtime>(
    app: AppHandle<R>,
    document_id: u64,
    key: TopicKey,
    focus_revision: u64,
    window_revision: u64,
    older: Option<bool>,
    pin_older: Option<bool>,
    limit: Option<usize>,
    known: Option<Vec<KnownWindowRow>>,
    observed_ids: Option<Vec<String>>,
    before: Option<WindowCursor>,
) -> Result<ChatWindowProjection, String> {
    window_projection(
        app,
        document_id,
        WindowId {
            key,
            focus_revision,
            window_revision,
        },
        if older == Some(true) {
            WindowRead::Older {
                pin: pin_older.unwrap_or(true),
                before: before.as_ref().ok_or("missing chat window cursor")?,
            }
        } else {
            WindowRead::Open
        },
        limit.unwrap_or(15),
        known.unwrap_or_default(),
        observed_ids.unwrap_or_default(),
    )
    .await
}

#[tauri::command]
pub async fn reconcile_chat_window<R: tauri::Runtime>(
    app: AppHandle<R>,
    document_id: u64,
    key: TopicKey,
    focus_revision: u64,
    window_revision: u64,
    known: Vec<KnownWindowRow>,
    follow_latest: bool,
    force: Option<bool>,
    observed_ids: Option<Vec<String>>,
) -> Result<ChatWindowProjection, String> {
    window_projection(
        app,
        document_id,
        WindowId {
            key,
            focus_revision,
            window_revision,
        },
        WindowRead::Reconcile { follow_latest },
        15,
        if force == Some(true) {
            Vec::new()
        } else {
            known
        },
        observed_ids.unwrap_or_default(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::{
        chat::message_service::{begin_stream_message, finalize_stream_message},
        topic_types::MessageKey,
        vcp_client::{ActiveRequestLease, ActiveRequests},
    };

    #[test]
    fn document_and_anchor_changes_invalidate_reads_before_any_replacement_page_runs() {
        let calls = VoiceCallState::default();
        let (doc, _) = calls.attach_chat_document().unwrap();
        let key = TopicKey::new("agent", "a", "t");
        let target = super::super::chat_focus::FocusTarget::Topic {
            owner_type: key.owner_type.clone(),
            owner_id: key.owner_id.clone(),
            topic_id: key.topic_id.clone(),
        };
        calls.begin_navigation(doc, 1).unwrap();
        calls.validate_navigation(doc, 1, target.scope()).unwrap();
        let first = calls
            .commit_navigation(doc, 1, target.clone(), None)
            .unwrap();
        let id = WindowId {
            key,
            focus_revision: first.focus_revision,
            window_revision: first.window_revision,
        };
        let page = || RetainedWindow {
            id: id.clone(),
            members: vec![],
            pinned: true,
        };
        let (before_adoption, _, _) = calls.start_window_read(doc, &id).unwrap();
        let (next, _) = calls.attach_chat_document().unwrap();
        calls.document_focus(next).unwrap();
        assert_eq!(
            calls.finish_window_read(doc, before_adoption, page()),
            Err("stale_window".into())
        );
        let (accepted, _, _) = calls.start_window_read(next, &id).unwrap();
        // Rejected registration cannot supersede a valid read's ticket.
        assert!(matches!(calls.start_window_read(doc,&id),Err(e) if e=="stale_window"));
        calls.finish_window_read(next, accepted, page()).unwrap();
        let (before_anchor_change, _, _) = calls.start_window_read(next, &id).unwrap();
        calls.begin_navigation(next, 1).unwrap();
        calls.validate_navigation(next, 1, target.scope()).unwrap();
        let second = calls
            .commit_navigation(next, 1, target, Some("new-anchor".into()))
            .unwrap();
        assert_eq!(first.focus_revision, second.focus_revision);
        assert_ne!(first.window_revision, second.window_revision);
        assert_eq!(
            calls.finish_window_read(next, before_anchor_change, page()),
            Err("stale_window".into())
        );
    }

    async fn fixture() -> (DbState, TopicKey) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/0100_baseline_v2.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0102_stream_stop_intent.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql("INSERT INTO agents(owner_type,agent_id,name,model,updated_at) VALUES('agent','a','Agent','test',1);
            INSERT INTO topics(owner_type,owner_id,topic_id,title,created_at,updated_at) VALUES('agent','a','t','Topic',1,1);").execute(&pool).await.unwrap();
        (
            DbState::new(pool, Default::default()),
            TopicKey::new("agent", "a", "t"),
        )
    }

    async fn insert_rows(db: &DbState, start: i64, end: i64) {
        sqlx::query("WITH RECURSIVE n(i) AS (VALUES(?) UNION ALL SELECT i+1 FROM n WHERE i<?)
            INSERT INTO messages(owner_type,owner_id,topic_id,msg_id,role,content,timestamp,updated_at,created_at)
            SELECT 'agent','a','t',printf('%04d',i),'user','body',10,i,i FROM n")
            .bind(start).bind(end).execute(&db.pool).await.unwrap();
    }

    fn retain(key: &TopicKey, w: WindowData) -> RetainedWindow {
        RetainedWindow {
            id: WindowId {
                key: key.clone(),
                focus_revision: 1,
                window_revision: 1,
            },
            members: w.members,
            pinned: w.pinned,
        }
    }

    #[tokio::test]
    async fn released_offline_terminal_is_read_from_the_observed_window() {
        let (db, key) = fixture().await;
        let app = tauri::test::mock_app();
        app.manage(db.clone());
        app.manage(ActiveRequests::default());
        let message = MessageKey::new(key.clone(), "reply");
        let (lease, _) = ActiveRequestLease::try_acquire(
            app.state::<ActiveRequests>().0.clone(),
            message.clone(),
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        lease
            .bind_output(
                app.handle(),
                Some(tauri::ipc::Channel::new(move |_| {
                    sender
                        .send(())
                        .map_err(|_| std::io::Error::other("WebView disconnected").into())
                })),
            )
            .unwrap();
        begin_stream_message(&db, &message, Some("a"), Some("Agent"))
            .await
            .unwrap();
        let old_window = select_window(
            &mut db.pool.acquire().await.unwrap(),
            &key,
            None,
            None,
            WindowRead::Open,
            5,
        )
        .await
        .unwrap();
        let old = &old_window.members[0];
        let known = KnownWindowRow {
            id: old.id.clone(),
            content_hash: Some(old.content_hash.clone()),
            finish_reason: None,
            updated_at: Some(old.updated_at),
        };
        drop(receiver);
        // Same empty body before and after commit. The disappeared lease and the
        // content hash alone cannot tell a resumed WebView what happened.
        finalize_stream_message(
            app.handle().clone(),
            &db.pool,
            &message,
            String::new(),
            false,
            Some("stop".into()),
            None,
            Some("a".into()),
        )
        .await
        .unwrap();
        drop(lease);
        assert!(app.state::<ActiveRequests>().0.is_empty());
        let prev = retain(&key, old_window);
        let mut conn = db.pool.acquire().await.unwrap();
        let repaired = select_window(
            &mut conn,
            &key,
            Some(&prev),
            None,
            WindowRead::Reconcile {
                follow_latest: false,
            },
            5,
        )
        .await
        .unwrap();
        let (rows, _) = changed_rows(&mut conn, &key, &repaired.members, &[known])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String, _>("finish_reason"), "stop");
        drop(conn);
    }

    #[tokio::test]
    async fn anchored_members_survive_arrivals_and_anchor_deletion_with_real_page_edges() {
        let (db, key) = fixture().await;
        insert_rows(&db, 1, 25).await;
        let mut conn = db.pool.acquire().await.unwrap();
        let base = select_window(&mut conn, &key, None, Some("0025"), WindowRead::Open, 5)
            .await
            .unwrap();
        assert_eq!(base.members.first().unwrap().id, "0013");
        assert!(base.has_older);
        assert!(!base.has_newer);
        let pinned = retain(&key, base);
        drop(conn);
        insert_rows(&db, 26, 29).await;
        sqlx::query("UPDATE messages SET deleted_at=100 WHERE msg_id='0025'")
            .execute(&db.pool)
            .await
            .unwrap();
        let mut conn = db.pool.acquire().await.unwrap();
        let patch = select_window(
            &mut conn,
            &key,
            Some(&pinned),
            Some("0025"),
            WindowRead::Reconcile {
                follow_latest: true,
            },
            5,
        )
        .await
        .unwrap();
        assert!(patch.anchor_removed && patch.has_newer);
        assert_eq!(patch.replacement_anchor_id.as_deref(), Some("0024"));
        assert_eq!(patch.members.len(), 12);
        assert_eq!(patch.members.last().unwrap().id, "0024");
        let extended = select_window(
            &mut conn,
            &key,
            Some(&pinned),
            Some("0025"),
            WindowRead::Older {
                pin: true,
                before: &WindowCursor {
                    timestamp: 10,
                    id: "0013".into(),
                },
            },
            14,
        )
        .await
        .unwrap();
        assert_eq!(extended.members.len(), 24);
        assert!(!extended.has_older);
        assert!(extended.has_newer);
    }

    #[tokio::test]
    async fn old_document_cannot_page_a_retained_window_after_live_rows_are_pinned() {
        let (db, key) = fixture().await;
        sqlx::raw_sql(include_str!("../../../migrations/0103_config_domains.sql"))
            .execute(&db.pool)
            .await
            .unwrap();
        insert_rows(&db, 1, 10).await;
        let app = tauri::test::mock_app();
        app.manage(db.clone());
        app.manage(crate::vcp_modules::settings_manager::SettingsState::default());
        app.manage(VoiceCallState::default());
        let calls = app.state::<VoiceCallState>();
        let (doc, _) = calls.attach_chat_document().unwrap();
        let target = super::super::chat_focus::FocusTarget::Topic {
            owner_type: key.owner_type.clone(),
            owner_id: key.owner_id.clone(),
            topic_id: key.topic_id.clone(),
        };
        calls.begin_navigation(doc, 1).unwrap();
        calls.validate_navigation(doc, 1, target.scope()).unwrap();
        let focus = calls.commit_navigation(doc, 1, target, None).unwrap();
        let fetch = |document, older, observed| {
            read_chat_window(
                app.handle().clone(),
                document,
                key.clone(),
                focus.focus_revision,
                focus.window_revision,
                Some(older),
                None,
                Some(2),
                None,
                Some(observed),
                older.then(|| WindowCursor {
                    timestamp: 10,
                    id: "0009".into(),
                }),
            )
        };
        let base = fetch(doc, false, vec![]).await.unwrap();
        assert_eq!(base.member_ids, ["0009", "0010"]);
        let id = WindowId {
            key: key.clone(),
            focus_revision: focus.focus_revision,
            window_revision: focus.window_revision,
        };
        let (slow_ticket, prior, _) = calls.start_window_read(doc, &id).unwrap();
        let slow_window = RetainedWindow {
            id,
            members: prior.unwrap().members.clone(),
            pinned: false,
        };
        insert_rows(&db, 11, 11).await;
        let mut observed = base.member_ids;
        observed.push("0011".into());
        // A slow render still holds its own snapshot while the newer page
        // completes. Once resumed it cannot publish the superseded members.
        let pinned = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fetch(doc, true, observed.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(pinned.member_ids, ["0007", "0008", "0009", "0010", "0011"]);
        assert!(pinned.pinned);
        assert_eq!(
            calls.finish_window_read(doc, slow_ticket, slow_window),
            Err("stale_window".into())
        );
        // Simulate the previous page's successful invoke result being lost.
        let repeated = fetch(doc, true, observed).await.unwrap();
        assert_eq!(repeated.member_ids, pinned.member_ids);
        let (next, _) = calls.attach_chat_document().unwrap();
        calls.document_focus(next).unwrap();
        assert!(matches!(fetch(doc,true,vec![]).await,Err(error) if error=="stale_window"));
        insert_rows(&db, 12, 12).await;
        let restored = fetch(next, false, vec![]).await.unwrap();
        assert_eq!(restored.member_ids, pinned.member_ids);
        assert!(restored.has_newer && restored.pinned);
    }

    #[tokio::test]
    async fn first_screen_fill_and_user_paging_keep_different_follow_intents() {
        let (db, key) = fixture().await;
        insert_rows(&db, 1, 20).await;
        let mut conn = db.pool.acquire().await.unwrap();
        let base = select_window(&mut conn, &key, None, None, WindowRead::Open, 5)
            .await
            .unwrap();
        let initial = retain(&key, base);
        let before = WindowCursor {
            timestamp: 10,
            id: "0016".into(),
        };
        let auto = select_window(
            &mut conn,
            &key,
            Some(&initial),
            None,
            WindowRead::Older {
                pin: false,
                before: &before,
            },
            5,
        )
        .await
        .unwrap();
        let manual = select_window(
            &mut conn,
            &key,
            Some(&initial),
            None,
            WindowRead::Older {
                pin: true,
                before: &before,
            },
            5,
        )
        .await
        .unwrap();
        assert!(!auto.pinned && manual.pinned);
        drop(conn);
        insert_rows(&db, 21, 21).await;
        let mut conn = db.pool.acquire().await.unwrap();
        let auto = retain(&key, auto);
        let manual = retain(&key, manual);
        let resumed = WindowRead::Reconcile {
            follow_latest: true,
        };
        let live = select_window(&mut conn, &key, Some(&auto), None, resumed, 5)
            .await
            .unwrap();
        let deep = select_window(&mut conn, &key, Some(&manual), None, resumed, 5)
            .await
            .unwrap();
        assert_eq!(live.members.last().unwrap().id, "0021");
        assert!(!live.has_newer);
        assert_eq!(deep.members.last().unwrap().id, "0020");
        assert!(deep.has_newer);
    }

    #[tokio::test]
    async fn latest_reconciles_unseen_and_backfilled_rows_but_does_not_bridge_capacity_gap() {
        let (db, key) = fixture().await;
        insert_rows(&db, 1, 3).await;
        sqlx::query("DELETE FROM messages WHERE msg_id='0002'")
            .execute(&db.pool)
            .await
            .unwrap();
        let initial = select_window(
            &mut db.pool.acquire().await.unwrap(),
            &key,
            None,
            None,
            WindowRead::Open,
            5,
        )
        .await
        .unwrap();
        let prev = retain(&key, initial);
        insert_rows(&db, 2, 2).await;
        insert_rows(&db, 4, 6).await;
        let repaired = select_window(
            &mut db.pool.acquire().await.unwrap(),
            &key,
            Some(&prev),
            None,
            WindowRead::Reconcile {
                follow_latest: true,
            },
            5,
        )
        .await
        .unwrap();
        assert_eq!(repaired.members.len(), 6);
        assert_eq!(repaired.members[1].id, "0002");
        assert!(!repaired.has_older && !repaired.has_newer);
        let prev = retain(&key, repaired);
        insert_rows(&db, 7, 520).await;
        let deep = select_window(
            &mut db.pool.acquire().await.unwrap(),
            &key,
            Some(&prev),
            None,
            WindowRead::Reconcile {
                follow_latest: true,
            },
            5,
        )
        .await
        .unwrap();
        assert!(deep.pinned && deep.has_newer);
        assert_eq!(deep.members.last().unwrap().id, "0006");
    }
}
