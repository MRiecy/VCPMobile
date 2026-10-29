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
        use crate::vcp_modules::aurora_pipeline::rebuild_aurora_snapshot;
        use crate::vcp_modules::chat_manager::{
            append_single_message, delete_messages, edit_resend_message, load_chat_history, load_chat_history_around,
            load_chat_history_streamed, patch_single_message, truncate_history_after_message,
        };
        use crate::vcp_modules::chat::voice_service::{
            begin_voice_conversation, end_voice_conversation, get_voice_conversation_snapshot,
            start_voice_microphone, close_voice_microphone,
            allocate_tts_document, connect_tts_document,
            start_tts_session, stop_tts, submit_tts_text,
            preview_tts, preview_voice_clone_sample,
            import_voice_clone_sample, get_voice_clone_sample,
            list_voice_clone_samples, delete_voice_clone_sample,
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
        use crate::vcp_modules::context_sanitizer::{chat_sanitize_content, chat_strip_thought_chains};
        use crate::vcp_modules::db_manager::search_messages_fts;
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
            begin_file_operation, cancel_attachment_import, check_attachment_support,
            export_attachment_file, finish_file_operation, get_attachment_image,
            get_attachment_real_path, open_file, prepare_rendered_image, read_attachment_text_preview,
            register_local_file, release_attachment_image, release_rendered_image,
            release_temporary_export, store_file, store_temporary_export,
        };
        use crate::vcp_modules::forum::{
            forum_create_post, forum_delete, forum_get_post, forum_list_posts, forum_reply,
        };
        use crate::vcp_modules::group_chat_application_service::{
            control_group_session, handle_group_chat_message, invite_group_member_to_speak,
        };
        use crate::vcp_modules::group_service::{
            create_group, delete_group, get_groups, read_group_config, save_group_config,
            update_group_config,
        };
        use crate::vcp_modules::boot_trace::{get_boot_trace, save_boot_trace};
        use crate::vcp_modules::lifecycle_manager::{
            get_core_status, get_last_error, get_system_snapshot, reconcile_distributed_node_cmd,
            restart_or_exit_app,
        };
        use crate::vcp_modules::logcenter::{logcenter_clear_server, logcenter_fetch};
        use crate::vcp_modules::mail::{
            mail_attachment, mail_folders, mail_list, mail_mark, mail_move, mail_read, mail_reply,
            mail_search, mail_send, mail_state, mail_trash,
        };
        use crate::vcp_modules::maintenance_manager::{clear_webview_cache, reconstruct_system_cache};
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
            export_config_domains, export_group_preferences, import_settings_backup,
            parse_config_backup, read_settings,
            update_settings,
        };
        use crate::vcp_modules::infra::share_inbox::{
            accept_android_share, bind_current_share, commit_share_message, discard_current_share,
            finish_android_share, prepare_current_share, read_android_share, read_current_share,
            update_share_draft,
        };
        use crate::vcp_modules::sync_service::{
            clear_old_sync_logs, list_sync_log_files, prepare_sync_log_share_file,
            read_sync_log_file, release_sync_log_share_file, start_manual_sync, stop_sync,
        };
        use crate::vcp_modules::taskcenter::{
            delegation_cancel, delegation_list, task_agent_list, task_create, task_delete,
            task_get_config, task_get_status, task_set_enabled, task_set_global_enabled,
            task_trigger, task_update,
        };
        use crate::vcp_modules::topic_service::{
            create_topic, delete_topic, get_topics, get_topics_streamed, get_unread_counts,
            increment_topic_unread_count, regenerate_topic_response, set_topic_unread,
            summarize_topic, toggle_topic_lock, update_topic_title,
        };
        use crate::vcp_modules::update_manager::{
            cancel_update_download, check_for_update, get_update_status, install_update,
            start_update_download,
        };
        use crate::vcp_modules::vcp_client::{
            reconnect_chat_streams, interruptGroupTurn, interruptRequest, preview_chat_endpoint,
            test_vcp_connection,
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
                // ── 对话核心（chat_manager / message_service / message_repository / db_manager / context_sanitizer）──
                append_single_message,
                chat_sanitize_content,
                chat_strip_thought_chains,
                delete_message_attachment,
                delete_messages,
                edit_resend_message,
                fetch_raw_message_content,
                handle_agent_chat_message,
                load_chat_history,
                load_chat_history_around,
                load_chat_history_streamed,
                patch_single_message,
                process_message_content,
                re_render_message,
                rebuild_all_pre_renders,
                rebuild_aurora_snapshot,
                search_messages_fts,
                truncate_history_after_message,
                // ── 实时语音交互（仅当前语音拥有的识别、朗读与请求）──
                begin_voice_conversation,
                end_voice_conversation,
                get_voice_conversation_snapshot,
                close_voice_microphone,
                start_voice_microphone,
                allocate_tts_document,
                connect_tts_document,
                start_tts_session,
                stop_tts,
                submit_tts_text,
                preview_tts,
                preview_voice_clone_sample,
                import_voice_clone_sample,
                get_voice_clone_sample,
                list_voice_clone_samples,
                delete_voice_clone_sample,
                // ── VCP 连接与生成控制（vcp_client）──
                reconnect_chat_streams,
                interruptGroupTurn,
                interruptRequest,
                preview_chat_endpoint,
                test_vcp_connection,
                // ── 话题（topic_service）──
                create_topic,
                delete_topic,
                get_topics,
                get_topics_streamed,
                get_unread_counts,
                increment_topic_unread_count,
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
                control_group_session,
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
                export_config_domains,
                export_group_preferences,
                import_settings_backup,
                parse_config_backup,
                read_settings,
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
                // ── 附件与文件（file_manager）──
                begin_file_operation,
                cancel_attachment_import,
                check_attachment_support,
                export_attachment_file,
                finish_file_operation,
                get_attachment_image,
                get_attachment_real_path,
                open_file,
                prepare_rendered_image,
                read_attachment_text_preview,
                register_local_file,
                release_attachment_image,
                release_rendered_image,
                release_temporary_export,
                store_file,
                store_temporary_export,
                // ── 当前一次分享（share_inbox）──
                accept_android_share,
                read_android_share,
                bind_current_share,
                commit_share_message,
                discard_current_share,
                finish_android_share,
                prepare_current_share,
                read_current_share,
                update_share_draft,
                // ── 系统维护（maintenance_manager）──
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
                // ── 生命周期与系统状态（lifecycle_manager / boot_trace）──
                get_boot_trace,
                get_core_status,
                get_last_error,
                get_system_snapshot,
                restart_or_exit_app,
                save_boot_trace,
                // ── 同步（sync_service）──
                clear_old_sync_logs,
                list_sync_log_files,
                prepare_sync_log_share_file,
                read_sync_log_file,
                release_sync_log_share_file,
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
            // App commands do not inherit plugin ACLs without an app ACL manifest.
            // The production app has one trusted main WebView; archived portal labels
            // must not acquire all custom commands through the default dispatcher.
            if invoke.message.webview().label() != "main" {
                invoke.resolver.reject("unsupported: app commands require the main WebView");
                return true;
            }
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
