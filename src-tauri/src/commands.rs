//! IPC 命令注册表 + 防爆栈总闸。
//!
//! 本模块是 app 层全部 Tauri 命令的**唯一注册点**（`lib.rs` 只负责启动编排，
//! 不再持有命令清单）。命令清单按领域分组、组内字母序排列；新增命令的步骤：
//! 在宏内对应领域的 `use` 块导入 handler，然后把它加进下方对应分组的列表中
//! 即可，无需评估 future 尺寸（见宏内防爆栈注释）。
//!
//! 为什么注册表是 `macro_rules!` 而不是普通函数（本次收口的实证结论）：
//! `generate_handler!` 展开时会以「与命令函数相同的路径」引用 `#[tauri::command]`
//! 生成的伴生宏（`__tauri_command_name_*` / `__cmd__*`）。这些伴生宏经
//! `#[macro_export]` 锚定在 crate root：在 crate root 可无限定解析，在普通
//! 子模块中无限定解析直接失败（实测 330 个 `cannot find macro`）；而按模块
//! 路径解析又要求实现模块公开——diary / forum / logcenter / mail / taskcenter /
//! agentmgr 六个领域把实现放在私有子模块里，伴生宏没有可达路径。因此清单
//! 必须在 crate root 的上下文中展开。`macro_rules!` 的路径在**调用点**解析，
//! 本宏的全部 token 在 `lib.rs` 的 `.invoke_handler(commands::app_command_handler!())`
//! 处展开，解析行为与过去直接写在 lib.rs 完全一致，且无需改动任何领域模块。

/// 构建 app 命令的统一 `InvokeHandler`（含防爆栈 offload），在 crate root 展开。
macro_rules! app_command_handler {
    () => {{
        // ── 命令 handler 导入（块作用域，不污染 lib.rs 命名空间）──
        use crate::distributed;
        use crate::vcp_modules::agent_chat_application_service::handle_agent_chat_message;
        use crate::vcp_modules::agent_service::{
            create_agent, delete_agent, get_agents, get_assistants_snapshot, read_agent_config,
            save_agent_config, update_agent_config,
        };
        use crate::vcp_modules::agentmgr::{
            agentmgr_get_config, agentmgr_list_models, agentmgr_save_config,
        };
        use crate::vcp_modules::avatar_service::{
            batch_get_avatars, get_avatar, save_avatar_data, store_dominant_color,
        };
        use crate::vcp_modules::chat_manager::{
            append_single_message, delete_messages, load_chat_history, load_chat_history_around,
            load_chat_history_streamed, patch_single_message, truncate_history_after_timestamp,
        };
        use crate::vcp_modules::cli::{
            close_vcp_mobile_cli_terminal, commit_vcp_mobile_cli_skill_import,
            discard_vcp_mobile_cli_skill_import, execute_vcp_mobile_cli_action,
            get_vcp_mobile_cli_manifest, get_vcp_mobile_cli_skill_catalog,
            get_vcp_mobile_cli_status, inspect_vcp_mobile_cli_skill_import,
            open_vcp_mobile_cli_terminal, read_vcp_mobile_cli_terminal,
            resize_vcp_mobile_cli_terminal, write_vcp_mobile_cli_terminal,
        };
        use crate::vcp_modules::context_injection::{
            delete_tarven_rule, get_tarven_rules, preview_tarven_injection, reorder_rules,
            save_tarven_rule, toggle_rule_enabled,
        };
        use crate::vcp_modules::db_manager::{
            get_fts_index_status, rebuild_messages_fts, search_messages_fts,
        };
        use crate::vcp_modules::diary::{
            diary_cancel_search, diary_cancel_semantic_search, diary_create_note,
            diary_delete_empty_folder, diary_delete_notes, diary_get_note, diary_list_folders,
            diary_list_notes, diary_move_notes, diary_rename_note, diary_save_note, diary_search,
            diary_semantic_search,
        };
        use crate::vcp_modules::emoticon_manager::{
            fix_emoticon_url, get_emoticon_library, regenerate_emoticon_library,
        };
        use crate::vcp_modules::file_manager::{
            check_attachment_support, get_attachment_real_path, open_file, register_local_file,
            store_file,
        };
        use crate::vcp_modules::forum::{
            forum_create_post, forum_delete, forum_get_post, forum_list_posts, forum_reply,
        };
        use crate::vcp_modules::group_chat_application_service::{
            handle_group_chat_message, invite_group_member_to_speak,
        };
        use crate::vcp_modules::group_service::{
            create_group, delete_group, get_groups, read_group_config, save_group_config,
            update_group_config,
        };
        use crate::vcp_modules::high_speed_channel::prepare_vcp_upload;
        use crate::vcp_modules::lifecycle_manager::{
            get_core_status, get_last_error, get_system_snapshot, reconcile_distributed_node_cmd,
            restart_or_exit_app, set_app_foreground_state,
        };
        use crate::vcp_modules::logcenter::{logcenter_clear_server, logcenter_fetch};
        use crate::vcp_modules::mail::{
            mail_attachment, mail_folders, mail_list, mail_mark, mail_move, mail_read, mail_reply,
            mail_search, mail_send, mail_state, mail_trash,
        };
        use crate::vcp_modules::maintenance_manager::{
            cleanup_orphaned_attachments, cleanup_single_orphaned_attachment, clear_webview_cache,
            reconstruct_system_cache,
        };
        use crate::vcp_modules::message_repository::{
            process_message_content, rebuild_all_pre_renders,
        };
        use crate::vcp_modules::message_service::{
            delete_message_attachment, fetch_raw_message_content, re_render_message,
        };
        use crate::vcp_modules::model_manager::{
            get_cached_models, get_favorite_models, get_hot_models, record_model_usage,
            refresh_models, start_batch_model_test, stop_all_model_tests, test_model_connectivity,
            toggle_favorite_model,
        };
        use crate::vcp_modules::settings_manager::{
            get_settings_recovery_status, read_settings, set_theme, update_settings,
        };
        use crate::vcp_modules::sync_service::{
            clear_old_sync_logs, get_sync_session_log_path, get_sync_status, list_sync_log_files,
            read_sync_log_file, start_manual_sync, stop_sync,
        };
        use crate::vcp_modules::taskcenter::{
            delegation_cancel, delegation_list, task_agent_list, task_create, task_delete,
            task_get_config, task_get_status, task_set_enabled, task_set_global_enabled,
            task_trigger, task_update,
        };
        use crate::vcp_modules::topic_service::{
            create_topic, delete_topic, get_topics, get_topics_streamed, get_unread_counts,
            regenerate_topic_response, set_topic_unread, summarize_topic, toggle_topic_lock,
            update_topic_title,
        };
        use crate::vcp_modules::update_manager::{
            cancel_update_download, check_for_update, get_update_status, install_update,
            start_update_download,
        };
        use crate::vcp_modules::vcp_client::{
            get_active_generations, interruptGroupTurn, interruptRequest, recover_active_generation,
            sendToVCP, test_vcp_connection,
        };
        use crate::vcp_modules::vcp_info_service::{
            clear_vcp_info, get_vcp_info_connection_status, get_vcp_info_metadata_list,
            get_vcp_info_payload, init_vcp_info_connection,
        };
        use crate::vcp_modules::vcp_log_service::{
            init_vcp_log_connection, send_vcp_log_message, set_vcp_log_heartbeat,
        };

        // ──────────────────────────────────────────────────────────────
        // IPC 防爆栈总闸（治理 1.1.4 Release「输入附件必闪退」的同类隐患）
        //
        // 机制背景：Android 上 Tauri 的 IPC 在 WebView 的 JavaBridge 线程
        // （ART HandlerThread，栈仅 ~1MB）上【同步】完成命令分发——匹配命令名、
        // 反序列化参数、【按值构造】命令 future、再把 future【按值】移交
        // tokio::spawn。而 async 命令的 future 类型尺寸 = 各 await 点存活变量
        // 总和的最大值（编译期固定），业务一复杂就会胖到几十~几百 KB；Release
        // 的 LTO + codegen-units=1 又会把按值搬运链摊平成巨型单栈帧（实测
        // 266KB 的 future 放大出 ~1.15MB 一帧）。栈探测踩穿 guard page →
        // SIGSEGV。这不是 panic，Result/catch_unwind 都拦不住，进程直接被
        // 内核杀死；且 Debug 构建因布局不同往往不发作，极具隐蔽性。
        //
        // 治理方式：JavaBridge 线程上从此只做一件事——把 Invoke【整体】spawn
        // 到 tokio worker 线程（栈 2MB）；命令分发、参数反序列化、future 构造
        // 全部在 worker 上发生。单条命令的 future 尺寸从此与栈安全彻底解耦，
        // 新增命令无需再人肉评估「这条命令的 future 会不会太胖」。
        //
        // 不受影响的路径（tauri 在 webview on_message 里前置分流，不经本闭包）：
        //   - `plugin:*` 命令（插件命令 future 实测 ≤15.5KB，本就安全）；
        //   - Channel 流式通道（聊天 SSE 等）、ACL 检查、invoke key 校验。
        //
        // 语义说明：
        //   - 命令体原本就在 worker 上执行（respond_async 内部即 spawn），此处
        //     仅把「构造 future」这半步一并挪走，行为与响应通道完全不变；
        //   - 每次 invoke 多一次 spawn 调度（微秒级），无热路径影响；
        //   - 未知命令在 worker 上补发 not found 拒绝，与原生行为一致。
        // ──────────────────────────────────────────────────────────────

        // 显式钉住 Wry 运行时类型：generate_handler! 脱离 invoke_handler 的
        // 直接上下文后无法自行推断 R。
        let app_commands: std::sync::Arc<tauri::ipc::InvokeHandler<tauri::Wry>> =
            std::sync::Arc::new(tauri::generate_handler![
                // ── 对话核心（chat_manager / message_service / message_repository / db_manager）──
                append_single_message,
                delete_message_attachment,
                delete_messages,
                fetch_raw_message_content,
                get_fts_index_status,
                handle_agent_chat_message,
                load_chat_history,
                load_chat_history_around,
                load_chat_history_streamed,
                patch_single_message,
                process_message_content,
                re_render_message,
                rebuild_all_pre_renders,
                rebuild_messages_fts,
                search_messages_fts,
                truncate_history_after_timestamp,
                // ── VCP 连接与生成控制（vcp_client）──
                get_active_generations,
                interruptGroupTurn,
                interruptRequest,
                recover_active_generation,
                sendToVCP,
                test_vcp_connection,
                // ── 话题（topic_service）──
                create_topic,
                delete_topic,
                get_topics,
                get_topics_streamed,
                get_unread_counts,
                regenerate_topic_response,
                set_topic_unread,
                summarize_topic,
                toggle_topic_lock,
                update_topic_title,
                // ── 智能体与头像（agent_service / avatar_service）──
                batch_get_avatars,
                create_agent,
                delete_agent,
                get_agents,
                get_assistants_snapshot,
                get_avatar,
                read_agent_config,
                save_agent_config,
                save_avatar_data,
                store_dominant_color,
                update_agent_config,
                // ── 群组（group_service / group_chat_application_service）──
                create_group,
                delete_group,
                get_groups,
                handle_group_chat_message,
                invite_group_member_to_speak,
                read_group_config,
                save_group_config,
                update_group_config,
                // ── 上下文注入（context_injection）──
                delete_tarven_rule,
                get_tarven_rules,
                preview_tarven_injection,
                reorder_rules,
                save_tarven_rule,
                toggle_rule_enabled,
                // ── 设置（settings_manager）──
                get_settings_recovery_status,
                read_settings,
                set_theme,
                update_settings,
                // ── 日记（diary）──
                diary_cancel_search,
                diary_cancel_semantic_search,
                diary_create_note,
                diary_delete_empty_folder,
                diary_delete_notes,
                diary_get_note,
                diary_list_folders,
                diary_list_notes,
                diary_move_notes,
                diary_rename_note,
                diary_save_note,
                diary_search,
                diary_semantic_search,
                // ── 移动端 CLI（cli）──
                close_vcp_mobile_cli_terminal,
                commit_vcp_mobile_cli_skill_import,
                discard_vcp_mobile_cli_skill_import,
                execute_vcp_mobile_cli_action,
                get_vcp_mobile_cli_manifest,
                get_vcp_mobile_cli_skill_catalog,
                get_vcp_mobile_cli_status,
                inspect_vcp_mobile_cli_skill_import,
                open_vcp_mobile_cli_terminal,
                read_vcp_mobile_cli_terminal,
                resize_vcp_mobile_cli_terminal,
                write_vcp_mobile_cli_terminal,
                // ── 任务中心与委派（taskcenter）──
                delegation_cancel,
                delegation_list,
                task_agent_list,
                task_create,
                task_delete,
                task_get_config,
                task_get_status,
                task_set_enabled,
                task_set_global_enabled,
                task_trigger,
                task_update,
                // ── AgentManager（agentmgr）──
                agentmgr_get_config,
                agentmgr_list_models,
                agentmgr_save_config,
                // ── 论坛（forum）──
                forum_create_post,
                forum_delete,
                forum_get_post,
                forum_list_posts,
                forum_reply,
                // ── 邮件（mail）──
                mail_attachment,
                mail_folders,
                mail_list,
                mail_mark,
                mail_move,
                mail_read,
                mail_reply,
                mail_search,
                mail_send,
                mail_state,
                mail_trash,
                // ── 日志中心（logcenter）──
                logcenter_clear_server,
                logcenter_fetch,
                // ── 附件与文件（file_manager / high_speed_channel）──
                check_attachment_support,
                get_attachment_real_path,
                open_file,
                prepare_vcp_upload,
                register_local_file,
                store_file,
                // ── 系统维护（maintenance_manager）──
                cleanup_orphaned_attachments,
                cleanup_single_orphaned_attachment,
                clear_webview_cache,
                reconstruct_system_cache,
                // ── 模型管理（model_manager）──
                get_cached_models,
                get_favorite_models,
                get_hot_models,
                record_model_usage,
                refresh_models,
                start_batch_model_test,
                stop_all_model_tests,
                test_model_connectivity,
                toggle_favorite_model,
                // ── VCP 信息 / 日志通道（vcp_info_service / vcp_log_service）──
                clear_vcp_info,
                get_vcp_info_connection_status,
                get_vcp_info_metadata_list,
                get_vcp_info_payload,
                init_vcp_info_connection,
                init_vcp_log_connection,
                send_vcp_log_message,
                set_vcp_log_heartbeat,
                // ── 生命周期与系统状态（lifecycle_manager）──
                get_core_status,
                get_last_error,
                get_system_snapshot,
                restart_or_exit_app,
                set_app_foreground_state,
                // ── 同步（sync_service）──
                clear_old_sync_logs,
                get_sync_session_log_path,
                get_sync_status,
                list_sync_log_files,
                read_sync_log_file,
                start_manual_sync,
                stop_sync,
                // ── 分布式（distributed；reconcile 寄居 lifecycle_manager）──
                distributed::execute_distributed_tool,
                distributed::get_distributed_status,
                distributed::get_distributed_tool_config_status,
                distributed::get_registered_tools_metadata,
                distributed::reconnect_distributed_client,
                distributed::reset_distributed_tools_disabled,
                distributed::update_enabled_tools,
                reconcile_distributed_node_cmd,
                // ── 更新（update_manager）──
                cancel_update_download,
                check_for_update,
                get_update_status,
                install_update,
                start_update_download,
                // ── 表情包（emoticon_manager）──
                fix_emoticon_url,
                get_emoticon_library,
                regenerate_emoticon_library,
                // ── 插件命令转发（vcp-mobile 流式保活）──
                tauri_plugin_vcp_mobile::stream::set_keepalive_mode,
            ]);

        // 壳闭包本体：极薄，任何命令的胖瘦都不再经过 JavaBridge 的栈。
        move |invoke| {
            let commands = std::sync::Arc::clone(&app_commands);
            tauri::async_runtime::spawn(async move {
                let cmd = invoke.message.command().to_string();
                let resolver = invoke.resolver.clone();
                if !commands(invoke) {
                    resolver.reject(format!("Command {cmd} not found"));
                }
            });
            true
        }
    }};
}

pub(crate) use app_command_handler;
