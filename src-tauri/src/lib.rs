// 命令注册表模块：以 macro_rules! 形式提供（原因见 commands.rs 顶部文档），
// 宏在下方 invoke_handler 调用点展开，与 mod 声明顺序无关。
mod commands;
mod distributed;
mod vcp_modules;

use tauri::Manager;
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

    let builder = tauri::Builder::default()
        .setup(|app| {
            // 0. 启动分段计时：t0 近似进程起点，必须最先注册
            let boot_trace = vcp_modules::boot_trace::BootTraceState::new();
            boot_trace.mark("rs:setup_begin");
            app.manage(boot_trace);

            // 2. 初始化核心状态
            app.manage(app.handle().clone());
            app.manage(LifecycleState::new());
            app.manage(ActiveRequests::default());
            app.manage(ActiveGroupTurns::new(&app.state::<LifecycleState>()));
            app.manage(ContextSanitizer::default());
            app.manage(vcp_modules::chat::voice_service::VoiceServiceState::default());
            app.manage(vcp_modules::chat::voice_service::VoiceCallState::default());
            app.manage(vcp_modules::chat::voice_service::VoiceDocumentState::default());
            app.manage(vcp_modules::chat::voice_service::TtsServiceState::default());
            app.manage(vcp_modules::infra::share_inbox::ShareInboxState::default());
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

            // Android 前端使用 APK 内嵌资源；旧 OTA 清理仅属于 Android 更新链。
            #[cfg(target_os = "android")]
            {
                let legacy_cleanup_handle = handle.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    vcp_modules::update_manager::cleanup_legacy_frontend_ota(
                        &legacy_cleanup_handle,
                    );
                });
            }

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

            vcp_modules::boot_trace::boot_mark(app.handle(), "rs:setup_end");
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
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_vcp_mobile::init())
        // 命令注册表与「IPC 防爆栈总闸」均已收口至 commands.rs（含完整机制注释）；
        // 此处仅挂载。新增命令请直接编辑 commands.rs 的注册清单。
        .invoke_handler(commands::app_command_handler!())
        .on_page_load(|_webview, _payload| {
            #[cfg(any(target_os = "android", target_os = "ios"))]
            if matches!(_payload.event(), tauri::webview::PageLoadEvent::Started) {
                vcp_modules::chat::voice_service::invalidate_document(_webview.app_handle(), None);
            }
        });

    #[cfg(target_os = "ios")]
    let builder = builder.on_web_content_process_terminate(|webview| {
        vcp_modules::chat::voice_service::invalidate_document(webview.app_handle(), None);
        // A custom termination hook replaces Wry's default reload handler.
        if let Err(error) = webview.reload() {
            log::error!("[VoiceDocument] WebContent reload failed: {error}");
        }
    });

    let app = builder
        .build(context)
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        #[cfg(target_os = "ios")]
        if let Err(error) = vcp_modules::infra::ios_window::handle_run_event(_app_handle, &event) {
            log::error!("[iOSWindow] {}", error);
        }

        match event {
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
        }
    });
}
