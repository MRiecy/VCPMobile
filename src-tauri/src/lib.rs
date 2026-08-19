mod distributed;
mod vcp_modules;

const DISTRIBUTED_NETWORK_EVENT: &str = "vcp-mobile://vcp-network-status-changed";

use tauri::{Listener, Manager};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
use vcp_modules::agent_chat_application_service::handle_agent_chat_message;
use vcp_modules::agent_service::{
    create_agent, delete_agent, get_agents, get_assistants_snapshot, read_agent_config,
    save_agent_config, update_agent_config,
};
use vcp_modules::avatar_service::{
    batch_get_avatars, get_avatar, save_avatar_data, store_dominant_color,
};
use vcp_modules::chat_manager::{
    append_single_message, delete_messages, load_chat_history, load_chat_history_streamed,
    patch_single_message, truncate_history_after_timestamp,
};
use vcp_modules::cli::{
    close_vcp_mobile_cli_terminal, commit_vcp_mobile_cli_skill_import,
    discard_vcp_mobile_cli_skill_import, execute_vcp_mobile_cli_action,
    get_vcp_mobile_cli_manifest, get_vcp_mobile_cli_skill_catalog, get_vcp_mobile_cli_status,
    inspect_vcp_mobile_cli_skill_import, open_vcp_mobile_cli_terminal,
    read_vcp_mobile_cli_terminal, resize_vcp_mobile_cli_terminal, write_vcp_mobile_cli_terminal,
    MobileCliRuntimeState,
};
use vcp_modules::context_injection::{
    delete_tarven_rule, get_tarven_rules, preview_tarven_injection, reorder_rules,
    save_tarven_rule, toggle_rule_enabled,
};
use vcp_modules::context_sanitizer::ContextSanitizer;
use vcp_modules::db_manager::search_messages_fts;
use vcp_modules::diary::{
    diary_cancel_search, diary_cancel_semantic_search, diary_create_note,
    diary_delete_empty_folder, diary_delete_notes, diary_get_note, diary_list_folders,
    diary_list_notes, diary_move_notes, diary_rename_note, diary_save_note, diary_search,
    diary_semantic_search, DiaryServiceState,
};
use vcp_modules::emoticon_manager::{
    fix_emoticon_url, get_emoticon_library, regenerate_emoticon_library,
};
use vcp_modules::file_manager::{
    check_attachment_support, get_attachment_real_path, open_file, register_local_file, store_file,
};
use vcp_modules::group_chat_application_service::{
    handle_group_chat_message, invite_group_member_to_speak,
};
use vcp_modules::group_service::{
    create_group, delete_group, get_groups, read_group_config, save_group_config,
    update_group_config,
};
use vcp_modules::high_speed_channel::prepare_vcp_upload;
use vcp_modules::lifecycle_manager::{
    bootstrap, get_core_status, get_last_error, get_system_snapshot,
    reconcile_distributed_node_cmd, restart_or_exit_app, set_app_foreground_state, LifecycleState,
};
use vcp_modules::maintenance_manager::{
    cleanup_orphaned_attachments, cleanup_single_orphaned_attachment, clear_webview_cache,
    init_automatic_maintenance, reconstruct_system_cache,
};
use vcp_modules::message_repository::{process_message_content, rebuild_all_pre_renders};
use vcp_modules::message_service::delete_message_attachment;
use vcp_modules::message_service::{fetch_raw_message_content, re_render_message};
use vcp_modules::model_manager::{
    get_cached_models, get_favorite_models, get_hot_models, record_model_usage, refresh_models,
    start_batch_model_test, stop_all_model_tests, test_model_connectivity, toggle_favorite_model,
};
use vcp_modules::settings_manager::{
    get_settings_recovery_status, read_settings, set_theme, update_settings,
};

use vcp_modules::sync_service::{
    clear_old_sync_logs, get_sync_session_log_path, get_sync_status, list_sync_log_files,
    read_sync_log_file, start_manual_sync, stop_sync,
};
use vcp_modules::topic_service::{
    create_topic, delete_topic, get_topics, get_topics_streamed, get_unread_counts,
    regenerate_topic_response, set_topic_unread, summarize_topic, toggle_topic_lock,
    update_topic_title,
};
use vcp_modules::update_manager::{
    cancel_update_download, check_for_update, get_update_status, install_update,
    start_update_download, UpdateSession,
};
use vcp_modules::vcp_client::{
    get_active_generations, interruptGroupTurn, interruptRequest, recover_active_generation,
    sendToVCP, test_vcp_connection, ActiveRequests, CancelledGroupTurns,
};
use vcp_modules::vcp_info_service::{
    clear_vcp_info, get_vcp_info_connection_status, get_vcp_info_metadata_list,
    get_vcp_info_payload, init_vcp_info_connection,
};
use vcp_modules::vcp_log_service::{
    init_vcp_log_connection, send_vcp_log_message, set_vcp_log_heartbeat,
};
use vcp_modules::logcenter::{logcenter_clear_server, logcenter_fetch};
use vcp_modules::agentmgr::{agentmgr_get_config, agentmgr_list_models, agentmgr_save_config};
use vcp_modules::taskcenter::{
    delegation_cancel, delegation_list, task_agent_list, task_create, task_delete,
    task_get_config, task_get_status, task_set_enabled, task_set_global_enabled, task_trigger,
    task_update,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let context = tauri::generate_context!();

    let app = tauri::Builder::default()
        .setup(|app| {
            // 2. 初始化核心状态
            app.manage(app.handle().clone());
            app.manage(LifecycleState::new());
            app.manage(ActiveRequests::default());
            app.manage(CancelledGroupTurns::default());
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
        .invoke_handler(tauri::generate_handler![
            sendToVCP,
            get_active_generations,
            recover_active_generation,
            get_tarven_rules,
            save_tarven_rule,
            delete_tarven_rule,
            toggle_rule_enabled,
            reorder_rules,
            preview_tarven_injection,
            interruptRequest,
            interruptGroupTurn,
            test_vcp_connection,
            handle_agent_chat_message,
            load_chat_history,
            load_chat_history_streamed,
            append_single_message,
            patch_single_message,
            delete_messages,
            delete_message_attachment,
            search_messages_fts,
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
            get_assistants_snapshot,
            read_agent_config,
            save_agent_config,
            update_agent_config,
            save_avatar_data,
            get_avatar,
            batch_get_avatars,
            store_dominant_color,
            read_settings,
            get_vcp_mobile_cli_manifest,
            get_vcp_mobile_cli_status,
            execute_vcp_mobile_cli_action,
            get_vcp_mobile_cli_skill_catalog,
            inspect_vcp_mobile_cli_skill_import,
            commit_vcp_mobile_cli_skill_import,
            discard_vcp_mobile_cli_skill_import,
            open_vcp_mobile_cli_terminal,
            read_vcp_mobile_cli_terminal,
            write_vcp_mobile_cli_terminal,
            resize_vcp_mobile_cli_terminal,
            close_vcp_mobile_cli_terminal,
            get_settings_recovery_status,
            update_settings,
            diary_list_folders,
            diary_list_notes,
            diary_get_note,
            diary_search,
            diary_cancel_search,
            diary_semantic_search,
            diary_cancel_semantic_search,
            diary_save_note,
            diary_rename_note,
            diary_create_note,
            diary_move_notes,
            diary_delete_notes,
            diary_delete_empty_folder,
            logcenter_fetch,
            logcenter_clear_server,
            task_get_config,
            task_get_status,
            task_trigger,
            task_set_enabled,
            task_set_global_enabled,
            task_create,
            task_update,
            task_delete,
            task_agent_list,
            delegation_list,
            delegation_cancel,
            agentmgr_get_config,
            agentmgr_save_config,
            agentmgr_list_models,
            handle_group_chat_message,
            invite_group_member_to_speak,
            create_agent,
            create_group,
            save_group_config,
            update_group_config,
            delete_group,
            delete_agent,
            set_theme,
            store_file,
            check_attachment_support,
            register_local_file,
            prepare_vcp_upload,
            fetch_raw_message_content,
            re_render_message,
            get_attachment_real_path,
            open_file,
            clear_webview_cache,
            reconstruct_system_cache,
            cleanup_orphaned_attachments,
            cleanup_single_orphaned_attachment,
            get_cached_models,
            refresh_models,
            get_hot_models,
            get_favorite_models,
            toggle_favorite_model,
            record_model_usage,
            test_model_connectivity,
            start_batch_model_test,
            stop_all_model_tests,
            summarize_topic,
            init_vcp_log_connection,
            send_vcp_log_message,
            set_vcp_log_heartbeat,
            set_app_foreground_state,
            init_vcp_info_connection,
            get_vcp_info_connection_status,
            get_vcp_info_metadata_list,
            get_vcp_info_payload,
            clear_vcp_info,
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
            reconcile_distributed_node_cmd,
            distributed::get_distributed_status,
            distributed::get_registered_tools_metadata,
            distributed::get_distributed_tool_config_status,
            distributed::update_enabled_tools,
            distributed::reset_distributed_tools_disabled,
            distributed::execute_distributed_tool,
            distributed::reconnect_distributed_client,
            check_for_update,
            start_update_download,
            cancel_update_download,
            get_update_status,
            install_update,
            tauri_plugin_vcp_mobile::stream::set_keepalive_mode,
            restart_or_exit_app,
        ])
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
        let lib_source = include_str!("lib.rs");
        let distributed_source = include_str!("distributed/mod.rs");
        let production_lib_source = lib_source
            .split("#[cfg(test)]")
            .next()
            .expect("production lib source");

        assert!(production_lib_source.contains("distributed::update_enabled_tools,"));
        assert!(!production_lib_source.contains("distributed::update_disabled_tools,"));
        assert!(distributed_source.contains("enabled_names: Vec<String>"));
        assert!(!distributed_source.contains("disabled_names: Vec<String>"));
    }
}
