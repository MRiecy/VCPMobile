# 03｜Android 本地运行时与长任务

## 1. 目标与非目标

首个 Android Runtime 的目标是：在 `arm64-v8a / minSdk 26` 上提供 Android host 非 Root、应用沙箱内、可取消、有界输出的 Alpine Linux Bash CLI，使 Agent 和用户都能使用同一文件系统，但不会把聊天连接、WebView 或 VCP WebSocket 当作任务 owner。Agent 命令固定通过 `/bin/bash -lc` 执行；不得在 Bash、BusyBox `ash` 和 Android shell 之间按命令静默切换。PRoot guest 可以模拟 root 以管理自己的 rootfs 和执行 `apk add`，但不获得 Android root 权限。

首期不承诺：

- 完整 GNU/Linux 发行版兼容；
- 任意 APK/pip/npm 包都可安装；
- Root、Shizuku、跨应用私有数据访问；
- 进程被 OS 强杀后继续执行原命令；
- 任意时长后台常驻；
- 把 Android shell、Toybox 或现有 `run_root_command` 直接暴露给 Agent。

## 2. 推荐边界

```text
Vue feature
  └─ 只持 UI state、工具块、终端输入和用户动作

Rust vcp_modules/cli
  ├─ VcpCliProtocol
  ├─ MobileCliRuntimeState
  ├─ Job/session ledger
  ├─ output budget / redaction / persistence
  ├─ LocalLoopbackTurnAdapter
  └─ DistributedVcpAdapter

Android plugin
  ├─ rootfs/proot/pty 平台资源
  ├─ 实际 Process/PTY handle
  ├─ process-group signal/cancel
  ├─ foreground lease bridge
  └─ 可选 native offload handlers
```

业务真源仍在 Rust：谁拥有 Job、当前 attempt、状态迁移、是否允许 commit、是否可重试。Kotlin 只拥有 Android 才能安全持有的进程/PTY/Service handle，并通过带 `job_id + attempt_id + sequence` 的事件回报。任何旧 sequence 都不得写回新 attempt。

P0 必须做一个最小 spike，比较两种进程宿主：

1. Rust `tokio::process` 直接启动随 APK 打包的 PRoot；
2. Kotlin `ProcessBuilder`/JNI 启动，Rust 通过插件命令和事件管理。

选择依据是 Android 可执行文件限制、PTY、进程组取消、Tauri 生命周期、包体与测试可控性，而不是语言偏好。

## 3. 沙箱文件系统

参考 OpenMinis 的语义，不照搬目录和代码：

```text
/workspace                当前 chat/topic 的持久工作区
/attachments              当前会话显式提供的附件
/shared                   用户明确共享的全局目录
host Skill catalog        Rust action-only 目录；不挂载给 PRoot guest
/tmp                      attempt 临时目录，按预算清理
/mnt/<grant>              SAF 明确授权的外部目录
```

安全合同：

- 默认 cwd 是当前 topic workspace，不是应用配置根或 Android 共享存储根；
- 所有 host bind mount 先经过 Rust canonical policy，再交给原生层；
- SAF 权限按 grant 列表显式暴露，不扫描整个设备；
- Skill 由 Mobile 管理的 host catalog 持有，不向 guest 做 bind；安全列举/阅读只由 `VCPMobileCLI` 的 `list_skills/read_skill` action 提供，manifest 如何进入角色提示词仍由用户在 VCPToolBox 配置；
- 不把 VCP 记忆数据库或自动生成的“记忆投影”挂载进 CLI；需要的材料由用户或 VCP 流程显式写入 workspace；
- 不把 VCP API key、模型 key、签名密码自动注入 shell env；
- 用户自定义 env 只按白名单名字注入，结果回灌前做 secret redaction；
- rootfs、包索引和下载内容有独立磁盘预算及清理入口。
- rootfs 随版本带 `command-profile.json`，记录发行版、Bash、musl、基线包和每个广告命令的探测结果；manifest 只能描述探测通过的命令集。

### 3.1 Skill action 读取桥

Skill 不能只是一个 Agent 不知如何使用的宿主目录，也不应要求 Agent 从一段很长的 manifest 中记住隐藏内部命令。`VcpCliProtocol` 将以下两个 action 直接路由到 Rust 侧受控 Skill catalog；它们不启动 Bash、不创建 `CliJob`、不依赖网络：

```text
action=list_skills
action=read_skill, skill_id=<id>, resource_path=SKILL.md, max_bytes=<n>
```

- Skill catalog 扫描应用私有 host 目录中经校验的 `<id>/SKILL.md`，建立包含 `id/name/description/source/hash/version/integrity_status` 的有界索引；`list_skills` 只返回已安装且校验通过的项，不递归打印正文。该 catalog 由 Rust 独立拥有，不假定当前产品已有同名 Store。
- `read_skill` 仅接受索引中的稳定 `skill_id`；`resource_path` 默认 `SKILL.md`，也可读取同一 Skill 下的 `references/`、`assets/` 或脚本文本。禁止绝对路径、`..`、符号链越界、特殊文件和超预算输出。
- `read_skill` 返回有界正文、hash、截断状态和逻辑引用 `skill_root=vcp-skill://<id>`；该 URI 不是 Bash 路径。Agent 理解说明后若需运行脚本，必须先经明确的物化动作把指定资源复制到 `/workspace`，再用另一次 `action=run` 执行。
- Skill action 不提供 `install`、`enable` 或隐式 `run`，不自动 source 脚本，不给 Skill 注入 API key、SAF 或 Android 权限。后续脚本执行仍经过普通 `run` 的确认门、超时、输出和网络策略。
- manifest 直接列出两个 action 及字段，不把全部 Skill 名称或正文注入系统提示词。

API 36 真机证伪了“chmod 后只读 bind”这一安全假设：PRoot `-0` 下 guest 仍可改写绑定的 host 文件，而 PRoot 没有只读 bind 选项。因此 action-only 不是临时 UI 约定，而是首发安全边界；`/skills`、host Skill 绝对路径和 host-managed output artifact 均不得出现在 PRoot bind argv。

## 4. Session 与 Job 模型

### 4.1 两层身份

```text
CliSession
  id
  scope = agent/topic/manual
  workspace
  environment_snapshot
  runtime_generation

CliJob
  job_id
  session_id
  attempt_id
  command_digest
  state
  process_identity
  started_at / finished_at
  timeout_deadline
  output_cursor / output_bytes / truncated
  exit_code / terminal_reason
```

每次 `action=run` 都创建独立 Bash 进程与进程组，不复用隐藏的长期 shell；因此 `cd`、`export`、alias 和 shell function 不跨调用持续。文件、安装包和 workspace 会持久。P1 人工调用尚无 agent/topic 会话 owner，因此只执行全局并发额度；P2 将 route 与 chat owner 接入后再冻结 `CliSession` 并叠加 per-session 额度。同一路径并发写的冲突由 Runtime 风险门和 Agent 结果承担，不靠共享 shell 偶然串行。人工 PTY 使用独立 `manual` session，关闭终端不得取消 Agent Job。

### 4.2 状态机

```text
queued
  → starting
  → running
      ├─ waiting_user
      ├─ completed
      ├─ failed
      ├─ timed_out
      ├─ cancelled
      └─ interrupted
```

- `completed/failed/timed_out/cancelled/interrupted` 都是终态，不回退 running。
- App 进程死亡后，ledger 中未拿到可信退出回执的 Job 在下次启动统一记为 `interrupted`，不能伪造 completed，也不能自动重跑。
- `waiting_user` 只用于显式授权或人工终端接管；等待期间不持有不必要的 CPU/Wifi lock。
- 每次重建 Runtime/ProcessHost 都增加 `runtime_generation`；旧 reader/event 不能提交到新 Job attempt。

## 5. 取消与超时是硬边界

OpenMinis Android 当前超时只取消等待回调，底层命令仍可能继续。本项目明确否决该行为。

`run` 必须建立可验证的进程树所有权：

1. 启动时获得 process group/session 身份；
2. timeout 或 cancel 先使 attempt generation 失效；
3. 发送 TERM/interrupt，给一个短 grace period；
4. 仍存活则 KILL 整个目标进程组/可识别子孙；
5. join reader 与 wait handle；
6. 丢弃 terminal sequence 之后的迟到 stdout/stderr；
7. exact-once 提交 `timed_out` 或 `cancelled`。

若 Android/PRoot 组合无法可靠定位和终止子进程，Runtime spike 判 `NO-GO`，不能用“UI 已显示取消”掩盖后台仍运行。

用户取消人工 PTY 与取消 Agent Job 是两个目标明确的动作；通知 Stop 也必须携带目标 `job_id`，不得一键误杀所有聊天任务，除非 UI 明确写“停止全部”。

## 6. 输出与内存预算

CLI 输出不能跟随 child stdout 无界堆积：

- stdout/stderr 分通道采集并带单调 sequence；
- 宿主直接持续 drain stdout/stderr 到 App 私有、按 Job/attempt 隔离的有界文件；内存不累积完整输出，前端只保留当前有界 UI tail；
- 输出达到每 Job 落盘上限后停止追加并置 `truncated=true`；终态只暴露绑定 job/attempt 的 opaque artifact handle，不暴露宿主路径；
- `poll` 使用 cursor/最大字节数，不重复发送全部历史；
- 回灌模型的 text 再设独立 Token/字节预算，包含 exit code、截断提示和 artifact handle；
- ANSI/OSC 控制序列先净化，禁止通过终端输出构造 WebView 脚本或伪造 Tauri action；
- env secret、Authorization、私钥样式和用户标注秘密在模型回灌前 redaction；
- 二进制输出不按 UTF-8 强解，返回文件 artifact 与 MIME 摘要。

P0 以以下较宽松的首发预算做真机压测和 golden fixture；它们是资源上限与默认值，不是 Android 后台存活承诺：

| 预算 | 默认 | 硬上限/策略 |
|---|---:|---|
| `run` 前台等待 | 8 秒 | 固定小于 VCP 10 秒通信 timeout；未完成自动返回 Job |
| Job 执行期限 | 30 分钟 | Agent 可请求 1 秒–12 小时；到期必须杀完整进程组 |
| `poll` 单次等待 | 0 秒 | 可请求 0–8 秒长轮询 |
| 单次模型回灌 | 64 KiB UTF-8 字节 | 可请求至 256 KiB；按合法字符边界截断，更大输出必须 cursor 分页或 artifact |
| UI 内存 tail | 每视图 256 KiB | stdout/stderr 分通道；按 cursor 增量去重，达到上限丢弃旧 tail，不 OOM |
| 完整输出 artifact | 每 Job 256 MiB | 超限停止落盘并标记 `truncated`; 不杀仍有价值的计算 |
| CLI workspace | 默认 2 GiB | 用户可在存储设置调整；需保留系统安全余量和一键清理 |
| 并发 | 默认 2 个运行 Job | P0 根据低/中/高端 arm64 真机把硬上限定在 2–4 |

模型回灌预算按字符/字节和模型上下文再取较小值，不能为了“允许 256 KiB”就每次注入 256 KiB。artifact 上限也不包含命令本身产生的 workspace 文件；workspace 受独立总配额管理。12 小时只是用户显式请求时允许的 deadline，应用被系统终止仍记 `interrupted`，不伪装成可靠 daemon。

## 7. 风险与确认门

CLI 是高权限执行面，但低门槛不等于无边界：

| 风险 | 默认行为 |
|---|---|
| 沙箱内只读、查询、编译 | 可按 Agent route 执行，仍受 budget |
| 写 workspace | 允许并在工具块显示变更摘要 |
| 删除/覆盖大量文件 | confirmation gate，显示精确 scope |
| 安装包/下载可执行内容 | 首次确认 + 来源/磁盘提示；发行政策通过前可禁用 |
| SAF 外部目录写入 | 每个 grant 单独确认 |
| 网络上传、凭据使用 | 明确目标和数据范围，不自动泄漏 env |
| Root/Shizuku/系统设置 | 默认不可用；未来独立 elevated backend 与逐次授权 |

确认状态属于 Runtime，不由模型返回“我已确认”绕过。远端 VCP route 也必须经过同一 gate。

## 8. ForegroundGuardian 复用方式

当前前台守护基础设施可以扩展，但不能直接把 CLI 塞进 `start_stream_service_inner(agent_name)`：

- tag 是 `stream:{agent_name}`，同 Agent 多话题会覆盖；
- Kotlin 未知 tag 有默认超时，Rust acquire DTO 目前不传 `timeoutMs`；
- 所有 consumer 当前共同持 CPU 与 Wifi lock，缺少资源需求区分；
- 服务声明、通知和 `stopWithTask=true` 是为远程消息流设计，不等价于本地计算；
- 通知没有按 Job 的 Stop action。

推荐扩展同一个 Guardian，而不是再建第二套锁管理器：

```text
tag = cli:<job_id>:<attempt_id>
kind = cli
timeout_ms = explicit budget
needs_cpu = true while running
needs_network = command/job declared fact
screen_keep_on = false by default
cancel_target = job_id + attempt_id
label = concise user-visible command purpose
```

实施顺序：

1. P1 先只承诺应用前台执行，证明 Runtime/cancel/output；
2. 再扩展 Guardian DTO、通知与 resource arbitration；
3. 核对 Android 14+ foreground service type 与 Play/发行政策；
4. API 26、34、36 与代表 OEM 真机验证 screen-off、划卡、Doze、Stop action；
5. 只有证据通过后才把能力文案从 `foreground_only` 改为 `best_effort_background`。

严禁为了绕开限制声明与真实用途无关的 `mediaPlayback`，也不承诺用户强杀、系统重启或 OEM 清理后继续。

## 9. 人工终端的移动表达

参考 OpenMinis 的交互原则，不照搬视觉：

- 终端为独立页面/Sheet，真实 PTY；
- 打开时不强制弹键盘，点击画布后再唤起；
- 底部紧凑 accessory bar：Ctrl、Esc、Tab、方向键、键盘开关；
- 使用已有键盘 Insets 通道，避免软键盘遮挡；
- 支持 Ctrl+C、窗口 resize、长按选择与复制；
- Agent 给出的 init command 只预填，用户按回车后才执行；
- 顶部显示 session、cwd、shell 与 `LOCAL` 来源，技术字段使用 monospace；
- 实色表面、细分隔线、2px accent，不用内容 blur、大圆角或厚阴影。

Agent Job 默认不自动弹终端；只有 `waiting_user` 或用户主动打开时才切换，避免工具执行抢焦点和软键盘。

## 10. 原生能力的 CLI 门面

在基础 shell 稳定后，可把已有 Android 插件能力映射成 guest 中的 `vcp-*` 命令：

```text
vcp-device
vcp-clipboard
vcp-notification
vcp-file-picker
vcp-network-status
```

guest 命令只负责参数与 stdout/exit code；本地 socket/JNI bridge 调用既有 Kotlin manager。这样 Agent 只学习 CLI，而不需要为每项能力新增远端 VCP manifest。

必须满足：

- socket 仅 App 内可达并校验调用 session；
- argv/env/cwd 有大小上限；
- Android 权限结果映射为稳定 exit code + JSON；
- 高频事件不走一次命令一条 Tauri invoke；
- 不复制 OpenMinis GPL native-offload patch，除非许可证和分发方案明确获批。

## 11. Android 验收核心

1. Distributed 关闭、飞行模式、VCPToolBox 不可达时，本地无网命令仍可执行。
2. 每个 `run` 使用独立 Bash 进程；P1 全局并发受额度约束，P2 接入 chat owner 后再验收同 session/跨 session 额度，cwd/env/output 始终不互相污染。
3. timeout/cancel 后进程树确实退出，随后命令不会收到迟到输出。
4. App 进程死亡后 running Job 记为 interrupted，不自动重跑。
5. 输出超限不会 OOM，模型与 UI 都收到明确截断事实。
6. secret 不进入日志、Toast、VCP payload 或工具块。
7. 前台阶段不申请 Root；后台阶段没有与用途不符的 FGS 类型。
8. 通知 Stop 只取消目标 Job，release 的 lease 与 attempt 精确匹配。

### 11.1 P1 API 36 设备证据（2026-08-14）

当前 OPPO PHZ110（Android 16 / API 36 / arm64-v8a）只闭合人工前台 Runtime，不外推到后台：

- APK `nativeLibraryDir` 中的 PRoot 与 unbundled loader 逐字匹配 profile SHA，产品 App domain 成功执行
  `/bin/bash -lc`，解释器为 GNU Bash 5.3.9、cwd 为 `/workspace`；日志没有 App-data W^X denial。
- airplane mode 开启、Wi-Fi 关闭、active default network 为 none，且 Distributed 为 false 时，本地命令
  completed；`/skills` 下无 canonical Skill bind，`list_skills/read_skill` 返回内置 Skill 且不创建 Job。
- 活动 poll 与普通 cancel 并发时状态为 `cancelled`；absolute deadline 为 `timed_out`；两者父/子 PID
  都从 `/proc` 消失。`setsid`/`nohup` 的启动 PID 与实际 PID 共八个探针均为 gone。
- 400,000 字节输出落入 400,000 字节 opaque artifact；UI/模型读取保持有界。控制序列被移除，Authorization、
  常见 secret 赋值和 PEM body 被遮蔽。
- App 强制停止前记录 generation 4 的 running Job；重启后 generation 5 将其标为 `interrupted`，workspace
  marker 仍恰好一行，证明没有重跑。App 主进程及其 PRoot/sleep 子进程在停止后都消失。

这些证据不证明 Doze、划卡、通知 Stop、FGS 合规、OEM 杀后台或 12 小时长稳；它们仍由 P5 独立验收。
