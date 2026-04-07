mod vcp_modules;

use tauri::Manager;
use vcp_modules::agent_service::{
    create_agent, delete_agent, get_agents, read_agent_config,
    save_agent_config, update_agent_config, save_avatar_color,
};
use vcp_modules::app_settings_manager::{
    notify_app_state, notify_network_state, read_app_settings,
    set_theme, update_app_settings, write_app_settings,
};
use vcp_modules::chat_manager::{
    append_single_message, delete_messages, get_topic_delta, get_topic_fingerprint,
    load_chat_history, patch_single_message, process_regex_for_message,
    truncate_history_after_timestamp,
};
use vcp_modules::context_sanitizer::ContextSanitizer;
// use vcp_modules::db_manager::DbState;
use tauri_plugin_log::{Target, TargetKind};
use vcp_modules::emoticon_manager::{
    fix_emoticon_url, get_emoticon_library, regenerate_emoticon_library,
};
use vcp_modules::file_manager::{
    cleanup_orphaned_attachments, get_attachment_real_path, open_file, pick_and_store_attachment,
    read_local_file_base64, store_file,
};
use vcp_modules::group_chat_application_service::handle_group_chat_message;
use vcp_modules::group_service::{
    create_group, get_groups, read_group_config, save_group_config, update_group_config,
};
use vcp_modules::lifecycle_manager::{bootstrap, get_core_status, get_last_error, LifecycleState};
use vcp_modules::message_processor::process_message_content;
use vcp_modules::message_stream_protocol::handle_vcp_request;
use vcp_modules::model_manager::{
    get_favorite_models, get_hot_models, record_model_usage, refresh_models,
    toggle_favorite_model,
};
use vcp_modules::topic_service::{
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
        .register_uri_scheme_protocol("vcp", move |ctx, request| {
            // 暂时占位，或者重构 `register_vcp_protocol` 以返回一个可以调用的函数/闭包
            handle_vcp_request(ctx, request)
        })
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
            append_single_message,
            patch_single_message,
            delete_messages,
            truncate_history_after_timestamp,
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
            save_agent_config,
            update_agent_config,
            save_avatar_color,
            read_app_settings,
            write_app_settings,
            update_app_settings,
            handle_group_chat_message,
            create_agent,
            create_group,
            save_group_config,
            update_group_config,
            delete_agent,
            set_theme,
            notify_app_state,
            notify_network_state,
            store_file,
            pick_and_store_attachment,
            read_local_file_base64,
            get_attachment_real_path,
            open_file,
            cleanup_orphaned_attachments,
            get_topic_delta,
            get_topic_fingerprint,
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
            get_last_error,
            ])
            .register_asynchronous_uri_scheme_protocol("vcp-avatar", |context, request, responder| {
            let app_handle = context.app_handle().clone();
            let uri = request.uri().to_string();
            // uri format: vcp-avatar://agent/{id} or vcp-avatar://user/default

            tauri::async_runtime::spawn(async move {
                let db_state = app_handle.state::<vcp_modules::db_manager::DbState>();
                let pool = &db_state.pool;

                let path = uri.strip_prefix("vcp-avatar://").unwrap_or("");
                let parts: Vec<&str> = path.split('/').collect();

                if parts.len() >= 2 {
                    let owner_type = parts[0];
                    let owner_id = parts[1];

                    let row_res: Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error> = sqlx::query("SELECT mime_type, image_data, dominant_color FROM avatars WHERE owner_type = ? AND owner_id = ?")
                        .bind(owner_type)
                        .bind(owner_id)
                        .fetch_optional(pool)
                        .await;

                    match row_res {
                        Ok(Some(row)) => {
                            use sqlx::Row;
                            let mime: String = row.get("mime_type");
                            let data: Vec<u8> = row.get("image_data");
                            let color: Option<String> = row.get("dominant_color");
                            
                            let mut builder = tauri::http::Response::builder()
                                .header("Content-Type", mime)
                                .header("Access-Control-Allow-Origin", "*")
                                .header("Cache-Control", "max-age=3600");
                                
                            if let Some(c) = color {
                                builder = builder.header("X-Avatar-Color", c);
                            }
                            
                            let response = builder.body(data).unwrap();
                            responder.respond(response);
                            return;
                        }
                        _ => {}
                    }
                }

                // Fallback: 404
                let response = tauri::http::Response::builder()
                    .status(404)
                    .body(Vec::new())
                    .unwrap();
                responder.respond(response);
            });
            })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
