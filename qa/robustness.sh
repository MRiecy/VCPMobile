#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ROUNDS="${ROUNDS:-2}"
RUN_ANDROID="${RUN_ANDROID:-0}"

log() {
  printf '\n==> %s\n' "$1"
}

run() {
  printf '+ %s\n' "$*"
  "$@"
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

require_rg_count() {
  local expected="$1"
  local pattern="$2"
  shift 2
  local count
  count="$(rg -n "$pattern" "$@" | wc -l | tr -d ' ')"
  if [[ "$count" != "$expected" ]]; then
    fail "模式数量异常: $pattern 期望=$expected 实际=$count"
  fi
}

log "鲁棒性静态哨兵"
require_rg_count 1 "export function findAgentMessagePayload" src
require_rg_count 1 "function findAgentMessageToolPayload" src
rg -q "agentNotificationDedupLock" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 去重锁缺失"
rg -q "AtomicInteger" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 通知 ID 原子计数器缺失"
rg -q "nextAgentMessageNotificationId" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 通知 ID 生成函数缺失"

log "Tauri command 注册一致性"
for command in \
  sendToVCP \
  handle_agent_chat_message \
  handle_group_chat_message \
  append_single_message \
  patch_single_message \
  delete_messages \
  truncate_history_after_timestamp \
  get_topics_streamed \
  start_manual_sync \
  stop_sync \
  init_vcp_log_connection \
  send_vcp_log_message \
  get_distributed_status \
  execute_distributed_tool \
  check_for_update \
  check_for_frontend_update
do
  rg -q "$command" src-tauri/src/lib.rs || fail "主 invoke handler 缺少: $command"
done

log "插件与后端互通一致性"
rg -q "tauri_plugin_vcp_mobile::init\\(\\)" src-tauri/src/lib.rs \
  || fail "主后端未注册 tauri-plugin-vcp-mobile"
rg -q "register_android_plugin\\(\"com.vcp.mobile\", \"VcpMobilePlugin\"\\)" src-tauri/plugins/vcp-mobile/src/lib.rs \
  || fail "Rust 插件未注册 Android VcpMobilePlugin"
for bridge in \
  checkAllPermissions \
  requestAndroidPermission \
  moveTaskToBack \
  pickFile \
  showSystemNotification \
  startSensorCollection \
  stopSensorCollection \
  getSensorData \
  acquireWakeLock \
  releaseWakeLock \
  startNetworkMonitoring
do
  rg -q "\"$bridge\"" src-tauri/plugins/vcp-mobile/src/system.rs \
    || fail "Rust 插件 wrapper 缺少 run_mobile_plugin: $bridge"
  rg -q "fun $bridge\\(invoke: Invoke" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
    || fail "Android Kotlin 插件缺少命令实现: $bridge"
done
for distributed_bridge in \
  src-tauri/src/distributed/tools/notification.rs \
  src-tauri/src/distributed/tools/agent_message.rs
do
  rg -q "tauri_plugin_vcp_mobile::system::show_system_notification" "$distributed_bridge" \
    || fail "分布式工具未直连 Android 系统通知: $distributed_bridge"
done
rg -q "\"distributed-notification\"" src-tauri/src/distributed/tools/notification.rs \
  || fail "MobileNotification 缺少前端兜底事件"
rg -q "\"vcp-system-event\"" src-tauri/src/distributed/tools/agent_message.rs \
  || fail "AgentMessage 缺少前端系统事件"

log "分布式工具注册一致性"
for tool in \
  DeviceInfoTool \
  NotificationTool \
  ClipboardTool \
  AgentMessageTool \
  MobileAgentMessageTool \
  TopicMemoTool \
  TopicSponsorTool \
  BatteryInfoTool \
  MemoryInfoTool \
  CpuInfoTool \
  GpuInfoTool \
  NetworkInfoTool \
  StorageInfoTool \
  LocationTool \
  MotionSensorTool \
  AmbientSensorTool \
  DeviceStatusSummaryTool
do
  rg -q "$tool" src-tauri/src/distributed/tools/mod.rs || fail "分布式工具未注册: $tool"
done

log "前端通知解析去重检查"
if rg -n "findAgentMessagePayload\\s*=|function findAgentMessagePayload" src/App.vue src/core/composables/useNotificationProcessor.ts; then
  fail "AgentMessage payload 解析 helper 不应回到调用方重复定义"
fi

log "重复执行类型检查与关键 Rust 测试"
for round in $(seq 1 "$ROUNDS"); do
  log "第 ${round}/${ROUNDS} 轮"
  run pnpm exec vue-tsc --noEmit
  run cargo test --manifest-path src-tauri/Cargo.toml agent_message --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_memo --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_sponsor --lib
  run cargo test --manifest-path src-tauri/Cargo.toml stream_block_parser --lib
  run cargo test --manifest-path src-tauri/Cargo.toml context_sanitizer --lib
  run cargo test --manifest-path src-tauri/Cargo.toml vcp_log --lib
done

log "并发编译压力检查"
run cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=4

if [[ "$RUN_ANDROID" == "1" ]]; then
  log "Android 原生编译鲁棒性检查"
  (cd src-tauri/gen/android && run bash ./gradlew :tauri-plugin-vcp-mobile:compileDebugKotlin :app:compileUniversalDebugKotlin)
else
  log "跳过 Android 原生编译鲁棒性检查"
  printf '如需覆盖 Android 原生插件，请运行: RUN_ANDROID=1 ROUNDS=%s bash qa/robustness.sh\n' "$ROUNDS"
fi

log "鲁棒性脚本完成"
