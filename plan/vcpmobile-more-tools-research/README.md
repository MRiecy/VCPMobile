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
| [05 · Magi 三方审查与开放问题](./05-Magi三方审查与开放问题.md) | Melchior/Balthasar/Casper 裁决、6 个决策点（已裁决） | ✅ 已裁决 |
| [06 · HTTP 连接管理重构评估](./06-HTTP连接管理重构评估.md) | reqwest Client 现状 12 处摸排、A/B/C 方案四维评分、S0 迭代建议 | ✅ 已实施（S0） |
| [07 · 施工验收记录](./07-施工验收记录-2026-08.md) | S0/S1/S2a/S2b 交付清单、静态门结果、偏差记录、真机验收项 | ✅ 已完成 |
| [08 · Agent 管理 —— 上游契约与移植方案](./08-Agent管理-上游契约与移植方案.md) | config.json 浅合并语义、7+7 字段字典、并发风险、入口位置裁决（独立「更多」入口） | ✅ 待评审 |
| [09 · VCP 论坛 —— 上游契约与移植方案](./09-VCP论坛-上游契约与移植方案.md) | 一帖一文件模型、6 端点契约、human/tool 发帖通道、消毒渲染与 MVP 分层 | ✅ 待评审 |
| [10 · clawEmail —— 上游契约与移植方案](./10-clawEmail-上游契约与移植方案.md) | 云端私有 API 架构、4 端点契约、能力缺口与上游补丁清单、代理路线裁决 | ✅ 待评审 |

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

## 决策记录（2026-08 已全部裁决）

1. 入口位置：两个新功能都放「更多」popover（改三列网格）；
2. 日志默认行数 500；
3. 任务中心 S2a（只读+启停+触发+历史）→ S2b（编辑器+委托）分阶段；
4. Agent 列表用 `/admin_api/agent-assistant/config` 的 `agents[].chineseName`（已代码核实）；
5. 异步委托：只读展示 + 取消，不做深链；
6. 插件中心灵感本期保留不动。

**追加议题已立项**：HTTP 连接管理重构评估 → 06 篇，结论为「方案 B 命名画像注册表」，
建议编为 S0 迭代与 S1 并行。

## 第二批次（2026-08-19 立项，待评审）

- **Agent 管理（08）**：契约极简（GET/POST config 浅合并），工程量集中在客户端校验
  （chineseName 唯一性、必填、防并发覆写 RMW）与引用完整性（改名/删除扫描任务引用）。
  **入口裁决建议：独立「更多」入口**，任务编辑器 Agent 选择器加「管理 Agent…」联动。
- **VCP 论坛（09）**：浏览/回帖/编辑/删除走 admin Basic；**发帖需第二套凭据**
  （`/v1/human/tool` Bearer + TOOL_REQUEST 私有文本协议）。MVP = 列表→详情→回帖。
- **clawEmail（10）**：三候选中复杂度最高（近乎从零）。现有 4 端点够 MVP
  （账户切换+列表+详情+垃圾箱）；**发信/回复需上游 <150 行补丁**；
  直连 IMAP/SMTP 不可行，裁决走 VCP 后端代理路线。
