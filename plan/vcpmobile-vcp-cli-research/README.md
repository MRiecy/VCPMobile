# VCPMobile VCP CLI 本地回环与生态兼容专项

> 状态：`P0-P4-CODE-COMPLETE / P5-CODE-COMPLETE-DEVICE-ACCEPTANCE-PENDING / LOCAL-RIVER-VREF-NOT-SUPPORTED`
>
> 研究日期：2026-08-13
>
> 当前范围：P0 已冻结工具协议、显式工具授权、Android Bash 资产与 manifest 交付；P1 已实现
> `MobileCliRuntime`、Android ProcessHost、Jobs UI、Skills action 与持久 Job/输出边界，并完成 API 36
> 真机验收；P2 已接入单聊/Group 本地多轮 coordinator、持久恢复、VCP 元字段和 typed route。当前只剩
> 自动 transport guard 与断网续轮的 API 36 最终复验；P3 已完成 Distributed CLI adapter、离线授权与插件中心收口，
> 仍需真实 VCPToolBox + API 36 端到端验收。P4.1 已完成 Skill catalog v2 与显式物化。2026-08-14
> 最小收敛裁决撤回 P4.2/P4.3 的 River 产品路径并延期 P4.4；本地回环只兼容解析合法
> `river/vref`，不做聊天快照、附件投影、向量召回或文件物化；命令照常执行，并通过同一 ToolResult
> 给 Agent 一句稳定提示。Distributed route 仍严格拒绝这些字段。

## P0 实施快照（2026-08-13）

- `VCPMobileCLI` 的 canonical manifest、VCP block parser、typed action、结果 envelope 与 local/distributed
  两路投影已落地，并由 golden fixture 固定；`list_skills/read_skill` 是独立 action。
- Distributed ToolRegistry 已改为“全 catalog 可见 + enabled allowlist”，fresh install、旧 disabled-name 配置、
  损坏配置与新扫描工具都 fail-closed；关闭全部工具会向服务端发送空注册以撤下旧 manifest。
- Android runtime profile 固定为非 Root App UID、Alpine 3.24.1(musl)、`/bin/bash -lc`、41 个广告命令。
  PRoot 和 72 个增量 APK 均锁定来源/版本/SHA；rootfs 独立重建与仓库资产逐字一致。
- Android 16/API 36/arm64 真机已验证 Bash 解释器、41 命令和负 PGID 整树终止。此证据仅支持前台
  Runtime 开工，不代表 Doze、划卡或后台长稳。
- “更多”托盘已有 VCP CLI manifest 查看、逐字复制和导出入口；Mobile 没有新增 prompt 注入或
  SkillBridge。

## P1 实施快照（2026-08-14）

- Runtime 以 generation、job、attempt 和 durable operation binding 为事实源；`run/poll/cancel/list` 共用同一
  ledger。取消、超时、启动含糊与存储越界都先持久化 attempt 级 terminal intent，再跨 Android bridge
  终止进程树，迟到的 Bash 255 不能抢写 `failed`。
- Android ProcessHost 以 App UID 启动 Alpine 3.24.1 + GNU Bash 5.3.9；PRoot 与独立 loader 均从 APK
  `nativeLibraryDir` 执行，绕开 targetSdk 29+ 对 App 可写目录的 W^X 禁止。guest 只 bind `/dev`、`/proc`
  和 `/workspace`，不 bind Skill catalog、host artifact 或 `/sys`。
- stdout/stderr 持续 drain 到 App 私有有界文件，cursor 按 attempt 校验；UI 每视图只保留 256 KiB tail。
  终态 artifact 是 opaque `vcp-cli-output-pair.v1` 引用，不暴露宿主路径；ANSI/OSC/C0/C1 与常见凭据形态
  在展示/回灌投影前净化。
- P1 当时的同一 SlidePage 已提供结构化运行、Skills 与 Manifest；P5 已将其收敛为
  `终端 | Jobs | Skills`，Manifest 进入 Info 二级页。Jobs 中显式按钮启动命令，线性列表可 poll/cancel；
  `list_skills/read_skill` 在 Rust catalog 内用 `openat + O_NOFOLLOW` 读取，返回 `vcp-skill://<id>`。
- API 36/arm64 真机已证明：Distributed 关闭且无活动网络时 Bash 与 Skills 可用；普通 cancel、absolute
  timeout、`setsid`、`nohup` 的父子 PID 全部消失且终态分别为 `cancelled/timed_out`；强制停止 Debug App
  后旧 Job 在 generation 前移时记 `interrupted`，marker 仍为一行，未重跑。

## P2 实施快照（2026-08-14）

- `mobileCliAgentRoute` 是闭合的 `localLoopback | vcpPlugin` wire，缺失字段默认本地；一次 turn 开始后冻结，
  旧 `enableVcpToolInjection` 不再反推 CLI route。设置页切换远端只做只读 preflight，不替用户开启
  Distributed、写凭据或授权工具。
- 单聊与 Group 在同一可见 assistant skeleton、同一 `ActiveRequestLease` 和唯一 finalizer 内运行多 step
  coordinator。每步使用独立 transport ID，UI 以 `turnAttempt/stepIndex/projectionReset` 拒绝旧帧，
  中间 Aurora 不发布假终态。
- SQLite local-turn ledger 在执行前原子 claim 完整闭合 batch，按 stable operation ID 复用 Runtime 幂等；
  工具结果先持久化再续模型。断网/进程回调含糊时保留 `continuation_pending`，恢复入口早于旧 helper，
  已完成命令不重跑。
- `ink=mark_history`、`archery=true/no_reply` 由本地 meta owner 执行。`river/vref` 仅保留协议解析和
  raw tool digest 兼容，既不进入 Bash，也不生成 snapshot、附件副本或语义选择；同一调用最多向 Agent
  返回一条“未应用可选上下文”的提示。
- 模型单 step 上限 512 KiB、规范 tool payload 256 KiB、工具调用最多 8 次、turn 最长 30 分钟；越界
  以 typed 终态结束，不执行截断后的调用。累计 ToolResult 以稳定 parser blocks 跨 step 投影，最终历史
  与恢复 finalizer byte-stable、exact-once。
- 用户实际 VCPToolBox 已证明普通 `/v1/chat/completions` 会抢执行显式工具块；首个 system 中的
  `[[VCPToolUse=Forbidden]]` 会在模型前被服务端剥离并关闭中央 loop。P2 因而只在 typed
  `localLoopback` 的临时 transport 投影自动加入该标记，`vcpPlugin` 不加入；manifest/Agent prompt
  仍完全由 VCPToolBox 和用户管理。
- API 36 已完成一次真实 Agent 两步 loop：ToolResult 可见、最终历史 marker 存在、命令 marker 恰为一行、
  中央 VCP 摘要不存在。历史 River 投影实验的读取/写攻击证据不再属于当前产品验收；当前仍待自动
  transport guard 重装复验与“命令完成后断网再恢复”故障注入。

## P3 实施快照（2026-08-14）

- `VCPMobileCLI` 已扫描进现有 Distributed catalog，但 fresh install、旧 disabled-name 配置和新增工具均保持未授权。enabled allowlist 在网络调和前离线加载，所以关闭 Distributed 时插件中心也不会丢失持久授权。
- CLI 只在 Android + `vcpPlugin` route + 显式授权 + Runtime prewarm 成功时发布。`register_tools.tools[]` 通过 `RawValue` 嵌入同一 canonical manifest；route 单独变更只重注册，不重连或自动授权。
- Distributed adapter 只翻译 transport：严格 JSON 先进入 canonical validator，再调用唯一 `MobileCliRuntimeState`。operation/session 身份稳定；断线只丢回包 waiter，不终止已启动 Job。
- `_vcpContext` 仅接受 `execute_tool.data` 顶层的受信任值，128 KiB 上限并纳入重放 digest；`toolArgs` 内伪造字段不会被提升。P3 不物化 river/vref/`file://`，对这些请求在 Runtime 前稳定拒绝。
- 插件中心使用单 mutation owner、view/scan generation 与可访问 `role=switch`。展开未授权行只审阅元数据；只有 Streaming 工具的显式“读取样本”会执行，OneShot/Interactive/CLI 不因浏览产生副作用。
- 当前静态/回归门已通过；`registered_tools` 只表示 manifest frame 已交给当前本地 WebSocket writer，因协议无服务端注册 ACK，不把它写成远端确认。

## P4 当前裁决（2026-08-14）

- P4.1 Skill catalog v2 与显式物化保留；它直接服务 CLI 任务，且不把 canonical catalog 挂载进 Bash。
- P4.2 River text/last/full 与 P4.3 semantic 的实验实现已从产品路径回退：聊天记录、附件和本地向量
  不会投影给 Bash，Android 不再打包语义模型，Runtime/ProcessHost 不再接受 River projection。
- P4.4 vref 知识物化不进入当前产品。协议仍严格解析 river/vref，便于接收旧请求并保持 tool digest；
  localLoopback 始终忽略并提示，vcpPlugin 始终 fail-closed。
- 旧 turn/job ledger 的 River DTO、路径字段与有界 GC 暂留一个兼容周期，只用于读取和清理由旧 Debug
  构建产生的数据，不构成新能力。`0008_create_local_semantic_cache.sql` 保持原字节以维护 SQLx checksum；
  `0009` 在兼容执行 `0008` 后删除该派生缓存，新代码不再读写它。

## P5 代码快照（2026-08-14）

- Agent Job 仍由 Rust ledger 唯一持久拥有；Kotlin ProcessHost 改为应用进程 singleton，plugin/Activity
  重建不再杀死全部 Job。每个 Job 使用 generation-fenced `cli:<job>:<attempt>` Guardian lease，CPU/Wi-Fi
  需求分离，并在 FGS readiness 成功后才启动命令。
- 同一 FGS 壳按活动消费者选择 `remoteMessaging`/`specialUse`，CLI 使用独立低打扰通知；每 Job 通知只含
  脱敏短标签，`停止…` 先打开准确 Job 并二次确认。stale generation/attempt 不会取消新任务；通知权限
  不可用时降级为原有前台 Job，而不是禁用 Bash。
- FGS 意外消失会精确终止受影响进程树，并让 Rust 记录 `interrupted`；inspect/cancel 已从单一控制队列
  拆出，某个 Job 的 drain 不再阻塞其他 Job 查询/取消。子进程继承 4 GiB `RLIMIT_AS`（bionic linker 的
  ~2 GiB CFI shadow 使 512 MiB 无法 exec 主机 PRoot，2026-08-14 API 36 复验修正），输出仍受原有
  256 MiB/job 落盘上限约束。
- CLI 一级导航改为 `终端 | Jobs | Skills`，默认进入独立人工终端；Manifest 移入标题栏 Info 二级页。
  终端由 xterm.js + arm64 `libvcp_pty.so` 实现真实 PTY，包含 session/PGID、login Bash、cursor long-poll、
  16 KiB 写/64 KiB 读、resize/SIGWINCH、Ctrl/Esc/Tab/方向键与 IME Insets。离开页面只 detach，明确
  “结束会话…”才终止 PTY 进程树，且不操作 Agent Job。
- PRoot/rootfs 的既有 GPL/LGPL/Alpine 义务继续保留；xterm.js MIT 与仓库自有 PTY helper 已进入发行清单。
  未复制 OpenMinis native-offload；`vcp-*` 原生命令桥仍按“基础 shell 稳定后逐项审计”处理，不为 P5
  首版扩张权限面。

非阻塞后续：P2/P3 列明设备复验，以及 P5 的 API 26/34/36、Doze、锁屏、划卡、通知 Stop、IME/旋转
和代表 OEM 真机旅程。设备证据完成前，对外能力等级仍冻结为 `foreground_only`，不能据此宣称后台长稳。

## 结论

VCPMobile 的 CLI 不应把“始终连接 VCP 插件中心”作为默认生存条件。推荐建立一个由移动端自己拥有的 `VCPMobileCLI` 能力，默认走**进程内本地回环**；只有用户明确开启时，才把同一能力作为分布式 VCP 插件注册给 VCPToolBox。

```text
同一份 VCPMobileCLI manifest、请求语法和结果语义
                         │
                  Agent route
             ┌───────────┴───────────┐
             │                       │
 localLoopback（默认）        vcpPlugin（显式开启）
 Mobile 本地工具循环          VCPToolBox 工具循环
 进程内直调 Runtime           既有 Distributed WS
             │                       │
             └──── 同一 MobileCliRuntime ────┘
```

这不是“在手机里伪造一个 VCP 服务器”。本地回环不启动 localhost HTTP、SSE 或 WebSocket，而是让本地执行结果经过与 VCP 一致的规范化器，再以 `<!-- VCP_TOOL_PAYLOAD -->` 回灌模型续轮，同时向 UI 发送可渲染的工具状态。这样让 Agent 只需理解 manifest 中自洽的移动 CLI 合同，也避开插件中心的连接、注册和多次网络往返。

## 已冻结决策

1. **默认路由是 `localLoopback`**。它不依赖 Distributed WebSocket，飞行模式下也能运行不需要网络的本地命令。
2. **分布式工具采用“显式扫描 + 显式授权”，默认全部关闭**。Registry 扫描得到完整工具清单，UI 对每个工具单独展示和授权；后端只持久化 `enabled_tools` allowlist，未在 allowlist 的工具一律不发布、不执行。干净安装、旧配置升级和新扫描出的工具都保持关闭。
3. **一个工具身份、一个 manifest、一个 Runtime**。本地和远端只替换 transport/turn adapter，不复制 shell、job、工具说明或结果状态机。
4. **manifest 是主要提示词源**。`invocationCommands[].description/example` 完整说明 Shell、参数、限制和示例，由 VCPToolBox 提取并允许用户手动微调；Mobile 不另建 `CliPromptCatalog`，也不改写 Agent 提示词。
5. **本地回环不等于绕过 VCPToolBox 的提示词治理**。本地 route 只改变工具执行 owner；用户仍在 VCPToolBox 侧决定如何通过 `{{VCPVCPMobileCLI}}`、DynamicTools 或自定义提示词让 Agent 看见能力。
   因真实插件默认关闭，本地 route 首次使用前需要用户把 Mobile 导出的规范 manifest/说明导入或放置到 VCPToolBox 提示词配置中；本专项不再宣称 Agent CLI 是零配置提示词能力。
6. **本地模式与 VCP 模式只有一个工具循环 owner**。本地模式由 Mobile 截获 VCP block；VCP 模式由 VCPToolBox 截获。禁止双重执行和断线时静默换路由。
7. **人工终端与 Agent Job 分离**。普通任务用 `run/poll/cancel/list`；密码、SSH、vim、TUI 等场景由用户显式打开交互终端，预填命令但不自动执行。
8. **Android host 坚持非 Root 用户态沙箱**。首发目标 Shell 固定为 PRoot guest 内的 Alpine Linux (musl) + GNU Bash，Agent 命令以 `/bin/bash -lc` 执行；guest 可模拟 root 管理自身 rootfs 和 `apk`，但不能越过 App UID。P0 验证可行性；若不可行必须回到 manifest 重新裁决，不得静默改用 `ash`。Android Root/Shizuku 只能作为未来显式 elevated backend。
9. **长任务由 job 生命周期拥有，不由聊天 SSE 拥有**。模型续轮断线不能导致命令重跑；超时/取消必须终止目标进程组并阻止迟到输出污染后续任务。
10. **VCP 通用元字段不支配本地命令可用性**。本地只执行 `ink: mark_history` 与 `archery=true/no_reply`；合法 `river/vref` 始终剥离、继续命令并在 ToolResult 中提示 Agent，不建立本地召回能力。语法错误、伪造物化字段和远端不可达引用仍 fail-closed。
11. **Skill 是 manifest 中可见的一等 action，不是隐藏 Shell 子命令**。`VCPMobileCLI` 直接提供 `action=list_skills|read_skill`；前者列出校验通过的 Skill，后者按 `skill_id + resource_path` 阅读 `SKILL.md` 或受控资源并返回 `vcp-skill://<id>` 逻辑引用。Skill catalog 不挂载进 PRoot guest；若要执行脚本，必须先经明确动作物化到 `/workspace`。它不增加第二个 VCP 工具，也不自动执行 Skill 脚本。
12. **iOS 只作为未来可选方向**。同一 VCP 协议可以复用，CLI Runtime 必须另做平台实现；本专项不改变 Android-only 当前产品边界。

## 为什么不是直接照搬 VChat 或 OpenMinis

- VChat 的人工入口会直接打开本机 `PowerShellExecutor` GUI；Agent 路径则通过 Distributed WebSocket 把同一插件注册给 VCPToolBox。它没有解决手机弱连接下的默认离线闭环。
- OpenMinis 已证明“本地沙箱 + Agent 工具 + 独立人工 PTY”在移动端可行，也提供了会话级长期 shell、Skills 目录与原生能力 CLI 化的参考；但其 Android 普通超时目前只停止等待，不终止底层命令，不能照搬。
- VCPToolBox 的价值在于统一文本协议、工具结果回灌、动态工具说明、记忆和上下文编排。移动端应兼容这些语义，不需要在本机重建整套 VCP 服务端。

## 文档导航

| 文档 | 内容 | 用途 |
|---|---|---|
| [01-上游事实与当前缺口.md](./01-上游事实与当前缺口.md) | VChat、VCPToolBox、OpenMinis、VCPMobile 的真实调用链和可复用面 | 防止凭印象移植 |
| [02-统一协议与双路由架构.md](./02-统一协议与双路由架构.md) | 独立工具名、Alpine Bash 命令集、规范 manifest、四层结果与互斥路由 | 施工主契约 |
| [03-Android本地运行时与长任务.md](./03-Android本地运行时与长任务.md) | 沙箱、job/session、取消、输出、ForegroundGuardian 和人工终端 | Android Runtime 设计 |
| [04-提示词Skills记忆与召回语义.md](./04-提示词Skills记忆与召回语义.md) | manifest 提示词所有权、Skill 文件边界、ink/river/vref | VCPToolBox 提示词治理与高级语义 |
| [05-iOS未来可选方向.md](./05-iOS未来可选方向.md) | 原生命令、a-Shell/WASI、iSH、后台策略与 App Review 边界 | 后续预研，不进入当前施工 |
| [06-Magi综合裁决与分期验收.md](./06-Magi综合裁决与分期验收.md) | 三方审查、否决线、P0–P5 路线和硬验收 | 开工与交付门禁 |
| [07-P4.4本机知识授权与vref合同.md](./07-P4.4本机知识授权与vref合同.md) | 独立知识 CAS、显式授权、召回、attempt copy、删除和预算 | 已延期的未来参考 |
| [08-本地回环收敛与保活分层ADR.md](./08-本地回环收敛与保活分层ADR.md) | localLoopback 前提证伪、保活三层、收敛为 vcpPlugin 单路由的提案 | 待核准的架构收敛（**提案**） |
| [09-真机验收与回归修复存档-2026-08-14.md](./09-真机验收与回归修复存档-2026-08-14.md) | RLIMIT_AS 回归修复 + API 36 自动化验收 + VCPToolBox 源码核证 | 本轮实测证据存档 |

## 证据快照

本研究固定到以下提交；未来施工前需要重新核对官方 HEAD 和用户实际部署：

| 项目 | 快照 |
|---|---|
| VCPMobile | `b34406066b4178f0bccb01a9b6bb89839aecda2d` |
| VCPChat 官方 `main` | `8a65eb780974eed93da449167f4385d16f8aa1ab` |
| VCPToolBox 官方 `main` | `311dc42e8374afd1867bd1b5c06217baf8b0f463` |
| OpenMinis 官方 `main` | `9cf3a855fecd27bb5735b84cacbd56852a3ab8dd` |

证据优先级：实际部署脱敏 fixture → 官方固定提交源码 → 当前 VCPMobile 源码/测试 → 本专项裁决 → README 或产品描述。用户实际 Agent 使用 `{{VCPPowerShellExecutor}}`、`{{VCPDynamicTools}}` 还是 `{{VCPAllTools}}` 尚未取证，不能在实现中假定。

## 当前完成定义

允许的准确表述是：

> VCPMobile CLI 的 P0 协议、P1 人工前台 Runtime、P2 本地 Agent 多轮 loop、P3 可选 Distributed adapter
> 以及 P4.1 Skill 产品代码已完成；单聊和 Group 共用可恢复、exact-once 的本地 coordinator。
> River/vref 仅作协议兼容解析，不是本地 CLI capability；本地向量模型、投影和知识物化均不随产品交付。
> 当前代码范围已经收敛到 P5；P2/P3/P5 的列明设备复验仍是发布证据，后台能力声明保持 `foreground_only`。

不得表述为“Agent CLI 已可用”“后台长任务已稳定”“真实 VCP route 已验收”“OpenMinis 代码可直接合并”
或“iOS 已支持本地 Linux”。
