use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, Runtime};

use crate::VcpMobileState;

// =============================================================================
// Public Rust API (for internal Rust callers)
// =============================================================================

/// Start the stream keepalive service.
/// Counter 0→1 triggers actual Android foreground service start.
pub fn start_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    let state = app.state::<VcpMobileState>();
    let count = state.streaming_count.fetch_add(1, Ordering::SeqCst);

    if count == 0 {
        #[cfg(target_os = "android")]
        {
            start_android_service(app, agent_name)?;
        }
    }

    log::info!(
        "[VcpMobilePlugin] Stream started for '{}'. Active count: {}",
        agent_name,
        count + 1
    );

    Ok(())
}

/// Stop the stream keepalive service.
/// Counter reaches 0 triggers actual Android service stop.
pub fn stop_stream_service_inner<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let state = app.state::<VcpMobileState>();
    let count = state.streaming_count.fetch_sub(1, Ordering::SeqCst);

    if count <= 1 {
        state.streaming_count.store(0, Ordering::SeqCst);
        #[cfg(target_os = "android")]
        {
            stop_android_service(app)?;
        }
    }

    log::info!(
        "[VcpMobilePlugin] Stream stopped. Active count: {}",
        state.streaming_count.load(Ordering::SeqCst)
    );

    Ok(())
}

// =============================================================================
// Tauri Commands (for frontend invoke)
// =============================================================================

#[tauri::command]
pub fn start_stream_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
) -> Result<(), String> {
    start_stream_service_inner(&app, &agent_name)
}

#[tauri::command]
pub fn stop_stream_service<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    stop_stream_service_inner(&app)
}

// =============================================================================
// Android JNI implementation
// =============================================================================

#[cfg(target_os = "android")]
fn start_android_service<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    use jni::objects::JValue;

    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let agent_name = agent_name.to_string();

    window
        .as_ref()
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = (|| -> Result<(), String> {
                    let intent_cls = env
                        .find_class("android/content/Intent")
                        .map_err(|e| format!("Find Intent failed: {:?}", e))?;

                    let activity_cls = env
                        .call_method(activity, "getClass", "()Ljava/lang/Class;", &[])
                        .map_err(|e| format!("getClass failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("getClass result is not an object: {:?}", e))?;
                    let class_loader = env
                        .call_method(
                            &activity_cls,
                            "getClassLoader",
                            "()Ljava/lang/ClassLoader;",
                            &[],
                        )
                        .map_err(|e| format!("getClassLoader failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("getClassLoader result is not an object: {:?}", e))?;
                    let service_name = env
                        .new_string("com.vcp.mobile.service.StreamKeepaliveService")
                        .map_err(|e| format!("new_string failed: {:?}", e))?;
                    let service_cls = env
                        .call_method(
                            &class_loader,
                            "loadClass",
                            "(Ljava/lang/String;)Ljava/lang/Class;",
                            &[JValue::Object(&service_name)],
                        )
                        .map_err(|e| format!("loadClass failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("loadClass result is not an object: {:?}", e))?;

                    let intent = env
                        .new_object(
                            &intent_cls,
                            "(Landroid/content/Context;Ljava/lang/Class;)V",
                            &[JValue::Object(activity), JValue::Object(&service_cls)],
                        )
                        .map_err(|e| format!("New Intent failed: {:?}", e))?;

                    let key = env
                        .new_string("agent_name")
                        .map_err(|e| format!("New string key failed: {:?}", e))?;
                    let value = env
                        .new_string(&agent_name)
                        .map_err(|e| format!("New string value failed: {:?}", e))?;

                    env.call_method(
                        &intent,
                        "putExtra",
                        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&key), JValue::Object(&value)],
                    )
                    .map_err(|e| format!("putExtra failed: {:?}", e))?;

                    let version_cls = env
                        .find_class("android/os/Build$VERSION")
                        .map_err(|e| format!("Find VERSION failed: {:?}", e))?;
                    let version = env
                        .get_static_field(&version_cls, "SDK_INT", "I")
                        .map_err(|e| format!("Get SDK_INT failed: {:?}", e))?
                        .i()
                        .unwrap_or(0);

                    let (method_name, method_sig) = if version >= 26 {
                        (
                            "startForegroundService",
                            "(Landroid/content/Intent;)Landroid/content/ComponentName;",
                        )
                    } else {
                        (
                            "startService",
                            "(Landroid/content/Intent;)Landroid/content/ComponentName;",
                        )
                    };

                    env.call_method(
                        activity,
                        method_name,
                        method_sig,
                        &[JValue::Object(&intent)],
                    )
                    .map_err(|e| format!("{} failed: {:?}", method_name, e))?;

                    Ok(())
                })();

                if let Err(ref e) = result {
                    log::error!("[VcpMobilePlugin] start_android_service failed: {}", e);
                }
            });
        })
        .map_err(|e| format!("with_webview failed: {:?}", e))?;

    Ok(())
}

#[cfg(target_os = "android")]
fn stop_android_service<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use jni::objects::JValue;

    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;

    window
        .as_ref()
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = (|| -> Result<(), String> {
                    let intent_cls = env
                        .find_class("android/content/Intent")
                        .map_err(|e| format!("Find Intent failed: {:?}", e))?;

                    let activity_cls = env
                        .call_method(activity, "getClass", "()Ljava/lang/Class;", &[])
                        .map_err(|e| format!("getClass failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("getClass result is not an object: {:?}", e))?;
                    let class_loader = env
                        .call_method(
                            &activity_cls,
                            "getClassLoader",
                            "()Ljava/lang/ClassLoader;",
                            &[],
                        )
                        .map_err(|e| format!("getClassLoader failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("getClassLoader result is not an object: {:?}", e))?;
                    let service_name = env
                        .new_string("com.vcp.mobile.service.StreamKeepaliveService")
                        .map_err(|e| format!("new_string failed: {:?}", e))?;
                    let service_cls = env
                        .call_method(
                            &class_loader,
                            "loadClass",
                            "(Ljava/lang/String;)Ljava/lang/Class;",
                            &[JValue::Object(&service_name)],
                        )
                        .map_err(|e| format!("loadClass failed: {:?}", e))?
                        .l()
                        .map_err(|e| format!("loadClass result is not an object: {:?}", e))?;

                    let intent = env
                        .new_object(
                            &intent_cls,
                            "(Landroid/content/Context;Ljava/lang/Class;)V",
                            &[JValue::Object(activity), JValue::Object(&service_cls)],
                        )
                        .map_err(|e| format!("New Intent failed: {:?}", e))?;

                    env.call_method(
                        activity,
                        "stopService",
                        "(Landroid/content/Intent;)Z",
                        &[JValue::Object(&intent)],
                    )
                    .map_err(|e| format!("stopService failed: {:?}", e))?;

                    Ok(())
                })();

                if let Err(ref e) = result {
                    log::error!("[VcpMobilePlugin] stop_android_service failed: {}", e);
                }
            });
        })
        .map_err(|e| format!("with_webview failed: {:?}", e))?;

    Ok(())
}
