use crate::vcp_modules::infra::lifecycle_state::LifecycleState;
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub fn is_app_in_foreground<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    if let Some(state) = app.try_state::<LifecycleState>() {
        state.is_foreground.load(Ordering::SeqCst)
    } else {
        true
    }
}

#[tauri::command]
pub async fn set_app_foreground_state(app: AppHandle, is_foreground: bool) {
    set_app_foreground_state_internal(app, is_foreground).await;
}

pub async fn set_app_foreground_state_internal(app: AppHandle, is_foreground: bool) {
    let state = match app.try_state::<LifecycleState>() {
        Some(s) => s,
        None => {
            log::warn!("[Lifecycle] LifecycleState not registered, skipping foreground transition");
            return;
        }
    };

    let was_foreground = state.is_foreground.swap(is_foreground, Ordering::SeqCst);
    if was_foreground == is_foreground {
        return;
    }
    log::info!(
        "[Lifecycle] App foreground state transitioned: {} -> {}",
        was_foreground,
        is_foreground
    );

    // 1. 调整心跳频率
    crate::vcp_modules::infra::vcp_log_service::handle_foreground_state_change(&app, is_foreground)
        .await;

    // 向前端广播最新的前台状态（Tauri 官方单通道）
    let _ = app.emit(
        "vcp-lifecycle-changed",
        serde_json::json!({
            "state": if is_foreground { "resume" } else { "pause" }
        }),
    );

    if !is_foreground {
        // --- 进入后台 ---
        // 1.1 取消旧倒计时任务
        {
            let mut cancel_lock = state.linger.log_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        {
            let mut cancel_lock = state.linger.dist_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        state
            .linger
            .is_log_disconnected
            .store(false, Ordering::SeqCst);
        state
            .linger
            .is_dist_disconnected
            .store(false, Ordering::SeqCst);

        // 1.1b 申请持有 vcp_log 对应的原生进程级前台锁，以保证 10 分钟内后台存活
        let _ = tauri_plugin_vcp_mobile::stream::acquire_foreground_inner(
            &app,
            "vcp_log",
            10,
            "VCP Log Linger",
            false,
        );

        // 1.2 开启 VCPLog/Info (10分钟延迟断连任务)
        let log_token = tokio_util::sync::CancellationToken::new();
        {
            let mut cancel_lock = state.linger.log_cancel.lock().await;
            *cancel_lock = Some(log_token.clone());
        }
        let app_clone = app.clone();
        crate::vcp_modules::infra::utils::spawn_linger_task(
            Duration::from_secs(600),
            log_token,
            move || async move {
                log::info!(
                    "[Lifecycle] Background linger expired (10m). Disconnecting VCPLog/Info."
                );
                let _ = crate::vcp_modules::infra::vcp_log_service::disconnect_log_connections(
                    &app_clone,
                )
                .await;
                if let Some(s) = app_clone.try_state::<LifecycleState>() {
                    s.linger.is_log_disconnected.store(true, Ordering::SeqCst);
                    // 10 分钟到期，释放 vcp_log 的前台锁
                    let _ = tauri_plugin_vcp_mobile::stream::release_foreground_inner(
                        &app_clone, "vcp_log",
                    );
                }
            },
        );

        // 1.3 开启 Distributed (5分钟保活冷却任务)
        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_settings(app.clone(), settings_state).await {
            if settings.distributed_enabled {
                log::info!("[Lifecycle] Distributed enabled. Active FGS lock is already managed by distributed client.");

                let dist_token = tokio_util::sync::CancellationToken::new();
                {
                    let mut cancel_lock = state.linger.dist_cancel.lock().await;
                    *cancel_lock = Some(dist_token.clone());
                }
                let app_clone = app.clone();
                crate::vcp_modules::infra::utils::spawn_linger_task(
                    Duration::from_secs(300),
                    dist_token,
                    move || async move {
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
                    },
                );
            }
        }
    } else {
        // --- 返回前台 ---
        // 2.1 立即取消所有倒计时并释放 vcp_log 锁
        {
            let mut cancel_lock = state.linger.log_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        {
            let mut cancel_lock = state.linger.dist_cancel.lock().await;
            if let Some(token) = cancel_lock.take() {
                token.cancel();
            }
        }
        let _ = tauri_plugin_vcp_mobile::stream::release_foreground_inner(&app, "vcp_log");

        // 2.2 恢复分布式保活状态 (前台关闭保活通知 - 如果仍然运行的话)
        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_settings(app.clone(), settings_state).await {
            if settings.distributed_enabled {
                let _ = tauri_plugin_vcp_mobile::stream::set_keepalive_mode_inner(&app, false);
            }
        }

        // 2.3 若此前已冷断开，一键拉起恢复
        let was_log_disconnected = state
            .linger
            .is_log_disconnected
            .swap(false, Ordering::SeqCst);
        let was_dist_disconnected = state
            .linger
            .is_dist_disconnected
            .swap(false, Ordering::SeqCst);

        let settings_state = app.state::<SettingsState>();
        if let Ok(settings) = read_settings(app.clone(), settings_state).await {
            if was_log_disconnected {
                let log_url = settings.vcp_log_url;
                let log_key = settings.vcp_log_key;
                if !log_url.trim().is_empty() && !log_key.trim().is_empty() {
                    log::info!("[Lifecycle] App returned to foreground. Reconnecting VCPLog/Info.");
                    let _ = crate::vcp_modules::infra::vcp_log_service::reconnect_log_connections(
                        &app, log_url, log_key,
                    )
                    .await;
                }
            } else {
                // 如果连接没有断开，则冲刷后台缓存的日志消息到前端 WebView
                crate::vcp_modules::infra::vcp_log_service::flush_background_logs(&app);
            }

            if was_dist_disconnected && settings.distributed_enabled {
                log::info!(
                    "[Lifecycle] App returned to foreground. Reconnecting distributed client."
                );
                crate::vcp_modules::infra::lifecycle_reconciler::reconcile_distributed_node(
                    &app, true, false,
                )
                .await;
            }
        }
    }
}
