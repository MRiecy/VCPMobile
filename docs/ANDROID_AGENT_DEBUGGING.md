# Android Debug Agent 工具规范

## 1. 目标与唯一边界

`tests/e2e-android/scripts/android-debug-agent.cjs` 是 VCPMobile 唯一受支持的
Agent Android Debug 入口。它解决四个问题：代理/TUN 环境下的 USB Dev、可判定的
设备与进程状态、严格有界的日志输出、不会把截图和全量系统诊断灌入 Agent 响应。

硬边界：

- 所有写操作只允许 `com.vcp.avatar.debug`；
- `com.vcp.avatar` 是用户自用 Release，工具没有 Release 模式、包名覆盖或对应写路径；
- Dev 只支持 USB + `adb reverse`，不自动猜测 WiFi、热点或物理网卡；
- 多设备时必须传 `--serial`，不得“取第一台”；
- 不执行 `logcat -c`，不执行 `adb reverse --remove-all`；
- 只移除当前 Dev 会话亲自创建的 1420/1421 reverse；已有同值映射复用但不认领；
- 默认输出和落盘证据都不是 Release、性能 SLA 或完整 UI 验收。

## 2. 命令面

| 命令 | 用途 | 是否写设备 |
|---|---|---|
| `pnpm android:debug:doctor -- --json` | ADB、pnpm、USB 设备与当前 Dev 就绪度 | 否 |
| `pnpm android:debug:dev -- --serial <id>` | 建立 1420/1421 reverse 并前台运行 Tauri Android Dev | 是，仅 reverse、Debug 构建安装 |
| `pnpm android:debug:status -- --json` | 设备、WebView、viewport、Debug 包、PID、前台与 reverse | 否 |
| `pnpm android:debug:logs -- --lines 80 --level i` | 当前 Debug 主进程 PID-scoped logcat | 否 |
| `pnpm android:debug:snapshot -- --screenshot` | 状态、有限日志及可选单张截图落盘 | 否 |
| `pnpm android:debug:screenshot -- --name <slug>` | 只抓一张图，stdout 只返回路径和字节数 | 否 |
| `pnpm android:debug:reload` | Dev 就绪后仅重启 Debug 包 | 是，仅 Debug 包 |
| `pnpm android:debug:stop` | 通知当前 Dev supervisor 停止并清理自有 reverse | 是，仅自有 reverse |
| `pnpm android:debug:install -- --apk <path>` | 用 `aapt` 验证 application id 后安装 Debug APK | 是，仅验证通过的 Debug APK |
| `pnpm android:debug:grant` | Debug 包普通运行时权限 best-effort 预授权 | 是，仅 Debug 包 |

所有命令接受 `--serial <id>`。`doctor/status/logs/snapshot/screenshot/install/grant`
支持 `--json`；`dev --json` 输出逐行 NDJSON 事件。

## 3. Agent 输出契约

1. `dev` 的完整 stdout/stderr 写入 `.agent/android-debug/dev-logs/`；控制台只输出阶段、
   最多 20 条错误诊断、30 秒心跳和失败尾部摘要。
2. `logs` 默认 80 行，硬上限 200 行；只读当前 `com.vcp.avatar.debug` 主进程 PID。
   进程不存在时直接报告，不回退到设备全局 logcat。
3. `status` 是 `vcp.android-debug.status.v1` 单对象；不得用整份 `dumpsys activity top`
   代替状态摘要。
4. `screenshot` 不输出 base64 或像素；`snapshot --screenshot` 也只输出 manifest 路径。
5. 本地产物统一进入已忽略的 `.agent/android-debug/`。需要进入长期证据时，应人工选取
   最小文件集即可；诊断内容保持原文，不改写 token、URL、路径或错误文本。

## 4. 推荐 Agent 旅程

```bash
# 1. 一次性确认目标设备；多设备必须显式 serial
pnpm android:debug:doctor -- --serial <adb-serial> --json

# 2. 在一个长驻终端启动，完整构建日志不会进入对话
pnpm android:debug:dev -- --serial <adb-serial>

# 3. 其他调用只取当前需要的有限证据
pnpm android:debug:status -- --serial <adb-serial> --json
pnpm android:debug:logs -- --serial <adb-serial> --lines 80 --level i
pnpm android:debug:screenshot -- --serial <adb-serial> --name current-state

# 4. 只有需要成套诊断时才生成 snapshot；截图仍是 opt-in
pnpm android:debug:snapshot -- --serial <adb-serial> --lines 120 --screenshot

# 5. 完成后让 supervisor 清理自己创建的 reverse
pnpm android:debug:stop
```

不得用 `pnpm tauri android dev` 作为 Agent 默认入口：它不负责选定 USB 设备、代理隔离、
reverse 所有权、控制台限流和状态文件。底层 Tauri CLI 仍由统一工具调用，不删除通用
`pnpm tauri` 脚本。

## 5. 与 E2E、性能证据的区别

- 本工具证明指定 Debug 设备的启动与诊断事实，不自动遍历 UI，也不等于
  `DEVICE-VERIFIED`。
- `tests/perf/scripts/collect_android_dumpsys.cjs` 是显式的本地全量性能诊断，可能产生
  大文件；不得作为 Agent 日常日志命令。
- 冷启动与 dumpsys 性能脚本同样固定 Debug 包，不再接受 `--mode release`。
- 通知监听、OEM 自启动、电池无限制和最近任务锁仍需人工预配置。

## 6. 失败语义

- 无设备或多设备未指定：失败，不猜测目标；
- 网络 ADB serial：`dev` 失败，要求 USB；
- 1420/1421 已映射到不同远端：失败，不覆盖；
- Dev server 或 reverse 未就绪：`reload` 失败，不启动连接错误页；
- `aapt` 不存在或 APK application id 不是 `com.vcp.avatar.debug`：安装失败闭合；
- 状态文件陈旧：`stop` 只删除陈旧状态，不擅自删除 reverse。
