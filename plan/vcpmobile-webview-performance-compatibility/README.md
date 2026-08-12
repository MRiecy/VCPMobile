# VCPMobile WebView 性能与兼容性专项研讨

> 状态：`RESEARCHED / IMPLEMENTATION-PENDING / DEVICE-EVIDENCE-PENDING`
>
> 审计日期：2026-08-12
>
> 审计快照：`99fce5f`，另有未提交的 Diary 功能工作区变更
>
> 主题：Release 首屏、旧 Android WebView、平板响应式布局、消息强渲染性能

## 结论

这不是一个“把 Vue 换掉”或“把消息气泡做简单”的问题。当前代码已经具备静态启动占位、Rust AST 管线、流式差分、`v-memo`、KaTeX/Mermaid 动态导入和消息分页等正确基础；真正需要处理的是四个更具体的边界：

1. **首屏测量口径缺失。** 现有脚本只记录 Android Activity 的 `am start -W`，尚未测到 Vue mount、`AppLifecycle.READY`、首个可交互帧或富内容稳定帧，因此目前不能把延迟归因给 Vue、IPC 或消息渲染中的任何一项。
2. **启动门禁装入了非首屏必需工作。** `READY` 当前等待设置、助手/群组、所有头像二进制和恢复会话的 5 条历史；其中“批量头像全部转 Blob”是明确的高风险候选，但仍需分段实测定责。
3. **拆包并未等于延迟执行。** Chat 路由、`MessageRenderer`、全局覆盖层和更新提示仍通过静态引用进入首屏依赖图；多个异步 Overlay 组件又在 `READY` 后立即挂载，从而立刻请求其 chunk。
4. **旧机问题首先是兼容契约与布局问题。** `minSdk 26` 只定义 Android API 安装下限，不定义用户设备实际采用的 WebView 包和版本。当前还存在一个源码可证的平板根布局矛盾：根容器始终是纵向 flex，而左右侧栏在 `768px` 后变为普通流中的相对定位元素，极可能被排到主界面下方并被根容器裁掉。

本专项坚持一条不可协商的产品约束：

> **性能优化不得关闭、删除、纯文本化或降级消息的 Markdown、代码高亮、KaTeX、Mermaid、SVG/MathML、raw HTML、Tool、Thought、Diary、附件与流式 AST 差分能力。**

允许优化的是加载时机、调度、缓存、离屏挂载、兼容性 fallback 和资源优先级；不允许把“少渲染一种内容”包装成性能提升。

## 文档导航

| 文档 | 主要内容 | 用途 |
|---|---|---|
| [01-现状证据与问题分层.md](./01-现状证据与问题分层.md) | 启动调用链、构建产物、明确事实、瓶颈假设与证据缺口 | 确定问题边界 |
| [02-WebView兼容与UI响应式规范.md](./02-WebView兼容与UI响应式规范.md) | WebView 支持基线、CSS 分级、平板布局、安全区与样式准则 | 冻结兼容设计规范 |
| [03-首屏与运行时性能优化方案.md](./03-首屏与运行时性能优化方案.md) | 首屏关键路径、候选实验、运行时调度与回滚条件 | 指导代码施工 |
| [04-消息强渲染能力保护契约.md](./04-消息强渲染能力保护契约.md) | 能力清单、允许/禁止优化、fixture 与等价性验收 | 防止性能工程伤害内容能力 |
| [05-测量矩阵与验收门禁.md](./05-测量矩阵与验收门禁.md) | 埋点、设备矩阵、统计方法、视觉/交互门禁与 CI 分工 | 形成可复现证据 |
| [06-分期实施与Magi综合裁决.md](./06-分期实施与Magi综合裁决.md) | 三贤者审查、优先级、文件落点、提交策略与停止条件 | 直接交付实施负责人 |

## 当前证据等级

| 等级 | 含义 | 本轮结果 |
|---|---|---|
| `CONFIRMED-CODE` | 当前源码或生成产物可以直接证明 | 启动门禁组成、依赖图、平板根布局矛盾、CSS fallback 缺口 |
| `DIAGNOSTIC-BUILD` | 当前 dirty worktree 的一次生产 Web build 快照 | `pnpm build` 通过；首屏资源约 926 KB raw / 264 KB gzip，尚不能由单一 commit 复现 |
| `HYPOTHESIS-HIGH` | 有明确机制与高风险数据上限，但尚未分段计时 | 全量头像 IPC/Blob、READY 后 Overlay 导入风暴、全局 KaTeX CSS |
| `DEVICE-PENDING` | 必须在实际 Android WebView 上确认 | Release 首屏耗时、旧机具体丢样式、平板截图、GPU 合成与最终阈值 |

任何后续报告都必须保留这些标签。特别是，Rust 亚毫秒 benchmark、Debug 构建、`am start -W`、happy-dom 单测和现代桌面 Chrome 都不能替代 Release APK 的真机首屏结论。

## 本轮事实快照

- 当前分支：`main`，HEAD `99fce5f`，相对远端领先 1 个提交。
- 工作区已有 Diary 相关未提交改动；本专项只新增本目录文档，没有修改这些代码。
- `pnpm build` 在当前工作区通过，Vite 共转换 4450 个模块。
- 当前忽略的 `dist/index.html` 将 main、Vue vendor、Tauri vendor、render vendor 和主 CSS 全部列入初始脚本或 `modulepreload`。
- `dist/` 被 `.gitignore` 忽略，且构建包含当前 Diary WIP，因此数值只能作为诊断快照，不能直接冻结为长期基线。
- 当前没有连接 Android 设备，也没有为本专项构建 Release APK；所有真机耗时和截图结论仍待采集。

## 已冻结的总体决策

1. 先补全测量面，再逐个候选 A/B；不凭 bundle 名称或主观手感重构。
2. 沿用现有 `AppLifecycle` 状态所有权，在其内部区分“阻塞交互的核心工作”和“READY 后的暖机工作”；不另建第二套启动状态机。
3. 兼容策略采用同一份 DOM、同一份主题语义和“基础声明在前、现代增强在后”；不做 UA sniff，不创建旧机专用主题。
4. 移动基线保持抽屉覆盖；平板是否常驻双栏以可用宽度为主、方向只作辅助信号，不能用单一 `768px` 断点把所有设备强制成双栏。
5. Android 安全区以原生 `WindowInsetsCompat` 的四边数据为权威，CSS `env()` 作为 Web 环境 fallback；功能组件不得各自发明安全区公式。
6. 消息强渲染的语义与最终 DOM/视觉结果是硬门禁；性能优化若需要删能力，方案直接否决。
7. 首轮只做可独立回滚的低风险实验；不进行依赖全升级、God File 拆分、全面虚拟列表或全局 polyfill。

## 外部依据

- Android 官方说明：Android 7.0 起用户可选择 WebView 提供程序，应用可通过 `WebViewCompat.getCurrentWebViewPackage()` 取得实际包与版本；因此 Android API 级别不能替代 WebView 版本记录：[Manage WebView objects](https://developer.android.com/develop/ui/views/layout/webapps/managing-webview)。
- Android 官方进一步说明：应用可以控制 Jetpack WebKit 库版本，但不能控制用户设备上的 WebView APK 更新，应采用特性检测：[Simplify your WebView implementation with Jetpack WebKit](https://developer.android.com/develop/ui/views/layout/webapps/jetpack-webkit-overview)。
- Android edge-to-edge 指南要求同时考虑 system bars 与 display cutout；横屏切口可能位于垂直边缘：[Display content edge-to-edge in views](https://developer.android.com/develop/ui/views/layout/edge-to-edge)。
- Chrome 官方兼容资料显示，`color-mix()` 从 Chrome 111 才进入支持矩阵：[CSS color-mix()](https://developer.chrome.com/docs/css-ui/css-color-mix)。
- Chrome 官方资料显示，`content-visibility: auto` 从 Chrome 85 提供；本项目只能把它视为性能增强，不能作为内容存在的前提：[New in Chrome 85](https://developer.chrome.com/blog/new-in-chrome-85)。

## 一句话实施准则

> 先测出时间花在哪里，再把非关键工作移出首屏；用兼容 fallback 保住结构，用调度和缓存保住强渲染，绝不靠删内容换速度。
