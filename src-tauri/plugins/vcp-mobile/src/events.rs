use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

#[derive(Debug, Deserialize)]
#[serde(tag = "event", content = "payload")]
pub enum NativeEvent {
    #[serde(rename = "lifecycle")]
    Lifecycle(NativeLifecycleEvent),
    #[serde(rename = "network")]
    Network { connected: bool },
    #[serde(rename = "notificationClick")]
    NotificationClick(NativeNotificationClick),
    /// A native resource failure does not finish or restart a Chat request.
    #[serde(rename = "backgroundExecutionFailure")]
    BackgroundExecutionFailure { reason: String },
    // Configuration/memory notifications do not describe an app phase.
    #[serde(rename = "configuration", alias = "memory-pressure")]
    Other {},
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeNotificationClick {
    pub click_id: String,
    pub sequence: u64,
    pub kind: NotificationClickKind,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationClickKind {
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LifecyclePhase {
    Active,
    Inactive,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLifecycleEvent {
    pub phase: LifecyclePhase,
    /// Monotonic within the native process event source; independent of Rust epochs.
    pub native_sequence: u64,
    pub snapshot: bool,
}

/// One native subscription per process. The Channel callback runs on the native
/// calling thread: consumers must only enqueue work, never invoke native APIs here.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub async fn subscribe<R: Runtime>(
    app: &AppHandle<R>,
    on_event: impl Fn(NativeEvent) + Send + Sync + 'static,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    let handle = app
        .state::<crate::VcpMobileState<R>>()
        .mobile_plugin_handle()?;
    let channel = tauri::ipc::Channel::<serde_json::Value>::new(move |body| {
        match body.deserialize::<NativeEvent>() {
            Ok(event) => on_event(event),
            Err(error) => log::warn!("[VcpMobilePlugin] Invalid native event: {error}"),
        }
        Ok(())
    });
    // Do not cancel this future on a timeout: the mobile bridge must receive the
    // eventual native response. Kotlin resolves after installing and replaying.
    #[cfg(target_os = "android")]
    handle
        .run_mobile_plugin_async::<serde_json::Value>(
            "registerNativeEvents",
            serde_json::json!({ "channel": channel }),
        )
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "ios")]
    crate::ios::register_native_events(app, channel).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceFailure {
    Guardian,
    Helper,
    Events,
}

/// Bounded to the three capability failures. Event registration has no retry in
/// this process, so its failure must remain visible to replacement WebViews.
#[derive(Default)]
pub(crate) struct ServiceFailureNotices(Mutex<BTreeSet<ServiceFailure>>);

impl ServiceFailureNotices {
    fn collect(&self) -> Vec<ServiceFailure> {
        let Ok(mut failures) = self.0.lock() else {
            return Vec::new();
        };
        let notices = failures.iter().copied().collect();
        // Guardian/helper keep their existing one-shot snapshot semantics.
        failures.retain(|failure| *failure == ServiceFailure::Events);
        notices
    }
}

pub fn report_service_failure<R: Runtime>(app: &AppHandle<R>, service: &str, error: &str) {
    log::warn!("[VcpMobilePlugin] {service} failed: {error}");
    let failure = match service {
        "guardian" => ServiceFailure::Guardian,
        "helper" => ServiceFailure::Helper,
        "events" => ServiceFailure::Events,
        _ => return,
    };
    if let Some(notices) = app.try_state::<ServiceFailureNotices>() {
        if let Ok(mut failures) = notices.0.lock() {
            failures.insert(failure);
        }
    }
    let _ = app.emit(
        "vcp-system-event",
        serde_json::json!({ "type": "vcp-background-service-error", "service": failure }),
    );
}

pub fn collect_service_failure_notices<R: Runtime>(app: &AppHandle<R>) -> Vec<ServiceFailure> {
    let Some(notices) = app.try_state::<ServiceFailureNotices>() else {
        return Vec::new();
    };
    notices.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_replay_events_but_consume_guardian_and_helper_notices() {
        let notices = ServiceFailureNotices::default();
        assert!(notices.collect().is_empty());
        notices.0.lock().unwrap().extend([
            ServiceFailure::Guardian,
            ServiceFailure::Helper,
            ServiceFailure::Events,
        ]);
        assert_eq!(
            notices.collect(),
            vec![
                ServiceFailure::Guardian,
                ServiceFailure::Helper,
                ServiceFailure::Events,
            ]
        );
        assert_eq!(notices.collect(), vec![ServiceFailure::Events]);
        notices.0.lock().unwrap().insert(ServiceFailure::Helper);
        assert_eq!(
            notices.collect(),
            vec![ServiceFailure::Helper, ServiceFailure::Events]
        );
        assert_eq!(notices.collect(), vec![ServiceFailure::Events]);
    }
}
