mod distributed;
mod vcp_modules;

use tauri::Manager;
use vcp_modules::agent_chat_application_service::handle_agent_chat_message;
use vcp_modules::agent_service::{
    create_agent, delete_agent, get_agents, read_agent_config, save_agent_config,
    update_agent_config,
};
use vcp_modules::avatar_service::{get_avatar, save_avatar_data, store_dominant_color};
use vcp_modules::chat_manager::{
    append_single_message, delete_messages, load_chat_history, load_chat_history_streamed,
    patch_single_message, truncate_history_after_timestamp,
};
use vcp_modules::context_sanitizer::ContextSanitizer;
use vcp_modules::settings_manager::{read_settings, set_theme, update_settings, write_settings};
// use vcp_modules::db_manager::DbState;
use tauri_plugin_log::{Target, TargetKind};
use vcp_modules::emoticon_manager::{
    fix_emoticon_url, get_emoticon_library, regenerate_emoticon_library,
};
use vcp_modules::file_manager::{
    get_attachment_real_path, open_file, register_local_file, store_file,
};
use vcp_modules::frontend_update_manager::{
    apply_frontend_update, check_for_frontend_update, clear_frontend_updates,
    confirm_frontend_boot, download_frontend_update, get_active_frontend_version,
};
use vcp_modules::group_chat_application_service::handle_group_chat_message;
use vcp_modules::group_service::{
    create_group, delete_group, get_groups, read_group_config, save_group_config,
    update_group_config,
};
use vcp_modules::lifecycle_manager::{
    bootstrap, get_core_status, get_last_error, get_system_snapshot, LifecycleState,
};
use vcp_modules::maintenance_manager::{
    cleanup_orphaned_attachments, cleanup_single_orphaned_attachment, clear_webview_cache,
    init_automatic_maintenance,
};
use vcp_modules::message_repository::{
    process_message_content, rebuild_all_pre_renders,
};
use vcp_modules::message_service::{fetch_raw_message_content, re_render_message};
use vcp_modules::model_manager::{
    get_cached_models, get_favorite_models, get_hot_models, record_model_usage, refresh_models,
    toggle_favorite_model,
};
use vcp_modules::high_speed_channel::prepare_vcp_upload;

use vcp_modules::sync_service::{
    clear_old_sync_logs, get_sync_session_log_path, get_sync_status, list_sync_log_files,
    read_sync_log_file, start_manual_sync, stop_sync,
};
use vcp_modules::topic_service::{
    create_topic, delete_topic, get_topics, get_topics_streamed, get_unread_counts,
    regenerate_topic_response, set_topic_unread, summarize_topic, toggle_topic_lock,
    update_topic_title,
};
use vcp_modules::update_manager::{check_for_update, download_update, install_update};
use vcp_modules::vcp_client::{
    interruptGroupTurn, interruptRequest, sendToVCP, test_vcp_connection, ActiveRequests,
    CancelledGroupTurns,
};
use vcp_modules::vcp_log_service::{
    init_vcp_log_connection, send_vcp_log_message, set_vcp_log_heartbeat,
};



#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();

    // 注入 OtaAssets：优先从文件系统读取前端热更新资源
    {
        let identifier = context.config().identifier.clone();
        #[cfg(target_os = "android")]
        let active_version_path = format!(
            "/data/data/{}/files/frontend_updates/active_version",
            identifier
        );
        #[cfg(not(target_os = "android"))]
        let active_version_path = String::new();

        let update_dir = if cfg!(target_os = "android") {
            if let Ok(version) = std::fs::read_to_string(&active_version_path) {
                let v = version.trim();
                if v.is_empty() {
                    std::path::PathBuf::new()
                } else {
                    std::path::PathBuf::from(format!(
                        "/data/data/{}/files/frontend_updates/{}",
                        identifier, v
                    ))
                }
            } else {
                std::path::PathBuf::new()
            }
        } else {
            std::path::PathBuf::new()
        };

        let embedded = context.set_assets(Box::new(vcp_modules::ota_assets::EmptyAssets));
        let ota_assets = vcp_modules::ota_assets::OtaAssets::new(embedded, update_dir);
        context.set_assets(Box::new(ota_assets));
    }

    let builder = tauri::Builder::default();

    builder
        .setup(|app| {
            // 2. 初始化核心状态
            app.manage(LifecycleState::new());
            app.manage(ActiveRequests::default());
            app.manage(CancelledGroupTurns::default());
            app.manage(ContextSanitizer::default());
            app.manage(distributed::DistributedState::new());

            // 提前注册纯内存状态，防范前端在 bootstrap 完成前调用 command 导致 state() panic
            app.manage(vcp_modules::agent_service::AgentConfigState::new());
            app.manage(vcp_modules::group_service::GroupManagerState::new());
            app.manage(vcp_modules::settings_manager::SettingsState::new());
            app.manage(vcp_modules::model_manager::ModelManagerState::new());
            app.manage(vcp_modules::emoticon_manager::EmoticonManagerState::default());

            let handle = app.handle().clone();

            // 0. 前端 OTA：APK 升级清理 & 损坏版本回滚 & 安全期冗余垃圾清理
            vcp_modules::frontend_update_manager::clear_on_apk_upgrade(&handle);
            vcp_modules::frontend_update_manager::rollback_if_needed(&handle);
            vcp_modules::frontend_update_manager::safe_cleanup_old_versions(&handle);

            // 1. 清理上传缓存
            vcp_modules::file_manager::clear_upload_cache(&handle);

            // 2. 异步引导核心服务与系统维护
            tauri::async_runtime::spawn(async move {
                if let Err(e) = bootstrap(&handle).await {
                    eprintln!("[VCPCore] Bootstrap failed: {}", e);
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

            Ok(())
        })
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets({
                    let mut targets = vec![
                        Target::new(TargetKind::Stdout),
                        Target::new(TargetKind::LogDir { file_name: None }),
                    ];
                    #[cfg(any(debug_assertions, not(mobile)))]
                    targets.push(Target::new(TargetKind::Webview));
                    targets
                })
                .level(log::LevelFilter::Info)
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
        .invoke_handler(tauri::generate_handler![
            sendToVCP,
            interruptRequest,
            interruptGroupTurn,
            test_vcp_connection,
            handle_agent_chat_message,
            load_chat_history,
            load_chat_history_streamed,
            append_single_message,
            patch_single_message,
            delete_messages,
            truncate_history_after_timestamp,
            process_message_content,
            rebuild_all_pre_renders,
            get_topics,
            get_topics_streamed,
            get_unread_counts,
            get_groups,
            read_group_config,
            create_topic,
            delete_topic,
            update_topic_title,
            toggle_topic_lock,
            set_topic_unread,
            regenerate_topic_response,
            get_agents,
            read_agent_config,
            save_agent_config,
            update_agent_config,
            save_avatar_data,
            get_avatar,
            store_dominant_color,
            read_settings,
            write_settings,
            update_settings,
            handle_group_chat_message,
            create_agent,
            create_group,
            save_group_config,
            update_group_config,
            delete_group,
            delete_agent,
            set_theme,
            store_file,
            register_local_file,
            prepare_vcp_upload,
            fetch_raw_message_content,
            re_render_message,
            get_attachment_real_path,
            open_file,
            clear_webview_cache,
            cleanup_orphaned_attachments,
            cleanup_single_orphaned_attachment,
            get_cached_models,
            refresh_models,
            get_hot_models,
            get_favorite_models,
            toggle_favorite_model,
            record_model_usage,
            summarize_topic,
            init_vcp_log_connection,
            send_vcp_log_message,
            set_vcp_log_heartbeat,
            get_system_snapshot,
            get_emoticon_library,
            regenerate_emoticon_library,
            fix_emoticon_url,
            get_core_status,
            get_last_error,
            get_sync_status,
            start_manual_sync,
            stop_sync,
            get_sync_session_log_path,
            list_sync_log_files,
            read_sync_log_file,
            clear_old_sync_logs,
            distributed::start_distributed_node,
            distributed::stop_distributed_node,
            distributed::get_distributed_status,
            distributed::update_sensor_data,
            check_for_update,
            download_update,
            install_update,
            check_for_frontend_update,
            download_frontend_update,
            apply_frontend_update,
            get_active_frontend_version,
            clear_frontend_updates,
            confirm_frontend_boot,
        ])
        .run(context)
        .expect("error while running tauri application");
}
