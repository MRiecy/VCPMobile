//! Visual handoff only: native owners retain one foreground snapshot and wait
//! for their renderer before removing it. Chat and application state stay with
//! their existing owners; these acknowledgements never mean generation ended.

#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct WebViewResumeState {
    pub token: Option<String>,
}

async fn call<R: Runtime, T: serde::de::DeserializeOwned + Send + 'static>(
    app: AppHandle<R>,
    operation: &'static str,
    args: serde_json::Value,
) -> Result<T, String> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        let handle = app
            .state::<crate::VcpMobileState<R>>()
            .mobile_plugin_handle()?;
        // Native resolves admission promptly. PixelCopy, presentation updates
        // and retry UI are native-owned work, not a long-lived IPC invocation.
        tauri::async_runtime::spawn_blocking(move || {
            handle
                .run_mobile_plugin(operation, args)
                .map_err(|error| format!("webview_resume: {operation}: {error}"))
        })
        .await
        .map_err(|error| format!("webview_resume_worker: {error}"))?
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let _ = (app, operation, args);
        Err("WebView visual resume is only available on mobile".into())
    }
}

/// Called after the document installs its listener. Replays a pending token
/// when a reload missed the native foreground notification.
#[tauri::command]
pub async fn connect_webview_resume<R: Runtime>(
    app: AppHandle<R>,
    document_id: String,
) -> Result<WebViewResumeState, String> {
    call(
        app,
        "connectWebViewResume",
        serde_json::json!({ "documentId": document_id }),
    )
    .await
}

/// Vue has finished this document's visible layout. Native still checks the
/// document/token and waits for its own drawing barrier before revealing it.
#[tauri::command]
pub async fn complete_webview_resume<R: Runtime>(
    app: AppHandle<R>,
    document_id: String,
    token: String,
) -> Result<(), String> {
    call::<R, serde_json::Value>(
        app,
        "completeWebViewResume",
        serde_json::json!({ "documentId": document_id, "token": token }),
    )
    .await
    .map(|_| ())
}
