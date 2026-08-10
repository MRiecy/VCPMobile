#[allow(unused_imports)]
use tauri::{AppHandle, Manager, Runtime};

#[cfg(target_os = "android")]
fn set_manual_screen_owner<R: Runtime>(app: &AppHandle<R>, requested: bool) -> Result<(), String> {
    use jni::objects::JValue;

    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    window
        .as_ref()
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                if let Err(error) = env.call_static_method(
                    "com/vcp/mobile/ScreenKeepOnArbiter",
                    "setManualRequested",
                    "(Landroid/app/Activity;Z)V",
                    &[
                        JValue::Object(activity),
                        JValue::Bool(if requested { 1 } else { 0 }),
                    ],
                ) {
                    log::error!(
                        "[VcpMobilePlugin] ScreenKeepOnArbiter.setManualRequested failed: {:?}",
                        error
                    );
                }
            });
        })
        .map_err(|error| format!("with_webview failed: {:?}", error))?;
    Ok(())
}

/// Set screen to keep awake during sync / streaming.
#[tauri::command]
#[allow(unused_variables)]
pub fn set_keep_screen_on<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        set_manual_screen_owner(&app, true)?;
    }

    Ok(())
}

/// Clear keep-screen-on flag.
#[tauri::command]
#[allow(unused_variables)]
pub fn clear_keep_screen_on<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        set_manual_screen_owner(&app, false)?;
    }

    Ok(())
}
