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
    #[serde(rename = "voiceAudio")]
    VoiceAudio(crate::voice::NativeVoiceAudioEvent),
    #[serde(rename = "voiceDocument")]
    VoiceDocument {
        #[serde(rename = "documentId")]
        document_id: String,
    },
    #[serde(rename = "voicePlayback")]
    VoicePlayback(NativeVoicePlaybackEvent),
    // Configuration/memory notifications do not describe an app phase.
    #[serde(rename = "configuration", alias = "memory-pressure")]
    Other {},
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NativeVoicePlaybackEvent {
    Started {
        #[serde(rename = "playbackId")]
        playback_id: String,
    },
    Message {
        #[serde(rename = "playbackId")]
        playback_id: String,
        #[serde(rename = "messageId")]
        message_id: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeNotificationClick {
    pub click_id: String,
    pub sequence: u64,
    #[serde(flatten)]
    pub target: NotificationClickTarget,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum NotificationClickTarget {
    App,
    Chat {
        owner_type: String,
        owner_id: String,
        topic_id: String,
        message_id: String,
    },
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
    fn notification_event_preserves_the_committed_message_target() {
        let target = serde_json::json!({
            "clickId": "first", "sequence": 1, "kind": "chat", "ownerType": "group",
            "ownerId": "g", "topicId": "t", "messageId": "persisted-assistant-message"
        });
        let NativeEvent::NotificationClick(click) = serde_json::from_value(serde_json::json!({
            "event": "notificationClick", "payload": target,
        }))
        .unwrap() else {
            panic!("unexpected event");
        };
        assert_eq!(serde_json::to_value(click).unwrap(), target);

        let generic = serde_json::json!({"clickId": "second", "sequence": 2, "kind": "app"});
        let click: NativeNotificationClick = serde_json::from_value(generic.clone()).unwrap();
        assert_eq!(serde_json::to_value(click).unwrap(), generic);
    }

    #[test]
    fn native_document_loss_is_delivered_without_an_audio_session() {
        let event: NativeEvent = serde_json::from_value(serde_json::json!({
            "event": "voiceDocument", "payload": { "documentId": "42" },
        }))
        .unwrap();
        assert!(matches!(event, NativeEvent::VoiceDocument { document_id } if document_id == "42"));
        assert!(serde_json::from_value::<NativeEvent>(serde_json::json!({
            "event": "voiceDocument", "payload": {},
        }))
        .is_err());
    }

    #[test]
    fn native_playback_started_keeps_its_exact_output_identity() {
        let event: NativeEvent = serde_json::from_value(serde_json::json!({
            "event": "voicePlayback",
            "payload": { "playbackId": "current-output", "kind": "started" },
        }))
        .unwrap();
        assert!(matches!(event, NativeEvent::VoicePlayback(
            NativeVoicePlaybackEvent::Started { playback_id }
        ) if playback_id == "current-output"));
        assert!(serde_json::from_value::<NativeEvent>(serde_json::json!({
            "event": "voicePlayback",
            "payload": { "kind": "started" },
        }))
        .is_err());
    }

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
