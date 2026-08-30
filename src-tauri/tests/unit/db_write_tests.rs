use super::*;
use std::sync::{Condvar, Mutex as StdMutex};

async fn file_backed_writer() -> (tempfile::TempDir, Pool<Sqlite>, WriteCoordinator) {
    let temp_dir = tempfile::tempdir().expect("create write owner database directory");
    let db_path = temp_dir.path().join("write-owner.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(2));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .expect("open write owner SQLx pool");
    sqlx::query("CREATE TABLE write_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .expect("create write owner probe table");
    (temp_dir, pool, WriteCoordinator::new())
}

async fn wait_until_unlocked(coordinator: &WriteCoordinator) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while coordinator.is_locked() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("write coordinator remained locked");
}

#[tokio::test]
async fn committed_write_emits_one_complete_metric() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let mut metrics = coordinator.subscribe_metrics();

    let tx = coordinator
        .write_transaction(&pool, "test.metric-commit")
        .await
        .expect("begin metric transaction");
    tx.commit().await.expect("commit metric transaction");

    let metric = tokio::time::timeout(Duration::from_secs(1), metrics.recv())
        .await
        .expect("write metric timed out")
        .expect("write metric stream closed");
    assert_eq!(metric.operation, "test.metric-commit");
    assert_eq!(metric.outcome, "committed");
    assert!(metric.begin_duration.is_some());
    assert!(metric.finish_duration.is_some());
    assert!(
        !coordinator.is_locked(),
        "metric must be observable only after the writer guard is released"
    );
    assert!(matches!(
        metrics.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn subscription_observes_a_preexisting_lease_when_it_finishes() {
    let (_temp_dir, _pool, coordinator) = file_backed_writer().await;
    let holder = coordinator.acquire_lease("test.preexisting-holder").await;
    let mut metrics = coordinator.subscribe_metrics();

    drop(holder);

    let metric = tokio::time::timeout(Duration::from_secs(1), metrics.recv())
        .await
        .expect("preexisting holder metric timed out")
        .expect("preexisting holder metric stream closed");
    assert_eq!(metric.operation, "test.preexisting-holder");
    assert_eq!(metric.outcome, "released");
    assert!(!coordinator.is_locked());
}

#[tokio::test]
async fn cancelled_gate_waiter_never_begins_a_transaction() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let holder = coordinator.acquire_lease("test.holder").await;

    let waiting_coordinator = coordinator.clone();
    let waiting_pool = pool.clone();
    let waiter = tokio::spawn(async move {
        waiting_coordinator
            .write_transaction(&waiting_pool, "test.cancelled-waiter")
            .await
    });
    tokio::task::yield_now().await;
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    drop(holder);

    let tx = coordinator
        .write_transaction(&pool, "test.after-cancelled-waiter")
        .await
        .expect("begin after cancelled gate waiter");
    tx.commit()
        .await
        .expect("commit after cancelled gate waiter");
}

#[tokio::test]
async fn cancelling_during_begin_keeps_the_lease_until_cleanup() {
    let (temp_dir, pool, coordinator) = file_backed_writer().await;
    let db_path = temp_dir.path().join("write-owner.sqlite");
    let mut external = rusqlite::Connection::open(db_path).expect("open external writer");
    external
        .busy_timeout(Duration::from_secs(2))
        .expect("configure external writer timeout");
    let held = external
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .expect("reserve external writer");

    let waiting_coordinator = coordinator.clone();
    let waiting_pool = pool.clone();
    let caller = tokio::spawn(async move {
        waiting_coordinator
            .write_transaction(&waiting_pool, "test.cancelled-begin")
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator.is_locked() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("BEGIN caller never acquired the coordinator");

    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        coordinator.is_locked(),
        "caller cancellation released the lease while BEGIN was still waiting"
    );

    held.commit().expect("release external writer");
    wait_until_unlocked(&coordinator).await;
    let tx = coordinator
        .write_transaction(&pool, "test.after-cancelled-begin")
        .await
        .expect("begin after cancelled BEGIN cleanup");
    tx.commit()
        .await
        .expect("commit after cancelled BEGIN cleanup");
}

#[tokio::test]
async fn dropped_and_panicking_transaction_bodies_roll_back_before_the_next_writer() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let mut metrics = coordinator.subscribe_metrics();

    let mut dropped = coordinator
        .write_transaction(&pool, "test.dropped-body")
        .await
        .expect("begin dropped transaction");
    sqlx::query("INSERT INTO write_probe VALUES (1, 'dropped')")
        .execute(&mut *dropped)
        .await
        .expect("write dropped transaction row");
    drop(dropped);
    wait_until_unlocked(&coordinator).await;
    let dropped_metric = metrics.recv().await.expect("dropped transaction metric");
    assert_eq!(dropped_metric.operation, "test.dropped-body");
    assert_eq!(dropped_metric.outcome, "rolled_back");
    assert!(dropped_metric.finish_duration.is_some());

    let panic_coordinator = coordinator.clone();
    let panic_pool = pool.clone();
    let panicking = tokio::spawn(async move {
        let mut tx = panic_coordinator
            .write_transaction(&panic_pool, "test.panicking-body")
            .await
            .expect("begin panicking transaction");
        sqlx::query("INSERT INTO write_probe VALUES (2, 'panicking')")
            .execute(&mut *tx)
            .await
            .expect("write panicking transaction row");
        panic!("intentional transaction body panic");
    });
    assert!(panicking.await.expect_err("body must panic").is_panic());
    wait_until_unlocked(&coordinator).await;

    let mut next = coordinator
        .write_transaction(&pool, "test.after-body-failures")
        .await
        .expect("begin after body failures");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&mut *next)
        .await
        .expect("count rolled back rows");
    assert_eq!(count, 0);
    next.rollback()
        .await
        .expect("explicitly roll back verification transaction");
}

#[tokio::test]
async fn dropping_on_a_non_runtime_thread_uses_the_stored_runtime_for_rollback() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let mut tx = coordinator
        .write_transaction(&pool, "test.cross-thread-drop")
        .await
        .expect("begin cross-thread transaction");
    sqlx::query("INSERT INTO write_probe VALUES (1, 'cross-thread')")
        .execute(&mut *tx)
        .await
        .expect("write cross-thread row");

    std::thread::spawn(move || drop(tx))
        .join()
        .expect("drop transaction outside Tokio runtime");
    wait_until_unlocked(&coordinator).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&pool)
        .await
        .expect("count rows after cross-thread rollback");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn internal_task_panic_or_abort_closes_the_connection_before_releasing_the_lease() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;

    let mut panic_tx = coordinator
        .write_transaction(&pool, "test.internal-task-panic")
        .await
        .expect("begin internal panic transaction");
    sqlx::query("INSERT INTO write_probe VALUES (1, 'internal-panic')")
        .execute(&mut *panic_tx)
        .await
        .expect("write internal panic row");
    let panic_active = panic_tx
        .active
        .take()
        .expect("take active write for internal panic");
    let panic_task = coordinator.runtime.spawn(async move {
        let _active = panic_active;
        panic!("intentional durable task panic");
    });
    assert!(panic_task
        .await
        .expect_err("durable task must panic")
        .is_panic());
    wait_until_unlocked(&coordinator).await;
    let count_after_panic: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&pool)
        .await
        .expect("count rows after internal task panic");
    assert_eq!(count_after_panic, 0);

    let mut abort_tx = coordinator
        .write_transaction(&pool, "test.internal-task-abort")
        .await
        .expect("begin internal abort transaction");
    sqlx::query("INSERT INTO write_probe VALUES (2, 'internal-abort')")
        .execute(&mut *abort_tx)
        .await
        .expect("write internal abort row");
    let abort_active = abort_tx
        .active
        .take()
        .expect("take active write for internal abort");
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let abort_task = coordinator.runtime.spawn(async move {
        let _active = abort_active;
        let _ = entered_tx.send(());
        std::future::pending::<()>().await;
    });
    entered_rx.await.expect("internal abort task never started");
    abort_task.abort();
    assert!(matches!(abort_task.await, Err(error) if error.is_cancelled()));
    wait_until_unlocked(&coordinator).await;
    let count_after_abort: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&pool)
        .await
        .expect("count rows after internal task abort");
    assert_eq!(count_after_abort, 0);

    let next = coordinator
        .write_transaction(&pool, "test.after-internal-task-failures")
        .await
        .expect("begin after internal task failures");
    next.commit()
        .await
        .expect("commit after internal task failures");
}

#[tokio::test]
async fn cancelling_the_commit_waiter_does_not_cancel_the_commit() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let mut tx = coordinator
        .write_transaction(&pool, "test.cancelled-commit-waiter")
        .await
        .expect("begin commit cancellation transaction");
    sqlx::query("INSERT INTO write_probe VALUES (1, 'committed')")
        .execute(&mut *tx)
        .await
        .expect("write committed row");

    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let release = Arc::new((StdMutex::new(false), Condvar::new()));
    let hook_release = release.clone();
    {
        let mut handle = tx.lock_handle().await.expect("lock SQLite handle");
        handle.set_commit_hook(move || {
            let _ = entered_tx.send(());
            let (lock, condvar) = &*hook_release;
            let mut released = lock.lock().unwrap_or_else(|error| error.into_inner());
            while !*released {
                released = condvar
                    .wait(released)
                    .unwrap_or_else(|error| error.into_inner());
            }
            true
        });
    }

    let committing = tokio::spawn(async move { tx.commit().await });
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(2)))
        .await
        .expect("join commit-hook observer")
        .expect("COMMIT never entered the blocking hook");
    committing.abort();
    assert!(matches!(committing.await, Err(error) if error.is_cancelled()));

    let next_coordinator = coordinator.clone();
    let next_pool = pool.clone();
    let mut next_writer = tokio::spawn(async move {
        let mut next = next_coordinator
            .write_transaction(&next_pool, "test.after-cancelled-commit")
            .await?;
        sqlx::query("INSERT INTO write_probe VALUES (2, 'next')")
            .execute(&mut *next)
            .await
            .map_err(|error| error.to_string())?;
        next.commit().await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut next_writer)
            .await
            .is_err(),
        "next writer entered before the detached COMMIT completed"
    );

    let (lock, condvar) = &*release;
    *lock.lock().unwrap_or_else(|error| error.into_inner()) = true;
    condvar.notify_all();
    tokio::time::timeout(Duration::from_secs(2), next_writer)
        .await
        .expect("next writer timed out after COMMIT release")
        .expect("next writer task failed")
        .expect("next writer transaction failed");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&pool)
        .await
        .expect("count rows after detached COMMIT");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn commit_error_closes_or_rolls_back_before_releasing_the_lease() {
    let (_temp_dir, pool, coordinator) = file_backed_writer().await;
    let mut metrics = coordinator.subscribe_metrics();
    let mut tx = coordinator
        .write_transaction(&pool, "test.rejected-commit")
        .await
        .expect("begin rejected commit transaction");
    sqlx::query("INSERT INTO write_probe VALUES (1, 'rejected')")
        .execute(&mut *tx)
        .await
        .expect("write rejected commit row");
    {
        let mut handle = tx.lock_handle().await.expect("lock SQLite handle");
        handle.set_commit_hook(|| false);
    }

    tx.commit()
        .await
        .expect_err("commit hook must reject the transaction");
    assert!(!coordinator.is_locked());
    let metric = metrics.recv().await.expect("rejected commit metric");
    assert_eq!(metric.operation, "test.rejected-commit");
    assert!(metric.outcome.starts_with("commit_failed_"));
    assert!(metric.is_failure());
    assert!(metric.finish_duration.is_some());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM write_probe")
        .fetch_one(&pool)
        .await
        .expect("count rows after rejected commit");
    assert_eq!(count, 0);

    let next = coordinator
        .write_transaction(&pool, "test.after-rejected-commit")
        .await
        .expect("begin after rejected commit");
    next.commit().await.expect("commit after rejected commit");
}
