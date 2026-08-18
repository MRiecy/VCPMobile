# VCPMobile 右边栏「更多」功能移植研究

> 立项：2026-08 · 目标：移植 VCPChat 桌面端三件套——**VCP 日志中心**、
> **VCP Task Assistant（任务调度中心）**、**VCP 插件管理器**（后者仅灵感扩散，不改代码）。
> 本文档集为施工前的调研与方案，供逐篇评审讨论。

## 文档导航

| 篇目 | 内容 | 状态 |
| --- | --- | --- |
| [01 · 日志中心 —— 上游契约与移植方案](./01-日志中心-上游契约与移植方案.md) | server-log 增量协议、双参考实现对比、虚拟滚动/生命周期轮询方案、施工清单 | ✅ 待评审 |
| [02 · 任务调度中心 —— 上游契约与移植方案](./02-任务调度中心-上游契约与移植方案.md) | Task 数据模型逐字段、10+ 端点契约、移动端 UI/UX 主战场方案、S2a/S2b 拆分 | ✅ 待评审 |
| [03 · 插件管理器 —— 灵感扩散](./03-插件管理器-灵感扩散.md) | 桌面端设计亮点、10 条可落地灵感（不改代码、不排期） | ✅ 待评审 |
| [04 · 移动端共享架构与施工计划](./04-移动端共享架构与施工计划.md) | 三层架构落位、集成点 6 步清单、工程纪律检查单、S1/S2 分期、风险表 | ✅ 待评审 |
| [05 · Magi 三方审查与开放问题](./05-Magi三方审查与开放问题.md) | Melchior/Balthasar/Casper 裁决、6 个待讨论决策点 | ✅ 待讨论 |

## 一句话结论

- **日志中心（S1 先行）**：以 AdminPanel-Vue 的 `useServerLogViewer.ts` 为蓝本
  （半行拼接 + 虚拟滚动已就绪），Rust 加 2 条 command 做认证代理，
  顺带修复桌面端无暂停、无生命周期感知、清空语义混淆三大短板。
- **任务调度中心（S2 = a+b 两批）**：后端契约完整（细粒度 CRUD 比桌面端全量保存更优），
  难度集中在任务编辑器的移动端表单设计与占位符输入体验。
- **插件管理器**：本期零代码；10 条灵感入库，优先候选为异常第三态、统计横幅、分组计数。

## 上游事实来源（均已精读核实）

- `VCPChat/Logmodules/log.{html,js,css}`、`VCPToolBox-main/routes/admin/logs.js`、
  `AdminPanel-Vue/src/features/server-log-viewer/useServerLogViewer.ts`
- `VCPChat/Agenttaskmodules/task.{html,js}`、
  `VCPToolBox-main/Plugin/VCPTaskAssistant/vcp-task-assistant.js`（854 行全文）、
  `routes/admin/taskAssistant.js`、`routes/admin/agentAssistant.js`
- `VCPChat/PluginManagerModules/plugin-manager.{html,js,css}`、
  `modules/ipc/desktopHandlers.js`（插件管理 IPC 段）

## 待讨论决策点（详见 05 篇 §5）

1. 入口位置：popover 内 vs 提升为托盘主按钮；
2. 日志默认行数 500 是否接受；
3. S2a/S2b 分批交付 vs 一次性完整交付；
4. Agent 列表数据源的真实环境验证；
5. 异步委托追踪的深度（只读+取消 or 深链对话）；
6. 插件中心灵感的 backlog 优先级。
