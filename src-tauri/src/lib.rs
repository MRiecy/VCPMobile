mod vcp_modules;

use tauri::{Emitter, Manager};
use tauri_plugin_log::{Target, TargetKind};
use vcp_modules::agent_config_manager::{
    get_agents, read_agent_config, rebuild_db_index, save_agent_config, update_agent_config,
    write_agent_config, AgentConfigState,
};
use vcp_modules::app_settings_manager::{
    read_app_settings, update_app_settings, write_app_settings, AppSettingsState,
};
use vcp_modules::avatar_color_extractor::extract_avatar_color;
use vcp_modules::chat_manager::{
    get_topic_delta, load_chat_history, process_regex_for_message, save_chat_history,
};
use vcp_modules::context_sanitizer::ContextSanitizer;
use vcp_modules::db_manager::{init_db, DbState};
use vcp_modules::emoticon_manager::{
    fix_emoticon_url, get_emoticon_library, internal_generate_library, regenerate_emoticon_library,
    EmoticonManagerState,
};
use vcp_modules::file_manager::{
    cleanup_orphaned_attachments, get_attachment_real_path, open_file, pick_and_store_attachment,
    read_local_file_base64, store_file,
};
use vcp_modules::file_watcher::{init_watcher, signal_internal_save, WatcherState};
use vcp_modules::group_manager::{
    create_group, get_groups, load_all_groups, read_group_config, GroupManagerState,
};
use vcp_modules::index_service::full_scan;
use vcp_modules::lifecycle_manager::{bootstrap, get_core_status, get_last_error, LifecycleState};
use vcp_modules::ipc::agent_handlers::{create_agent, delete_agent, save_agent_avatar};
use vcp_modules::ipc::group_handlers::handle_group_chat_message;
use vcp_modules::ipc::settings_handlers::{
    notify_app_state, notify_network_state, save_avatar_color, save_user_avatar, set_theme,
};
use vcp_modules::ipc::sync_handlers::{
    sync_download_file, sync_fetch_manifest, sync_get_local_manifest, sync_ping,
};
use vcp_modules::message_processor::process_message_content;
use vcp_modules::model_manager::{
    get_cached_models, get_favorite_models, get_hot_models, init_model_manager, record_model_usage,
    refresh_models, toggle_favorite_model, ModelManagerState,
};
use vcp_modules::topic_list_manager::{
    create_topic, delete_topic, get_topics, set_topic_unread, summarize_topic, toggle_topic_lock,
    update_topic_title,
};
use vcp_modules::vcp_client::{interruptRequest, sendToVCP, test_vcp_connection, ActiveRequests};
use vcp_modules::vcp_log_service::{init_vcp_log_connection, send_vcp_log_message};

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 初始化生命周期状态
            app.manage(LifecycleState::new());

            let handle = app.handle().clone();
            // 异步启动核心服务
            tauri::async_runtime::spawn(async move {
                if let Err(e) = bootstrap(&handle).await {
                    eprintln!("[VCPCore] Bootstrap failed: {}", e);
                }
            });

            Ok(())
        })
        .manage(ActiveRequests::default())
        .manage(ContextSanitizer::default())
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                    Target::new(TargetKind::Webview),
                ])
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            sendToVCP,
            interruptRequest,
            test_vcp_connection,
            load_chat_history,
            save_chat_history,
            process_regex_for_message,
            process_message_content,
            get_topics,
            get_groups,
            read_group_config,
            create_topic,
            delete_topic,
            update_topic_title,
            toggle_topic_lock,
            set_topic_unread,
            get_agents,
            read_agent_config,
            write_agent_config,
            save_agent_config,
            update_agent_config,
            rebuild_db_index,
            read_app_settings,
            write_app_settings,
            update_app_settings,
            save_avatar_color,
            save_user_avatar,
            save_agent_avatar,
            handle_group_chat_message,
            create_agent,
            create_group,
            delete_agent,
            set_theme,
            notify_app_state,
            notify_network_state,
            signal_internal_save,
            store_file,
            pick_and_store_attachment,
            read_local_file_base64,
            get_attachment_real_path,
            open_file,
            cleanup_orphaned_attachments,
            get_topic_delta,
            sync_download_file,
            sync_ping,
            sync_fetch_manifest,
            sync_get_local_manifest,
            extract_avatar_color,
            get_cached_models,
            refresh_models,
            get_hot_models,
            get_favorite_models,
            toggle_favorite_model,
            record_model_usage,
            summarize_topic,
            init_vcp_log_connection,
            send_vcp_log_message,
            get_emoticon_library,
            regenerate_emoticon_library,
            fix_emoticon_url,
            get_core_status,
            get_last_error
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
