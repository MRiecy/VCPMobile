//! The iOS bridge installs the process-wide native event Channel and retains
//! its response receiver until Swift finishes registration.
#[cfg(target_os = "ios")]
use serde::Deserialize;

use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[cfg(target_os = "ios")]
use tauri::{AppHandle, Manager, Runtime};

/// Synchronous mobile calls keep Tauri's native receiver on this stack until the
/// final response. Call only on an async-command or blocking worker, never UIKit.
#[cfg(target_os = "ios")]
pub(crate) fn run<R: Runtime, T: serde::de::DeserializeOwned>(
    app: AppHandle<R>,
    operation: &str,
    args: serde_json::Value,
) -> Result<T, String> {
    app.state::<crate::VcpMobileState<R>>()
        .mobile_plugin_handle()?
        .run_mobile_plugin(operation, args)
        .map_err(|error| format!("native_bridge: {operation}: {error}"))
}

/// Asynchronous frontend requests wait outside Tokio's network workers. Once
/// started, this blocking task retains the native receiver even if its caller leaves.
#[cfg(target_os = "ios")]
pub(crate) async fn run_async<R: Runtime, T: serde::de::DeserializeOwned + Send + 'static>(
    app: AppHandle<R>,
    operation: &'static str,
    args: serde_json::Value,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || run(app, operation, args))
        .await
        .map_err(|error| format!("native_worker: {operation}: {error}"))?
}

#[cfg(target_os = "ios")]
fn validate_protocol(version: u32) -> Result<(), String> {
    if version != 1 {
        return Err(format!(
            "native_protocol: unsupported iOS bridge version {version}"
        ));
    }
    Ok(())
}

struct CallSlot(Arc<AtomicBool>);

impl Drop for CallSlot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The caller may stop waiting, but the registration slot stays occupied until
/// Swift's final acknowledgement. Keep its receiver alive to preserve that boundary.
async fn receive_native<T, F>(
    busy: Arc<AtomicBool>,
    operation: &'static str,
    response: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| format!("native_busy: {operation} still awaits its native response"))?;
    let slot = CallSlot(busy);
    let receiver = tauri::async_runtime::spawn(async move {
        let _slot = slot;
        response.await
    });
    // Cancelling this wait drops only the JoinHandle. Tokio detaches the task,
    // retaining the mobile receiver until Swift resolves/rejects and releases it.
    receiver
        .await
        .map_err(|error| format!("native_bridge: {operation}: {error}"))?
}

#[cfg(target_os = "ios")]
pub(crate) async fn register_native_events<R: Runtime>(
    app: &AppHandle<R>,
    channel: tauri::ipc::Channel<serde_json::Value>,
) -> Result<(), String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Registration {
        protocol_version: u32,
    }
    let state = app.state::<crate::VcpMobileState<R>>();
    let handle = state.mobile_plugin_handle()?;
    let busy = state.ios_event_registration.clone();
    // The independent bootstrap subscription task consumes the final native
    // acknowledgement without imposing a deadline on registration.
    let registration = receive_native(busy, "registerNativeEvents", async move {
        handle
            .run_mobile_plugin_async::<Registration>(
                "registerNativeEvents",
                serde_json::json!({ "channel": channel }),
            )
            .await
            .map_err(|error| error.to_string())
    })
    .await?;
    validate_protocol(registration.protocol_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::oneshot;

    async fn wait_until_idle(busy: &AtomicBool) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while busy.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the late native response must release its call slot");
    }

    #[tokio::test]
    async fn cancelling_the_caller_does_not_cancel_native_work() {
        let busy = Arc::new(AtomicBool::new(false));
        let (started, start) = oneshot::channel();
        let (sender, response) = oneshot::channel::<u32>();
        let (finished, completion) = oneshot::channel();
        let caller_busy = busy.clone();
        let caller = tokio::spawn(async move {
            receive_native(caller_busy, "test", async move {
                let _ = started.send(());
                let value = response.await.map_err(|error| error.to_string())?;
                let _ = finished.send(());
                Ok(value)
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), start)
            .await
            .unwrap()
            .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(busy.load(Ordering::Acquire));
        let rejected = receive_native(busy.clone(), "test", async { Ok(0_u32) }).await;
        assert!(rejected.unwrap_err().starts_with("native_busy:"));
        sender.send(9).unwrap();
        tokio::time::timeout(Duration::from_secs(1), completion)
            .await
            .unwrap()
            .unwrap();
        wait_until_idle(&busy).await;
        assert_eq!(
            receive_native(busy, "test", async { Ok(8_u32) })
                .await
                .unwrap(),
            8
        );
    }

    #[tokio::test]
    async fn retained_wait_delivers_the_final_native_success_or_failure() {
        for outcome in [Ok(7_u32), Err("native_events: rejected".to_string())] {
            let busy = Arc::new(AtomicBool::new(false));
            let (sender, response) = oneshot::channel();
            let caller = receive_native(busy.clone(), "test", async move {
                response.await.map_err(|error| error.to_string())?
            });
            tokio::pin!(caller);

            // Borrow the wait so this observation does not cancel its owner.
            assert!(tokio::time::timeout(Duration::from_millis(10), &mut caller)
                .await
                .is_err());
            assert!(busy.load(Ordering::Acquire));
            sender.send(outcome.clone()).unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), caller)
                    .await
                    .unwrap(),
                outcome
            );
            assert!(!busy.load(Ordering::Acquire));
        }
    }
}
