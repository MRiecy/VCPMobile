# 06｜Magi 综合裁决与分期验收

> 审查日期：2026-08-13
>
> 方法：Melchior、Balthasar、Casper 分别只读检查 VCP/VChat 协议、移动交互/iOS、当前 VCPMobile 交付边界，再由主审综合。三方未修改参考工程或产品代码。
>
> 追加裁决：用户随后明确以 VCPToolBox/manifest 为提示词真源、删除 Mobile PromptCatalog，并要求独立工具语义、明确 Bash 命令集和更宽松预算；Skill 读取进一步提升为 manifest 一等 action，桌面端 Skill 注入方案完全排除。下文将“原审建议”和“用户最终裁决”分开记录；用户裁决优先。
>
> 2026-08-14 覆盖裁决：本地 CLI 不支持 River/vref 投影或向量召回；合法字段只作协议兼容解析并
> 提示忽略。此前 P4.2/P4.3 的实现与设备证据属于已撤回实验，不再构成产品能力或验收路线。

## 1. 综合裁决

Magi 给出：

```text
PASS FOR P0 PROTOCOL + RUNTIME SPIKE
NOT YET PASS FOR FEATURE CONSTRUCTION
```

结合用户最终裁决，方向已经足够明确：默认本地回环、真实 VCP 插件显式开启、同一工具身份/manifest/Runtime、manifest 由 VCPToolBox 管理提示词、人工终端与 Agent Job 分离。正式施工前仍必须用 P0 关闭三个高风险未知：Android PRoot/Bash/PTY/进程组可行性、第三方许可证/包体、用户实际 VCP 部署对禁用中央工具 loop 与结果格式的兼容。

## 2. 三方审查

### 2.1 Melchior｜逻辑与系统

确认：

- VChat Chat HTTP 与 Distributed WS 是两条链；本机 CLI 执行发生在插件 Runtime，不在聊天 WebView。
- 本地 VCP 兼容必须同时实现模型 `VCP_TOOL_PAYLOAD` 回灌和 UI VCPInfo 展示；只伪造结果卡片无法续轮。
- `ink=mark_history`、`river`、`vref`、`archery`、DynamicTools/`vcp_fold` 属于不同层，不能合称一个模糊“召回开关”。
- 当前 VCPMobile 已有 wire DTO/Distributed client，但没有本地多 step turn owner。
- 远端回包需绑定 connection epoch/server/request，Job 与模型续轮都需幂等 ledger。

系统否决线：

- 用正则扫描半截 SSE 并立即执行；
- 本地与 VCPToolBox 同时拥有工具循环；
- 工具完成后断网就重跑命令；
- timeout 只停止等待、底层进程继续；
- 把 `requestId` 当唯一信任边界；
- 未实现的 `river/vref/archery` 进入 shell argv，或在本地软降级时没有向 Agent 提示；
- UI 卡片被当成模型结果真源。

### 2.2 Balthasar｜移动直觉与 iOS

确认：

- OpenMinis 最值得复用的是“结构化 Agent 命令 + 独立人工 PTY”的交互分工。
- Agent 不应自动弹终端或执行预填的密码/TUI 命令；用户主动接管才进入终端。
- UI 只需显示一个工具身份，来源用低干扰 `LOCAL`/`VCP` 标识；不让 Agent 学两套名称。
- 终端需要移动键盘 accessory、真实 Insets、Ctrl/Esc/Tab/方向键、选择复制，不照搬其大圆按钮视觉。
- iOS 与 Android 底层可以不同，但上层 VCP/Job 合同应相同；iOS 后台必须使用公开系统能力，不能复制静音音频/虚假定位保活。

体验否决线：

- Agent 普通执行每次都抢焦点、弹键盘或打开终端；
- 用户看不到执行位置、Job 状态、取消目标或输出是否截断；
- `VCP` 断线后悄悄改在手机执行同一破坏性命令；
- 终端内容区毛玻璃、大圆角、厚阴影、按压缩放；
- 用“后台可靠”掩盖 Android/iOS 的系统终止事实。

### 2.3 Casper｜务实与交付

确认：

- 原审建议一个 PromptCatalog、一个 Protocol、一个 Runtime、两个 adapter，并把 PromptCatalog 放入 context assembler；不要 localhost server 或第二套 registry。
- 用户后续只覆盖提示词部分：改为一个规范 manifest、一个 Protocol、一个 Runtime、两个 adapter；manifest 由 VCPToolBox/用户管理，Mobile 不新增 PromptCatalog 或 context-assembler 注入。其余最小架构结论保留。
- ToolRegistry 的 disabled-name 取反迁移会让升级用户的新工具意外启用；必须改为全量扫描 catalog + 默认空 enabled allowlist，UI 对每个工具逐个授权。
- 当前 ForegroundGuardian 的 stream tag、默认 timeout、统一 CPU/Wifi lock 和通知语义不足以直接承载 CLI。
- 现有 Root command 缺少 exit/stderr/timeout/cancel，不应充当默认 Runtime。
- 文档阶段只需静态审计；产品代码阶段每个跨层变更必须按项目门禁检查。

交付否决线：

- 一开始同时落地 PRoot、完整包管理、Skills 商店、MCP、iOS、Root 和远端 VCP；
- 为本地回环新建 localhost HTTP/SSE/WS；
- local adapter 也经过 ToolRegistry enabled 开关；
- 新增第二个前台守护服务或第二套 Job store；
- 只测 clean install，不测旧配置升级；
- JVM/host 测试代替 Android 真机 screen-off/Doze/OEM 证据。

## 3. 主要分歧与用户最终裁决

### A. 工具名与 Shell 是否借用其他执行器概念

- 复用旧名或“沿用某执行器”措辞要求 Agent 理解未提供的项目历史，还会掩盖 Android 命令集差异。
- 独立命名与完整 Shell 事实能让 manifest 自洽，也允许按本工具实际行为演进。

**裁决**：规范名 `VCPMobileCLI`，不提供旧名 alias。Android manifest 明确写 PRoot/Alpine(musl)、`/bin/bash -lc`、`apk`、命令基线和非持久 shell state，不提其他执行器。

### B. 本地回环是否通过 ToolRegistry

- 经过 registry 看似“复用插件系统”。
- 但 registry 是远端发布/授权面，耦合后插件关闭会错误杀死本地默认能力，并制造两套状态。

**裁决**：本地 adapter 直接调用 `MobileCliRuntime`；远端 adapter 才通过 Distributed ToolRegistry 发布同一 Runtime。

### C. 谁拥有 CLI 工具提示词

- Mobile 自行注入可在本地 route 下强制可见，但会与 VCPToolBox manifest 提取和用户手工配置形成双真源。
- 完全交给 VCPToolBox 符合现有 VCP 提示词治理，也让用户能在一个位置微调 Agent 行为。

**裁决**：manifest description/example 是唯一规范工具说明；VCPToolBox/用户决定注入方式。Mobile 不改 Agent prompt；route 不可用时以稳定工具错误返回真实状态。

### D. Android 是否直接照搬 OpenMinis PRoot

- OpenMinis 已验证架构可行并有丰富功能。
- 其 GPLv3、native patch、包体、Android FGS 发行策略和 timeout 缺陷都不能直接继承。

**裁决**：把 PRoot + Alpine 当 Runtime 候选与行为参考；P0 自主 spike 与许可证审计后再定依赖/实现来源。

### E. 首期是否承诺后台长任务

- 手机 CLI 的价值包含长任务。
- 当前 Guardian 与 Process owner 还不足以证明可靠取消、FGS 合规和 OEM 行为。

**裁决**：P1 先交付前台、可取消、有界 Job；P5 有真机证据后再升级为 Android best-effort background。Job API 从 P1 就支持异步，使后续不改 Agent 协议。

## 4. 分期路线

### P0｜协议、运行时与证据冻结

实施状态（2026-08-13）：`CODE + API36 DEVICE EVIDENCE COMPLETE`。协议/manifest/结果 golden、
ToolRegistry allowlist、可复现 PRoot/rootfs、许可证与包体记录、API 36 命令探测和整树取消已通过。
用户实际 VCPToolBox 的提示词微调仍由用户拥有；central-loop fixture 已在 P2 设备验收中补齐：普通
`/v1/chat/completions` 会执行显式工具块，而首个 system 中的 `[[VCPToolUse=Forbidden]]` 会在模型前
被剥离并关闭中央 parser/executor。这不把 manifest 或提示词所有权转交 Mobile，也不证明真实
Distributed adapter 已完成。

交付：

- 固定 `VCPMobileCLI` manifest/action/request/result golden fixtures，其中 `list_skills/read_skill` 必须作为 action 出现而非内部 Shell 命令；
- 冻结 Mobile manifest 查看/复制/导出格式，以及 VCPToolBox 本地 route 的一次性导入/放置指引；
- 冻结 Distributed `ScannedToolCatalog + EnabledToolNames` 配置 schema、旧 disabled-name 失效迁移提示与默认全关策略；
- 冻结 PRoot/Alpine(musl) + `/bin/bash -lc` Shell 合同、`command-profile.json` 和首发基线命令实机探测；
- 从 VCPToolBox 固定提交移植或独立实现 parser fixture：marker、escape、think、保留字段；
- 用用户实际部署验证 `[[VCPToolUse=Forbidden]]`、普通/chatvcp route、VCP_TOOL_PAYLOAD 和 VCPInfo；
- PRoot/Alpine/Bash/PTY 最小 spike：启动、cwd、stdout/stderr、process group kill；
- 列出 PRoot、rootfs、Bash/Alpine 包、PTY/offload 的许可证与 APK 增量；
- 冻结命令/输出/并发/磁盘/超时初值和确认矩阵；
- 施工前按仓库存档协议 checkpoint。

硬验收：

- parser golden fixtures 与上游一致；
- manifest 声明的每个基线命令都由 arm64 rootfs 探测通过；实际解释器确为 `/bin/bash`，无 `ash` 静默降级；
- manifest snapshot 明确包含 `list_skills/read_skill` 字段与示例，且 rootfs 不需要隐藏的 Skill 管理命令；
- Mobile 导出 manifest 与 Distributed 注册 manifest 逐字一致；local route 未连接 WS 时也能按指引完成 VCPToolBox 提示词配置；
- 扫描可展示全部工具，但 fresh install/旧配置升级的 allowlist 都为空；新扫描工具不改变旧授权并保持关闭；
- 同一规范结果可编码成本地 payload 与 distributed tool_result；
- timeout/cancel spike 能证明整个进程树退出；
- 部署端禁用中央 loop 后仍保留所需记忆/变量预处理；
- 法律/包体/Android 可执行限制没有未记录 blocker。

若进程树无法可靠杀、许可证不可接受或中央 VCP 无法让出 loop，状态为 `BLOCKED-P0`，不得继续堆 UI。

### P1｜MobileCliRuntime + 人工前台调用（完成，2026-08-14）

交付：

- Android host 非 Root 的 PRoot sandbox、持久 workspace、`run/poll/cancel/list`；guest 可模拟 root 管理隔离 rootfs；P1 人工调用只有 runtime 级 owner，agent/topic session 由 P2 接入；
- Rust Job ledger + Android ProcessHost；
- App 私有有界输出文件、opaque artifact/cursor/redaction 与前端有界 tail；
- `list_skills/read_skill` 一等 action 与 Rust 侧受控、action-only Skill catalog 读取桥；Skill 不 bind 进 guest；
- 简单 CLI 页面和 Job 列表；人工 PTY 可放在 P1 末或 P5 前，但不阻塞结构化命令；
- 前台运行能力 truth。

硬验收：

- Distributed off + 飞行模式执行本地无网命令；
- timeout/cancel 无残留进程和迟到输出；
- 每个 Job 的 cwd/env/output 独立，P1 全局并发受额度约束；P2 冻结 agent/topic session 后再叠加 per-session 额度；
- 进程死亡后状态为 interrupted；
- 无 Android Root/Shizuku、无 API key/env 泄漏、无 OOM；guest 模拟 root 不越过 App UID；
- `list_skills/read_skill` 可在飞行模式列出和阅读 Skill，调用不创建伪 Job，不能路径越界或因阅读自动执行脚本；PRoot argv 不含 `/skills` 或 host Skill 路径，返回值只含 `vcp-skill://<id>` 逻辑引用。

设备结论：API 36 已按上述门禁通过。普通 cancel、absolute timeout、`setsid/nohup` 清树、并发隔离、
opaque artifact、无网 Bash/Skill、进程死亡后 `interrupted + no-rerun` 都有 Debug 产品路径证据；能力 truth
仍为 `foreground_only`，人工 PTY 延后且不影响结构化 P1 完成。

### P2｜默认本地 Agent 工具循环

实施状态（2026-08-14）：`CODE COMPLETE / API36 FINAL GUARD-AND-RECOVERY RETEST PENDING`。typed route、
单聊/Group coordinator、SQLite turn ledger、累计 ToolResult、元字段、恢复/finalizer 与预算门均已实现。
API 36 已完成真实两步 Agent loop、exact-once marker 和进程重启恢复；产品自动 transport guard 与命令完成后
断网续轮的最后两项故障注入因设备锁屏暂停，不能用 host/JVM 结果冒充真机完成。

交付：

- VCPToolBox 从 golden manifest 提取工具说明，用户在 VCPToolBox 侧完成 local-route 提示词配置；
- typed route 默认 `local_loopback`；
- LocalVcpTurnOwner 多 step 模型续轮；
- LocalVcpMetaProcessor：执行 `ink=mark_history`、`archery=true/no_reply`，并剥离/提示 river/vref；
- assistant request + `VCP_TOOL_PAYLOAD` 回灌；
- step/digest ledger、pending continuation、max-step；
- 单聊与 Group 同时接入，或 Group 显式 capability off。

硬验收：

- fresh install 默认 local，WS 从未因 CLI 启动；
- SSE 分帧/replay/重试不重复执行；
- 命令完成后断网，恢复时只续模型、不重跑命令；
- 工具错误可被 Agent 修正，安全拒绝不可绕过；
- local 模式中央 VCPToolBox 不抢执行；
- finalizer exact-once，历史无重复 assistant/result；
- 合法 river/vref 不生成 projection、不进入 Bash，local 命令继续且最多一条提示；remote 仍明确 `unsupported_mode`。

### P3｜可选真实 VCP 插件

交付：

- Distributed adapter 复用同一 Runtime/Protocol；
- 扫描 catalog、逐工具 enabled allowlist 与版本化授权策略；
- route preflight、online epoch、manifest 发布/撤下；
- `vref_files` 引用物化能力协商；只有 VCPToolBox 主机 `file://` 路径时保持 unsupported，不空想跨机可读；
- requestId 对应 cancel 能力若宣称支持则真实实现。

硬验收：

- clean install 与旧配置升级均扫描可见但全部未授权/不注册；
- 开启后只注册一份，关闭/断线后撤下并清零状态；
- local route 不受 registry/WS 影响；
- VCP 断线不杀已启动异步 Job，重连可 poll；
- 伪造/不可达的 `file://` vref 不进入 guest；若未落地安全物化协议，能力响应明确为 unsupported；
- 不做静默 local fallback；
- 真实 VCPToolBox 端到端短命令、长 Job、错误和取消 fixture 通过。

2026-08-14 实施状态：代码与回归门已完成，真实 VCPToolBox + API 36 端到端仍待验收。当前 adapter 对不可达 `file://`、river 物化与 vref 稳定返回 unsupported，未宣称 requestId 外的远程 cancel wire。已启动 Job 由唯一 Mobile Runtime 拥有，WebSocket session 只持有回包 waiter。

### P4｜Skill 保留，River/vref 本地能力撤回

2026-08-14 的覆盖裁决优先于此前 P4.2/P4.3 施工记录：

- P4.1 Skill catalog v2、显式 inspect/commit/materialize 与 non-writeback workspace 副本保留；
- River `text/last/full/semantic` 与 vref 只保留 protocol parser、严格范围校验和 raw digest；
- localLoopback 始终忽略并合并提示，Runtime/ProcessHost 不接收 projection，Bash 无专用 env/bind；
- vcpPlugin 的 river/vref、远端 `file://`、伪造 `river_context/vref_files` 继续 fail-closed；
- 本地 embedding 模型、tokenizer、缓存执行、Android staging 和约 35 MiB 原始资产全部移除；
- 旧 turn/job ledger DTO、projection path 与有界 GC 暂留一个兼容周期，仅负责解码和清理旧数据；
- P4.4 ADR 降为历史参考，不代表未来承诺，也不在 CLI UI 增加“本地知识”入口。

当前 P4 代码门已闭合：合法 river/vref 不阻塞 local 命令且最多一条 notice，字段不进入 action/argv/env；
非法字段继续拒绝；remote 保持 unsupported；Release APK 门明确拒绝 semantic 资产；旧 ledger 仍可解码和
清理。`0008` 保持既有 checksum，`0009` 只删除已退休的派生缓存表。

### P5｜Android best-effort 后台、人工 PTY 与原生命令桥

交付：

- 扩展现有 ForegroundGuardian 的 `cli:<job>:<attempt>` lease、显式 timeout/资源需求；
- 按 Job 通知与 Stop action；
- screen-off/Doze/划卡/回前台 reconcile；
- 完整人工 PTY 交互；
- 审计通过后逐步提供 `vcp-*` Android 原生命令。

硬验收：

- API 26/34/36 与代表 OEM 真机旅程；
- screen-off 后输出连续，通知 Stop 只取消目标 Job；
- 无 stale lease、无常驻空通知、无错误 FGS 类型；
- 进程被系统终止时 UI/ledger 诚实标记 interrupted；
- 终端关闭不误杀 Agent Job，Agent Job 不抢终端焦点；
- 原生命令权限拒绝返回稳定 JSON/exit code。

## 5. 建议代码落点

仅作为 P0 后施工导航，不要求本轮创建空壳：

```text
src-tauri/src/vcp_modules/cli/
  protocol.rs
  manifest.rs
  runtime.rs
  job_store.rs
  local_turn_coordinator.rs
  adapters/{local,distributed}.rs

src/features/cli/
  components/
  stores/
  types.ts

src-tauri/plugins/vcp-mobile/
  src/cli.rs
  android/.../cli/
```

若创建模块目录或修改 `mod.rs`/`lib.rs`，必须先按 AGENTS 存档协议建立 checkpoint。Android 新插件命令必须四重注册，并在跨层变更后运行 `pnpm check`。

## 6. 测试矩阵

| 层 | 必测内容 |
|---|---|
| Rust L1 | parser/meta fixtures、typed validation、result normalize、Job state、digest/idempotency、output budget、Skill ID/path policy |
| Rust L2 | local turn 多 step、river/vref 忽略 notice、旧投影 ledger 解码/清理、断网 pending continuation、进程事件 generation、SQLite ledger |
| Vue L4 | route/blocked reason、工具块、Job tail/cancel、人工终端返回顺序 |
| 契约 L5 | manifest 三名一致、local/distributed schema、插件四重注册、catalog/allowlist 默认全关升级 |
| Kotlin L3/L6 | ProcessHost、进程组 kill、PTY resize、Guardian lease、通知 target |
| Android L7 | 飞行模式、本地执行、screen-off、Doze、划卡、Stop、进程死亡恢复 |
| L8 | 包体、rootfs 首启、内存/热量、并发 Job、长稳输出与磁盘清理 |

Host/CI 绿灯只能证明软件合同，不能替代 Android 后台与 OEM 接受。

## 7. 已裁决项与剩余实测

截至 2026-08-14，四个产品/施工选择均已关闭：ProcessHost 使用 Kotlin 应用进程 owner，PTY 的最小
fd/进程操作由仓库自有 arm64 JNI helper 承担；用户接受 PRoot/rootfs/PTY 的许可义务与 APK 增量；真实
VCPToolBox 的中央 loop 禁用合同已有 fixture；人工 PTY 进入 P5，且与 Agent Job 完全分离。OpenMinis
native-offload 未被复制，未来 `vcp-*` bridge 仍需逐命令审计。

剩余项不是架构开放问题，而是用户启动后的 API 26/34/36、Doze、OEM、通知与 PTY 触控真机验收。

命令基线与时间/输出/磁盘/并发预算已在 02/03 冻结为首发默认值和硬上限；P0 只负责实机验证它们是否可承受，不再回退到“几十秒/几 MiB”的保守占位。

iOS Runtime、iSH、a-Shell 和 App Store 不属于这些开工 blocker；它们保留在未来专项。

## 8. 最终一句话

> 先把手机做成一个在断开插件中心时仍能可靠执行、可取消、可追溯的 VCP CLI 节点；再把它作为可选插件接回 VCP 生态，而不是让生态连接反过来决定本地能力是否存在。
