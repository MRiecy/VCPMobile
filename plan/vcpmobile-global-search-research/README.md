# 全局消息搜索调研（vcpmobile-global-search-research）

目标：为 VCPMobile 新增**全局消息多条件检索**（非现有 topic/agent 名称过滤）。

| 文档 | 内容 | 状态 |
|---|---|---|
| `01-技术基础调研与方案草案.md` | 技术基础盘点、差距清单、Phase 0-3 分层方案、决策记录（A-H 已定稿）、Magi 预审查 | ✅ 决策已定稿，待施工 |

## 核心结论（TL;DR）

1. **后端地基已上线**：`messages_fts` FTS5 表 + 删除触发器 + 多条件命令 `search_messages_fts`（`db_manager.rs:637`）均已注册，前端零调用。
2. **纯本地可行**：消息全量同步在本地 SQLite，服务端无搜索 API，无需 fallback。
3. **已定稿的关键决策**（详见正文 §5）：trigram 分词（bundled SQLite 3.46.0 内置，零新依赖，已验证）/ agentId=会话归属 / 不做语义搜索 / 附件不入索引 / 入口在左侧边栏 header / 默认时间倒序 / 回填在首次打开搜索页触发 / **采纳 Baseline 迁移机制**（全新安装一步到位终态 schema）。
4. 施工主线：migration 0008（trigram 重建 + 索引）→ 回填命令（首开触发）→ 搜索命令升级（snippet/分页/归属过滤）→ SlidePage 搜索页 → 跳转定位闭环。
