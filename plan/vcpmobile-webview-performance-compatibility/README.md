# VCPMobile Android WebView 多设备兼容与性能专项

> 兼容状态：`IMPLEMENTED / SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PENDING`
>
> 性能状态：`PERFORMANCE-REACTIVATED / PHASE-1-D0/D1-A/B-PASS / RELEASE-EVIDENCE-PENDING`
>
> 实施日期：2026-08-13
>
> 恢复基线：clean checkpoint `cecdbe4`（已包含最新 Diary 功能）
>
> 主题：Android arm64 平台治理、多设备适配、WebView CSS/Insets、强渲染保护与证据驱动的运行时减负

## 结论

本轮目标已经收敛为**Android 多设备兼容与项目规范**。实现已落地并通过当前工作树的软件验证：工作区采用横向 shell，窄窗口为双抽屉，中等窗口常驻左栏，宽窗口可双栏；CSS 建立稳定 fallback；原生 Insets 覆盖四边并以 IME 净增量避让；平台、测试与文档口径统一。

产品支持只覆盖 Android `arm64-v8a` 触控手机、平板和按当前窗口宽度响应的折叠屏。Windows/macOS/Linux desktop、iOS、Android TV 均不支持；桌面端由 VCPChat 承担。host scaffold、Vite preview 与非 Android fallback 仅用于开发测试。

当前只剩具名多设备验收尚未归档。软件测试绿色不能替代 Android WebView 真机证据。

兼容专项坚持一条不可协商的产品约束：

> **兼容修复不得关闭、删除、纯文本化或降级消息的 Markdown、代码高亮、KaTeX、Mermaid、SVG/MathML、受控 raw HTML、Tool、Thought、Diary、附件与流式 AST 差分能力。**

性能工作已于 2026-08-13 重新激活，但仍坚持“先证明、再修改、一次一个机制”。本阶段不追求首屏瞬时化，也不冻结毫秒 SLA；先消除已被真机 A/B 证明的常驻刷新、未打开页面的提前实例化，以及生产热路径中无意义的调试数据构造。Debug/HMR 结果只标记为因果证据，不冒充签名 Release 或能耗结论。

## 文档导航

| 文档 | 主要内容 | 用途 |
|---|---|---|
| [01-现状证据与问题分层.md](./01-现状证据与问题分层.md) | `cecdbe4` 基线、已解决问题与剩余证据边界 | 说明为何进入已实现态 |
| [02-WebView兼容与UI响应式规范.md](./02-WebView兼容与UI响应式规范.md) | Android 支持基线、CSS、窗口布局与 Insets | 兼容规范；长期权威版在 `docs/ANDROID_UI_COMPATIBILITY.md` |
| [03-首屏与运行时性能优化方案.md](./03-首屏与运行时性能优化方案.md) | 当前性能证据、已实施候选与后续实验队列 | `ACTIVE`，按独立 A/B 推进 |
| [04-消息强渲染能力保护契约.md](./04-消息强渲染能力保护契约.md) | 能力清单、fixture 与语义/交互等价性 | 兼容修复硬门禁 |
| [05-测量矩阵与验收门禁.md](./05-测量矩阵与验收门禁.md) | 兼容设备矩阵与性能证据分级 | 关闭设备证据缺口并约束性能声明 |
| [06-分期实施与Magi综合裁决.md](./06-分期实施与Magi综合裁决.md) | Magi 裁决、完成清单与交付边界 | 专项收尾记录 |

## 当前证据等级

| 等级 | 含义 | 本轮结果 |
|---|---|---|
| `IMPLEMENTED` | 兼容实现与规范已落地 | 横向 workspace、三种宽度表现、CSS fallback、四边 Insets |
| `SOFTWARE-VERIFIED` | 当前未提交工作树的软件验证已完成 | 静态检查、前端测试、生产构建、Android 插件 JVM 测试与生成树初始化均通过 |
| `DEVICE-EVIDENCE-PENDING` | 必须在实际 Android WebView 上确认 | 具名手机、平板、折叠/分屏窗口的截图与触控可达性 |
| `PHASE-1-D0/D1-A/B-PASS` | Dev/HMR 因果定位与 packaged Debug 成对复验均已完成 | Chat 与 RAG 稳态帧提交、首屏资源图、独立 Debug APK |
| `RELEASE-EVIDENCE-PENDING` | 尚无同 commit、同版本、签名 Release 的成对 APK | 不声明 Release 启动、功耗或长期内存收益 |

任何后续报告都必须保留证据标签。尤其 happy-dom、host browser 与 Rust benchmark 不能替代 Android WebView 的真实 CSS 几何、系统 Insets 和触控结论。

## 本轮事实快照

- 恢复基线为 clean `cecdbe4`，且已包含正式集成的 Diary 功能。
- 生产发布目标是签名 Android `arm64-v8a` APK；desktop/iOS/TV 不在支持矩阵。
- 仓库当前 E2E 是 Node.js + adb smoke，不是 Maestro；没有 Playwright desktop E2E。
- 软件与生成物验证由实际 runner 输出记录，不在计划里维护易漂移的模块数、测试数、bundle KB 或 CSS 命中数。
- 已取得一台现代 Android 手机的局部真机证据，但多设备矩阵仍未归档，因此不得标记 `DEVICE-VERIFIED`。
- 性能实现以 clean `50d782f` 为父基线；本提交只接纳已经有机制证据且不改变消息语义的候选。

## 局部真机证据

- 2026-08-13 在 commit `5a56fea` 的 arm64 Debug 包上完成 OPPO PHZ110 快速 UI 验收；APK SHA-256 为 `5b155e418d463e740006720ad37fe518ec4db2a468944261e7b96e234c62e0a7`。设备为 Android 16 / API 36、Google WebView `150.0.7871.181`、360 × 792 CSS px、DPR 3、字体缩放 1.0、手势导航、竖屏。
- 验收人快速检查了当前可达 UI、滚动、左右抽屉与键盘，未发现可见回归；受控交互为 5715 frames / 9 frame deadline misses（0.16%），未观察到 FATAL、ANR 或 Chromium fatal error。
- 原生顶部 cutout 120 physical px 经 DPR 3 转为 `--vcp-safe-top: 40px`；键盘 900 physical px 转为 `--vcp-ime-offset: 300px`，visual viewport 同步从 792px 降至 492px，没有单位错误或重复累加。
- 本机自动旋转关闭，本轮没有归档 landscape 证据；平板、折叠/分屏、1.5× 字体、三键导航与最低 WebView 仍待验收，所以状态继续为 `DEVICE-EVIDENCE-PENDING`。

## 本轮软件验证记录

- `pnpm check`：通过（Vue TypeScript + Rust `cargo check --locked`）。
- `pnpm test:run -- --reporter=verbose`：26 个文件、129 项测试通过。
- `pnpm build`：通过，候选生产构建转换 4449 个模块。
- Android 插件 strict Gradle JVM 测试：37 项通过，0 failure/error/skip。
- `pnpm tauri android init --ci`：通过；生成树保留本轮 Android manifest 契约。
- 专项治理测试：7 项通过；`git diff --check` 通过。

性能 Phase 1 增加了稳态动画、首开挂载和调试热路径治理测试；最新数量以 runner 输出为准。上述结果只证明当前工作树达到 `SOFTWARE-VERIFIED`，不代表签名 APK、功耗或完整设备矩阵已验收。

## 性能 Phase 1 证据

- PHZ110 Debug/HMR 的静态 Chat：`CORE ACTIVE` 呼吸灯运行时 10 秒约 `+610` 帧，移除永久动画后 10 秒 `+0`；READY、初始化、断连与错误继续由颜色和文字区分。
- 同一设备、同一已打开 RAG 页面：旧频谱 Canvas 空闲 10 秒 `+598` 帧；改为事件触发、回落后停止 rAF 后 10 秒 `+0`。
- 首次冷开 Debug 页面 ResourceTiming 从 226 项降至 173 项，减少 53 项（23.5%）；Settings、Agent/Group、Tarven、Distributed、RAG 与 Diary 不再在关闭状态提前加载。验收人已打开并关闭 Settings/RAG，首开加载与关闭后 DOM 保留正常。
- 同机同源生产 Web 构建中，主 JS 从 456.06 kB / gzip 135.88 kB 降到 416.50 kB / gzip 126.79 kB；主 CSS 从 211.22 kB / gzip 39.01 kB 降到 208.92 kB / gzip 38.57 kB。体积变化是 import graph 与无效调试代码消除的生成物证据，不等同于启动耗时收益。
- production dist 已不再包含 AST/stream debug 的重对象字段、序列化日志字符串，以及被移除的常驻动画 keyframes。
- D1 使用父基线 `50d782f` 与候选工作树分别构建 production-frontend Debug APK；两者均为 `com.vcp.avatar.debug`、`1.1.4-debug`、versionCode `1001004`。基线 `f80bdcc9…` 与候选 `6d4bc156…` 在同一 PHZ110、同一数据上成对复验：稳定 Chat 10 秒为 `609 → 0` frames，首屏 ResourceTiming 为 `27 → 10` 项，运行中的 CSS 动画为 `2 → 0`。
- D1 单点内存快照为 PSS `265561 → 238060 KiB`、RSS `403768 → 374912 KiB`，只能作为后续长稳复测线索；5 次 process-cold `am start -W` 的中位数为 `480 → 506 ms`，未显示启动收益，样本量也不足以建立 SLA。
- 当前设备上的候选 Debug APK SHA-256 为 `6d4bc156e87d60797a2c6a7623d6fd45b7d385e150e7f6407337e3d25402e31c`。正式 Release `com.vcp.avatar` 未参与安装、清数据或性能实验，其 SHA-256 始终保持官方 v1.1.3 资产 `d5ad6378…`。

## 已冻结的总体决策

1. 生产支持仅限 Android arm64 触控手机、平板与按窗口宽度响应的折叠屏；桌面由 VCPChat。
2. 兼容策略采用同一份 DOM/Store 和“基础声明在前、现代增强在后”；不做 UA/机型分支。
3. Vite JS/CSS target 显式冻结为 `chrome87`，只作为生成语法契约，不冒充设备支持证明。
4. 窄窗口为 overlay，1024px 起可常驻左栏，1280px 起可双栏；窗口变化重新推导而不复制业务状态。
5. Android 安全区以原生四边 Insets 为权威，前端只消费一次 DPR 转换后的语义变量；IME 使用扣除 safe bottom 后的净增量。
6. 消息强渲染的语义、交互与安全边界是硬门禁。
7. 性能优化作为独立 report-only 轨道推进；既有 `tests/perf/` 与 Criterion 保持人工诊断定位，不设置未经校准的 CI/Release SLA。

## 外部依据

- Android 官方说明：Android 7.0 起用户可选择 WebView 提供程序，应用可通过 `WebViewCompat.getCurrentWebViewPackage()` 取得实际包与版本；因此 Android API 级别不能替代 WebView 版本记录：[Manage WebView objects](https://developer.android.com/develop/ui/views/layout/webapps/managing-webview)。
- Android 官方进一步说明：应用可以控制 Jetpack WebKit 库版本，但不能控制用户设备上的 WebView APK 更新，应采用特性检测：[Simplify your WebView implementation with Jetpack WebKit](https://developer.android.com/develop/ui/views/layout/webapps/jetpack-webkit-overview)。
- Android edge-to-edge 指南要求同时考虑 system bars 与 display cutout；横屏切口可能位于垂直边缘：[Display content edge-to-edge in views](https://developer.android.com/develop/ui/views/layout/edge-to-edge)。
- Chrome 官方兼容资料显示，`color-mix()` 从 Chrome 111 才进入支持矩阵：[CSS color-mix()](https://developer.chrome.com/docs/css-ui/css-color-mix)。
- Chrome 官方资料显示，`content-visibility: auto` 从 Chrome 85 提供；本项目只能把它视为性能增强，不能作为内容存在的前提：[New in Chrome 85](https://developer.chrome.com/blog/new-in-chrome-85)。

## 一句话交付准则

> 兼容结论只由具名 Android 设备证据关闭；性能结论只由同身份 A/B 关闭，Debug 因果证据不得升级成 Release、能耗或全设备结论。
