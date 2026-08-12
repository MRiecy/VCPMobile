# 02. WebView 兼容与 UI 响应式规范

## 1. 兼容基线的定义

VCPMobile 的兼容性不能用一句“支持 Android 8”表达。至少要同时记录四个维度：

| 维度 | 当前事实 | 它能证明什么 | 它不能证明什么 |
|---|---|---|---|
| Android API | `minSdk 26` | 应用可安装的系统 API 下限 | CSS/JS 引擎能力 |
| Android target | `targetSdk 36` | Android 15+ 会按 target 35+ 规则强制 edge-to-edge | 不会自动替应用处理 Web 内容安全区 |
| WebView provider | 当前未采集 | 设备实际使用哪个 WebView 包 | 单凭 OEM/系统版本无法推断 |
| WebView version | 当前未采集 | Chromium/CSS/JS 特性的大致能力 | 不代替运行时 feature probe |
| Web build target | Vite 未显式声明；当前安装版本解析出的默认目标为 `es2020 / edge88 / firefox78 / chrome87 / safari14` | JS 转译和部分 CSS 处理的构建目标 | 不会自动为所有新 CSS 生成视觉等价 fallback，也不是产品支持承诺 |

Android 官方明确指出，Android 7.0 起用户可选择 WebView 提供程序；Jetpack WebKit 的版本由应用控制，但用户设备上的 WebView APK 更新不受应用控制。因此发布证据必须记录 `WebViewCompat.getCurrentWebViewPackage()` 的 packageName/versionName，并对必要能力做 feature check，而不是通过 UA 字符串猜测。`CSS.supports()` 适合语法探测，但不能可靠证明 flex gap 的几何行为；这类能力需要一次性 DOM 几何探针或目标设备布局验收。

### 1.1 支持级别

建议把产品支持契约冻结为三层：

| 层级 | 硬要求 |
|---|---|
| L0 启动与恢复 | 能进入应用、看到错误或主界面；Boot/PermissionGate 不被裁切；恢复路径可操作 |
| L1 核心交互 | Chat 可读写、抽屉/Overlay 可达、键盘不遮输入、旋转后布局稳定、设置可保存 |
| L2 强渲染 | Markdown、代码、KaTeX、Mermaid、SVG/MathML、raw HTML、Tool/Thought/Diary、附件与流式最终结果完整 |

任何受支持设备都必须满足 L0—L2。现代 CSS 动画、混色和模糊可以渐进增强，但不得决定文字是否可见、控件是否可点击或内容能力是否存在。

最终最低 WebView 版本不能由本文件臆造，应由“真实受影响旧手机 + 真实受影响平板 + 现代对照机”的数据决定。产品若决定停止支持某个引擎版本，必须形成显式发布策略，不能靠页面自然坏掉来隐式淘汰。

## 2. CSS 三层模型

所有产品外壳样式按影响分级：

### A. 结构性声明

包括 `display`、布局方向、定位、尺寸、overflow、滚动、可点击区域、z-index、安全区和键盘避让。

规则：

- 必须在支持基线内原生可用，或提供等价 fallback；
- 不得只存在于 `@supports` 的现代分支；
- 不得依赖装饰属性“恰好生效”来维持可读性；
- 生成 CSS 必须被检查，不能只检查 Vue 源文件。

### B. 语义视觉声明

包括背景、边框、选中/错误状态和对比度。

规则：

- 先给稳定的实色/rgba/主题 token，再写现代覆盖；
- 未支持增强属性时仍能区分层级、选中、警告和禁用；
- ID、UUID、状态码继续使用 monospace，颜色不能成为唯一状态通道。

### C. 装饰与性能增强

包括 `color-mix()` 的细腻混色、`content-visibility`、mask 合成、轻微 blur 和动画。

规则：

- 特性缺失时只允许“少一点精致/少一点快”，不允许内容缺失；
- 必须可通过 `@supports` 或声明覆盖自然降级；
- 性能提示不能成为 `display`/visibility 的唯一控制条件；
- 动画降级不得改变操作语义。

## 3. 当前高风险 CSS 清单

扫描基于 2026-08-12 当前工作树；其中包含 Diary WIP，仅作为定位清单。

| 特性 | 当前数量/位置 | 旧引擎行为 | 风险 | 规范 |
|---|---|---|---|---|
| `color-mix()` | 20 处（当前 dirty worktree） | 整条声明被忽略 | 无 fallback 时背景/边框消失 | 同属性先写稳定颜色，再写增强值 |
| `content-visibility` | 3 处 | 声明被忽略 | 长列表更慢，但内容应仍显示 | 只作性能增强；准备可测的离屏策略 |
| `gap` utilities | 约 302 个源码命中 | 老 WebView 的 flex gap 可能无效 | 密集布局挤在一起 | 核心交互行需在目标 WebView 验证；必要时用结构性 margin fallback |
| `inset-*` utilities | 源码中数十处；当前生产 CSS 已把关键 `inset-0/inset-x-0` 展开为 top/right/bottom/left | 构建链升级后仍可能残留 shorthand | 未展开的结构性 shorthand 会使 Overlay 几何失效 | 每次检查生成 CSS；只有残留结构性 shorthand 才补 fallback |
| mask composite | 2 个视觉位置、4 条前缀/标准声明 | 装饰 mask 不同或消失 | 流式边框动画退化 | 保留实色边框；同时有 `-webkit-` 与标准声明 |
| backdrop blur/filter | 5 处 | 忽略或触发昂贵合成 | 装饰退化、低端 GPU 卡顿 | 固定/单例且 ≤12px 才可候选，须实测合成层 |
| `transition-all` | 188 处 | 一般能工作，但扩大动画属性范围 | style/paint 不可控 | 逐个热点改为明确属性，不做无证据全局替换 |
| `will-change` | 11 处 | 可能长期建层 | 多消息时显存与合成压力 | 只在活跃动画期间启用，结束即释放 |
| `@supports` | 0 处 | 无显式增强边界 | 现代样式与基础样式混杂 | 对关键新特性建立少量语义化增强区 |

官方 Chrome 资料把 `color-mix()` 的 Chromium 支持列为 111 起；当前 Vite 产物仍保留该语法。因此所有关键 `color-mix()` 必须自带 fallback。`content-visibility: auto` 从 Chrome 85 提供，本项目当前使用方式是合格的渐进性能增强：旧引擎忽略时消息仍存在，只是可能更慢。

### 3.1 推荐写法

```css
/* 基础声明必须先出现 */
.surface-selected {
  background-color: var(--accent-bg);
  border-color: var(--border-color);
}

/* 新引擎覆盖，不影响结构和可读性 */
@supports (background-color: color-mix(in srgb, black, transparent)) {
  .surface-selected {
    background-color: color-mix(in srgb, var(--accent-bg) 25%, transparent);
    border-color: color-mix(in srgb, var(--highlight-text) 40%, transparent);
  }
}
```

动态 inline style 不能靠 CSS 声明顺序 fallback。以 `VcpAvatar.vue` 的动态边框为例，应先产生一个可用的 rgba/原色边框，仅在 `CSS.supports('color', 'color-mix(...)')` 成功时覆盖；更推荐把常用透明度预计算为主题语义 token，减少运行时分支。

### 3.2 当前应优先补 fallback 的位置

- `src/assets/themes.css` 的 `.glass-panel` 与 `.glass-panel-active`；
- `src/assets/themes.css` 的 `.message-bubble` 边框；
- `src/components/ui/VcpAvatar.vue` 的动态 borderColor；
- `src/features/topic/TopicList.vue` 的图标背景 inline style；
- 审计后续生成 CSS 中所有影响背景对比和控件边界的同类声明。

侧栏、Chat header/footer 已采用“先主题实色、后 `color-mix()`”的正确模式，可作为项目样板。

## 4. 平板响应式布局契约

### 4.1 当前缺陷

当前 DOM 顺序是：

```text
.vcp-app-root.flex-col
  ├─ main.flex-1
  ├─ overlay
  ├─ AgentSidebar
  ├─ RightSidebar
  ├─ GlobalOverlayManager
  └─ FeatureOverlays
```

两侧栏在手机上为 absolute drawer，结构正确；在 `>=768px` 后同时改为 `position: relative` 并强制显示。由于根容器没有改为横向布局，它们成为 main 下方的纵向兄弟，又受根节点 `overflow-hidden` 裁切。遮罩此时还被 `md:hidden` 关闭，使 open 状态与实际可见性契约进一步分离。

这不是通过提高 z-index 可以解决的问题，必须重建明确的 shell 几何关系。

### 4.2 推荐单一路径

保持同一份 DOM 和 store，不建立手机/平板两套页面：

```text
AppRoot（纵向：系统级 top/bottom）
  └─ WorkspaceRow（横向：仅承载主工作区）
      ├─ LeftPane（可选常驻或抽屉）
      ├─ MainPane（min-width: 0）
      └─ RightPane（可选常驻或抽屉）
```

响应式策略：

- 手机及窄平板 portrait：左右均保持 overlay drawer；
- 中等可用宽度：最多常驻一个侧栏，另一侧仍为 drawer；
- 宽平板 landscape：经实测确认主聊天区仍有足够宽度后，才允许双栏常驻；
- 断点按“主内容最小宽度 + 侧栏宽度 + 间距”推导，不直接把设备名称或 768px 当产品含义；
- `open` 状态仍由 `layoutStore` 单一拥有；响应式只决定 presentation mode，不篡改业务状态；
- 每个 flex 子项补齐 `min-width: 0` / `min-height: 0` 和明确 overflow owner，防止长代码、表格和 Mermaid 撑破布局。

建议先冻结三种布局模式 `overlay / single-pane / dual-pane`，但它们应是同一布局函数或 composable 推导的只读表现值，不是新的状态机。

### 4.3 必测尺寸

- 360×640、360×800、412×915 手机 portrait；
- 600×960、800×1280、834×1194 平板 portrait；
- 1280×800、1194×834 平板 landscape；
- 分屏后的窄宽度，而不是只测设备物理分辨率；
- 字体放大 1.5×、大显示尺寸、长中文名称、长 UUID、键盘显示和左右抽屉交替打开。

桌面 Chrome 的响应式模式可做快速回归，但最终必须由目标 WebView 截图和点击可达性验证。

## 5. 安全区与键盘契约

### 5.1 当前问题

- `themes.css` 只定义 top/bottom，且使用 `env(safe-area-inset-top, 24px)`；当浏览器认识 env 变量但返回 0 时，第二参数不会变成“最小 24px”。
- `KeyboardInsetsManager.kt` 读取 `systemBars.bottom` 与 IME，高层事件只发送 `safeAreaBottom`。
- `App.vue` 只把 bottom 写入 `--vcp-safe-bottom`。
- Insets 在原生 attach 后即可发射，而 App/Chat listener 到 Vue mount 后才注册；虽然 Kotlin 内部保留 `lastSent`/`queryCurrentState()`，当前并没有面向前端的重放路径，冷启动首个快照可能丢失。
- 原生当前把完整 `ime.bottom` 当作 keyboard height，Chat 又把 `--vcp-safe-bottom + --keyboard-offset` 相加；若某个 ROM 的 IME inset 已包含导航栏，底部会被重复计入。
- 多个 Viewer/Overlay 组件直接使用 `env(safe-area-inset-top...)`，公式各不相同。
- landscape 的 left/right cutout、状态栏 top 和系统栏变化没有统一真相源。
- 项目 `targetSdk 36`；Android 官方规定 target 35+ 的应用在 Android 15+ 强制 edge-to-edge，未正确处理 Insets 时内容确实可能被系统栏遮挡，因此这不是纯装饰问题。

### 5.2 目标契约

JS 接收端就绪后立即查询/重放当前快照，此后在每次 Insets 变化时推送。跨层字段统一标注为 Android 物理像素：

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

`safe*Px` 是 `systemBars` 与 `displayCutout` 逐边取最大值后的结果；`imeBottomPx` 保留原始 IME bottom，便于诊断不同 ROM。这样横屏侧边刘海不会被只读 system bars 的实现漏掉。

边界要求：

1. Kotlin 只发送物理像素快照，不操作 WebView padding；attach 后缓存最后值，并允许 Vue listener ready 时显式查询/重放。
2. Kotlin 从 `systemBars` 与 `displayCutout` 合并四边安全区，并保留原始 `imeBottomPx`；前端统一派生 `imeExtraBottom = max(0, imeBottomPx - safeBottomPx)`，组件只使用 `safeBottom + imeExtraBottom`，禁止自由累加 raw IME 与 safe bottom。
3. 前端在唯一桥接点按 `devicePixelRatio` 转成 CSS px，只转换一次。
4. 根节点写入 `--vcp-safe-top/right/bottom/left` 与 `--vcp-ime-offset`。
5. 产品组件只消费语义变量；Web 浏览器环境由 `env()` 初始化 fallback。
6. 是否应用 24px 最小顶距是产品规则，应通过 `max()` 或 JS 归一化明确表达，不能误解 env fallback 参数。
7. 旋转、分屏、沉浸式状态栏、键盘开合都要重新推送快照。

若扩展原生插件命令/事件字段，必须同步 Rust/Kotlin/TS 契约和测试；若新增命令，则按项目规定完成 `invoke_handler`、`build.rs`、权限 TOML 与 guest-js 四重注册。

## 6. GPU 与动画规范

- 内容区域禁止 backdrop blur；固定/粘性小面积单例元素若使用 blur，半径不得超过 12px，并需在低端真机证明没有 Composite Layer 尖峰。全屏 blur 与嵌套 backdrop 无论半径多小都禁止，面积和叠层同样决定成本。
- `backdrop-blur-xl` 不符合当前 UI 宪法的 ≤12px 上限，应替换为实色/半透明表面或受控轻量值。
- `will-change` 只属于“正在流式/正在过渡”的瞬时状态；禁止每个历史消息永久持有合成层提示。
- `transition-all` 逐热点收口为 opacity、transform、background-color、border-color 等实际变化属性。
- `prefers-reduced-motion` 可以关闭非必要动画，但不能关闭状态反馈、进度文本或内容。
- 流式边框 mask 失败时保留普通边框和 StreamingTag；动画是增强，不是流式状态的唯一提示。

## 7. 自动化与人工验收

PR 静态层：

- 检查生成 CSS 中无 fallback 的 `color-mix()`；
- 检查直接散落的 `env(safe-area-*)`、不受控 blur、全局魔法 z-index；
- 对关键 Overlay 生成几何断言：bounding box 位于 viewport 内、可点击、无水平溢出；
- 生产 Web build 输出初始资源 manifest。

设备层：

- 记录 WebView provider/version 和 CSS feature probes；
- 对上述手机/平板尺寸跑启动、主 Chat、双抽屉、键盘、各级 Overlay 和强渲染 fixture；
- 截图差异允许字体/OEM 抗锯齿细微变化，不允许裁切、重叠、透明到不可读、控件失联；
- 浅色/深色主题都要检查选中、错误、禁用和边界 fallback，不能只验证布局几何；
- 旧 WebView 未支持增强属性时，单独验证基础路径仍完整。

happy-dom 配置禁用 CSS，不能承担这里的兼容证明；桌面 Chromium 自动化也不能模拟任意旧 Android WebView。它们只能作为更快的前置筛查。

## 8. 禁止方案

- 根据 UA/机型维护两套 CSS 或两套主题；
- 通过隐藏复杂消息、禁止 Mermaid/KaTeX 或转纯文本来“兼容旧机”；
- 仅提高 minSdk 或要求用户更新 WebView，然后把问题标记为修复；
- 为布局问题堆叠更大 z-index；
- 把 `content-visibility`、blur、mask 或 `color-mix()` 用作内容存在条件；
- 未经真机 Profile 就全局删除动画、阴影或原子类。

兼容工程的目标是让单一路径具备稳健基础和可选增强，而不是让旧设备长期运行一个功能残缺的分支。
