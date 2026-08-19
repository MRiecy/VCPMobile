# Agent 管理（Agent Manager）

> 入口：右边栏「更多」工具盘 → Agent 管理。
> 远程管理 VCPToolBox AgentAssistant 的 Agent 定义（agents CRUD + 全局配置）。
> 方案档案：`plan/vcpmobile-more-tools-research/08-Agent管理-上游契约与移植方案.md`。

## 架构

```
AgentMgrView.vue ── AgentEditorView.vue（滑入子页，含模型选择器）
  └─ agentMgrStore.ts ── invoke('agentmgr_get_config' / 'agentmgr_save_config' / 'agentmgr_list_models')
       └─ Rust vcp_modules/agentmgr ── GET/POST /admin_api/agent-assistant/config
                                      GET /admin_api/ai/models（Basic Auth）
```

- **Rust 侧**（`src-tauri/src/vcp_modules/agentmgr/`）：认证代理 + 保存前
  read-modify-write（先 GET 最新做顶层浅合并再 POST）+ agents 防御性校验。
- **前端 Store**：config 缓存 + agents 指纹脏检测（并发冲突返回 'conflict' 由视图
  二次确认 force 覆盖）+ 模型列表懒加载。

## 关键机制

| 机制 | 说明 |
| --- | --- |
| 顶层浅合并 | 后端 POST 保留未提交的顶层键；`agents` 数组一旦提交即整体替换——agent 级 CRUD = 读全量→本地改→整体回写 |
| 防御性校验 | chineseName/modelId 非空 + chineseName trim 后唯一（前端 `validateAgentDraft` 与 Rust `validate_agents` 双重把关）；后端缺这两字段会静默跳过条目 |
| 未知字段透传 | `AgentEntry.extras` 收集未知键，保存时铺底保留（AdminPanel-Vue 重建对象丢字段的教训） |
| 引用完整性 | 改名/删除前 `collectTaskReferences` 扫描任务调度中心 `targets.agents`，确认弹窗列出受影响任务 |
| 并发防护 | 加载时记录 agents 指纹；保存前重新 GET 比对，不一致返回 conflict → 用户确认后 force |
| 模型选择器 | `GET /admin_api/ai/models` 投影为 id 数组；下拉 + 可手输（兼容 `default` 等特殊值） |
| 联动 | 任务编辑器 Agent 选择器底部「管理 Agent…」跳转；保存成功后失效任务中心 `agentsLoaded` 缓存 |

## 字段契约

- Agent 7 字段：chineseName（dispatch key 必填）/ baseName（留空回退大写）/ modelId
  （必填）/ description / systemPrompt（`{{MaidName}}`、`{{角色卡别名}}` 占位符 chips）/
  maxOutputTokens（默认 40000）/ temperature（0-2，默认 0.7）。
- 全局 7 字段：maxHistoryRounds(7) / contextTtlHours(24) / globalSystemPrompt /
  delegationMaxRounds(15) / delegationTimeout（毫秒存储、分钟展示）/
  delegationSystemPrompt / delegationHeartbeatPrompt（留空=内置模板）。

## 测试

- `src/tests/unit/agentmgr/agentMgr.test.ts`：归一化/草稿/校验/引用扫描 + Store
  读写流（含 conflict 检测与缓存失效联动）（14 例）；
- Rust 内联单测：agents 校验 + 顶层合并（6 例）。
