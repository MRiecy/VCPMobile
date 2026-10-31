# 06 - Magi 三贤者审议与实施路线图

## 1. Magi 三贤者协议三方思辩

依据项目根目录 `AGENTS.md` §8.2 铁律，在重大架构调整与核心功能重构前，强制执行 Magi 三方思辩：

### 1.1 Melchior (逻辑、系统与安全)
- **审查重点**：事务隔离、内存溢出防御、物理哈希保护、无死锁并发。
- **裁决意见**：
  1. **必须保证单事务提交（Single Write Transaction）**：全局设置写入、Agent 本地列更新、Group 本地参数写入、Model 收藏与 Tarven 规则写入必须包裹在 `SettingsRepository` 的同一个 SQLite 事务中。严禁跨多个事务分别提交，杜绝中途失败留下一半脏数据；
  2. **绝对禁止修改同步哈希**：Agent 本地列（`mobile_system_prompt`, `use_temperature`, `voice_config`）和 Group 本地设置更新时，SQL `UPDATE` 绝对禁止更新 `config_hash` 和 `updated_at`。这属于设备本地权威，任何改动不得触发反向同步脏标记；
  3. **严格 1 MiB 反序列化截断**：在 Rust 读入字节流前（`file.take(MAX_CONFIG_BACKUP_BYTES + 1)`），必须强制截断校验，防御由恶意构造或损坏的 JSON 文件引发的 WebView 爆栈与 OOM；
  4. **死锁防御与锁序铁律**：`SettingsRepository.writer` 锁的获取必须先于 SQLite 事务的开启，并与现有 `save_agent_edit` / `update_settings` 保持相同的加锁顺序，杜绝死锁。

### 1.2 Balthasar (直觉、交互与移动端原生美学)
- **审查重点**：移动端原生直觉、心智负担、视觉密度、微交互反馈。
- **裁决意见**：
  1. **克制弹窗，禁止阻塞性打扰**：首次同步后的弹窗必须是非模态或易于一键关闭的，用户即使点击“稍后再说”，也可随时在设置页手动触发还原；
  2. **高密度技术精确感**：导入报告弹窗中，分类显示各作用域的恢复结果（如 `[Server] 主服务器连接`、`[Agent] 助手移动端设置 (12项)`），使用 Monospace 字体展示数量与 ID 缩写；
  3. **严禁使用毛玻璃（No Backdrop Blur）**：在备份列表、进度展示及向导弹窗中，严格遵循 UI 宪法，背景采用纯色或实色遮罩，不得在滚动区域或弹窗容器中使用 `backdrop-filter: blur()`，保护移动端低端 GPU 帧率。

### 1.3 Casper (务实、工程尺度与交付收束)
- **审查重点**：研发成本、认知熵、架构简洁度、不盲目扩大重构。
- **裁决意见**：
  1. **版本号坚决保持 V2**：正如用户指示，仍在开发期，V2 扩展已完全足够，绝不引入破坏性的 V3 格式升级；
  2. **复用已有的 Patch 机制**：充分复用现有 `GroupLocalSettingsPatch` 和 `normalize_local_settings_patch` 的递归投影算法，为 Agent 创建同款 `AgentLocalSettingsPatch`，不另起炉灶；
  3. **分阶段可验证推进**：先保证 Rust 核心层与单测闭环，再接入前端偏好分流，最后串联 UI 向导与同步触发器，保证每一步均可执行 `pnpm check` 与单元测试。

---

## 2. 风险识别与防御矩阵

| 风险场景 | 根因分析 | 架构防御机制 |
|---|---|---|
| **空机导入幽灵 Agent** | 空库时写入 `agentPreferences` 找不到外键 | 强绑定匹配：本地找不到或已被墓碑化则静默跳过；记录缺失计数，引导后续同步恢复。 |
| **反向污染桌面端** | 导入本地音色或提示词改变了 `config_hash` | 纯本地更新：SQL 显式保留原 `config_hash` 与 `updated_at`，同步比对哈希保持一致。 |
| **音频样本丢失** | 换机后 Agent 独立音色 `cloneSampleId` 指向的文件不存在 | 解耦降级：TTS 引擎在找不到物理音频时安全降级回预置音色；导入时黄色 Toast 提示。 |
| **超大配置 JSON 爆栈** | 用户导入数百条带有冗长提示词的备份 | 预设 1 MiB 物理硬限（纯文本配置 1MB 约等于数十万中文字符，容量极度充裕）。 |
| **并发同步时导入配置** | 用户在 CDS 同步运行中途点击导入备份 | `SettingsState.writer` 与 `DbState` 互斥排队，保证写锁串行。 |

---

## 3. 分阶段实施路线图 (Phase 1 ~ 4)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ Phase 1: Rust 核心扩展与原子事务闭环                                        │
│   • 修改 schema.rs:88 解禁 Navigation 领域，同步更新 backup.rs:302,419 单测 │
│   • 扩展 backup.rs / schema.rs：定义 AgentLocalSettings & Patch             │
│   • 扩展 repository.rs：由 DbState::write_transaction 实现原子写事务提交    │
│   • 覆盖 config_domains、agents 本地列、group_local_settings、收藏与规则库  │
│   • 派生 ConfigBackupImportResult 并运行 pnpm config:generate 同步契约      │
│   • 编写 Rust 单元测试：覆盖正常备份、缺少实体跳过、墓碑跳过、排序/规则保护 │
├─────────────────────────────────────────────────────────────────────────────┤
│ Phase 2: 前端 Pinia 偏好分流与契约聚合                                      │
│   • 重构 useConfigBackup.ts：扩展 captureClientPreferences / restore       │
│   • 接入 topicListManager (sortMode, pinnedTopicKeys 三元组) & diaryStore   │
│   • 排除 developerSettings 等易失黑名单，类型安全读取 localPreference       │
│   • 完善 SettingsActions.test.ts 与 useConfigBackup.test.ts 单元测试        │
├─────────────────────────────────────────────────────────────────────────────┤
│ Phase 3: 引导向导与首次同步智能恢复闭环                                     │
│   • 实现空机开屏 Onboarding Wizard (WelcomeModal.vue)                        │
│   • 在 guide 引擎中实现 config-backup-workflow 并在 guides/index.ts 挂载    │
│   • 在 syncSessionStore.stopAndReload() 植入 checkAndPromptPendingConfig    │
│   • 接入 overlay.showConfirm() 模态对话框，支持暂存 Payload 一键极速应用    │
├─────────────────────────────────────────────────────────────────────────────┤
│ Phase 4: 全链路跨平台回归与工程交付                                         │
│   • 运行 pnpm check (TS 类型检查 + ESLint + Rust locked check)              │
│   • 运行 vitest 与 cargo test 全部自动化测试                                │
│   • Android USB 真机实测：验证导出、全新重装、空机导入、CDS 同步与一键补全   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. 验收与验证标准

1. **静态代码质量**：
   - 执行 `pnpm check` 零错误通过（配置契约校验 + ESLint + vue-tsc + cargo check --locked）；
2. **单元与集成测试**：
   - 现存 `src/tests/unit/settings/useConfigBackup.test.ts` 与 `SettingsActions.test.ts` 全部保持绿色通过；
   - 新增 `agent_preferences` 与 `client_preferences` 的序列化/反序列化测试覆盖率 100%；
3. **真实用户旅程验收（User Journey）**：
   - **步骤 1**：在旧设备配置 Agent 专用提示词、独立音色、群组发言顺序、日记折叠目录，导出 `backup.vcpcfg.json`；
   - **步骤 2**：在干净新设备上打开，首屏向导点击导入该备份；检查：全局服务器、API 密钥、主题立即恢复，提示“助手与群组配置待同步后生效”；
   - **步骤 3**：连接桌面端完成 CDS 数据同步；检查：同步收尾后自动弹出“是否恢复助手专用配置”提示；
   - **步骤 4**：点击确认；检查：所有 Agent 的移动端提示词、独立音色与 Group 发言模式全部精确还原，无任何数据残留或幽灵记录。
