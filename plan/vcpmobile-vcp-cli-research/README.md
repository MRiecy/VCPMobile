# VCPMobile VCP CLI 本地回环与生态兼容专项

> 状态：`MINIMUM-USABLE-SCOPE / P0-P4.3-CODE-COMPLETE / P4.4-P5-DEFERRED`
>
> 研究日期：2026-08-13
>
> 当前范围：P0 已冻结工具协议、显式工具授权、Android Bash 资产与 manifest 交付；P1 已实现
> `MobileCliRuntime`、Android ProcessHost、人工前台 UI、Skills action 与持久 Job/输出边界，并完成 API 36
> 真机验收；P2 已接入单聊/Group 本地多轮 coordinator、持久恢复、VCP 元字段和 typed route。当前只剩
> 自动 transport guard 与断网续轮的 API 36 最终复验；P3 已完成 Distributed CLI adapter、离线授权与插件中心收口，
> 仍需真实 VCPToolBox + API 36 端到端验收。P4.1 已完成 Skill catalog v2 与显式物化，P4.2 已完成
> 本地 `river=full` 的有界附件 attempt copy；两者仍需 P4.2 API 36 实际 guest 读取/写攻击复验。
> P4.3 已完成本地离线 `river=semantic:N`、确定性 `last:N` 回退和 durable projection。2026-08-14
> 最小收敛裁决将 P4.4 本机知识库/vref 物化与 P5 后台能力正式延期；本地回环把合法 `river/vref`
> 视为可选上下文，无法应用时继续执行命令，并通过同一 ToolResult 给 Agent 一句稳定提示。

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
- 同一 SlidePage 已提供“运行 / Skills / Manifest”：显式按钮启动命令，线性 Job 列表可 poll/cancel，
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
- `ink=mark_history`、`river=text/last:N/full`、`archery=true/no_reply` 已有有界语义；`river` 以单 attempt
  JSON snapshot 暴露到 `/run/vcp-river-context.json`，终态清理。`full` 最多复制 16 个附件、单个 64 MiB、
  总计 256 MiB，并在 `omissions` 中说明省略原因。P4.3 已为 `localLoopback` 打开
  `river=semantic:N`；`vref:N` 当前作为 best-effort 提示被剥离，不再阻塞本地命令。
- 模型单 step 上限 512 KiB、规范 tool payload 256 KiB、工具调用最多 8 次、turn 最长 30 分钟；越界
  以 typed 终态结束，不执行截断后的调用。累计 ToolResult 以稳定 parser blocks 跨 step 投影，最终历史
  与恢复 finalizer byte-stable、exact-once。
- 用户实际 VCPToolBox 已证明普通 `/v1/chat/completions` 会抢执行显式工具块；首个 system 中的
  `[[VCPToolUse=Forbidden]]` 会在模型前被服务端剥离并关闭中央 loop。P2 因而只在 typed
  `localLoopback` 的临时 transport 投影自动加入该标记，`vcpPlugin` 不加入；manifest/Agent prompt
  仍完全由 VCPToolBox 和用户管理。
- API 36 已完成 River 读取/写攻击 truth、projection GC、进程重启恢复和一次真实 Agent 两步 loop：
  ToolResult 可见、最终历史 marker 存在、命令 marker 恰为一行、中央 VCP 摘要不存在。该轮使用部署端
  sentinel fixture；当前产品自动注入 guard 的重装复验与“命令完成后断网再恢复”故障注入仍待手机解锁。

## P3 实施快照（2026-08-14）

- `VCPMobileCLI` 已扫描进现有 Distributed catalog，但 fresh install、旧 disabled-name 配置和新增工具均保持未授权。enabled allowlist 在网络调和前离线加载，所以关闭 Distributed 时插件中心也不会丢失持久授权。
- CLI 只在 Android + `vcpPlugin` route + 显式授权 + Runtime prewarm 成功时发布。`register_tools.tools[]` 通过 `RawValue` 嵌入同一 canonical manifest；route 单独变更只重注册，不重连或自动授权。
- Distributed adapter 只翻译 transport：严格 JSON 先进入 canonical validator，再调用唯一 `MobileCliRuntimeState`。operation/session 身份稳定；断线只丢回包 waiter，不终止已启动 Job。
- `_vcpContext` 仅接受 `execute_tool.data` 顶层的受信任值，128 KiB 上限并纳入重放 digest；`toolArgs` 内伪造字段不会被提升。P3 不物化 river/vref/`file://`，对这些请求在 Runtime 前稳定拒绝。
- 插件中心使用单 mutation owner、view/scan generation 与可访问 `role=switch`。展开未授权行只审阅元数据；只有 Streaming 工具的显式“读取样本”会执行，OneShot/Interactive/CLI 不因浏览产生副作用。
- 当前静态/回归门已通过；`registered_tools` 只表示 manifest frame 已交给当前本地 WebSocket writer，因协议无服务端注册 ACK，不把它写成远端确认。

## P4 实施快照（2026-08-14）

- P4 不再作为一个同时改 Skill、附件、索引和分布式协议的单块功能施工，而是固定为
  `P4.0 能力/投影合同 → P4.1 Skill catalog v2 → P4.2 river=full → P4.3 river=semantic → P4.4 vref`。
  本地与分布式 artifact 物化不是同一批：P4 先闭合本地 attempt copy，远端 hash/size/MIME 传输协议另立验收。
- 当前仓库只有 SQLite FTS5 词法检索；Diary semantic 依赖远端 LightMemo，RAG observer 只观察远端
  VCPInfo。三者均不能作为离线 `semantic`/`vref` 的实现或降级宣传。
- Skill canonical catalog、附件 CAS 和知识源永不挂载进 PRoot。运行时只按稳定 hash 与显式 grant
  复制到 attempt 私有 snapshot；guest 可以修改自己的副本，但没有 write-back，也看不到 host 路径。
  这是一条“source unreachable + non-writeback”边界，不冒充内核只读挂载。
- P4.1 已实现两阶段 Skill ZIP inspect/commit、catalog generation 与 hash/tree 校验；Bash 只有在
  `materialize_skill` 后才能看到 `/workspace/.vcp-skills` 中的非回写副本。
- P4.2 已把消息附件转换为 backend-owned CAS hash descriptor；模型 wire 会移除内部 descriptor，
  local coordinator 复核数据库关系、路径、size 与 SHA-256 后才生成 `AttemptProjectionBundleV1`。
  Android 只逐文件 bind attempt copy 到 `/run/river-artifact-*`，canonical CAS path 不进入 PRoot argv/env。
- 本地语义引擎已用固定 revision 的 64 维多语言 Model2Vec 模型完成 API 36/arm64 真机证伪。
  通用 Hugging Face tokenizer 路径峰值约 237 MiB，不予采用；相同 token 语义经紧凑 BPE pack + mmap
  后，首次进程峰值 18,748 KiB、总时长约 51.8 ms，热运行峰值约 16.5 MiB、总时长约 16.7 ms。
  该证据只批准进入产品实现与回归，不等于索引、召回质量或 App 进程集成已经验收。
- P4.3 产品代码已固定模型、上游 config/tokenizer 输入和紧凑 BPE pack 的 size/SHA-256；Rust 以只读 mmap
  执行 64 维加权池化，SQLite 只保存 `model_id + content hash + unit vector`，不保存 query 或消息正文。
  候选读取按本次 hash 分块，缓存最多 20,000 行并按 LRU/模型代际清理；有限非单位、零或 NaN 向量均
  视为损坏并重算。
- semantic selection 由现有 `LocalEmbeddingOwner` 单 permit 串行拥有，最多工作 60 秒，并贯穿检查 turn
  cancellation 与 30 分钟绝对 deadline。模型、缓存或资产不可用时，先把精确 `fallback_last` projection
  持久绑定到 operation，再启动 Bash；ToolResult 稳定显示 `semantic:N → last:N`，恢复不会重新选取或重跑。
- Android 模型/pack 只在首次有效 semantic 请求时按完整 identity 原子 staging；同进程热路径复用已验证
  identity，semantic 使用独立有界 executor，不阻塞 ProcessHost 的 start/inspect/cancel 控制队列。
- `assembleArm64Debug` 已实际成功；Debug APK 为 84,528,816 bytes。包内 model/tokenizer 分别压缩为
  22,368,109 / 5,666,724 bytes（合计 28,034,833 bytes），解压后的 size 与 profile SHA-256 逐字一致。
  该数字是 Debug 构建证据，不冒充 Release APK 体积。
- P4.4 的知识 catalog/CAS/index/Android bind 与治理 UI 已延期，不进入当前最小可用版本；ADR 仅作为
  未来重启时的设计参考。`localLoopback` 收到合法 `vref:N` 时不物化文件、不阻塞 Bash，只向 Agent
  提示本次未应用；`vcpPlugin`、远端 `file://` 与伪造物化字段继续 fail-closed。

非阻塞后续：P2 上述两项最终设备复验、P3 真实 VCPToolBox/API 36 短命令、长 Job、错误、取消与断网 replay 验收、P4.2 真机 guest 读取/写攻击复验、P4.3 产品设备门。P4.4 vref recall 与 P5 后台前台
服务与人工 PTY 已延期。其中 P4.3 剩余的是 API 26/36 产品 App PSS、低存储、50 次冷/热召回、温升与故障注入
设备门。P1/P2 当前准确能力等级仍是 `foreground_only`，不能据此宣称
Doze 后台长稳。

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
10. **VCP 通用元字段不支配本地命令可用性**。`ink: mark_history`、`river=text/last:N/full/semantic:N`、`archery=true/no_reply` 已在本地 loop 落地；semantic 不可用时回退 `last:N`。本地合法 `river/vref` 无法应用时剥离该可选上下文、继续执行命令，并在 ToolResult 中提示 Agent；语法错误、伪造物化字段和远端不可达引用仍 fail-closed。
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
> 以及 P4.1-P4.3 的 Skill、full/semantic River 产品代码已完成；单聊和 Group 共用可恢复、exact-once 的
> 本地 coordinator。当前前台最小可用范围已经收敛；P2/P3/P4 的列明设备复验仍是发布证据，
> P4.4 vref 物化与 P5 Android 后台/人工 PTY 已延期，不再阻塞当前 CLI 使用。

不得表述为“Agent CLI 已可用”“后台长任务已稳定”“真实 VCP route 已验收”“OpenMinis 代码可直接合并”
或“iOS 已支持本地 Linux”。
