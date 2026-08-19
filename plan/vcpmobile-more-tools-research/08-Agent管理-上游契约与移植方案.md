# 08 · Agent 管理（AgentAssistant agents CRUD）—— 上游契约与移植方案

> 目标：将「Agent 管理」移植到 VCPMobile，补齐任务调度中心（S2b）被排除的半边——
> 目前手机上只能执行任务，Agent 设置必须去后端网页编辑，而官方 AdminPanel 对手机用户很不友好。
> 本文档基于对后端路由、AgentAssistant 插件、桌面端与 AdminPanel-Vue 的全量精读。

参考实现：
- `/home/dudu/VCPToolBox-main/routes/admin/agentAssistant.js`（132 行，全部路由，已精读）；
- `/home/dudu/VCPToolBox-main/Plugin/AgentAssistant/AgentAssistant.js`（1241 行，配置加载 L160-218）；
- `/home/dudu/VCPChat/Agenttaskmodules/task.js`（桌面端 AA 管理 Tab，字段定义 L20-49，模态框 L542-617）；
- `/home/dudu/VCPToolBox-main/AdminPanel-Vue/src/{views/AgentAssistantConfig.vue, features/agent-assistant-config/useAgentAssistantConfig.ts}`。

---

## 1. 结论速览

- 真相源是**单个 JSON 文件** `Plugin/AgentAssistant/config.json`（gitignore 运行时文件）；
  全部 CRUD 只有 `GET/POST /admin_api/agent-assistant/config` 一对路由，**无单条 agent CRUD 端点**。
- POST 是**顶层浅合并**（`{...旧, ...请求体}`，agentAssistant.js:45-48）：未提交的顶层键保留，
  但 `agents` 数组一旦提交即**整体替换**。保存后自动 `reloadConfig()` 热重载，无需重启。
- 服务端**无校验、无 chineseName 去重、无并发锁**；插件加载时静默跳过缺
  chineseName/modelId 的条目，chineseName 重复时后写覆盖先写（AgentAssistant.js:189-218）。
  → 唯一性校验与防覆写合并必须由移动端实现。
- 模型选择器可行：`GET /admin_api/ai/models` 返回 OpenAI 格式 `{data:[{id}]}`（含语义路由虚拟模型）。
- Agent 与 `Agent/*.txt` 角色卡**无自动映射**，编辑 agent 不改写 txt；唯一纽带是
  systemPrompt 手写 `{{别名}}` 由主服务经 agent_map.json 展开。

---

## 2. API 契约（已核实）

基础路径 `/admin_api/agent-assistant`，Basic Auth（与日志/任务中心同一套凭据）。

| 方法/路径 | 请求体 | 响应 | 备注 |
| --- | --- | --- | --- |
| GET `/config` | — | **config.json 原始对象（无 envelope）**；缺失返回 `{}`；解析异常返回兜底 `{maxHistoryRounds:7, contextTtlHours:24, globalSystemPrompt:'', agents:[]}` | agentAssistant.js:12-27 |
| POST `/config` | 任意 JSON 对象 | `{success, message}`；失败 500 | 顶层浅合并 + 热重载，agentAssistant.js:29-69 |
| GET `/delegations` | — | `{success, data:{active, recent}}` | 任务中心已用 |
| GET `/delegations/:id` | — | `{success, data:{...snapshot}}` | 404 未找到 |
| POST `/delegations/:id/cancel` | `{reason?}` | `{success, message}` | 任务中心已用 |
| GET/POST `/scores` | 任意对象 | `agent_scores.json` 原始对象 | 积分系统，二期 |

**辅助 API**：

| 方法/路径 | 说明 |
| --- | --- |
| GET `/admin_api/ai/models` | 代理主服务 `/v1/models`，OpenAI 格式模型列表 → **模型选择器数据源**（aiChat.js:135-142） |
| GET `/admin_api/agents` / GET/POST `/agents/:fileName` / POST `/agents/new-file` / GET/POST `/agents/map` | 角色卡（Agent/*.txt）API 族，二期（agents.js 全文 123 行） |

---

## 3. config.json 字段字典

### 3.1 全局字段（顶层）

| 字段 | 类型 | 含义 | 默认 |
| --- | --- | --- | --- |
| `maxHistoryRounds` | int | 每 Agent 持久会话历史保留轮数（×20 条消息封顶） | 7 |
| `contextTtlHours` | int | 会话上下文 TTL（小时） | 24 |
| `globalSystemPrompt` | string | 追加到每个 agent systemPrompt 之后的共享提示词 | `''` |
| `delegationMaxRounds` | int | 异步委托最大唤醒轮数 | 15 |
| `delegationTimeout` | int | 委托总超时，**毫秒**（UI 按分钟展示 ×60000 存储） | 300000 |
| `delegationSystemPrompt` | string | 委托系统提示词模板（`{{SenderName}}`/`{{TaskPrompt}}`） | 内置（留空=默认） |
| `delegationHeartbeatPrompt` | string | 委托每轮未完成的催促提示词 | 内置（留空=默认） |
| `agents` | array | Agent 定义数组 | `[]` |
| 未知顶层键 | any | 浅合并语义下被保留（VCPChat 渲染为"扩展配置字段"） | 透传 |

### 3.2 agents[] 元素字段

| 字段 | 类型 | 含义 | 默认/兜底 |
| --- | --- | --- | --- |
| `chineseName` | string | **唯一 dispatch key**，工具调用 `agent_name` 必须等于它 | 无；缺失→**整个 agent 静默跳过** |
| `baseName` | string | 内部标识（积分系统键、会话 ID 组成部分） | `chineseName.toUpperCase()` |
| `modelId` | string | 绑定模型，直接作为回调 `/v1/chat/completions` 的 `model` | 无；缺失→静默跳过 |
| `description` | string | 角色/能力综述（供其他 Agent 了解它） | `Assistant ${chineseName}.` |
| `systemPrompt` | string | 系统提示词模板；`{{MaidName}}` 运行时替换为 chineseName；可写 `{{角色卡别名}}` 引用 Agent/*.txt | `You are a helpful AI assistant named {{MaidName}}.` |
| `maxOutputTokens` | int | 单次回复 max_tokens | **40000**（⚠️ AdminPanel-Vue 默认 8000，与插件不一致） |
| `temperature` | float | 温度 | 0.7 |
| 未知自定义键 | any | 插件忽略 | **必须透传保留**（见 §6 教训） |

⚠️ agents 元素里**不存在** `name`/`personality`/`maxRounds`——那是 AdminPanel-Vue 的前端别名。
移动端直接使用后端原生字段名。

---

## 4. 写入语义与并发风险

### 4.1 POST /config 精确语义

1. 读现有 config.json → 2. `config = {...existing, ...incoming}` 顶层浅合并 →
3. 全量覆写文件 → 4. `reloadConfig()` 热重载（异常仅 console.error，POST 仍返回 success）。

推论：
- 请求体不含的顶层键被保留（后端专为"旧版/移动端面板"做的兼容）；
- **`agents` 一旦提交即整体替换**，agent 级 CRUD = 读全量 → 本地改 → 整体回写；
- 无 ETag/锁：两端同时读-改-写 = last-write-wins。

### 4.2 移动端风险清单与缓解

| 风险 | 缓解 |
| --- | --- |
| 并发覆写（无乐观锁） | 保存前重新 GET，以服务端为基做合并；检测到脏冲突提示 |
| 静默丢 agent（缺 chineseName/modelId） | 客户端保存前强制两字段非空 |
| chineseName 重复（运行时后写覆盖先写） | 客户端唯一性校验（trim 后比较） |
| delegationTimeout 单位 | 分钟展示、毫秒存储 |
| maxOutputTokens 默认值分歧 | 跟随后端 40000 |
| 热重载失败不报错 | 保存后重新 GET 验证生效 |
| 数组顺序即展示顺序 | 保留读入顺序 |

---

## 5. 桌面端 / AdminPanel 现状评估

### 5.1 VCPChat（task.js 内嵌 AA 管理 Tab）

- 字段定义齐备（`AA_GLOBAL_FIELD_DEFS` L31-39 含中文 label/单位转换说明，可直接搬文案）；
- **未知字段自动渲染为额外表单项并原样保留**（L572-613）——值得借鉴；
- 校验极弱（仅名称非空，无唯一性/modelId 必填）；modelId 纯文本无下拉；
- 避免：alert/confirm 原生弹窗、字符串拼 HTML。

### 5.2 AdminPanel-Vue「简陋点」逐条确认

1. **baseName 完全不可编辑**（模板无输入框，只能创建时从"已注册 Agent"带入）；
2. modelId 无选择器（后端明明有 `/admin_api/ai/models`）；
3. 无 chineseName 唯一性校验；
4. 无并发防护（内存态直接整体 POST）；
5. **agent 内未知字段被 `normalizeAgentEntry` 重建对象永久丢弃**（L118-139/688-696）——移动端必须浅拷贝改键而非重建；
6. 默认值与插件不一致（8000 vs 40000）；
7. 有 config.env 时代的历史包袱（移动端不需要）。

---

## 6. 入口位置裁决：独立「更多」入口 vs 并入任务调度中心

| 维度 | A. 独立「更多」入口 | B. 任务调度中心内嵌 Tab |
| --- | --- | --- |
| 页面职责 | ✅ 单一明确（与日志中心/任务中心同款一页一功能） | ❌ 任务中心已是状态仪表盘+委托+任务列表的密集页，再嵌 Agent 表单成巨石页 |
| 使用频率 | Agent 编辑低频，独立入口不碍事 | 低频功能占据高频页面的 Tab 位 |
| 工程结构 | ✅ 复用既有 `features/<domain>` + overlay page 模式，零特例 | 需改造 TaskCenterView 为 Tab 容器，回归风险 |
| 上下文联动 | 需显式做跳转链接 | ✅ 与任务编辑同页 |
| 桌面端对应 | 桌面端单窗口双 Tab 是 Electron 窗口成本高的产物 | 移动端 SlidePage 页面栈天然支持多页，无需复制该妥协 |

**建议：A——独立入口「Agent 管理」放入「更多」popover**（与既有架构一致），
并做两条轻量联动弥补上下文：
1. 任务编辑器的 Agent 选择器底部加「管理 Agent…」条目 → 打开 Agent 管理页；
2. Agent 管理页保存后，任务中心 store 的 agent options 缓存失效重拉。

> ✅ **已裁决（2026-08-19）**：采纳 A，独立入口。二期角色卡/复杂占位符解析功能暂不做。

**额外的移动端增值点（引用完整性）**：chineseName 是 dispatch key，**改名/删除 Agent
会使引用它的任务静默失效**。后端无外键概念，移动端可在改名/删除时扫描
task-assistant config 的 `targets.agents`，弹确认列出受影响任务——这是桌面端和
AdminPanel 都没有的能力，成本低价值高。

---

## 7. 移动端功能范围建议

### MVP

- **Agent 列表**：GET config，chineseName（标题）+ modelId（mono）+ description 摘要；保持数组顺序；
- **新建/编辑表单（7 字段）**：chineseName（必填+唯一校验）、baseName（选填，留空提示回退
  `chineseName.toUpperCase()`）、modelId（必填，`/admin_api/ai/models` 下拉+可手输）、
  description、systemPrompt（多行，提示 `{{MaidName}}`/`{{角色卡别名}}` 占位符）、
  maxOutputTokens（默认 40000）、temperature（0-2 步进 0.1 默认 0.7）；
- **删除**：确认 → splice → 整体回写；改名/删除前扫描任务引用并提示（§6）；
- **全局设置**：7 个全局字段；delegationTimeout 分钟展示毫秒存储；两个委托提示词留空=内置默认；
- **保存管线**：保存前重新 GET → 以其为基（保留未知顶层键与未编辑 agent 的未知字段）
  → 替换 agents → POST → 再 GET 校验；编辑期间检测他端变更提示冲突；
- **保留未知字段**：浅拷贝改键，不重建对象（吸取 AdminPanel-Vue 教训）。

### 二期（已裁决：本期不做）

角色卡（Agent/*.txt）查看/编辑（`/admin_api/agents*` 族）——涉及复杂占位符解析，暂缓；
agent_scores 积分展示。

### 明确不做

env 迁移回退；scores 编辑；臆造单 agent RESTful 路由（后端不存在）。

---

## 8. 架构落位（沿用共享架构）

- Rust：`vcp_modules/agentmgr/agent_service.rs`——`agentmgr_get_config / agentmgr_save_config(RMW合并) / agentmgr_list_models`；复用 `infra/admin_api` + `HttpProfile::AdminApi`；
- 前端：`features/agentmgr/{agentMgrTypes.ts, agentMgrStore.ts, AgentMgrView.vue, AgentEditorView.vue}`；
- 集成：overlay page type `'agentMgr'` + 右边栏「更多」入口 + 懒加载 latch + 治理测试更新；
- 与任务中心的联动：agentMgrStore 保存成功后调用 taskCenterStore 的 agent options 失效方法。

---

## 9. 关键文件索引

| 内容 | 路径:行号 |
| --- | --- |
| 路由全部实现 | `VCPToolBox-main/routes/admin/agentAssistant.js`（config L12-69, delegations L71-114, scores L116-129） |
| 插件加载/默认值/静默跳过 | `VCPToolBox-main/Plugin/AgentAssistant/AgentAssistant.js:160-218` |
| 路由挂载/鉴权 | `VCPToolBox-main/routes/adminPanelRoutes.js:99`；`adminServer.js:73-158, 743-760` |
| 模型列表 | `VCPToolBox-main/routes/admin/aiChat.js:135-142`；`server.js:887-908` |
| 角色卡 API | `VCPToolBox-main/routes/admin/agents.js`（123 行）；`modules/agentManager.js:36-63, 272-289` |
| 桌面端 AA Tab | `VCPChat/Agenttaskmodules/task.js:20-68, 182-222, 280-617` |
| AdminPanel 现状 | `AdminPanel-Vue/src/features/agent-assistant-config/useAgentAssistantConfig.ts:118-139, 662-712`；`views/AgentAssistantConfig.vue:250-377` |
| 官方结构文档 | `VCPToolBox-main/docs/AGENT_AND_TASK_SYSTEM_GUIDE.md:25-55` |
