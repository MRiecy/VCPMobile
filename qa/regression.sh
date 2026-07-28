#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

RUN_ANDROID="${RUN_ANDROID:-0}"

log() {
  printf '\n==> %s\n' "$1"
}

run() {
  printf '+ %s\n' "$*"
  "$@"
}

require_file() {
  local file="$1"
  if [[ ! -f "$file" ]]; then
    printf '缺少关键文件: %s\n' "$file" >&2
    exit 1
  fi
}

require_pattern() {
  local pattern="$1"
  local file="$2"
  if ! rg -q "$pattern" "$file"; then
    printf '未找到关键模式: %s (%s)\n' "$pattern" "$file" >&2
    exit 1
  fi
}

reject_pattern() {
  local pattern="$1"
  local file="$2"
  if rg -q "$pattern" "$file"; then
    printf '发现禁止模式: %s (%s)\n' "$pattern" "$file" >&2
    exit 1
  fi
}

print_feature_map() {
  cat <<'FEATURES'
功能面清单:
- 启动与生命周期: 权限门禁、启动态、App 快照、后台/前台心跳、WebView 缓存清理。
- 聊天主流程: 单 Agent 聊天、群聊、流式响应、中断、重试/再生成、消息编辑/删除/截断、预渲染。
- 话题: 话题列表、创建、删除、重命名、锁定、未读状态、摘要。
- Agent 与群组: Agent/群组 CRUD、配置读写、头像与主色存储。
- 输入与附件: 文件选择、分享入口、附件缓存、上传准备、图片/音频/视频/文档处理、外部打开。
- 内容渲染: Markdown/代码/工具块/思考块/HTML 预览、图片查看、表情 URL 修复、Tarven 注入。
- 通知: 应用内通知过滤与展示、Android 系统通知、下载通知、AgentMessage 通知桥接。
- 设置: VCP 核心连接、模型缓存/收藏/使用记录、用户资料、主题、AI 逻辑、维护、更新。
- 同步: 手动同步、停止同步、同步状态、同步日志浏览与清理。
- 分布式节点: 连接/断开/重连、工具元数据、禁用工具同步、WebSocket 工具执行。
- 分布式工具: MobileDeviceInfo、MobileNotification、MobileClipboard、AgentMessage、MobileAgentMessage、TopicMemo、MobileTopicSponsor。
- 设备遥测: 电池、内存、CPU、GPU、网络、存储、定位、运动、环境传感器、设备状态摘要。
- Android 原生能力: 权限、悬浮球、息屏保持、WakeLock、前台保活、传感器采集、网络监控、Root 检查/命令。
- 插件与后端互通: Tauri 主后端注册 `tauri-plugin-vcp-mobile`，Rust 插件 wrapper 通过 `run_mobile_plugin` 调 Android Kotlin 插件，分布式通知/AgentMessage 后端工具可直达系统通知。
- 更新: APK 更新检查/下载/安装，前端 OTA 检查/下载/应用/确认启动。
- 本地服务与基础设施: VCPLog、系统快照、本地服务协调、高速通道、持久化数据库。
FEATURES
}

log "功能面"
print_feature_map

log "关键文件存在性"
for file in \
  package.json \
  src/App.vue \
  src/core/router/index.ts \
  src/core/composables/useNotificationProcessor.ts \
  src/core/utils/agentMessagePayload.ts \
  src/core/utils/safeMessageHtml.ts \
  src/core/utils/runtimeDiagnostics.ts \
  src-tauri/Cargo.toml \
  src-tauri/src/lib.rs \
  src-tauri/src/vcp_modules/infra/runtime_diagnostics.rs \
  src-tauri/src/vcp_modules/infra/runtime_diagnostics/export.rs \
  src-tauri/src/distributed/tools/mod.rs \
  src-tauri/plugins/vcp-mobile/src/lib.rs \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/CrashDiagnostics.kt \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  qa/regression.sh \
  qa/robustness.sh
do
  require_file "$file"
done

log "功能入口静态回归"
require_pattern "path: '/chat'" src/core/router/index.ts
require_pattern "path: '/assistant'" src/core/router/index.ts
require_pattern "handle_agent_chat_message" src-tauri/src/lib.rs
require_pattern "handle_group_chat_message" src-tauri/src/lib.rs
require_pattern "start_manual_sync" src-tauri/src/lib.rs
require_pattern "distributed::execute_distributed_tool" src-tauri/src/lib.rs
require_pattern "check_for_update" src-tauri/src/lib.rs
require_pattern "check_for_frontend_update" src-tauri/src/lib.rs
require_pattern "show_system_notification" src-tauri/plugins/vcp-mobile/src/lib.rs
require_pattern "!qa/\\*.sh" .gitignore
require_pattern "register_oneshot\\(agent_message::MobileAgentMessageTool\\)" src-tauri/src/distributed/tools/mod.rs
require_pattern "register_oneshot\\(topic_sponsor::TopicSponsorTool\\)" src-tauri/src/distributed/tools/mod.rs
require_pattern "MobileTopicSponsor" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "register_streaming\\(device_status_summary::DeviceStatusSummaryTool\\)" src-tauri/src/distributed/tools/mod.rs

log "移动端线路切换静态回归"
require_pattern "useConnectionProfilesStore" src/components/layout/RightSidebar.vue
require_pattern "输出中不可切换" src/components/layout/RightSidebar.vue
require_pattern "chatStreamStore\\.hasActiveStreams" src/components/layout/RightSidebar.vue
require_pattern "connectionProfilesStore\\.switching" src/components/layout/RightSidebar.vue
require_pattern "模型刷新中不可切换" src/components/layout/RightSidebar.vue
require_pattern "activeConnectionProfileId: profileId" src/core/stores/connectionProfiles.ts
require_pattern "vcpServerUrl: target\\.vcpServerUrl" src/core/stores/connectionProfiles.ts
require_pattern "vcpApiKey: target\\.vcpApiKey" src/core/stores/connectionProfiles.ts
require_pattern "vcpLogUrl: target\\.vcpLogUrl" src/core/stores/connectionProfiles.ts
require_pattern "vcpLogKey: target\\.vcpLogKey" src/core/stores/connectionProfiles.ts
require_pattern "syncServerUrl: target\\.syncServerUrl" src/core/stores/connectionProfiles.ts
require_pattern "syncHttpUrl: target\\.syncHttpUrl" src/core/stores/connectionProfiles.ts
require_pattern "syncToken: target\\.syncToken" src/core/stores/connectionProfiles.ts
require_pattern "distributedWsUrl: target\\.distributedWsUrl" src/core/stores/connectionProfiles.ts
require_pattern "distributedVcpKey: target\\.distributedVcpKey" src/core/stores/connectionProfiles.ts
require_pattern "syncActiveConnectionProfileFromSettings\\(newSettings\\)" src/core/stores/settings.ts
require_pattern "preparedUpdates\\.connectionProfiles = mergedSettings\\.connectionProfiles" src/core/stores/settings.ts
require_pattern "copyConnectionProfileToSettings\\(props\\.settings, profile\\)" src/features/settings/components/ConnectionProfilesSection.vue
require_pattern "syncActiveConnectionProfileFromSettings\\(settings\\)" src/core/stores/connectionProfiles.ts
require_pattern "modelStore\\.invalidatePersistedCache\\(\\)" src/core/stores/connectionProfiles.ts
require_pattern "modelStore\\.isLoading" src/core/stores/connectionProfiles.ts
require_pattern "cacheGeneration" src/core/stores/modelStore.ts
require_pattern "hasActiveStreams" src/core/stores/chatStreamStore.ts
require_pattern "globalActiveStreamMessageIds" src/core/stores/chatStreamStore.ts
require_pattern "isMessageInAnyActiveStream" src/features/chat/MessageRenderer.vue
require_pattern "pendingGenerationRequests" src/core/stores/chatStreamStore.ts
require_pattern "dailyNoteConfigKey" src/features/dailynote/DailyNoteView.vue
require_pattern "检测到线路或管理员配置变化" src/features/dailynote/DailyNoteView.vue
require_pattern "apiBase: currentDailyNoteApiBase\\.value" src/features/dailynote/DailyNoteView.vue
require_pattern "refresh_lock" src-tauri/src/vcp_modules/infra/model_manager.rs
require_pattern "connection_profiles: Vec<ConnectionProfile>" src-tauri/src/vcp_modules/infra/settings_manager.rs

log "插件与后端互通静态回归"
require_pattern "tauri_plugin_vcp_mobile::init\\(\\)" src-tauri/src/lib.rs
require_pattern "register_android_plugin\\(\"com.vcp.mobile\", \"VcpMobilePlugin\"\\)" src-tauri/plugins/vcp-mobile/src/lib.rs
require_pattern "\"showSystemNotification\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "\"checkAllPermissions\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "\"startSensorCollection\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "fun showSystemNotification\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "fun checkAllPermissions\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "fun startSensorCollection\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "tauri_plugin_vcp_mobile::system::dispatch_system_notification" src-tauri/src/distributed/tools/notification.rs
require_pattern "tauri_plugin_vcp_mobile::system::dispatch_system_notification" src-tauri/src/distributed/tools/agent_message.rs
require_pattern "\"distributed-notification\"" src-tauri/src/distributed/tools/notification.rs
require_pattern "\"vcp-system-event\"" src-tauri/src/distributed/tools/agent_message.rs

log "审查修复静态回归"
require_pattern "pub mod daily_note" src-tauri/src/vcp_modules/chat/mod.rs
require_pattern "parse_daily_note_tool" src-tauri/src/vcp_modules/chat/daily_note.rs
require_pattern "test_daily_note_static_and_stream_parsers_agree" src-tauri/src/vcp_modules/chat/stream_block_parser.rs
require_pattern "maid-diary-update-bubble" src/features/chat/MessageRenderer.vue
require_pattern "valet-diary-bubble" src/assets/message-blocks.css
require_pattern "html.dark \\.valet-diary-bubble" src/assets/message-blocks.css
require_pattern "formatDailyNoteNotificationMessage" src/core/composables/useNotificationProcessor.ts
require_pattern "vcpData\\.tool_name === 'DailyNote' && pluginOutputMessage" src/core/composables/useNotificationProcessor.ts
require_pattern "日记已成功创建" src/components/layout/RightSidebar.vue
require_pattern "日记已成功更新" src/components/layout/RightSidebar.vue
require_pattern "通知铃声" src/components/layout/PermissionGate.vue
require_pattern "requiredGranted" src/components/layout/PermissionGate.vue
require_pattern "status\\.value\\.notification &&" src/components/layout/PermissionGate.vue
require_pattern "status\\.value\\.ring &&" src/components/layout/PermissionGate.vue
require_pattern "status\\.value\\.storage &&" src/components/layout/PermissionGate.vue
require_pattern "status\\.value\\.battery" src/components/layout/PermissionGate.vue
require_pattern "ringBlockingMissing" src/components/layout/PermissionGate.vue
require_pattern "必须开启后才能继续引导" src/components/layout/PermissionGate.vue
reject_pattern "ringRecommendedMissing" src/components/layout/PermissionGate.vue
require_pattern "vcp-lifecycle" src/components/layout/PermissionGate.vue
require_pattern "permission-gate-bottom-action" src/components/layout/PermissionGate.vue
require_pattern "!pStatus\\.ring" src/core/stores/appLifecycle.ts
reject_pattern "continuing bootstrap with silent" src/core/stores/appLifecycle.ts
require_pattern "hasAgentMessageRingCapability" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "ACTION_APP_NOTIFICATION_SETTINGS" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "\"ring\"" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "select_apk_asset" src-tauri/src/vcp_modules/updater/update_manager.rs
require_pattern "打开 Release" src/components/ui/UpdatePrompt.vue
require_pattern "awei807-wei/VCPMobile" src/features/settings/components/AboutSection.vue
reject_pattern "github.com/MRiecy/VCPMobile" src/features/settings/components/AboutSection.vue
require_pattern "max-page-size=16384" scripts/android_env.fish
require_pattern "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER" scripts/android_env.fish
require_pattern "max-page-size=16384" scripts/android_env.sh
require_pattern "max-page-size=16384" .cargo/config.toml
require_pattern "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" .github/workflows/release.yml
require_pattern 'features = \[[^]]*"custom-protocol"' src-tauri/Cargo.toml
require_pattern "Notification requested but Android delivery failed" src-tauri/src/distributed/tools/notification.rs
require_pattern "LIKE \\? ESCAPE '\\\\\\\\'" src-tauri/src/distributed/tools/topic_memo.rs
require_pattern "LIKE \\? ESCAPE '\\\\\\\\'" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "select_topic_content_hash" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "contentCorrupted" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "parseAndroidNotification" src/core/utils/agentMessagePayload.ts
require_pattern "MAX_AGENT_PAYLOAD_DEPTH = 6" src/core/utils/agentMessagePayload.ts
require_pattern "\"vcp-mobile:allow-show-system-notification\"" src-tauri/capabilities/agent-notifications.json
reject_pattern "\"show_system_notification\"" src-tauri/plugins/vcp-mobile/permissions/all.toml
reject_pattern "\"show_system_notification\"" src-tauri/plugins/vcp-mobile/permissions/default.toml
reject_pattern "\"run_root_command\"" src-tauri/plugins/vcp-mobile/permissions/all.toml
reject_pattern "\"run_root_command\"" src-tauri/plugins/vcp-mobile/permissions/default.toml
reject_pattern "\"vcp-mobile:allow-run-root-command\"" src-tauri/capabilities/default.json
require_pattern "show_system_notification is only supported on Android" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "dispatch_system_notification" src-tauri/src/distributed/tools/agent_message.rs
require_pattern "Agent message event emit failed after notification dispatch" src-tauri/src/distributed/tools/agent_message.rs
require_pattern "attempted: false" src-tauri/plugins/vcp-mobile/src/system.rs
reject_pattern "Failed to emit agent message event" src-tauri/src/distributed/tools/agent_message.rs
reject_pattern 'title=\$title' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern 'bodyLength=\$\{body.length\}' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern 'AGENT_MESSAGE_CHANNEL_ID = "agent_message_alerts_v2"' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "setSound\\(agentMessageSoundUri, soundAttributes\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "enableVibration\\(true\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "schema_version: u32" src-tauri/src/distributed/tool_registry.rs
require_pattern "Unsupported disabled tools schemaVersion" src-tauri/src/distributed/tool_registry.rs
require_pattern "MAX_CHECK_NEW_TOPICS_DAYS" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "saturating_sub\\(days.saturating_mul\\(MILLIS_PER_DAY\\)\\)" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "DISTRIBUTED_HEARTBEAT_INTERVAL" src-tauri/src/distributed/client.rs
require_pattern "DISTRIBUTED_HEARTBEAT_TIMEOUT" src-tauri/src/distributed/client.rs
require_pattern "Message::Ping\\(Vec::new\\(\\)\\.into\\(\\)\\)" src-tauri/src/distributed/client.rs
require_pattern "Heartbeat timeout: no inbound WebSocket frame" src-tauri/src/distributed/client.rs
require_pattern "is_distributed_connection_stale" src-tauri/src/distributed/client.rs
require_pattern "recover_distributed_node_after_network_restore" src-tauri/src/vcp_modules/infra/lifecycle_manager.rs
require_pattern "recover_distributed_node_after_network_restore\\(&h\\)" src-tauri/src/lib.rs
require_pattern "renderMessageRawHtml" src/core/utils/astRenderer.ts
require_pattern "shouldRenderMessageHtml" src/core/utils/astExecutor.ts
require_pattern "test_unknown_html_like_tags_remain_visible_text" src-tauri/src/vcp_modules/chat/pre_renderer/markdown_parser.rs
require_pattern "find_matching_fence_end" src-tauri/src/vcp_modules/chat/stream_block_parser.rs
require_pattern "installRuntimeDiagnostics" src/main.ts
require_pattern "export_runtime_diagnostics" src/features/settings/components/MaintenanceSection.vue
require_pattern "record_frontend_diagnostic" src-tauri/src/lib.rs
require_pattern "install_panic_hook" src-tauri/src/lib.rs
require_pattern "getHistoricalProcessExitReasons" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/CrashDiagnostics.kt
require_pattern 'File\(context\.dataDir, "diagnostics"\)' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/CrashDiagnostics.kt
require_pattern "oomGuardExecutor.scheduleWithFixedDelay" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "share_file_native" src-tauri/plugins/vcp-mobile/src/lib.rs
reject_pattern 'console\.warn\(`\[AST createDomFromNode\]' src/core/utils/astExecutor.ts
require_pattern "ForegroundServiceStartNotAllowedException/IllegalStateException" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "onTaskRemoved" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt
require_pattern "createRecoveryIntent" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt
require_pattern "distributed_keepalive_active" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt
require_file src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/receiver/BootReceiver.kt
require_pattern "RECEIVE_BOOT_COMPLETED" src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml
require_pattern "ACTION_BOOT_COMPLETED" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/receiver/BootReceiver.kt
require_pattern "normalizeDistributedNotification" src/features/distributed/ToolInteractionOverlay.vue
require_pattern "androidNotification.delivered === true" src/features/distributed/ToolInteractionOverlay.vue
reject_pattern "event.payload.title.length" src/features/distributed/ToolInteractionOverlay.vue
reject_pattern "event.payload.body.length" src/features/distributed/ToolInteractionOverlay.vue
reject_pattern "findAgentMessagePayload\\(event.payload\\)" src/features/distributed/ToolInteractionOverlay.vue
require_pattern "最后更新：2026-06-15 \\| VCP Mobile v1\\.0\\.6" docs/vue_docs/features/distributed/19_分布式能力前端交互.md

log "前端类型检查"
run pnpm exec vue-tsc --noEmit

log "前端生产构建"
run pnpm exec vite build

log "Rust 单元测试"
run cargo test --manifest-path src-tauri/Cargo.toml --lib

log "Rust 编译检查"
run cargo check --manifest-path src-tauri/Cargo.toml

if [[ "$RUN_ANDROID" == "1" ]]; then
  log "Android Kotlin 编译"
  if [[ ! -f src-tauri/gen/android/gradlew ]]; then
    printf '缺少 Android Gradle wrapper: src-tauri/gen/android/gradlew\n' >&2
    exit 1
  fi
  (cd src-tauri/gen/android && run bash ./gradlew :app:compileUniversalDebugKotlin)
else
  log "跳过 Android Kotlin 编译"
  printf '如需覆盖 Android 原生插件编译，请运行: RUN_ANDROID=1 bash qa/regression.sh\n'
fi

log "Git 差异格式检查"
run git diff --check

log "回归测试完成"
