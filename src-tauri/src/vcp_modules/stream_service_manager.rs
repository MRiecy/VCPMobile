use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, Manager};

/// 流式服务状态：追踪当前有多少个活跃 SSE 流
///
/// 通过 AtomicU32 计数器实现无锁计数，多个并发流式请求共享同一状态。
pub struct StreamingServiceState {
    pub active_count: AtomicU32,
}

impl Default for StreamingServiceState {
    fn default() -> Self {
        Self {
            active_count: AtomicU32::new(0),
        }
    }
}

/// 启动流式前台服务
///
/// 每有一个新的 SSE 流开始时调用。计数器从 0→1 时真正启动 Android 前台服务；
/// 后续递增仅更新计数，避免通知闪烁。
pub fn start_streaming_service(app: &AppHandle, agent_name: &str) -> Result<(), String> {
    let state = app.state::<StreamingServiceState>();
    let count = state.active_count.fetch_add(1, Ordering::SeqCst);

    if count == 0 {
        #[cfg(target_os = "android")]
        if let Err(e) = start_android_service(app, agent_name) {
            state.active_count.fetch_sub(1, Ordering::SeqCst);
            return Err(e);
        }
    }

    log::info!(
        "[StreamingService] Started for '{}'. Active count: {}",
        agent_name,
        count + 1
    );

    Ok(())
}

/// 停止流式前台服务
///
/// 每有一个 SSE 流结束时调用。计数器减到 0 时真正停止 Android 前台服务。
pub fn stop_streaming_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<StreamingServiceState>();
    let count = state.active_count.fetch_sub(1, Ordering::SeqCst);

    if count <= 1 {
        state.active_count.store(0, Ordering::SeqCst);
        #[cfg(target_os = "android")]
        stop_android_service(app)?;
    }

    log::info!(
        "[StreamingService] Stopped. Active count: {}",
        state.active_count.load(Ordering::SeqCst)
    );

    Ok(())
}

// =============================================================================
// Android JNI 实现
// =============================================================================

#[cfg(target_os = "android")]
fn start_android_service(app: &AppHandle, agent_name: &str) -> Result<(), String> {
    use jni::objects::JValue;

    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let agent_name = agent_name.to_string();

    let (tx, rx) = std::sync::mpsc::channel();
    window
        .as_ref()
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = (|| -> Result<(), String> {
                    let intent_cls = env
                        .find_class("android/content/Intent")
                        .map_err(|e| format!("Find Intent failed: {:?}", e))?;

                    // 通过 Activity 的 ClassLoader 加载应用类，绕过 JNI FindClass 的 boot class loader 限制
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
                        .new_string("com.vcp.avatar.service.StreamKeepaliveService")
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
                            &[
                                JValue::Object(activity),
                                JValue::Object(&service_cls),
                            ],
                        )
                        .map_err(|e| format!("New Intent failed: {:?}", e))?;

                    // putExtra("agent_name", agentName)
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

                    // 判断 API 版本：26+ 使用 startForegroundService
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
                let _ = tx.send(result);
            });
        })
        .map_err(|e| format!("with_webview failed: {:?}", e))?;

    rx.recv()
        .map_err(|_| "JNI execution channel closed".to_string())?
}

#[cfg(target_os = "android")]
fn stop_android_service(app: &AppHandle) -> Result<(), String> {
    use jni::objects::JValue;

    let window = app
        .get_webview_window("main")
        .ok_or("main window not found")?;

    let (tx, rx) = std::sync::mpsc::channel();
    window
        .as_ref()
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = (|| -> Result<(), String> {
                    let intent_cls = env
                        .find_class("android/content/Intent")
                        .map_err(|e| format!("Find Intent failed: {:?}", e))?;

                    // 通过 Activity 的 ClassLoader 加载应用类，绕过 JNI FindClass 的 boot class loader 限制
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
                        .new_string("com.vcp.avatar.service.StreamKeepaliveService")
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
                            &[
                                JValue::Object(activity),
                                JValue::Object(&service_cls),
                            ],
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
                let _ = tx.send(result);
            });
        })
        .map_err(|e| format!("with_webview failed: {:?}", e))?;

    rx.recv()
        .map_err(|_| "JNI execution channel closed".to_string())?
}
