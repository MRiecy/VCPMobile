#[allow(unused_imports)]
use tauri::{AppHandle, Manager, Runtime};

#[allow(unused_imports)]
use crate::VcpMobileState;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_LEASE_ID: AtomicU64 = AtomicU64::new(1);

/// Business intent is explicit; a display name never determines resource policy.
#[derive(Clone, Copy)]
pub enum ForegroundTask {
    Stream,
    Sync,
    Prerender,
}

impl ForegroundTask {
    fn policy(self) -> (&'static str, i32, bool) {
        match self {
            Self::Stream => ("stream", 20, false),
            Self::Sync => ("sync", 40, true),
            Self::Prerender => ("prerender", 30, true),
        }
    }

    fn unique_tag(self) -> String {
        format!(
            "{}:{}",
            self.policy().0,
            NEXT_LEASE_ID.fetch_add(1, Ordering::Relaxed)
        )
    }
}

/// One operation owns one native consumer, including when its future is cancelled.
/// Not Clone: ownership and release cannot accidentally be duplicated.
pub struct ForegroundLease<R: Runtime> {
    app: AppHandle<R>,
    tag: Option<String>,
}

impl<R: Runtime> ForegroundLease<R> {
    pub fn acquire(app: &AppHandle<R>, task: ForegroundTask, label: &str) -> Result<Self, String> {
        let tag = task.unique_tag();
        let (_, _priority, screen_keep_on) = task.policy();
        #[cfg(target_os = "ios")]
        {
            // Chat reports progress separately from screen-on work.
            if !screen_keep_on {
                return Ok(Self {
                    app: app.clone(),
                    tag: None,
                });
            }
            crate::ios::run::<R, ()>(
                app.clone(),
                "acquireBackgroundExecution",
                serde_json::json!({
                    "tag": tag,
                    "screenKeepOn": screen_keep_on,
                }),
            )
            .inspect_err(|error| {
                crate::events::report_service_failure(app, "guardian", error);
            })?;
            log::info!("[VcpMobilePlugin] iOS task resource acquired: tag={tag}, label={label}");
        }
        #[cfg(not(target_os = "ios"))]
        acquire_foreground_inner(app, &tag, _priority, label, screen_keep_on)?;
        Ok(Self {
            app: app.clone(),
            tag: Some(tag),
        })
    }

    pub fn release(mut self) -> Result<(), String> {
        if let Some(tag) = self.tag.take() {
            release_foreground_inner(&self.app, &tag)?;
        }
        Ok(())
    }
}

/// Native calls used by the detached iOS reply display.
#[cfg(target_os = "ios")]
pub mod ios_chat {
    use super::*;
    use serde_json::json;

    async fn call<R: Runtime>(app: &AppHandle<R>, command: &str, args: serde_json::Value) -> bool {
        let result = match app.state::<VcpMobileState<R>>().mobile_plugin_handle() {
            Ok(handle) => handle
                .run_mobile_plugin_async::<()>(command, args)
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        result
            .inspect_err(|error| log::warn!("[iOS Chat] {command}: {error}"))
            .is_ok()
    }

    pub async fn begin<R: Runtime>(app: &AppHandle<R>, tag: &str, title: &str) -> bool {
        call(
            app,
            "beginChatBackgroundTask",
            json!({ "tag": tag, "title": title }),
        )
        .await
    }

    pub async fn finish<R: Runtime>(app: &AppHandle<R>, tag: &str, success: bool) {
        call(
            app,
            "finishChatBackgroundTask",
            json!({ "tag": tag, "success": success }),
        )
        .await;
    }
}

impl<R: Runtime> Drop for ForegroundLease<R> {
    fn drop(&mut self) {
        if let Some(tag) = self.tag.take() {
            let app = self.app.clone();
            // Mobile IPC waits for the native owner. Cancellation cleanup must not block
            // the thread dropping the future (or a native event callback).
            tauri::async_runtime::spawn_blocking(move || {
                let _ = release_foreground_inner(&app, &tag);
            });
        }
    }
}

/// 申请持有进程级前台锁 (语义化接口)
pub fn acquire_foreground_inner<R: Runtime>(
    _app: &AppHandle<R>,
    tag: &str,
    priority: i32,
    label: &str,
    screen_keep_on: bool,
) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    crate::ios_not_implemented::<()>("acquire_foreground")?;

    #[cfg(target_os = "android")]
    {
        let result = (|| {
            let state = _app.state::<VcpMobileState<R>>();
            let plugin_handle = state.mobile_plugin_handle()?;
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "acquireForeground",
                    serde_json::json!({
                        "tag": tag,
                        "priority": priority,
                        "label": label,
                        "screenKeepOn": screen_keep_on
                    }),
                )
                .map_err(|e| format!("acquireForeground failed: {}", e))
        })();
        result.inspect_err(|error| {
            crate::events::report_service_failure(_app, "guardian", error);
        })?;
    }

    log::info!(
        "[VcpMobilePlugin] acquire_foreground_inner: tag={}, priority={}, label={}, screenKeepOn={}",
        tag, priority, label, screen_keep_on
    );

    Ok(())
}

/// 释放进程级前台锁 (语义化接口)
pub fn release_foreground_inner<R: Runtime>(_app: &AppHandle<R>, tag: &str) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    crate::ios::run::<R, ()>(
        _app.clone(),
        "releaseBackgroundExecution",
        serde_json::json!({ "tag": tag }),
    )
    .inspect_err(|error| {
        crate::events::report_service_failure(_app, "guardian", error);
    })?;

    #[cfg(target_os = "android")]
    {
        let result = (|| {
            let state = _app.state::<VcpMobileState<R>>();
            let plugin_handle = state.mobile_plugin_handle()?;
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>(
                    "releaseForeground",
                    serde_json::json!({ "tag": tag }),
                )
                .map_err(|e| format!("releaseForeground failed: {}", e))
        })();
        result.inspect_err(|error| {
            crate::events::report_service_failure(_app, "guardian", error);
        })?;
    }

    log::info!("[VcpMobilePlugin] release_foreground_inner: tag={}", tag);

    Ok(())
}

/// 设置分布式保活模式 (兼容老版本接口)
pub fn set_keepalive_mode_inner<R: Runtime>(
    app: &AppHandle<R>,
    is_keepalive: bool,
) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    crate::ios_unsupported::<()>("set_keepalive_mode")?;

    if is_keepalive {
        acquire_foreground_inner(app, "distributed", 10, "distributed", false)
    } else {
        release_foreground_inner(app, "distributed")
    }
}

#[tauri::command]
pub fn acquire_foreground<R: Runtime>(
    app: AppHandle<R>,
    tag: String,
    priority: i32,
    label: String,
    screen_keep_on: bool,
) -> Result<(), String> {
    acquire_foreground_inner(&app, &tag, priority, &label, screen_keep_on)
}

#[tauri::command]
pub fn release_foreground<R: Runtime>(app: AppHandle<R>, tag: String) -> Result<(), String> {
    release_foreground_inner(&app, &tag)
}

#[tauri::command]
pub fn set_keepalive_mode<R: Runtime>(app: AppHandle<R>, is_keepalive: bool) -> Result<(), String> {
    set_keepalive_mode_inner(&app, is_keepalive)
}

#[tauri::command]
#[allow(unused_variables)]
pub fn start_helper_service<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    crate::ios_unsupported::<()>("start_helper_service")?;

    #[cfg(target_os = "android")]
    {
        let result = (|| {
            let state = app.state::<VcpMobileState<R>>();
            let plugin_handle = state.mobile_plugin_handle()?;
            plugin_handle
                .run_mobile_plugin::<serde_json::Value>("startHelperService", serde_json::json!({}))
                .map_err(|e| format!("startHelperService failed: {}", e))
        })();
        result.inspect_err(|error| {
            crate::events::report_service_failure(&app, "helper", error);
        })?;
    }
    log::info!("[VcpMobilePlugin] start_helper_service called");
    Ok(())
}
