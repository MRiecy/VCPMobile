use tauri::AppHandle;

#[cfg(target_os = "android")]
const FLAG_KEEP_SCREEN_ON: i32 = 0x00000080;

/// 设置屏幕常亮（同步期间防止息屏）
#[tauri::command]
pub fn set_keep_screen_on(_app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use jni::objects::JValue;

        app.run_on_android_context(|env, activity, _webview| {
            let window = env
                .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
                .map_err(|e| format!("getWindow failed: {:?}", e))?;

            env.call_method(
                window.l().unwrap(),
                "addFlags",
                "(I)V",
                &[JValue::Int(FLAG_KEEP_SCREEN_ON)],
            )
            .map_err(|e| format!("addFlags failed: {:?}", e))?;

            Ok(())
        })
        .map_err(|e| format!("Android context error: {:?}", e))?
    }

    #[cfg(not(target_os = "android"))]
    {
        Ok(())
    }
}

/// 清除屏幕常亮
#[tauri::command]
pub fn clear_keep_screen_on(_app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        use jni::objects::JValue;

        app.run_on_android_context(|env, activity, _webview| {
            let window = env
                .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
                .map_err(|e| format!("getWindow failed: {:?}", e))?;

            env.call_method(
                window.l().unwrap(),
                "clearFlags",
                "(I)V",
                &[JValue::Int(FLAG_KEEP_SCREEN_ON)],
            )
            .map_err(|e| format!("clearFlags failed: {:?}", e))?;

            Ok(())
        })
        .map_err(|e| format!("Android context error: {:?}", e))?
    }

    #[cfg(not(target_os = "android"))]
    {
        Ok(())
    }
}
