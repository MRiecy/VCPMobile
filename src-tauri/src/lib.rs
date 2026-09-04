// 命令注册表模块：以 macro_rules! 形式提供（原因见 commands.rs 顶部文档），
// 宏在下方 invoke_handler 调用点展开，与 mod 声明顺序无关。
mod commands;
mod distributed;
mod vcp_modules;

const DISTRIBUTED_NETWORK_EVENT: &str = "vcp-mobile://vcp-network-status-changed";

use tauri::{Listener, Manager};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
// 命令 handler 的导入全部位于 commands.rs；此处仅保留启动编排所需的状态与函数。
use vcp_modules::cli::MobileCliRuntimeState;
use vcp_modules::context_sanitizer::ContextSanitizer;
use vcp_modules::diary::DiaryServiceState;
use vcp_modules::lifecycle_manager::{bootstrap, LifecycleState};
use vcp_modules::maintenance_manager::init_automatic_maintenance;
use vcp_modules::update_manager::UpdateSession;
use vcp_modules::vcp_client::{ActiveGroupTurns, ActiveRequests};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();

    let app = tauri::Builder::default()
        .setup(|app| {
            // 0. 启动分段计时：t0 近似进程起点，必须最先注册
            let boot_trace = vcp_modules::boot_trace::BootTraceState::new();
            boot_trace.mark("rs:setup_begin");
            app.manage(boot_trace);

            // 2. 初始化核心状态
            app.manage(app.handle().clone());
            app.manage(LifecycleState::new());
            app.manage(ActiveRequests::default());
            app.manage(ActiveGroupTurns::default());
            app.manage(ContextSanitizer::default());
            app.manage(distributed::DistributedState::new());

            // 提前注册纯内存状态，防范前端在 bootstrap 完成前调用 command 导致 state() panic
            app.manage(vcp_modules::agent_service::AgentConfigState::new());
            app.manage(vcp_modules::group_service::GroupManagerState::new());
            app.manage(vcp_modules::settings_manager::SettingsState::new());
            app.manage(
                DiaryServiceState::new()
                    .map_err(|error| std::io::Error::other(error.command_string()))?,
            );
            app.manage(vcp_modules::model_manager::ModelManagerState::new());
            app.manage(vcp_modules::emoticon_manager::EmoticonManagerState::default());
            app.manage(MobileCliRuntimeState::new());
            app.manage(UpdateSession::new());

            let handle = app.handle().clone();

            // 前端始终使用 APK 内嵌资源；旧 OTA 目录的 best-effort 清理不阻塞冷启动。
            let legacy_cleanup_handle = handle.clone();
            tauri::async_runtime::spawn_blocking(move || {
                vcp_modules::update_manager::cleanup_legacy_frontend_ota(&legacy_cleanup_handle);
            });

            // 1. 清理上传缓存
            vcp_modules::file_manager::clear_upload_cache(&handle);
            vcp_modules::boot_trace::boot_mark(&handle, "rs:setup_states_ready");

            // 2. 异步引导核心服务与系统维护
            tauri::async_runtime::spawn(async move {
                if let Err(e) = bootstrap(&handle).await {
                    log::error!("[VCPCore] Bootstrap failed: {}", e);
                } else {
                    // 在核心引导成功后，安全地执行自动系统维护 (此时 DbState 保证已由 handle.manage 托管)
                    let h_maintenance = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        // 给予 30 秒冷启动后台静默期，避免抢占前台核心渲染周期的 CPU 与闪存 IO
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        init_automatic_maintenance(h_maintenance).await;
                    });
                }
            });

            // 3. 监听安卓原生网络状态变更，实现分布式连接的自主重连
            let handle_net = app.handle().clone();
            app.listen_any(DISTRIBUTED_NETWORK_EVENT, move |event| {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                    if payload.get("connected").and_then(|v| v.as_bool()).unwrap_or(false) {
                        let h = handle_net.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Some(state) = h.try_state::<distributed::DistributedState>() {
                                log::info!("[Distributed] Network restored! Triggering immediate reconnect in Rust backend.");
                                let client = state.client.read().await;
                                client.trigger_reconnect().await;
                            }
                        });
                    }
                }
            });



            // 5. 监听由 Kotlin LifecycleBridge 发回的原生进程级生命周期事件
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                let handle_lifecycle = app.handle().clone();
                app.listen_any("vcp-mobile://lifecycle", move |event| {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                        if let Some(state) = payload.get("state").and_then(|v| v.as_str()) {
                            let handle = handle_lifecycle.clone();
                            if state == "pause" || state == "stop" {
                                log::info!("[Lifecycle] App entered background (state={})", state);
                                let epoch = vcp_modules::lifecycle_manager::reserve_lifecycle_transition();
                                tauri::async_runtime::spawn(async move {
                                    vcp_modules::lifecycle_manager::set_app_foreground_state_for_epoch(handle, false, epoch).await;
                                });
                            } else if state == "resume" {
                                log::info!("[Lifecycle] App entered foreground (state={})", state);
                                let epoch = vcp_modules::lifecycle_manager::reserve_lifecycle_transition();
                                tauri::async_runtime::spawn(async move {
                                    vcp_modules::lifecycle_manager::set_app_foreground_state_for_epoch(handle, true, epoch).await;
                                });
                            }
                        }
                    }
                });
            }

            vcp_modules::boot_trace::boot_mark(&app.handle(), "rs:setup_end");
            Ok(())
        })
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets({
                    let targets = vec![
                        Target::new(TargetKind::Stdout),
                        Target::new(TargetKind::LogDir { file_name: None }),
                        #[cfg(any(debug_assertions, not(mobile)))]
                        Target::new(TargetKind::Webview),
                    ];
                    targets
                })
                .level(if cfg!(debug_assertions) {
                    log::LevelFilter::Debug
                } else {
                    log::LevelFilter::Warn
                })
                // 只保留最近一份日志，超过 5 MiB 触发轮转，避免 LogDir 无限增长
                .rotation_strategy(RotationStrategy::KeepOne)
                .max_file_size(5 * 1024 * 1024)
                .filter(|metadata| {
                    let target = metadata.target();
                    // 屏蔽高频 UI 交互、系统窗口以及 Android 系统底层冗余日志
                    !target.contains("pointer")
                        && !target.contains("touch")
                        && !target.contains("gesture")
                        && !target.contains("wry::event_loop")
                        && !target.contains("tao::window")
                        && !target.contains("wry::webview")
                        && !target.contains("DynamicFramerate")
                        && !target.contains("PowerHalMgrImpl")
                        && !target.contains("AnimationSpeedAware")
                        && !target.contains("InputEventInfo")
                })
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_vcp_mobile::init())
        // 命令注册表与「IPC 防爆栈总闸」均已收口至 commands.rs（含完整机制注释）；
        // 此处仅挂载。新增命令请直接编辑 commands.rs 的注册清单。
        .invoke_handler(commands::app_command_handler!())
        .build(context)
        .expect("error while building tauri application");

    app.run(|_app_handle, event| match event {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::Focused(focused),
            ..
        } => {
            log::info!(
                "[Lifecycle] Native WindowEvent::Focused: focused={}",
                focused
            );
            let handle = _app_handle.clone();
            let epoch = vcp_modules::lifecycle_manager::reserve_lifecycle_transition();
            tauri::async_runtime::spawn(async move {
                vcp_modules::lifecycle_manager::set_app_foreground_state_for_epoch(
                    handle, focused, epoch,
                )
                .await;
            });
        }
        _ => {}
    });
}

#[cfg(test)]
mod contract_tests {
    use super::DISTRIBUTED_NETWORK_EVENT;

    #[test]
    fn distributed_network_listener_matches_android_plugin_namespace() {
        let kotlin_plugin = include_str!(
            "../plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt"
        );
        assert!(kotlin_plugin.contains("trigger(\"vcp-network-status-changed\""));
        assert_eq!(
            DISTRIBUTED_NETWORK_EVENT,
            "vcp-mobile://vcp-network-status-changed"
        );
    }

    #[test]
    fn distributed_tool_authorization_command_uses_enabled_allowlist() {
        // 命令注册表位于 commands.rs（lib.rs 仅保留启动编排）。
        let commands_source = include_str!("commands.rs");
        let distributed_source = include_str!("distributed/mod.rs");

        assert!(commands_source.contains("distributed::update_enabled_tools,"));
        assert!(!commands_source.contains("distributed::update_disabled_tools,"));
        assert!(distributed_source.contains("enabled_names: Vec<String>"));
        assert!(!distributed_source.contains("disabled_names: Vec<String>"));
    }
}
