# VCPMobile VCP CLI 本地回环与生态兼容专项

> 状态：`P0-P1-CODE-AND-API36-EVIDENCE-COMPLETE / VCP-DEPLOYMENT-FIXTURE-DEFERRED / P2-NEXT`
>
> 研究日期：2026-08-13
>
> 当前范围：P0 已冻结工具协议、显式工具授权、Android Bash 资产与 manifest 交付；P1 已实现
> `MobileCliRuntime`、Android ProcessHost、人工前台 UI、Skills action 与持久 Job/输出边界，并完成 API 36
> 真机验收。用户实际 VCPToolBox 部署、Agent 本地多轮 loop 与后台能力分别留到 P2/P3/P5。

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

仍未完成：真实 VCPToolBox 部署 fixture、Agent 本地多轮 loop、真实 Distributed CLI adapter、recall modes、
后台前台服务与人工 PTY；它们分别属于 P2–P5。P1 的准确能力等级仍是 `foreground_only`。

## 结论

VCPMobile 的 CLI 不应把“始终连接 VCP 插件中心”作为默认生存条件。推荐建立一个由移动端自己拥有的 `VCPMobileCLI` 能力，默认走**进程内本地回环**；只有用户明确开启时，才把同一能力作为分布式 VCP 插件注册给 VCPToolBox。

```text
同一份 VCPMobileCLI manifest、请求语法和结果语义
                         │
                  Agent route
             ┌───────────┴───────────┐
             │                       │
 local_loopback（默认）       vcp_plugin（显式开启）
 Mobile 本地工具循环          VCPToolBox 工具循环
 进程内直调 Runtime           既有 Distributed WS
             │                       │
             └──── 同一 MobileCliRuntime ────┘
```

这不是“在手机里伪造一个 VCP 服务器”。本地回环不启动 localhost HTTP、SSE 或 WebSocket，而是让本地执行结果经过与 VCP 一致的规范化器，再以 `<!-- VCP_TOOL_PAYLOAD -->` 回灌模型续轮，同时向 UI 发送可渲染的工具状态。这样让 Agent 只需理解 manifest 中自洽的移动 CLI 合同，也避开插件中心的连接、注册和多次网络往返。

## 已冻结决策

1. **默认路由是 `local_loopback`**。它不依赖 Distributed WebSocket，飞行模式下也能运行不需要网络的本地命令。
2. **分布式工具采用“显式扫描 + 显式授权”，默认全部关闭**。Registry 扫描得到完整工具清单，UI 对每个工具单独展示和授权；后端只持久化 `enabled_tools` allowlist，未在 allowlist 的工具一律不发布、不执行。干净安装、旧配置升级和新扫描出的工具都保持关闭。
3. **一个工具身份、一个 manifest、一个 Runtime**。本地和远端只替换 transport/turn adapter，不复制 shell、job、工具说明或结果状态机。
4. **manifest 是主要提示词源**。`invocationCommands[].description/example` 完整说明 Shell、参数、限制和示例，由 VCPToolBox 提取并允许用户手动微调；Mobile 不另建 `CliPromptCatalog`，也不改写 Agent 提示词。
5. **本地回环不等于绕过 VCPToolBox 的提示词治理**。本地 route 只改变工具执行 owner；用户仍在 VCPToolBox 侧决定如何通过 `{{VCPVCPMobileCLI}}`、DynamicTools 或自定义提示词让 Agent 看见能力。
   因真实插件默认关闭，本地 route 首次使用前需要用户把 Mobile 导出的规范 manifest/说明导入或放置到 VCPToolBox 提示词配置中；本专项不再宣称 Agent CLI 是零配置提示词能力。
6. **本地模式与 VCP 模式只有一个工具循环 owner**。本地模式由 Mobile 截获 VCP block；VCP 模式由 VCPToolBox 截获。禁止双重执行和断线时静默换路由。
7. **人工终端与 Agent Job 分离**。普通任务用 `run/poll/cancel/list`；密码、SSH、vim、TUI 等场景由用户显式打开交互终端，预填命令但不自动执行。
8. **Android host 坚持非 Root 用户态沙箱**。首发目标 Shell 固定为 PRoot guest 内的 Alpine Linux (musl) + GNU Bash，Agent 命令以 `/bin/bash -lc` 执行；guest 可模拟 root 管理自身 rootfs 和 `apk`，但不能越过 App UID。P0 验证可行性；若不可行必须回到 manifest 重新裁决，不得静默改用 `ash`。Android Root/Shizuku 只能作为未来显式 elevated backend。
9. **长任务由 job 生命周期拥有，不由聊天 SSE 拥有**。模型续轮断线不能导致命令重跑；超时/取消必须终止目标进程组并阻止迟到输出污染后续任务。
10. **VCP 通用元字段也属于本地回环契约**。`ink: mark_history`、`river=text/last:N`、`archery=true/no_reply` 在本地 loop 首发落地；`river=full`、`river=semantic:N` 和 `vref:N` 按实际多模态/索引能力分期开放。未具备时返回 `unsupported_mode`，不能静默忽略或伪装等价。
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

> VCPMobile CLI 的 P0 协议/授权/可重建 Android Bash 资产与 P1 人工前台 Runtime 已完成；用户可以在
> CLI 页面运行、查询、取消结构化 Bash Job，并在无网时读取受控 Skill。它尚未接入 Agent 本地多轮
> loop、真实 VCP 插件或 Android 后台前台服务，因此只能称为人工前台 CLI，不是 Agent CLI 已完成。

不得表述为“Agent CLI 已可用”“后台长任务已稳定”“真实 VCP route 已验收”“OpenMinis 代码可直接合并”
或“iOS 已支持本地 Linux”。
