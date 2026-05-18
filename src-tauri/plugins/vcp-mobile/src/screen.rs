use tauri::{AppHandle, Manager, Runtime};

#[cfg(target_os = "android")]
const FLAG_KEEP_SCREEN_ON: i32 = 0x00000080;

/// Set screen to keep awake during sync / streaming.
#[tauri::command]
#[allow(unused_variables)]
pub fn set_keep_screen_on<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use jni::objects::JValue;

        let window = app
            .get_webview_window("main")
            .ok_or("main window not found")?;
        window
            .as_ref()
            .with_webview(|webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let Ok(window) =
                        env.call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
                    else {
                        log::error!("[VcpMobilePlugin] getWindow failed");
                        return;
                    };
                    if let Err(e) = env.call_method(
                        window.l().unwrap(),
                        "addFlags",
                        "(I)V",
                        &[JValue::Int(FLAG_KEEP_SCREEN_ON)],
                    ) {
                        log::error!("[VcpMobilePlugin] addFlags failed: {:?}", e);
                    }
                });
            })
            .map_err(|e| format!("with_webview failed: {:?}", e))?;
    }

    Ok(())
}

/// Clear keep-screen-on flag.
#[tauri::command]
#[allow(unused_variables)]
pub fn clear_keep_screen_on<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use jni::objects::JValue;

        let window = app
            .get_webview_window("main")
            .ok_or("main window not found")?;
        window
            .as_ref()
            .with_webview(|webview| {
                webview.jni_handle().exec(move |env, activity, _webview| {
                    let Ok(window) =
                        env.call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
                    else {
                        log::error!("[VcpMobilePlugin] getWindow failed");
                        return;
                    };
                    if let Err(e) = env.call_method(
                        window.l().unwrap(),
                        "clearFlags",
                        "(I)V",
                        &[JValue::Int(FLAG_KEEP_SCREEN_ON)],
                    ) {
                        log::error!("[VcpMobilePlugin] clearFlags failed: {:?}", e);
                    }
                });
            })
            .map_err(|e| format!("with_webview failed: {:?}", e))?;
    }

    Ok(())
}
