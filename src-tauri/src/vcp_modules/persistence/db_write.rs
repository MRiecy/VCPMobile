use sqlx::pool::PoolConnection;
use sqlx::{Pool, Sqlite, SqliteConnection};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::{Mutex, OwnedMutexGuard};

const SLOW_WRITE_THRESHOLD: Duration = Duration::from_millis(500);

/// Coordinates the process-local SQLite writer without exposing admission permits to business
/// modules. SQLite remains the final cross-process lock; this coordinator only orders writers
/// that share one [`DbState`](super::db_manager::DbState).
#[derive(Clone)]
pub(super) struct WriteCoordinator {
    gate: Arc<Mutex<()>>,
    runtime: Handle,
}

impl WriteCoordinator {
    pub(super) fn new() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            runtime: Handle::current(),
        }
    }

    /// Waits in Tokio mutex FIFO order. Admission itself has no global timeout: callers may
    /// cancel while waiting, but a writer that has entered SQLite must finish or roll back.
    pub(super) async fn acquire_lease(&self, operation: &'static str) -> WriteLease {
        let wait_started = Instant::now();
        let guard = self.gate.clone().lock_owned().await;
        WriteLease::new(operation, wait_started.elapsed(), guard)
    }

    pub(super) async fn write_transaction(
        &self,
        pool: &Pool<Sqlite>,
        operation: &'static str,
    ) -> Result<DbWriteTransaction, String> {
        // Waiting for the FIFO gate remains cancellation-safe. Once this await returns there is no
        // further yield before the durable BEGIN task owns the lease.
        let lease = self.acquire_lease(operation).await;
        let pool = pool.clone();
        let coordinator = self.clone();
        let task = self
            .runtime
            .spawn(async move { begin_active_write(pool, lease, coordinator).await });

        task.await
            .map_err(|error| format!("SQLite write BEGIN task failed for {operation}: {error}"))?
    }

    #[cfg(test)]
    pub(super) fn is_locked(&self) -> bool {
        self.gate.try_lock().is_err()
    }
}

/// Internal lease for the rusqlite Queue and the few non-SQLx persistence writers.
/// Business modules must use [`DbWriteTransaction`] instead.
pub(super) struct WriteLease {
    operation: &'static str,
    wait_duration: Duration,
    acquired_at: Instant,
    begin_duration: Option<Duration>,
    finish_duration: Option<Duration>,
    outcome: &'static str,
    _guard: OwnedMutexGuard<()>,
}

impl WriteLease {
    fn new(operation: &'static str, wait_duration: Duration, guard: OwnedMutexGuard<()>) -> Self {
        Self {
            operation,
            wait_duration,
            acquired_at: Instant::now(),
            begin_duration: None,
            finish_duration: None,
            outcome: "released",
            _guard: guard,
        }
    }

    fn mark_begin(&mut self, duration: Duration) {
        self.begin_duration = Some(duration);
    }

    pub(super) fn mark_outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }

    pub(super) fn finish(&mut self, outcome: &'static str, duration: Duration) {
        self.outcome = outcome;
        self.finish_duration = Some(duration);
    }
}

impl Drop for WriteLease {
    fn drop(&mut self) {
        let hold_duration = self.acquired_at.elapsed();
        let begin_ms = self
            .begin_duration
            .map_or(0.0, |duration| duration.as_secs_f64() * 1000.0);
        let finish_ms = self
            .finish_duration
            .map_or(0.0, |duration| duration.as_secs_f64() * 1000.0);
        let wait_ms = self.wait_duration.as_secs_f64() * 1000.0;
        let hold_ms = hold_duration.as_secs_f64() * 1000.0;

        if self.outcome.contains("failed") {
            log::error!(
                "[DbWrite] operation={} outcome={} wait_ms={:.3} begin_ms={:.3} hold_ms={:.3} finish_ms={:.3}",
                self.operation,
                self.outcome,
                wait_ms,
                begin_ms,
                hold_ms,
                finish_ms
            );
        } else if self.wait_duration >= SLOW_WRITE_THRESHOLD
            || hold_duration >= SLOW_WRITE_THRESHOLD
        {
            log::warn!(
                "[DbWrite] slow operation={} outcome={} wait_ms={:.3} begin_ms={:.3} hold_ms={:.3} finish_ms={:.3}",
                self.operation,
                self.outcome,
                wait_ms,
                begin_ms,
                hold_ms,
                finish_ms
            );
        } else {
            log::debug!(
                "[DbWrite] operation={} outcome={} wait_ms={:.3} begin_ms={:.3} hold_ms={:.3} finish_ms={:.3}",
                self.operation,
                self.outcome,
                wait_ms,
                begin_ms,
                hold_ms,
                finish_ms
            );
        }
    }
}

/// One active SQLx write and its process-local admission lease.
///
/// This intentionally owns `PoolConnection` instead of `sqlx::Transaction`: SQLx consumes a
/// `Transaction` even when `commit()` returns an error, which would prevent this layer from
/// explicitly rolling back (or closing) the uncertain connection before releasing the lease.
/// BEGIN/COMMIT/ROLLBACK are therefore issued explicitly on the retained connection.
///
/// The optional fields let `Drop` move both resources into a detached physical-close task if an
/// internal durable task itself panics or is aborted. Normal caller cancellation is handled by the
/// explicit detached commit/rollback tasks below.
struct ActiveWrite {
    connection: Option<PoolConnection<Sqlite>>,
    lease: Option<WriteLease>,
    runtime: Handle,
}

impl ActiveWrite {
    fn new(connection: PoolConnection<Sqlite>, lease: WriteLease, runtime: Handle) -> Self {
        Self {
            connection: Some(connection),
            lease: Some(lease),
            runtime,
        }
    }

    fn operation(&self) -> &'static str {
        self.lease
            .as_ref()
            .map_or("unknown", |lease| lease.operation)
    }

    fn connection(&self) -> &SqliteConnection {
        match &self.connection {
            Some(connection) => connection,
            None => panic!("SQLite write connection is no longer active"),
        }
    }

    fn connection_mut(&mut self) -> &mut SqliteConnection {
        match &mut self.connection {
            Some(connection) => connection,
            None => panic!("SQLite write connection is no longer active"),
        }
    }

    fn mark_begin(&mut self, duration: Duration) {
        if let Some(lease) = &mut self.lease {
            lease.mark_begin(duration);
        }
    }

    fn finish(mut self, outcome: &'static str, duration: Duration) {
        let connection = self.connection.take();
        let mut lease = self.lease.take();
        if let Some(lease) = &mut lease {
            lease.finish(outcome, duration);
        }
        // The physical transaction is already finished. Return the connection before the
        // coordinator lease is released.
        drop(connection);
        drop(lease);
    }

    async fn close(mut self, outcome: &'static str, duration: Duration) -> Result<(), String> {
        let operation = self.operation();
        let connection = self
            .connection
            .take()
            .ok_or_else(|| format!("SQLite write connection missing for {operation}"))?;
        let lease = self
            .lease
            .take()
            .ok_or_else(|| format!("SQLite write lease missing for {operation}"))?;
        let close_task =
            spawn_close_with_lease(&self.runtime, connection, lease, outcome, duration);
        close_task
            .await
            .map_err(|error| format!("SQLite write close task failed for {operation}: {error}"))?
    }
}

impl Drop for ActiveWrite {
    fn drop(&mut self) {
        let (Some(connection), Some(lease)) = (self.connection.take(), self.lease.take()) else {
            return;
        };
        // An internal durable task panicked or was aborted while a raw transaction might still be
        // active. Never return that connection to the pool; a detached close keeps the lease until
        // SQLite has released the physical connection.
        drop(spawn_close_with_lease(
            &self.runtime,
            connection,
            lease,
            "task_failed_closed",
            Duration::ZERO,
        ));
    }
}

/// Cancellation-safe owner of an immediate SQLite write transaction.
///
/// The wrapper dereferences directly to `SqliteConnection`, preserving existing
/// `execute(&mut *tx)` call sites. Dropping it schedules an explicit rollback on the runtime that
/// created the database state; calling `commit()` transfers the connection and lease to a task
/// that continues even if its caller is subsequently cancelled.
#[must_use = "SQLite write transactions roll back unless commit() completes"]
pub(crate) struct DbWriteTransaction {
    active: Option<ActiveWrite>,
    coordinator: WriteCoordinator,
}

impl DbWriteTransaction {
    pub(crate) async fn commit(mut self) -> Result<(), String> {
        let active = self
            .active
            .take()
            .ok_or_else(|| "SQLite write transaction is no longer active".to_string())?;
        let operation = active.operation();
        let task = self
            .coordinator
            .runtime
            .spawn(async move { commit_active_write(active).await });

        task.await
            .map_err(|error| format!("SQLite write COMMIT task failed for {operation}: {error}"))?
    }

    #[cfg(test)]
    pub(crate) async fn rollback(mut self) -> Result<(), String> {
        let active = self
            .active
            .take()
            .ok_or_else(|| "SQLite write transaction is no longer active".to_string())?;
        let operation = active.operation();
        let task = self
            .coordinator
            .runtime
            .spawn(async move { rollback_active_write(active, "rolled_back").await });

        task.await.map_err(|error| {
            format!("SQLite write ROLLBACK task failed for {operation}: {error}")
        })?
    }

    fn connection(&self) -> &SqliteConnection {
        match &self.active {
            Some(active) => active.connection(),
            None => panic!("SQLite write transaction is no longer active"),
        }
    }

    fn connection_mut(&mut self) -> &mut SqliteConnection {
        match &mut self.active {
            Some(active) => active.connection_mut(),
            None => panic!("SQLite write transaction is no longer active"),
        }
    }
}

impl Deref for DbWriteTransaction {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        self.connection()
    }
}

impl DerefMut for DbWriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection_mut()
    }
}

impl Drop for DbWriteTransaction {
    fn drop(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.coordinator.runtime.spawn(async move {
            if let Err(error) = rollback_active_write(active, "rolled_back").await {
                log::error!("[DbWrite] durable rollback failed: {error}");
            }
        });
    }
}

async fn begin_active_write(
    pool: Pool<Sqlite>,
    mut lease: WriteLease,
    coordinator: WriteCoordinator,
) -> Result<DbWriteTransaction, String> {
    let operation = lease.operation;
    let begin_started = Instant::now();
    let connection = match pool.acquire().await {
        Ok(connection) => connection,
        Err(error) => {
            lease.finish("connection_failed", begin_started.elapsed());
            return Err(format!(
                "Failed to acquire SQLite connection for {operation}: {error}"
            ));
        }
    };
    let mut active = ActiveWrite::new(connection, lease, coordinator.runtime.clone());

    if let Err(error) = sqlx::query("BEGIN IMMEDIATE")
        .execute(active.connection_mut())
        .await
    {
        // A failed BEGIN acknowledgement leaves the connection state uncertain. Closing this
        // physical connection is the only safe way to prove it cannot carry a hidden transaction
        // back into the pool.
        let close_result = active
            .close("begin_failed_closed", begin_started.elapsed())
            .await;
        return match close_result {
            Ok(()) => Err(format!(
                "Failed to begin immediate SQLite transaction for {operation}: {error}"
            )),
            Err(close_error) => Err(format!(
                "Failed to begin immediate SQLite transaction for {operation}: {error}; closing the uncertain connection also failed: {close_error}"
            )),
        };
    }

    active.mark_begin(begin_started.elapsed());
    Ok(DbWriteTransaction {
        active: Some(active),
        coordinator,
    })
}

async fn commit_active_write(mut active: ActiveWrite) -> Result<(), String> {
    let operation = active.operation();
    let finish_started = Instant::now();

    match sqlx::query("COMMIT").execute(active.connection_mut()).await {
        Ok(_) => {
            active.finish("committed", finish_started.elapsed());
            Ok(())
        }
        Err(commit_error) => match sqlx::query("ROLLBACK")
            .execute(active.connection_mut())
            .await
        {
            Ok(_) => {
                active.finish("commit_failed_rolled_back", finish_started.elapsed());
                Err(format!(
                        "Failed to commit SQLite transaction for {operation}; transaction was rolled back: {commit_error}"
                    ))
            }
            Err(rollback_error) => {
                let close_result = active
                    .close("commit_failed_closed", finish_started.elapsed())
                    .await;
                match close_result {
                        Ok(()) => Err(format!(
                            "Failed to commit SQLite transaction for {operation}: {commit_error}; rollback failed and the connection was closed: {rollback_error}"
                        )),
                        Err(close_error) => Err(format!(
                            "Failed to commit SQLite transaction for {operation}: {commit_error}; rollback failed: {rollback_error}; closing the connection also failed: {close_error}"
                        )),
                    }
            }
        },
    }
}

async fn rollback_active_write(
    mut active: ActiveWrite,
    success_outcome: &'static str,
) -> Result<(), String> {
    let operation = active.operation();
    let finish_started = Instant::now();

    match sqlx::query("ROLLBACK")
        .execute(active.connection_mut())
        .await
    {
        Ok(_) => {
            active.finish(success_outcome, finish_started.elapsed());
            Ok(())
        }
        Err(rollback_error) => {
            let close_result = active
                .close("rollback_failed_closed", finish_started.elapsed())
                .await;
            match close_result {
                Ok(()) => Err(format!(
                    "Failed to roll back SQLite transaction for {operation}; connection was closed: {rollback_error}"
                )),
                Err(close_error) => Err(format!(
                    "Failed to roll back SQLite transaction for {operation}: {rollback_error}; closing the connection also failed: {close_error}"
                )),
            }
        }
    }
}

fn spawn_close_with_lease(
    runtime: &Handle,
    connection: PoolConnection<Sqlite>,
    mut lease: WriteLease,
    outcome: &'static str,
    observed_duration: Duration,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let operation = lease.operation;
    runtime.spawn(async move {
        let close_started = Instant::now();
        let close_result = connection.close().await;
        lease.finish(outcome, observed_duration + close_started.elapsed());
        close_result.map_err(|error| {
            let message =
                format!("Failed to close uncertain SQLite connection for {operation}: {error}");
            log::error!("[DbWrite] {message}");
            message
        })
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/db_write_tests.rs"]
mod tests;
