use crate::vcp_modules::infra::lifecycle_state::{LifecycleSnapshot, LifecycleState};
#[cfg(not(target_os = "ios"))]
use crate::vcp_modules::settings_manager::read_config;
use crate::vcp_modules::settings_manager::SettingsState;
#[cfg(any(not(target_os = "ios"), test))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(target_os = "ios"))]
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_vcp_mobile::events::LifecyclePhase;
#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri_plugin_vcp_mobile::events::NativeLifecycleEvent;

static LIFECYCLE_TRANSITION_EPOCH: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE_TRANSITION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn reserve_lifecycle_transition() -> u64 {
    LIFECYCLE_TRANSITION_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_transition_epoch_current(epoch: u64) -> bool {
    epoch == LIFECYCLE_TRANSITION_EPOCH.load(Ordering::SeqCst)
}

#[cfg(any(not(target_os = "ios"), test))]
async fn lock_linger_interval(
    token: &tokio_util::sync::CancellationToken,
) -> Option<tokio::sync::MutexGuard<'static, ()>> {
    let guard = LIFECYCLE_TRANSITION_LOCK.lock().await;
    if token.is_cancelled() {
        None
    } else {
        Some(guard)
    }
}

#[cfg(any(not(target_os = "ios"), test))]
fn consume_reconnect_intent(flag: &AtomicBool, was_disconnected: bool) {
    if was_disconnected {
        flag.store(false, Ordering::SeqCst);
    }
}

pub fn is_app_in_foreground<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    if let Some(state) = app.try_state::<LifecycleState>() {
        state.ws_activity.is_foreground()
    } else {
        !cfg!(target_os = "ios")
    }
}

/// Native callbacks only enqueue work. Sequence rejection must precede epoch
/// reservation, otherwise a late native event could invalidate newer work.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn enqueue_native_lifecycle_event(app: AppHandle, event: NativeLifecycleEvent) {
    let epoch = {
        let Some(state) = app.try_state::<LifecycleState>() else {
            return;
        };
        let Ok(mut ordering) = state.native_lifecycle_ordering.lock() else {
            log::error!("[Lifecycle] Native ordering lock poisoned; event rejected");
            return;
        };
        if !ordering.accept(event.native_sequence) {
            return;
        }
        reserve_lifecycle_transition()
    };
    tauri::async_runtime::spawn(async move {
        apply_lifecycle_transition(app, LifecycleSnapshot::from_native(event, epoch)).await;
    });
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub async fn set_app_foreground_state_for_epoch(app: AppHandle, is_foreground: bool, epoch: u64) {
    apply_lifecycle_transition(
        app,
        LifecycleSnapshot {
            phase: if is_foreground {
                LifecyclePhase::Active
            } else {
                LifecyclePhase::Background
            },
            native_sequence: None,
            snapshot: false,
            transition_epoch: epoch,
        },
    )
    .await;
}

async fn apply_lifecycle_transition(app: AppHandle, snapshot: LifecycleSnapshot) {
    let _transition_guard = LIFECYCLE_TRANSITION_LOCK.lock().await;
    if !is_transition_epoch_current(snapshot.transition_epoch) {
        log::debug!(
            "[Lifecycle] Skipping stale transition epoch={}",
            snapshot.transition_epoch
        );
        return;
    }
    let state = match app.try_state::<LifecycleState>() {
        Some(s) => s,
        None => {
            log::warn!("[Lifecycle] LifecycleState not registered, skipping foreground transition");
            return;
        }
    };

    *state.lifecycle_snapshot.write().await = Some(snapshot);
    // Publish visibility before telling Vue to fetch its foreground snapshots.
    // Inactive changes rendering only and preserves the last networking phase.
    let is_foreground = match snapshot.phase {
        LifecyclePhase::Active => true,
        LifecyclePhase::Background => false,
        LifecyclePhase::Inactive => {
            let _ = app.emit("vcp-lifecycle-changed", snapshot);
            return;
        }
    };
    let changed = state.ws_activity.set_foreground(is_foreground);
    // The first same-phase snapshot is still needed by a newly attached WebView.
    let _ = app.emit("vcp-lifecycle-changed", snapshot);
    if !changed {
        return;
    }
    log::info!(
        "[Lifecycle] App foreground state transitioned: {} -> {}",
        !is_foreground,
        is_foreground
    );

    if is_foreground {
        super::vcp_log_service::flush_background_logs(&app);
    }
    #[cfg(target_os = "ios")]
    reconcile_ios_foreground_services(&app, is_foreground).await;
    #[cfg(not(target_os = "ios"))]
    reconcile_foreground_services(&app, &state, is_foreground).await;
}

#[cfg(target_os = "ios")]
async fn reconcile_ios_foreground_services(app: &AppHandle, is_foreground: bool) {
    if !is_foreground {
        let stop_distributed = async {
            if let Some(distributed) = app.try_state::<crate::distributed::DistributedState>() {
                let _reconciliation = distributed.reconciliation.lock().await;
                distributed.client.read().await.stop(app).await;
            }
        };
        let (_, sync_result) = tokio::join!(
            stop_distributed,
            crate::vcp_modules::sync_service::suspend_sync_for_background(app),
        );
        if let Err(error) = sync_result {
            log::warn!("[Lifecycle] Background sync shutdown: {error}");
        }
        return;
    }

    let settings = app.state::<SettingsState>();
    let _runtime_guard = settings.lock_runtime_reconcile().await;
    super::lifecycle_reconciler::reconcile_distributed_node(app, false).await;
    // Manual Sync is never restarted by foreground restoration. Chat reattaches
    // through its existing output owner; it is not reissued from lifecycle.
}

#[cfg(not(target_os = "ios"))]
async fn reconcile_foreground_services(
    app: &AppHandle,
    state: &LifecycleState,
    is_foreground: bool,
) {
    if !is_foreground {
        {
            let mut cancel_lock = state.linger.dist_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        state
            .linger
            .is_dist_disconnected
            .store(false, Ordering::SeqCst);

        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_config(app.clone(), settings_state).await {
            if settings.distributed.enabled {
                let is_running = if let Some(dist_state) =
                    app.try_state::<crate::distributed::DistributedState>()
                {
                    let client = dist_state.client.read().await;
                    client.is_running().await
                } else {
                    false
                };
                if !is_running {
                    log::debug!("[Lifecycle] Distributed enabled but client is idle; no background Guardian lease needed.");
                    return;
                }

                match tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(app, true) {
                    Ok(()) => {
                        log::info!("[Lifecycle] Distributed background Guardian lease acquired.")
                    }
                    Err(error) => log::warn!(
                        "[Lifecycle] Failed to acquire distributed background lease: {error}"
                    ),
                }

                let dist_token = tokio_util::sync::CancellationToken::new();
                {
                    let mut cancel_lock = state.linger.dist_cancel.lock().await;
                    *cancel_lock = Some(dist_token.clone());
                }
                let app_clone = app.clone();
                let dist_action_token = dist_token.clone();
                crate::vcp_modules::infra::utils::spawn_linger_task(
                    Duration::from_secs(300),
                    dist_token,
                    move || async move {
                        let Some(_transition_guard) =
                            lock_linger_interval(&dist_action_token).await
                        else {
                            return;
                        };
                        log::info!("[Lifecycle] Background distributed linger expired (5m). Stopping distributed client cleanly.");
                        if let Some(dist_state) =
                            app_clone.try_state::<crate::distributed::DistributedState>()
                        {
                            let client = dist_state.client.read().await;
                            client.stop(&app_clone).await;
                        }
                        if let Some(s) = app_clone.try_state::<LifecycleState>() {
                            s.linger.is_dist_disconnected.store(true, Ordering::SeqCst);
                        }
                        if let Err(error) =
                            tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(
                                &app_clone, false,
                            )
                        {
                            log::warn!(
                                "[Lifecycle] Failed to release expired distributed lease: {error}"
                            );
                        }
                    },
                );
            }
        }
    } else {
        {
            let mut cancel_lock = state.linger.dist_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        // The distributed Guardian remains owned by its background interval.
        if let Err(error) = tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(app, false) {
            log::warn!("[Lifecycle] Failed to release distributed lease on resume: {error}");
        }

        let was_dist_disconnected = state.linger.is_dist_disconnected.load(Ordering::SeqCst);

        let settings_state = app.state::<SettingsState>();
        let _runtime_guard = settings_state.lock_runtime_reconcile().await;
        if let Ok(settings) = read_config(app.clone(), app.state()).await {
            if was_dist_disconnected && settings.distributed.enabled {
                log::info!(
                    "[Lifecycle] App returned to foreground. Reconnecting distributed client."
                );
                crate::vcp_modules::infra::lifecycle_reconciler::reconcile_distributed_node(
                    app, false,
                )
                .await;
            }

            consume_reconnect_intent(&state.linger.is_dist_disconnected, was_dist_disconnected);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        consume_reconnect_intent, is_transition_epoch_current, lock_linger_interval,
        reserve_lifecycle_transition, LIFECYCLE_TRANSITION_LOCK,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn background_interval_survives_duplicate_epochs_but_not_resume_cancellation() {
        let older = reserve_lifecycle_transition();
        let interval = tokio_util::sync::CancellationToken::new();
        let newer = reserve_lifecycle_transition();
        assert!(newer > older);
        assert!(!is_transition_epoch_current(older));
        assert!(is_transition_epoch_current(newer));

        // 重复后台通知不创建新区间，也不延长期限；旧区间到期仍应执行。
        let interval_guard = lock_linger_interval(&interval).await;
        assert!(interval_guard.is_some());
        drop(interval_guard);

        let transition_guard = LIFECYCLE_TRANSITION_LOCK.lock().await;
        let action_ran = Arc::new(AtomicBool::new(false));
        let action_ran_in_task = action_ran.clone();
        let waiting_interval = interval.clone();

        let linger_action = tokio::spawn(async move {
            if let Some(_transition_guard) = lock_linger_interval(&waiting_interval).await {
                action_ran_in_task.store(true, Ordering::SeqCst);
            }
        });

        tokio::task::yield_now().await;
        assert!(!action_ran.load(Ordering::SeqCst));

        // 返回前台在同一 transition 锁内取消区间；已到期、正在等待锁的任务也必须退出。
        interval.cancel();
        drop(transition_guard);
        linger_action.await.expect("linger task should complete");

        assert!(!action_ran.load(Ordering::SeqCst));

        let reconnect_intent = AtomicBool::new(true);
        consume_reconnect_intent(&reconnect_intent, false);
        assert!(reconnect_intent.load(Ordering::SeqCst));
        consume_reconnect_intent(&reconnect_intent, true);
        assert!(!reconnect_intent.load(Ordering::SeqCst));
    }
}
