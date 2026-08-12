# VCPMobile Android WebView 多设备兼容专项

> 状态：`IMPLEMENTED / SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PENDING`
>
> 实施日期：2026-08-13
>
> 恢复基线：clean checkpoint `cecdbe4`（已包含最新 Diary 功能）
>
> 主题：Android arm64 平台治理、手机/平板/折叠屏窗口适配、WebView CSS/Insets 与强渲染保护

## 结论

本轮目标已经收敛为**Android 多设备兼容与项目规范**。实现已落地并通过当前工作树的软件验证：工作区采用横向 shell，窄窗口为双抽屉，中等窗口常驻左栏，宽窗口可双栏；CSS 建立稳定 fallback；原生 Insets 覆盖四边并以 IME 净增量避让；平台、测试与文档口径统一。

产品支持只覆盖 Android `arm64-v8a` 触控手机、平板和按当前窗口宽度响应的折叠屏。Windows/macOS/Linux desktop、iOS、Android TV 均不支持；桌面端由 VCPChat 承担。host scaffold、Vite preview 与非 Android fallback 仅用于开发测试。

当前只剩具名多设备验收尚未归档。软件测试绿色不能替代 Android WebView 真机证据。

兼容专项坚持一条不可协商的产品约束：

> **兼容修复不得关闭、删除、纯文本化或降级消息的 Markdown、代码高亮、KaTeX、Mermaid、SVG/MathML、受控 raw HTML、Tool、Thought、Diary、附件与流式 AST 差分能力。**

性能优化已延期且不是本轮完成条件；仓库既有 perf 脚本与 Criterion 资产保留，不删除、不扩建、不冻结 SLA。

## 文档导航

| 文档 | 主要内容 | 用途 |
|---|---|---|
| [01-现状证据与问题分层.md](./01-现状证据与问题分层.md) | `cecdbe4` 基线、已解决问题与剩余证据边界 | 说明为何进入已实现态 |
| [02-WebView兼容与UI响应式规范.md](./02-WebView兼容与UI响应式规范.md) | Android 支持基线、CSS、窗口布局与 Insets | 兼容规范；长期权威版在 `docs/ANDROID_UI_COMPATIBILITY.md` |
| [03-首屏与运行时性能优化方案.md](./03-首屏与运行时性能优化方案.md) | 历史性能研究 | `DEFERRED`，不再指导本轮施工 |
| [04-消息强渲染能力保护契约.md](./04-消息强渲染能力保护契约.md) | 能力清单、fixture 与语义/交互等价性 | 兼容修复硬门禁 |
| [05-测量矩阵与验收门禁.md](./05-测量矩阵与验收门禁.md) | 自动化与具名设备矩阵 | 关闭 `DEVICE-EVIDENCE-PENDING` |
| [06-分期实施与Magi综合裁决.md](./06-分期实施与Magi综合裁决.md) | Magi 裁决、完成清单与交付边界 | 专项收尾记录 |

## 当前证据等级

| 等级 | 含义 | 本轮结果 |
|---|---|---|
| `IMPLEMENTED` | 兼容实现与规范已落地 | 横向 workspace、三种宽度表现、CSS fallback、四边 Insets |
| `SOFTWARE-VERIFIED` | 当前未提交工作树的软件验证已完成 | 静态检查、前端测试、生产构建、Android 插件 JVM 测试与生成树初始化均通过 |
| `DEVICE-EVIDENCE-PENDING` | 必须在实际 Android WebView 上确认 | 具名手机、平板、折叠/分屏窗口的截图与触控可达性 |

任何后续报告都必须保留证据标签。尤其 happy-dom、host browser 与 Rust benchmark 不能替代 Android WebView 的真实 CSS 几何、系统 Insets 和触控结论。

## 本轮事实快照

- 恢复基线为 clean `cecdbe4`，且已包含正式集成的 Diary 功能。
- 生产发布目标是签名 Android `arm64-v8a` APK；desktop/iOS/TV 不在支持矩阵。
- 仓库当前 E2E 是 Node.js + adb smoke，不是 Maestro；没有 Playwright desktop E2E。
- 软件与生成物验证由实际 runner 输出记录，不在计划里维护易漂移的模块数、测试数、bundle KB 或 CSS 命中数。
- 多设备真机验收仍未归档，因此不得标记 `DEVICE-VERIFIED`。

## 本轮软件验证记录

- `pnpm check`：通过（Vue TypeScript + Rust `cargo check --locked`）。
- `pnpm test:run -- --reporter=verbose`：24 个文件、122 项测试通过。
- `pnpm build`：通过，生产构建转换 4450 个模块。
- Android 插件 strict Gradle JVM 测试：37 项通过，0 failure/error/skip。
- `pnpm tauri android init --ci`：通过；生成树保留本轮 Android manifest 契约。
- 专项治理测试：7 项通过；`git diff --check` 通过。

这些结果只证明以 `cecdbe4` 为恢复基线的当前工作树达到 `SOFTWARE-VERIFIED`，不代表 APK 或设备矩阵已验收。

## 已冻结的总体决策

1. 生产支持仅限 Android arm64 触控手机、平板与按窗口宽度响应的折叠屏；桌面由 VCPChat。
2. 兼容策略采用同一份 DOM/Store 和“基础声明在前、现代增强在后”；不做 UA/机型分支。
3. Vite JS/CSS target 显式冻结为 `chrome87`，只作为生成语法契约，不冒充设备支持证明。
4. 窄窗口为 overlay，1024px 起可常驻左栏，1280px 起可双栏；窗口变化重新推导而不复制业务状态。
5. Android 安全区以原生四边 Insets 为权威，前端只消费一次 DPR 转换后的语义变量；IME 使用扣除 safe bottom 后的净增量。
6. 消息强渲染的语义、交互与安全边界是硬门禁。
7. 性能优化延期，既有 `tests/perf/` 与 Criterion 资产原样保留为人工诊断工具。

## 外部依据

- Android 官方说明：Android 7.0 起用户可选择 WebView 提供程序，应用可通过 `WebViewCompat.getCurrentWebViewPackage()` 取得实际包与版本；因此 Android API 级别不能替代 WebView 版本记录：[Manage WebView objects](https://developer.android.com/develop/ui/views/layout/webapps/managing-webview)。
- Android 官方进一步说明：应用可以控制 Jetpack WebKit 库版本，但不能控制用户设备上的 WebView APK 更新，应采用特性检测：[Simplify your WebView implementation with Jetpack WebKit](https://developer.android.com/develop/ui/views/layout/webapps/jetpack-webkit-overview)。
- Android edge-to-edge 指南要求同时考虑 system bars 与 display cutout；横屏切口可能位于垂直边缘：[Display content edge-to-edge in views](https://developer.android.com/develop/ui/views/layout/edge-to-edge)。
- Chrome 官方兼容资料显示，`color-mix()` 从 Chrome 111 才进入支持矩阵：[CSS color-mix()](https://developer.chrome.com/docs/css-ui/css-color-mix)。
- Chrome 官方资料显示，`content-visibility: auto` 从 Chrome 85 提供；本项目只能把它视为性能增强，不能作为内容存在的前提：[New in Chrome 85](https://developer.chrome.com/blog/new-in-chrome-85)。

## 一句话交付准则

> 当前工作树已通过软件验证；下一步只以具名 Android 设备证据关闭 `DEVICE-EVIDENCE-PENDING`，性能扩建保持延期。
