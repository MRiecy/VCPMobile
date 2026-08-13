# Android UI 兼容性规范

> 文档类型：产品支持与工程规范
> 状态：`IMPLEMENTED / SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PENDING`
> 恢复基线：`cecdbe4`（包含最新 Diary 功能）；验证对象为当前未提交工作树
> 生效日期：2026-08-13

## 1. 产品支持边界

VCP Mobile 的生产运行、构建、发布与兼容性承诺仅覆盖以下目标：

- Android `arm64-v8a` 触控手机；
- Android `arm64-v8a` 触控平板；
- Android `arm64-v8a` 折叠屏，按**当前应用窗口的可用 CSS 宽度**响应，不按设备名称或物理形态分叉。

下列平台明确不受支持：

- Windows、macOS、Linux 桌面端；桌面用户应使用 VCPChat；
- iOS；
- Android TV、遥控器/键盘为主要输入方式的 Android 设备；
- 非 `arm64-v8a` Android ABI。

仓库中的 Vite 浏览器预览、Rust host 编译、非 Android fallback 与 Tauri desktop scaffold 只用于开发、静态检查和测试。它们不是可分发产品入口，也不建立桌面端兼容义务。`pnpm tauri` 脚本必须保留，因为 Android 子命令也通过它调用；`tauri.conf.json` 的 desktop bundle schema 同样不代表桌面发布支持。

## 2. 单一路径与窗口语义

手机、平板与折叠屏必须共用同一份 Vue 组件树、Pinia 状态与 Rust/Tauri 业务契约。禁止按机型、品牌、UA 或“手机/平板”复制业务页面。

布局只由当前 WebView viewport 的可用宽度决定。旋转、分屏、自由窗与折叠/展开都必须重新计算；物理屏幕分辨率、Android 设备类别和方向只能作为诊断字段，不能直接决定布局模式。

项目采用三种表现模式：

| 模式 | 窗口条件 | 侧栏表现 | 主内容要求 |
|---|---|---|---|
| `overlay` | `<1024px` | 左右侧栏均为覆盖式抽屉 | 主区占满可用宽度 |
| `single-pane` | `1024–1279px` | 左栏 280px 常驻，右栏仍为覆盖抽屉 | 保证聊天主区最小可读宽度 |
| `dual-pane` | `>=1280px` | 左栏 280px、右栏 300px 常驻 | 主区仍可滚动、输入和显示强渲染内容 |

当前 1024/1280 阈值由“侧栏宽度 + 主内容最小宽度 + 必要间距”推导。不得把它们命名为“桌面断点”，也不得仅凭 landscape 强制双栏。`layoutStore` 继续拥有抽屉 open 状态；响应式模式仅决定呈现方式，不创建第二套业务状态机。single-pane 的右抽屉仍必须显示遮罩并接受返回手势，只有 dual-pane 才能隐藏两侧抽屉遮罩。

所有横向 flex 子项必须明确 `min-width: 0`，纵向滚动链必须明确 `min-height: 0` 与唯一 overflow owner。长代码、表格、Mermaid、长中文名称和 UUID 不得撑破工作区。

## 3. WebView 与 CSS 契约

`minSdk` 只规定 Android API 安装下限，不代表 WebView 特性下限。`vite.config.ts` 显式冻结 `build.target = "chrome87"` 与 `cssTarget = "chrome87"`，用于防止依赖升级静默抬高生成 JS/CSS 语法门槛；它不是 Android/WebView 产品支持声明，也不能替代 fallback 和设备验收。设备证据必须记录 Android API、WebView provider/version、viewport、density 与字体缩放；不能通过系统版本或 UA 猜测 WebView 能力。

CSS 按影响分为三层：

1. **结构层**：display、定位、尺寸、overflow、滚动、z-index、安全区与键盘避让。必须在支持基线内可用，或有等价 fallback。
2. **语义视觉层**：背景、边框、选中/错误/禁用状态和对比度。先提供稳定实色或主题 token，再以现代语法增强。
3. **装饰/性能增强层**：`color-mix()`、`content-visibility`、mask、轻微 blur、动画。缺失时只能少一点装饰或速度，不能丢内容、交互或状态语义。

具体规则：

- 关键 `color-mix()` 前必须有同属性稳定 fallback；动态 inline style 使用 feature probe 或预计算 token；
- `content-visibility` 只能优化离屏工作，不能控制内容是否存在；
- 结构性 shorthand 与 UnoCSS 生成结果必须检查生产 CSS，不能只看 Vue 源码；
- 内容区域禁止 backdrop blur；fixed/sticky 单例元素最多轻度 blur（不超过 12px）；
- `transition-all` 与 `will-change` 只在明确交互热点使用，不能让历史消息长期持有合成层；
- 全局覆盖层只使用语义化 z-index。权威顺序见 [UI_LAYER_ARCHITECTURE.md](./UI_LAYER_ARCHITECTURE.md)。

## 4. 安全区与键盘 Insets

原生层提供四边安全区和 IME 快照，单位为 Android 物理像素：

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

## 5. 强渲染不退化

所有受支持窗口模式都必须保留同一消息能力：Markdown、代码高亮、KaTeX、Mermaid、SVG/MathML、受控 raw HTML、Tool、Thought、Diary/DailyNote、附件和流式 AST 收敛。

兼容性修复不得：

- 对窄屏、旧 WebView 或平板隐藏复杂块、改成摘要或纯文本；
- 破坏 Tool/Thought 展开、Diary 操作、代码复制、Mermaid 全屏、附件查看、文本选择、焦点或 scroll anchor；
- 绕过富 HTML 过滤或扩大 active content；
- 把异步资源移出 APK，导致离线首次使用失败。

完整不变量与 fixture 见计划目录中的 `04-消息强渲染能力保护契约.md`。

## 6. 可检测契约

每个修改 Android shell、侧栏、Overlay、CSS fallback 或 Insets 的 PR，至少满足：

- `pnpm check`；
- `pnpm test:run`；
- 生产 `pnpm build`，确认实际生成 CSS/JS；
- 布局契约测试覆盖三种宽度模式、窗口变化、抽屉可达性与主区 `min-width: 0`；
- 样式治理测试覆盖关键 `color-mix()` fallback、受控 blur、语义 z-index 与安全区变量；
- Insets 纯函数/事件契约覆盖四边、DPR 转换、IME 净增量、重放和旧字段兼容；
- 强渲染 fixture 的语义和交互不变量不退化。

Vitest + happy-dom 可以证明状态、DOM 契约和生成物静态约束，但不计算真实 CSS 几何。仓库当前没有 Playwright 桌面 E2E，也没有 Maestro flow；不得把二者写成现有门禁。Android 设备诊断采用仓库已有的 Debug-only Agent CLI，真机截图与触控可达性仍需具名设备证据，不能由 CLI 状态快照替代。

## 7. 设备验收矩阵

每条设备记录必须绑定：commit、APK SHA-256、设备型号、Android API、ABI、WebView provider/version、CSS viewport、物理分辨率、density、字体缩放、方向/分屏状态、导航模式和验收人。

最低场景矩阵：

| 设备/窗口 | 必测状态 |
|---|---|
| 窄手机 portrait | 启动、Chat、左右抽屉、输入法、主要 Overlay、强渲染 |
| 手机 landscape / 分屏窄窗 | 重新回到 overlay、无水平裁切、键盘可用 |
| 平板 portrait | overlay 或 single-pane 与窗口宽度一致 |
| 平板 landscape | single-pane / dual-pane 切换，主区不被两侧栏挤没 |
| 折叠屏折叠/展开与分屏 | 不重启业务状态；按新 viewport 重新布局 |
| 最低支持 WebView 锚点 | L0 启动、L1 核心交互、L2 强渲染全部通过 |
| 现代 WebView 对照机 | 防止兼容 fallback 破坏增强路径 |

每个状态同时检查浅色/深色、字体 1.0×/1.5×、长内容、四边安全区、键盘开合与 Android 返回手势。模拟 viewport 只能作为前置筛查，不能替代目标 Android WebView 的截图与触控验证。

当前专项状态为 `SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PENDING`：当前工作树已通过 `pnpm check`、前端全量测试、生产构建、Android 插件 strict JVM 测试与 Android 生成树初始化；这仍不等于手机、平板、折叠屏和最低 WebView 已通过发布验收。具名设备报告归档前不得写成 `DEVICE-VERIFIED`。

## 8. 性能边界

当前首屏体验不再作为本专项优化目标。仓库已有 `tests/perf/`、Criterion benchmark 与相关脚本继续保留，作为人工诊断和独立回归资产；本轮不删除、不扩建启动埋点、不冻结毫秒 SLA，也不以性能数字换取兼容性或强渲染能力。未来只有在可复现用户问题出现时，才以独立任务重新建立基线和验收标准。
