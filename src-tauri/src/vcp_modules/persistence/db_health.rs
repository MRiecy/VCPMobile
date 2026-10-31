//! Process-local write quarantine for the application's single SQLite database.
//! Both drivers use the same bundled SQLite. Its log has no connection handle,
//! so only the main pool's dedicated workers and scoped rusqlite calls count.
use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const POOL_THREAD: &str = "vcp-main-sqlite";
const RESTART: &str = "SQLite 检测到数据库异常，已暂停写入。请重启应用核查数据库。";
static MARKER: OnceLock<PathBuf> = OnceLock::new();
static QUARANTINED: AtomicBool = AtomicBool::new(false);
thread_local! { static MAIN_RUSQLITE: Cell<bool> = const { Cell::new(false) }; }

pub(super) fn marker_path(db_path: &Path) -> PathBuf {
    super::db_manager::sqlite_sidecar_path(db_path, ".needs-check")
}

pub(super) fn install(db_path: &Path) -> Result<(), String> {
    let path = marker_path(db_path);
    if MARKER.get_or_init(|| path.clone()) != &path {
        return Err("SQLite health monitor already owns another database".into());
    }
    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            // SAFETY: init_db is the sole bootstrap of the bundled SQLite in the
            // app, before opening a connection or publishing DbState to callers.
            // The callback never calls SQLite or an application logger.
            unsafe { rusqlite::trace::config_log(Some(observe_error)) }.map_err(|e| e.to_string())
        })
        .clone()
}

fn observe_error(code: i32, _: &str) {
    if !super::db_manager::is_corruption_code(code)
        || !(MAIN_RUSQLITE.get() || std::thread::current().name() == Some(POOL_THREAD))
    {
        return;
    }
    if let Some(path) = MARKER.get() {
        QUARANTINED.store(true, Ordering::Release);
        // Existence is the entire record. No DB calls, SQL text or detached
        // work which could be lost if this process is killed after the error.
        let _ = persist_marker(path);
    }
}

pub(super) fn persist_marker(path: &Path) -> io::Result<()> {
    match std::fs::File::create_new(path) {
        Ok(file) => file.sync_all(),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}

pub(super) fn resume_after_verification(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    QUARANTINED.store(false, Ordering::Release);
    Ok(())
}

pub(super) fn ensure_writable() -> Result<(), String> {
    if is_quarantined() {
        if let Some(marker) = MARKER.get() {
            persist_marker(marker).map_err(|e| format!("{RESTART} 无法保存核查标志：{e}"))?;
        }
        return Err(RESTART.into());
    }
    Ok(())
}

pub(super) fn is_quarantined() -> bool {
    QUARANTINED.load(Ordering::Acquire)
}

pub(super) fn pool_options(
    options: sqlx::sqlite::SqliteConnectOptions,
) -> sqlx::sqlite::SqliteConnectOptions {
    options.thread_name(|_| POOL_THREAD.into())
}

pub(super) async fn protect_sqlx(
    connection: &mut sqlx::SqliteConnection,
) -> Result<(), sqlx::Error> {
    connection
        .lock_handle()
        .await?
        .set_commit_hook(|| !is_quarantined());
    Ok(())
}

pub(super) fn protect_rusqlite(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.commit_hook(Some(is_quarantined))
}

/// A blocking worker can be reused for other tasks after this call.
pub(super) struct MainRusqliteScope(bool, std::marker::PhantomData<std::rc::Rc<()>>);
impl MainRusqliteScope {
    pub(super) fn enter() -> Self {
        Self(MAIN_RUSQLITE.replace(true), std::marker::PhantomData)
    }
}
impl Drop for MainRusqliteScope {
    fn drop(&mut self) {
        MAIN_RUSQLITE.set(self.0);
    }
}
