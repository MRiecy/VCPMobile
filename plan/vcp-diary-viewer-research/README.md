# VCPMemo 日记中心移动端移植研究

> 状态：`IMPLEMENTATION-COMPLETE / AUTOMATED-REGRESSION-PASS / DEPLOYMENT-ACCEPTANCE-PENDING / ANDROID-ACCEPTANCE-PENDING`（P1—P3 代码已落地，当前仓库门禁与专项契约回归通过；真实 VCP 部署与 Android 真机旅程仍待验收）
> 调研日期：2026-08-12  
> 施工 SSOT：本目录的 `01`—`06` 文档；旧版《排版与交互研究报告》仅作视觉研究附件

## 结论

本次研究已把桌面 VCPMemo 的真实实现、当前 VCPToolBox 服务端路由和 VCPMobile 基础设施对齐。推荐方案不是缩小桌面窗口，而是保留领域能力、重做移动端表达：

```text
右抽屉“日记中心”
  → 当前文件夹 + 窗口化 memo 文件列表
  → 普通 / LightMemo 语义搜索
  → 整文件阅读与显式编辑
  → 重命名、创建与批量管理
```

需要牢记的六条结论：

1. 真实数据模型是“文件夹 → memo 文件 → 整文件内容”，不存在已证实的“日期文件内多条日记”领域层。
2. 日记管理 API 使用管理员 Basic Auth；DailyNote/LightMemo 工具调用才使用 Bearer VCP API Key，两套凭据不能混用。
3. 网络、凭据、限流、响应体上限和保存冲突检查由 Rust 负责；Vue 只持有页面状态和编辑草稿。
4. 当前完整版本同时交付浏览、普通/LightMemo 语义搜索、整文件编辑与重命名、创建、移动、删除、批量管理和本机隐藏文件夹；仅工作台与联想发现后置。服务端语义排除是不可绕过的基线，本机隐藏是独立、可逆的附加过滤。
5. 日记正文属于用户自有 VCP 服务中的可信内容，保留 raw HTML，允许 `marked.parse` 后直接进入 `v-html`/`innerHTML`；文件名、错误和接口数据仍使用普通文本绑定。
6. 移动端使用线性虚拟列表、实色表面、细分隔线和 2px Accent Bar；不移植内容区域毛玻璃、大卡片、厚阴影、持续 Canvas 动画或人为等待。

## 文档导航

| 文档 | 用途 | 主要读者 |
|---|---|---|
| [01-VCPChat现状与接口契约.md](./01-VCPChat现状与接口契约.md) | 桌面调用链、服务端端点、鉴权、数据模型、已知缺陷与旧稿勘误 | 全栈开发、接口联调 |
| [02-移动端产品与UIUX方案.md](./02-移动端产品与UIUX方案.md) | 信息架构、线框、交互状态、视觉与无障碍规格 | 产品、设计、前端 |
| [03-VCPMobile技术架构与安全契约.md](./03-VCPMobile技术架构与安全契约.md) | Rust/Tauri/Vue 边界、DTO、并发、编辑冲突、渲染安全 | 全栈开发、审计者 |
| [04-功能迁移矩阵与分期施工.md](./04-功能迁移矩阵与分期施工.md) | 保留/改造/后置矩阵、文件落点、阶段、测试与验收 | 实施负责人 |
| [05-决策记录与开放问题.md](./05-决策记录与开放问题.md) | 已冻结 ADR、原开放问题结论、部署证据包和产品默认值 | 技术负责人、产品 |
| [06-Magi三方审查与综合裁决.md](./06-Magi三方审查与综合裁决.md) | Melchior/Balthasar/Casper 独立审查与最终裁决 | 评审会 |
| [VCPMobile日记查看器-排版与交互研究报告.md](./VCPMobile日记查看器-排版与交互研究报告.md) | 外部排版与阅读案例素材；部分工程前提已过时 | 视觉参考 |

## 事实快照与权威顺序

本研究固定在以下快照：

- VCPMobile 开工前 checkpoint：`99fce5f`（`save: checkpoint before diary center modules`）；本状态说明更新时实现仍位于后续工作区变更
- VCPChat 官方 `main`：`856c1db0404ebff0365aea8b16fdc0a4a68f9d5e`（本轮末次复核；Memo 相关文件与本地 `29ab88c` 快照逐字节一致）
- VCPToolBox：2026-08-12 检查官方 `main`，末次复核提交为 `351dadc74836ebf78d25fa942619cd34d9c82987`

发生冲突时按以下顺序裁决：

1. 当前部署服务器的脱敏响应 fixture 与契约测试；
2. 当前 VCPToolBox 官方路由/插件源码；
3. 当前 VCPChat 消费端源码；
4. 当前 VCPMobile 源码与 tracked `docs/`；
5. 本目录研究结论；
6. 外部案例与旧版排版研究稿。

## 开工条件

最新 VCPToolBox 源码已经确认管理路由、错误状态、最终一致索引队列和 Human Tool 转义协议。施工不再被旧版 Q-001/Q-004/Q-005/Q-006 阻断；部署联调仍应采集脱敏 fixture，防止本机配置漂移：

- `/folders`、`/folder/:folder`、`/note/:folder/:file`、`/search` 的成功与错误响应；
- 保存前后正文，以及异步索引最终可检索性；
- 认证失败、404、超时、超大正文和畸形 JSON；
- 已知规模边界的代表样本：正文平均数 KiB、数百字节至数十 KiB，单目录数百至数千文件；
- 旧/新/非结构化文件名与重命名成功、重名、部分失败样本。

当前上游没有 ETag、revision 或 `If-Match`。移动端采用“保存前复读 + hash 冲突拦截”的 best-effort 防护，不能描述成真正 CAS。管理 mutation 已由 VCPToolBox 排队并异步调度索引；HTTP 成功表示文件操作完成，不表示语义索引已经追平。

## 仓库状态说明

本目录的 README、`01`—`06`、旧研究附件和 PNG 资产都由 Git 跟踪。施工前已展示状态并创建 `99fce5f` checkpoint；实现新增 Diary Rust/Vue 领域并修改 `mod.rs`/`lib.rs`，未修改 VCPChat、Router、SQLite schema、Sync V2 或 Android 插件。

截至 2026-08-12，`pnpm check`、前端 113 项测试和应用 Rust lib 206 项测试（另含插件 Rust 3 项）均通过，production Web build、Rust Clippy、10 项 file-extractor integration 与 benchmark compile gate 也通过。自动化测试中 Diary 专项覆盖 26 项 Vue 行为与 26 项 Rust/HTTP 契约；这些结果证明当前 checkout 的实现与本地契约，不替代真实 VCP 部署或 Android L7/L8 验收。

## 一句话施工准则

> 把 VCPMemo 做成可靠、克制的移动端记忆文件管理器；不要把它伪装成小说阅读器，也不要把桌面玻璃工作台塞进 6 英寸屏幕。
