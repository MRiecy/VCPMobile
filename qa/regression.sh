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
- 分布式工具: MobileDeviceInfo、MobileNotification、MobileClipboard、AgentMessage、MobileAgentMessage、TopicMemo、TopicSponsor。
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
  src-tauri/Cargo.toml \
  src-tauri/src/lib.rs \
  src-tauri/src/distributed/tools/mod.rs \
  src-tauri/plugins/vcp-mobile/src/lib.rs \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
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
require_pattern "register_oneshot\\(agent_message::MobileAgentMessageTool\\)" src-tauri/src/distributed/tools/mod.rs
require_pattern "register_oneshot\\(topic_sponsor::TopicSponsorTool\\)" src-tauri/src/distributed/tools/mod.rs
require_pattern "register_streaming\\(device_status_summary::DeviceStatusSummaryTool\\)" src-tauri/src/distributed/tools/mod.rs

log "插件与后端互通静态回归"
require_pattern "tauri_plugin_vcp_mobile::init\\(\\)" src-tauri/src/lib.rs
require_pattern "register_android_plugin\\(\"com.vcp.mobile\", \"VcpMobilePlugin\"\\)" src-tauri/plugins/vcp-mobile/src/lib.rs
require_pattern "\"showSystemNotification\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "\"checkAllPermissions\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "\"startSensorCollection\"" src-tauri/plugins/vcp-mobile/src/system.rs
require_pattern "fun showSystemNotification\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "fun checkAllPermissions\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "fun startSensorCollection\\(invoke: Invoke\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt
require_pattern "tauri_plugin_vcp_mobile::system::show_system_notification" src-tauri/src/distributed/tools/notification.rs
require_pattern "tauri_plugin_vcp_mobile::system::show_system_notification" src-tauri/src/distributed/tools/agent_message.rs
require_pattern "\"distributed-notification\"" src-tauri/src/distributed/tools/notification.rs
require_pattern "\"vcp-system-event\"" src-tauri/src/distributed/tools/agent_message.rs

log "审查修复静态回归"
require_pattern "Notification requested but Android delivery failed" src-tauri/src/distributed/tools/notification.rs
require_pattern "LIKE \\? ESCAPE '\\\\\\\\'" src-tauri/src/distributed/tools/topic_memo.rs
require_pattern "LIKE \\? ESCAPE '\\\\\\\\'" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "select_topic_content_hash" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "contentCorrupted" src-tauri/src/distributed/tools/topic_sponsor.rs
require_pattern "parseAndroidNotification" src/core/utils/agentMessagePayload.ts
require_pattern "MAX_AGENT_PAYLOAD_DEPTH = 6" src/core/utils/agentMessagePayload.ts
require_pattern "findAgentMessagePayload\\(event.payload\\)" src/features/distributed/ToolInteractionOverlay.vue

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
