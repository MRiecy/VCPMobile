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
use vcp_modules::file_manager::{pick_and_store_attachment, read_local_image_base64, store_file};
use vcp_modules::file_watcher::{init_watcher, signal_internal_save, WatcherState};
use vcp_modules::group_manager::{
    create_group, get_groups, load_all_groups, read_group_config, GroupManagerState,
};
use vcp_modules::index_service::full_scan;
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
            let handle = app.handle().clone();

            // --- 关键修复：完全异步化初始化 (Async Boot) ---
            // 这样即使 DB 卡住，WebView 也能正常显示
            tauri::async_runtime::spawn(async move {
                println!("[VCPCore] Starting asynchronous initialization...");

                match init_db(&handle).await {
                    Ok(pool) => {
                        println!("[VCPCore] Database initialized successfully.");
                        handle.manage(DbState { pool: pool.clone() });

                        // 初始化其他状态
                        handle.manage(AgentConfigState::new());
                        handle.manage(GroupManagerState::new());
                        handle.manage(AppSettingsState::new());
                        handle.manage(WatcherState::default());

                        // 初始化模型管理器
                        let model_manager_state = ModelManagerState::new();
                        init_model_manager(&handle, &model_manager_state).await;
                        handle.manage(model_manager_state);

                        // 加载群组配置
                        let group_state = handle.state::<GroupManagerState>();
                        if let Err(e) = load_all_groups(&handle, &group_state, &pool).await {
                            eprintln!("[VCPCore] Group load failed: {}", e);
                        }

                        // 启动文件监控 (如果失败仅打印，不影响主流程)
                        if let Err(e) = init_watcher(handle.clone()) {
                            eprintln!("[VCPCore] Watcher init failed: {}", e);
                        }

                        // 运行初始全量扫描
                        if let Err(e) = full_scan(&handle, &pool).await {
                            eprintln!("[VCPCore] Initial scan failed: {}", e);
                        }

                        // 通知前端核心已就绪
                        let _ = handle.emit("vcp-core-ready", ());
                    }
                    Err(e) => {
                        eprintln!("[VCPCore] FATAL: Database initialization failed: {}", e);
                        // 发送错误通知，让前端显示“数据库错误”而不是白屏
                        let _ = handle.emit("vcp-core-error", format!("数据库初始化失败: {}", e));
                    }
                }
            });

            Ok(())
        })
        .manage(ActiveRequests::default())
        .manage(ContextSanitizer::default())
        .plugin(tauri_plugin_log::Builder::new()
            .targets([
                Target::new(TargetKind::Stdout),
                Target::new(TargetKind::LogDir { file_name: None }),
                Target::new(TargetKind::Webview),
            ])
            .level(log::LevelFilter::Info)
            .filter(|metadata| {
                let target = metadata.target();
                // 屏蔽高频 UI 交互、系统窗口以及 Android 系统底层冗余日志
                !target.contains("pointer") && 
                !target.contains("touch") && 
                !target.contains("gesture") && 
                !target.contains("wry::event_loop") &&
                !target.contains("tao::window") &&
                !target.contains("wry::webview") &&
                !target.contains("DynamicFramerate") &&
                !target.contains("PowerHalMgrImpl") &&
                !target.contains("AnimationSpeedAware") &&
                !target.contains("InputEventInfo")
            })
            .build())
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
            read_local_image_base64,
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
            send_vcp_log_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
