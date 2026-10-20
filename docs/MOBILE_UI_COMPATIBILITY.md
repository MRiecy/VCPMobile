# 移动端 UI 兼容性规范（Android / iOS）

> 文档类型：产品支持与工程规范
> 修订日期：2026-10-20

Vue 运行时平台分支、原生编译隔离、权限与用户提示见 [跨平台开发规范](./CROSS_PLATFORM_DEVELOPMENT.md)。本文件负责窗口、布局、CSS、Insets 和触控契约；阶段进度与验证结果留在对应计划记录中。

## 1. 产品支持边界

VCP Mobile 当前 Android 生产运行、构建与发布覆盖以下目标：

- Android `arm64-v8a` 触控手机；
- Android `arm64-v8a` 触控平板；
- Android `arm64-v8a` 折叠屏，按**当前应用窗口的可用 CSS 宽度**响应，不按设备名称或物理形态分叉。

Android 安装下限仍为 API 26；应用使用的 **Android WebView / Chromium 内核最低版本为 117**。两者分别约束系统安装条件与 Web 运行能力：较新的 Android 系统不保证实际 WebView provider 已更新，较旧的 Android 系统也不能单凭系统版本判定内核能力。低于 WebView 117 不在当前支持范围内。

iPhone/iPad 原生客户端的目标为同一 Vue/Rust 核心加 Swift adapter、单 Scene、当前窗口宽度响应，以及供用户自行签名的实验性 device 产物。最低支持 iOS/iPadOS 26.0（2026-10-09 用户决定）；高于该基线的 API 按 availability 分层。SDK 版本与最低运行版本分别管理，不随系统年度发布自动抬高最低版本。

功能范围以 [D1–D19](../plan/vcpmobile-ios-compatibility-research/09-Magi综合裁决与待决策清单.md) 及用户最新的 [功能审批](../plan/vcpmobile-ios-compatibility-research/phase-2/09-功能权限与提示审议表.md) 为准，最新逐项意见优先于旧建议。实施与证据见 [iOS 阶段记录](../plan/vcpmobile-ios-compatibility-research/README.md)，本规范不另行复制功能清单。

下列平台明确不受支持：

- Windows、macOS、Linux 桌面端；桌面用户应使用 VCPChat；
- Android TV、遥控器/键盘为主要输入方式的 Android 设备；
- 非 `arm64-v8a` Android ABI。

仓库中的 Vite 浏览器预览、Rust host 编译与 Tauri desktop scaffold 只用于开发、静态检查和测试。现有非 Android fallback 不能作为 iOS 实现或证据。开发路径不建立桌面端兼容义务。`pnpm tauri` 脚本必须保留，因为移动端子命令通过它调用；`tauri.conf.json` 的 desktop bundle schema 同样不代表桌面发布支持。

## 2. 单一路径与窗口语义

Android 手机/平板/折叠屏与 iPhone/iPad 必须共用同一份 Vue 组件树、Pinia 状态与 Rust/Tauri 业务契约。禁止按机型、品牌、UA 或“手机/平板”复制业务页面。

布局只由当前 WebView viewport 的可用宽度决定。旋转、分屏、自由窗与折叠/展开都必须重新计算；物理屏幕分辨率、Android 设备类别和方向只能作为诊断字段，不能直接决定布局模式。

项目采用三种表现模式：

| 模式 | 窗口条件 | 侧栏表现 | 主内容要求 |
|---|---|---|---|
| `overlay` | `<1024px` | 左右侧栏均为覆盖式抽屉 | 主区占满可用宽度 |
| `single-pane` | `1024–1279px` | 左栏 280px 常驻，右栏仍为覆盖抽屉 | 保证聊天主区最小可读宽度 |
| `dual-pane` | `>=1280px` | 左栏 280px、右栏 300px 常驻 | 主区仍可滚动、输入和显示强渲染内容 |

当前 1024/1280 阈值由“侧栏宽度 + 主内容最小宽度 + 必要间距”推导。不得把它们命名为“桌面断点”，也不得仅凭 landscape 强制双栏。`layoutStore` 继续拥有抽屉 open 状态；响应式模式仅决定呈现方式，不创建第二套业务状态机。single-pane 的右抽屉仍必须显示遮罩并接受返回手势，只有 dual-pane 才能隐藏两侧抽屉遮罩。

所有横向 flex 子项必须明确 `min-width: 0`，纵向滚动链必须明确 `min-height: 0` 与唯一 overflow owner。长代码、表格、Mermaid、长中文名称和 UUID 不得撑破工作区。

## 3. WebView / WKWebView 与 CSS 契约

`minSdk` 只规定 Android API 安装下限，不代表 WebView 特性下限。共享移动包的 `vite.config.ts` 必须将 `build.target` 与 `cssTarget` 同步指定为 `["chrome117", "safari26.0"]`，分别对应 Android WebView 117 与最低 iOS/iPadOS 26.0。构建目标限制生成 JS/CSS 的语法，不会自动补齐运行时内建 API，也不能替代设备验收。依赖升级时核对实际执行路径使用的 API，而非只看包声明的 ECMAScript 年份。

设备证据必须记录 Android API、WebView provider/version、viewport、density 与字体缩放。系统版本、UA 或单次 feature probe 各自只说明已取得的信息，不能推导未验证的完整兼容结论。

iOS 使用 WKWebView，Chromium 版本要求不适用于 iOS。`safari26.0` 构建目标也不构成 WebKit 交互验收；仍需沿实际 DOM、生成 CSS/JS 与平台 Insets 核对。iPhone/iPad 输入控件目标字号至少 16 CSS px，触控命中区至少 44×44 CSS px（可用 hit slop 保持视觉密度）；文本选择优先于祖先长按手势。

Android 兼容说明归现有“帮助与指引”入口，Android 启动后在指引可呈现的首个时机自动显示一次，完成后可从帮助入口重看；不依赖用户先打开设置或进入某个聊天。说明告知 WebView 117+ 要求，并指导用户更新设备实际使用的 WebView provider；iOS 不注册或显示这条 Android 指引。该说明依赖 Vue 与指引运行时已成功加载；低版本内核若在模块解析或启动执行时失败，应用内说明可能无法出现。不得把它描述为启动前兼容检测、强制门禁或旧内核白屏保障。

CSS 按影响分为三层：

1. **结构层**：display、定位、尺寸、overflow、滚动、z-index、安全区与键盘避让。必须在支持基线内可用，或有等价 fallback。
2. **语义视觉层**：背景、边框、选中/错误/禁用状态和对比度。使用主题 token 与受支持基线内的颜色语法，保持状态可辨认。
3. **装饰/性能增强层**：`content-visibility`、mask、轻微 blur、动画等。超出支持基线的能力须可降级，缺失时不能丢内容、交互或状态语义。

具体规则：

- 常用 `color-mix()` 已包含在当前 WebView 117 / WKWebView 26 基线内，可用于主题与语义颜色，不要求为低于基线的内核重复添加同属性 fallback；现有实色 token、`@supports` 与预计算颜色继续按其实际用途保留，无需因提高基线重写全部 CSS。新增高于基线的语法仍须 feature probe 或等价 fallback；[浏览器支持参考](https://developer.chrome.com/docs/css-ui/css-color-mix)。
- `content-visibility` 只能优化离屏工作，不能控制内容是否存在；
- 结构性 shorthand 与 UnoCSS 生成结果必须检查生产 CSS，不能只看 Vue 源码；
- 内容区域禁止 backdrop blur；fixed/sticky 单例元素最多轻度 blur（不超过 12px）；
- `transition-all` 与 `will-change` 只在明确交互热点使用，不能让历史消息长期持有合成层；
- 全局覆盖层只使用语义化 z-index。权威顺序见 [UI_LAYER_ARCHITECTURE.md](./UI_LAYER_ARCHITECTURE.md)。

## 4. 安全区与键盘 Insets

### 4.1 Android 既有契约

Android 原生层提供四边安全区和 IME 快照，单位为物理像素：

```ts
interface NativeInsetsSnapshot {
  safeTopPx: number
  safeRightPx: number
  safeBottomPx: number
  safeLeftPx: number
  imeBottomPx: number
  imeVisible: boolean
}
```

契约如下：

- `safe*Px` 为 system bars 与 display cutout 各边最大值；
- 原生层每次注入先更新 `window.__VCP_NATIVE_INSETS__`；WebView listener 就绪后由 App 根桥同步重放，不能依赖首次事件恰好不丢；
- 前端在唯一桥接点按 `devicePixelRatio` 转换为 CSS px，并写入 `--vcp-safe-top/right/bottom/left`；
- 键盘额外占用统一为 `max(0, imeBottomPx - safeBottomPx)`，写入 `--vcp-ime-offset`；
- 组件消费语义变量，禁止各自累加 raw IME 与 safe bottom；
- 旋转、分屏、折叠状态变化、沉浸式系统栏与键盘开合都必须刷新快照。

`env(safe-area-inset-*, fallback)` 的 fallback 只在语法/变量不可用时生效，返回 `0` 时不会自动提供最小间距；需要最小顶距时必须用明确的 `max()` 或归一化规则表达。

### 4.2 iOS 目标契约

- 安全区来自 WKWebView CSS 环境量或经坐标换算的原生快照，确定一个权威来源；不能同时叠加。
- Swift 的 point、屏幕坐标、WebView 坐标与 CSS px 显式转换，不能套用 Android raw px 除 DPR 的公式。
- 键盘避让扣除 `visualViewport` 已反映的缩小量；浮动/分离键盘按实际相交区域处理，硬件键盘不伪造底部高度。
- 归一化后继续消费共享 `--vcp-safe-*` 与 `--vcp-ime-offset`，不在组件中新增第二套 Insets owner。
- iPad picker/dialog/popover 的锚点、旋转与分屏更新需沿实际 presentation owner 处理；应用内返回复用 modal/page/session 顺序，根返回保持应用，不调用 Android 最小化语义。

原生 DTO、快照重放、单位和去重字段沿实际实现核对；本节的目标契约不表示 Swift 适配已经完成。

## 5. 强渲染不退化

所有受支持窗口模式都必须保留同一消息能力：Markdown、代码高亮、KaTeX、Mermaid、SVG/MathML、受控 raw HTML、Tool、Thought、Diary/DailyNote、附件和流式 AST 收敛。

兼容性修复不得：

- 对窄屏、支持基线内的 WebView 或平板隐藏复杂块、改成摘要或纯文本；
- 破坏 Tool/Thought 展开、Diary 操作、代码复制、Mermaid 全屏、附件查看、文本选择、焦点或 scroll anchor；
- 绕过富 HTML 过滤或扩大 active content；
- 把异步资源移出平台应用包（Android APK / iOS app bundle），导致离线首次使用失败。

完整不变量与 fixture 见计划目录中的 `04-消息强渲染能力保护契约.md`。

## 6. 可检测契约

以下保留 Android UI 改动的验证要求。iOS 日常按源码入口、目标 cfg、字段、分支和资源释放逐条审计，必要时运行 host 辅助检查；Apple 构建只在 G1/G2 阶段通过 GitHub Actions 手动执行。项目现在及后续均不建设 iOS 实机测试体系，也不将下节 Android 设备矩阵扩展为 iOS 门禁。具体执行边界见 [07](../plan/vcpmobile-ios-compatibility-research/07-CI测试矩阵与无设备验证边界.md)。

修改 Android shell、侧栏、Overlay、CSS fallback 或 Insets 时，按根规范完成 `pnpm check`，并围绕本次尚未确定的问题选择验证。布局变化关注实际窗口与抽屉可达性；构建或样式转换关注生产生成物；Insets 变化关注单位、键盘净增量和事件重放；消息界面变化关注受影响的渲染与交互。无需为无关改动重复整套测试矩阵。

Vitest + happy-dom 可以证明状态、DOM 契约和生成物静态约束，但不计算真实 CSS 几何。仓库当前没有 Playwright 桌面 E2E，也没有 Maestro flow；不得把二者写成现有门禁。Android 设备诊断采用仓库已有的 Debug-only Agent CLI，真机截图与触控可达性仍需具名设备证据，不能由 CLI 状态快照替代。

## 7. Android 设备验收矩阵

每条设备记录必须绑定：commit、APK SHA-256、设备型号、Android API、ABI、WebView provider/version、CSS viewport、物理分辨率、density、字体缩放、方向/分屏状态、导航模式和验收人。

最低场景矩阵：

| 设备/窗口 | 必测状态 |
|---|---|
| 窄手机 portrait | 启动、Chat、左右抽屉、输入法、主要 Overlay、强渲染 |
| 手机 landscape / 分屏窄窗 | 重新回到 overlay、无水平裁切、键盘可用 |
| 平板 portrait | overlay 或 single-pane 与窗口宽度一致 |
| 平板 landscape | single-pane / dual-pane 切换，主区不被两侧栏挤没 |
| 折叠屏折叠/展开与分屏 | 不重启业务状态；按新 viewport 重新布局 |
| 最低支持 WebView 117 锚点 | L0 启动、L1 核心交互、L2 强渲染全部通过 |
| 现代 WebView 对照机 | 防止兼容 fallback 破坏增强路径 |

每个状态同时检查浅色/深色、字体 1.0×/1.5×、长内容、四边安全区、键盘开合与 Android 返回手势。模拟 viewport 只能作为前置筛查，不能替代目标 Android WebView 的截图与触控验证。

设备兼容结论应指向相应具名设备记录；源码、host 检查和构建结果不代替实际设备证据。

## 8. 性能边界

当前首屏体验不再作为本专项优化目标。仓库已有 `tests/perf/`、Criterion benchmark 与相关脚本继续保留，作为人工诊断和独立回归资产；本轮不删除、不扩建启动埋点、不冻结毫秒 SLA，也不以性能数字换取兼容性或强渲染能力。未来只有在可复现用户问题出现时，才以独立任务重新建立基线和验收标准。

iOS 沿用 D16：以静态正确性与明确的像素、缓冲、副本、并发硬上限审计资源预算；设备性能专项为范围外。不得把静态预算描述成设备性能测量结果。
