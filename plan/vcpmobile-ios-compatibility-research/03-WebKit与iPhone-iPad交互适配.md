# 03. WebKit 与 iPhone/iPad 交互适配

## 1. 总体判断

现有前端不是 iOS 的主要重写对象。窗口宽度响应、单组件树、语义层级、集中 safe-area 变量和强渲染链都有复用价值。真正的 P0 是：**WebKit 尚无构建契约，键盘、导航/手势、附件入口、活跃的 Android-only UI 和 iPhone/iPad presentation 仍按 Android 假设工作。** 语音功能已从 iOS 范围删除，不再把修复音频 MIME 作为 iOS P0。

UI 适配应延续当前产品宪法：高密度、线性、灰度优先、语义 z-index、内容区不使用 backdrop blur。iOS 适配不是给界面套一层“仿 UIKit 毛玻璃”。

## 2. 可复用基础

| 基础 | 当前实现 | iOS 处理 |
|---|---|---|
| 窗口宽度模式 | `<1024` 双抽屉、`1024–1279` 左栏常驻、`>=1280` 双栏 | 原样作为 iPhone/iPad/分屏共同模型，不按机型分叉 |
| safe area seed | `viewport-fit=cover` + 根 `env(safe-area-inset-*)` | 保留单一变量 owner；验证刘海、Home Indicator、横屏四边 |
| 全屏壳 | `.vcp-safe-inline` 统一左右安全区 | 继续复用，不允许组件各自读取 `env()` |
| 语义层级 | content → gate 的统一 layer | iOS 弹层继续使用同一层级表 |
| 强渲染 | Markdown、代码、KaTeX、Mermaid、raw HTML、Tool、Diary、附件 | 不做“iOS 简化渲染器”，建立 WebKit fixture 门禁 |
| Overlay owner | Pinia/Teleport/ModalHistory | presentation 可按宽度变，业务 open 状态不复制 |

相关证据：

- [index.html](../../index.html) 已含 `viewport-fit=cover`；
- [themes.css](../../src/assets/themes.css) 集中定义四边 safe area；
- [App.vue](../../src/App.vue) 使用 1024/1280 宽度模式；
- [ANDROID_UI_COMPATIBILITY.md](../../docs/ANDROID_UI_COMPATIBILITY.md) 保护强渲染和窗口宽度模型。

## 3. WebKit 构建契约缺口

[vite.config.ts](../../vite.config.ts) 当前只冻结 `target/cssTarget: chrome87`。这保护 Android WebView 生成语法，不是 Safari/WKWebView 兼容声明。

当前 `tauri.conf.json` 只有一个 `beforeBuildCommand` 和一个 `frontendDist`，所以“同时跑两次 fixture build”不能证明哪个产物被实际打包。Phase 1 必须冻结唯一 artifact owner：优先生成同时满足 Chrome 87 与最低 WebKit 语法约束的共同产物；若确实需要 Android/iOS 两份产物，则必须有各自命令、环境、dist 路径和 Tauri bundle 入口。

建议在真正实施阶段新增 WebKit 治理，而不是无证据地把 Android target 替换掉：

1. 明确最低 iOS 后，建立对应 Safari/WebKit syntax baseline；
2. 对**实际进入 iOS Tauri artifact** 的最终 JS/CSS 生成物做 WebKit syntax check；
3. 保留 Android `chrome87` 合同，避免一平台修复抬高另一平台语法下限；
4. 对 `color-mix()`、`content-visibility`、pointer events、Canvas/WebGL 等继续采用基础 fallback + feature enhancement；
5. 在 GitHub Actions 的 Tauri WKWebView Simulator artifact 上验证；社区真机报告只能补充设备事实，macOS Safari 不是产品 target。

Tauri iOS 使用系统 WKWebView/WebKit，因此 WebView 能力随系统版本变化，参见 [Tauri WebView Versions](https://v2.tauri.app/reference/webview-versions/)。

## 4. P0 UI/交互缺口

### 4.1 键盘与 viewport

[useKeyboardInsets.ts](../../src/core/composables/useKeyboardInsets.ts) 当前顺序为：

```text
Android evaluateJavascript event
  -> Virtual Keyboard API
  -> focus + (scrollHeight - innerHeight) 估算
```

生产路径没有使用 `visualViewport`。在 iOS WKWebView 上，中文/第三方键盘、旋转、iPad 浮动或分体键盘都不能从这套逻辑推导为正确。

safe area 与 IME 是两个不同事实。建议保持一个语义 Insets owner，但不抢走浏览器的动态能力：

- iOS safe area 默认继续由根部动态 `env(safe-area-inset-*)` 提供，旋转、分屏和 scene resize 后由 CSS 自动更新；现有 top 的固定下限是否合理另做设备验证；
- IME 候选为经 Simulator/真机证明可靠的 `visualViewport`，或 Swift keyboard/frame 事件；
- 只有真机证明 `env()` 在目标 WKWebView 场景失真时，Swift 才可覆盖相应 safe-area 变量，并必须定义 CSS/native 优先级、失效回退和每次 resize 的 generation；不能用一次原生 snapshot 冻住动态 `env()`；
- 事件携带 `source`、单位和 snapshot generation，避免把 Android physical px 与 iOS points/CSS px 混用；
- safe bottom 和 IME extra bottom 只派生一次；
- snapshot 可重放，解决原生事件早于 Vue listener；
- 所有 Chat/Diary/Editor 继续消费同一语义变量，不逐组件修补。

### 4.2 根导航与“退出应用”

[useModalHistory.ts](../../src/core/composables/useModalHistory.ts) 用 history dummy 管理 overlay/router 返回；这部分仍可作为跨平台 modal history owner。[App.vue](../../src/App.vue) 在根状态实现“双按退出”并调用 `move_task_to_back`，这一根退出分支才是明确的 Android 语义，iOS 不应继承。

建议冻结唯一关闭顺序：

```text
不可 dismiss 长任务持有导航权
  -> 关闭顶层 dialog/viewer/editor/sheet
  -> 关闭抽屉
  -> 路由返回
  -> 回到主 Chat
  -> 根状态不执行程序化退出
```

当前锁定的 Wry 版本不能让本文直接假定 WKWebView 已启用 back/forward navigation gesture。Phase 1 应先证明 Tauri 容器是否存在 `UINavigationController` pop、WebView back/forward gesture 或其他原生返回通道，再决定与应用内横滑/浏览历史的仲裁；在此之前统一标记为待证。

### 4.3 全局横滑与系统边缘手势

[useSidebarSwipe.ts](../../src/core/composables/useSidebarSwipe.ts) 当前允许从全应用非滚动区域横滑 60px 打开左右抽屉，没有边缘起点约束。这已确定会与文本选择、图片平移和横向内容竞争；若容器 spike 证明存在原生/系统边缘返回，还需再加入该手势仲裁。

建议：

- 显式侧栏按钮始终是可靠入口；
- iOS 默认不让任意位置右滑打开左栏；
- 若保留边缘滑动，必须限定起点保护区、方向角、速度和 overlay 状态；
- 右侧抽屉手势也需避开系统手势和横向内容；
- 不通过大范围 `preventDefault` 粗暴抢占系统手势。

### 4.4 保留但未激活的 Android PermissionGate

[PermissionGate.vue](../../src/components/layout/PermissionGate.vue) 包含：

- 标题 `VCP Mobile Android`；
- storage/battery 三布尔权限；
- OEM 自启动、电池优化、通知监听；
- 5GB 与 Android 进程回收解释；
- 暂不授权后退出 App。

当前 [App.vue](../../src/App.vue) 未挂载该组件，且 Release governance 测试禁止复活它。因此它是保留的 Android 资产，不是当前活跃首启入口；iOS 不移植、不复活，也不应给它增加零散 `v-if="isIOS"`。

当前真正需要 capability 隔离的活跃面包括 UpdatePrompt，以及 Settings 中仍挂载的 BatteryOptimizationGuide。iOS 首启只做能力发现与解释；首期不申请通知、麦克风、位置或 Motion。Document Picker 不需要把 Android storage 布尔权限照搬过来；照片/相机/Local Network 只有后续真实功能触发时才按用途请求。

### 4.5 APK 更新 UI 泄漏

[useAutoUpdate.ts](../../src/core/composables/useAutoUpdate.ts)、[useUpdateDownloader.ts](../../src/core/composables/useUpdateDownloader.ts) 和 UpdatePrompt 都是 APK 下载/安装器语义。iOS 必须在 capability 层彻底禁用自动检查、下载、通知和安装，且不挂载 UpdatePrompt；不能只隐藏 Android 通知栏进度。

### 4.6 语音入口退出 iOS 能力面

[useAudioRecorder.ts](../../src/core/composables/useAudioRecorder.ts) 会探测 `audio/mp4`，但当前跨层契约仍存在已知不一致：

- [InputEnhancer.vue](../../src/features/chat/InputEnhancer.vue) 固定文件名 `.webm`；
- [useSpeechRecognition.ts](../../src/core/composables/useSpeechRecognition.ts) Whisper 上传也固定 `recording.webm`。

这同时说明现有录音不能被视为“已有跨平台能力”。由于 Android 版本中的语音录音当前也不可用，冻结决策是：iOS 隐藏录音/语音识别入口、不初始化 recorder、不请求 `NSMicrophoneUsageDescription`，不为 iOS 单独修复该链。若未来全平台重新启用语音，应作为独立项目从真实 MIME/魔数和服务端接受格式重建契约。

## 5. 可访问性与触控

### 5.1 缩放策略

`index.html` 当前 `maximum-scale=1.0, user-scalable=no`，而 [themes.css](../../src/assets/themes.css) 的全局 `touch-action: pan-x pan-y` 也会排除 pinch zoom。两层都是需要产品决策的可访问性约束；只删除 viewport 禁缩放并不能完成整改。

建议讨论两种方案：

| 方案 | 优点 | 风险 |
|---|---|---|
| 允许页面缩放 | 更符合低视力用户预期 | 需验证全屏 editor、viewer 和手势冲突 |
| 保持不可缩放，但支持系统字体/可访问设置 | 减少布局漂移 | 必须证明 Dynamic Type/字体缩放、VoiceOver 和放大器可用 |

无论选择哪种，不能只因为“移动端 App”就跳过可访问性验收。

### 5.2 输入字号

核心 textarea 为 15px，项目中还有 10–14px 输入/状态文本。iOS 对小输入字号可能产生焦点缩放或可读性问题。建议：

- 可编辑输入的视觉字号与焦点缩放单独测试；
- 技术状态、UUID 可保留高密度，但不能把可操作输入压到不可读；
- 深浅色对比、Bold Text、Larger Text 先进入 Simulator/Accessibility Inspector；真机状态保持社区未验证，不阻塞实验产物。

### 5.3 命中区

Chat 输入区存在 36px 语音/附件按钮和 32px 发送按钮。若 D11 决定采用 44pt 作为发布验收线，可通过透明 padding/伪元素扩大命中区，不必破坏高密度视觉。

若 D11 获批，治理规则为：

- 主要可操作控件有效命中区至少 44 × 44pt；
- 列表密度可以保留，但行内 icon 要有足够 hit slop；
- disabled/hidden 控件不得继续截获触摸；
- 用 Simulator UI test 做基础几何；社区真机触摸报告用于后续修正，不升级为项目正式验收。

### 5.4 Modal 与外接输入

通用 [BottomSheet.vue](../../src/components/ui/BottomSheet.vue) 缺完整 dialog 语义、焦点进入/恢复和背景 inert；部分 Diary sheet 已有更好的语义模式。

iPad 若纳入首发，应至少验证：

- 硬件键盘 Tab/Shift+Tab；
- Cmd/Control+Enter 与 Escape/返回；
- focus ring 不被全局样式移除；
- VoiceOver modal focus trap；
- 触控板 pointer/hover 不成为核心操作前提。

## 6. iPad presentation

现有 `BottomSheet`、ModelSelector 和分享选择器在宽屏仍左右贴边，可能形成整宽底板。建议复用同一 overlay owner，并继续以当前 CSS viewport/window 宽度改变 presentation；UIKit trait/size class 只有在桥接确有必要时才作为输入事实，不能成为第二状态 owner：

| 当前窗口 | 推荐呈现 |
|---|---|
| compact width | 现有 bottom sheet |
| regular width | 居中或锚定的受限宽度 sheet/popover |
| 多窗口/分屏缩窄 | 自动回到 compact，不复制 open 状态 |

不要新建“iPad store”或第二套 sheet 状态机。断点是否继续使用 1024/1280，应由 iPad Simulator 的可用 CSS 宽度和主内容最小宽度决定，而不是设备名。

## 7. 强渲染 WebKit 门禁

以下能力必须保留，并用静态 fixture 与 Simulator 做功能回归。项目没有 iOS 设备，因此触控、热压力和真实内存行为只能标记为 `COMMUNITY-DEVICE-UNVERIFIED / PERFORMANCE-NOT-EVALUATED`，不作为实验产物门禁：

| 能力 | 特别风险 |
|---|---|
| Markdown/AST diff | 流式 partial → final 不丢节点、不重复提交 |
| Syntax highlight | 长代码、横向滚动、复制；内存未评估 |
| KaTeX/SVG/MathML | 字体加载、缩放、selection |
| Mermaid | pointer capture、缩放/平移、SVG/2D Canvas 导出 |
| raw HTML | `iframe.srcdoc`、sandbox、postMessage、CSP/asset scope |
| Tool/Thought/Diary | 展开、动画、编辑、键盘避让 |
| 图片 viewer | pinch/pan/double-tap 与系统边缘手势 |
| WebGL 背景 | rAF visibility、reduced motion、context loss/restore；热压力未评估 |
| 长会话 | 保留既有上限与收敛逻辑；iOS memory pressure 不认证 |

“通过 happy-dom”只证明 DOM 逻辑，不证明 WebKit CSS、Canvas、触控或内存。

## 8. P1 体验项

- 统一 sheet grabber：能拖就实现，不能拖就不显示误导性把手；
- 所有装饰动画响应 `prefers-reduced-motion`，不可见时停止 rAF；
- WebGL 背景处理 visibility、context loss 和热压力；
- 文本选择、消息长按菜单、系统 Copy/Lookup/Translate 共存；
- 可选使用平台触觉桥，能力不可用时静默降级；
- iOS 不展示 Android Root、OEM 电池指南、notification listener、悬浮助手或 APK updater；
- 不为“iOS 风格”恢复被 UI 宪法禁止的内容区 blur。

## 9. 验证边界

| 环境 | 本文可关闭项 |
|---|---|
| Linux/Vitest | capability-driven DOM、导航顺序、MIME 派生、静态 CSS/源码治理 |
| macOS Safari | JS/CSS/iframe/Canvas 的浏览器预筛，不代表 Tauri |
| iPhone/iPad Simulator | Tauri IPC、窗口模式、基础 keyboard/safe area、路由、modal、Accessibility Inspector/可访问树初筛 |
| 社区真机（非门禁） | 第三方键盘、系统边缘手势、文本选择、WebGL、触控与内存的用户回报；语音不在范围内 |

当前所有 UI 结论最多是 `STATIC-AUDITED`，不能标为 `WEBKIT-VERIFIED`。
