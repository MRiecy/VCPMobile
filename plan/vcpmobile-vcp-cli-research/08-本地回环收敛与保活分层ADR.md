# 08｜本地回环收敛与保活分层 ADR

> 日期：2026-08-14
> 状态：**已实施**（S2 收敛代码已落地：commit `36cff3d` Rust 侧 + `508d97e` 前端侧）
> 触发：用户指出 `localLoopback` 的「离线/保活」前提不成立。本 ADR 记录该判断、源码证据与收敛方向。
> 关联存档：[09-真机验收与回归修复存档](./09-真机验收与回归修复存档-2026-08-14.md)

## 1. 背景与触发

专项 README 把 `localLoopback` 定位为「断开插件中心时仍能执行」的默认路线。真机与 VCPToolBox 源码核证后，这个前提被证明**过度外推**：

- Agent 的**模型永远在 VCPToolBox**，走 `VCPLOG`（`/v1/chat/completions`）这条最高层连接。
- `localLoopback` 只把**工具执行**留在手机；模型请求仍要经过 VCPToolBox。
- 因此「断网也能跑 Agent」不成立：VCPLOG 一断，模型消失，本地工具再多也无法续轮。

真正零网络的能力只有**手动终端（PTY）**——人直接敲命令，不经模型、不经 VCPToolBox。

## 2. 保活分层（用户框架，本 ADR 采纳）

```text
VCPLOG（最高层）    控制所有与服务的连接；是 Agent 的真正依赖
分布式 WS（插件层）  register_tools / execute_tool；只要 VCPLOG 活，它不易断
PRoot（本地 shell）  本地执行；天然离线，但无模型
```

结论：`localLoopback` 想「绕开插件中心连接以保活」，但插件 WS 与模型连接同属 VCPLOG 一层。绕开它省掉的只是「工具执行腿」的往返，模型腿的依赖一点没少——这是 `localLoopback` 的鸡肋所在。

## 3. 关键事实（源码证据）

1. **两种模式执行面相同**：`src-tauri/src/distributed/tools/vcp_mobile_cli.rs:82-126` 的 `execute_with_context`（vcpPlugin 路径）最终调用 `runtime.execute(...)`，与 localLoopback 共用 `MobileCliRuntimeState`。分布式只是「谁拥有 loop」，不是「在哪里执行」；执行永远在手机 PRoot。
2. **vcpPlugin 下 Job 断线存活**：已启动 Job 由 `MobileCliRuntimeState` 拥有，WS 断只丢回包 waiter，不杀 Job、重连可 poll（README P3）。
3. **localLoopback 相对 vcpPlugin 的真实差异很小**：
   - loop owner = Mobile（需 `[[VCPToolUse=Forbidden]]` 让 VCPToolBox 让位）→ 引入双路由 + 双重执行风险；
   - 少一次 `execute_tool` WS 往返 → 微优化；
   - 不用注册 → 但提示词仍靠 VCPToolBox manifest，省不掉。
4. **Forbidden 是请求级全局禁执行**：VCPToolBox `modules/handlers/streamHandler.js:441` 的 `toolCalls = vcpToolUseForbidden ? [] : ...` 对**整轮**生效，会禁用该轮所有已注册插件（PowerShellExecutor 等）。副作用比预想大，且它本不是 VCPToolBox 的「用户开关」，而是客户端可注入的请求级控制面。
5. **「没注册工具」是同一根因的另一面**：`vcp_mobile_cli.rs:59-76` 的 `is_publishable` 要求 `route == VcpPlugin`。路由停在默认 `localLoopback` 时，CLI 故意不 `register_tools`——这正是 P3 真机看到的「双方都没工具」。

## 4. 决策

**Agent 循环收敛为单一 `vcpPlugin` 路线；移除 `localLoopback` 及配套的 `[[VCPToolUse=Forbidden]]` 注入与双路由互斥。**

保活/离线由三层承担，不再依赖 localLoopback：

1. **手动终端（PTY）＝ 唯一真离线**（无模型、无网络；与 Agent Job 分离）。
2. **Job 归属 Mobile runtime ＝ 断线后命令存活、可 poll**（vcpPlugin 同样成立）。
3. **turn ledger 的 `continuation_pending` ＝ 断网后 exact-once 续轮**（`recover_local_cli_turn`，只续模型不重跑）。

## 5. 保留 / 退役清单

**保留**（跨模式共享、不因收敛而变）：

- `MobileCliRuntimeState` + job ledger + 有界 output/redaction/artifact；
- `turn_coordinator.rs` / `turn_ledger.rs` 的续轮、exact-once、`continuation_pending`；
- 手动 PTY 终端（`terminal.rs` + `CliPtyHost` + `libvcp_pty.so`）；
- Skill catalog v2 与 `list_skills/read_skill/materialize_skill`；
- canonical manifest 与 `VCPMobileCLI` 协议；
- Distributed adapter + `ScannedToolCatalog` + `EnabledToolNames` allowlist（默认全关）。

**退役**（需逐项代码审计后执行，不在本 ADR 内直接删）：

- `localLoopback` 路由变体与默认值（`src/core/stores/settings.ts` 的 `DEFAULT_MOBILE_CLI_AGENT_ROUTE` 及 `MobileCliAgentRoute` 的 `localLoopback` 分支）；
- `src-tauri/src/vcp_modules/infra/vcp_client.rs` 的 `inject_local_loopback_transport_guard` / `should_inject_local_loopback_transport_guard`；
- 本地 loop owner 对 localLoopback 独有的 transport 投影、元字段（ink/archery 的本地处理路径）与 UI 路由开关；
- 相关 L1/L4 用例与 golden fixture 同步收敛（`MobileCliAgentRoute.test.ts`、`VcpCliGovernance.test.ts` 等）。

## 6. 「非 VCP 协议」方向的排除

用户提出「除非本地回环用非 VCP 协议才成立」。判断：当前产品边界内不成立，两条路都排除：

- **本地模型驱动 loop**：arm64 上跑能可靠做工具决策的 LLM 是 GB 级体量与另一量级工程，且与「复用 VCPToolBox 生态」根本冲突。
- **直连上游模型、绕过 VCPToolBox**：等于在手机重造 VCPToolBox 的 manifest/提示词/记忆/工具回灌/中央 loop，正是研究要避免的。

故「离线」这条线的终点是**手动终端**；Agent 永远以 VCPToolBox 为中心。

## 7. Magi 三方审查

### Melchior（逻辑与系统）

- 收敛后 `MobileCliAgentRoute` 只剩一个变体，`publication_route_enabled` / `require_vcp_plugin_route` 的互斥分支可删除，消除双 owner 与 double-execution 风险。
- 移除 Forbidden 注入后，`vcp_client.rs` 的请求组装路径变短；需确认 localLoopback 独享的元字段（ink=mark_history、archery）在 vcpPlugin 下由 VCPToolBox 拥有（README 已如此冻结），不产生孤儿状态。
- Job 归属 runtime + turn ledger 不变，断线续轮与 exact-once 语义不因收敛退化。✅

### Balthasar（直觉与美学）

- UI 少一个「路由」概念，用户不再需要理解 localLoopback vs vcpPlugin 的互斥；分布式面板的「工具注册」状态成为唯一真源。
- 手动终端仍是「离线也能敲命令」的直观入口，符合移动端直觉。✅

### Casper（务实与交付）

- 收敛净减复杂度（双路由、Forbidden、互斥），但涉及回退已提交代码与多处测试，是一次需要分期的重构；必须先审计「退役清单」再动手。
- 风险集中在「vcpPlugin 真实端到端尚未验收」——收敛后这条路径成为唯一 Agent 路径，P3 验收从「可选」升级为「必须」。✅（附条件）

## 8. 分期与风险

| 阶段 | 内容 | 门禁 |
|---|---|---|
| S1 审计 | 逐项核对退役清单的调用点、测试、golden fixture | 只读，产出删除映射表 |
| S2 收敛 | 移除 localLoopback 路由/Forbidden/双路由，收敛设置与 UI | 存档 checkpoint + `pnpm check` 绿 |
| S3 复验 | vcpPlugin 单路由 register_tools + 短/长/错误/取消端到端 | 真实 VCPToolBox + API 36 |
| S4 收尾 | README/02/04/06 的路线图与完成定义改写 | 文档与代码一致 |

**风险**：

1. P3 真实端到端尚未过，收敛后它是唯一路径——S2 前先做一次只读诊断坐实「没注册工具 = 路由未切」。
2. 回退已冻结的 localLoopback 相关 L1/L4 用例时，需同步改 governance 契约测试，避免「测试数下降掩盖功能移除」。
3. 26 个提交（含整个 CLI）仍在 `agent/sync-error-contract-1-2` 未合 main，收敛前先定合并策略。

## 9. 一句话

> 模型腿永远在 VCPToolBox，所以 Agent 只走它最一致的一条路（vcpPlugin）；「离线」由手动终端承担，「断线韧性」由 Job 归属 runtime + 续轮 ledger 承担——localLoopback 与 Forbidden 是这两条真正能力之外的多余复杂度，应撤回。
