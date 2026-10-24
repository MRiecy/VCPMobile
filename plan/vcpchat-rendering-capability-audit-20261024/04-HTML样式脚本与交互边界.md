# HTML、样式、脚本与交互边界专项审计

审计日期：2026-10-24。桌面源码：`E:\VCPChat`，HEAD `60f87010`；移动源码：`E:\VCPMobile`，HEAD `4c54cba3`。本篇只读两仓生产代码与 Git 历史，新增本报告；没有启动 Electron、Android 或 iOS，也没有把 host 执行结果当作设备证据。

## 1. 结论与接入优先级

桌面当前富内容有两条明确不同的执行路径：**消息主 DOM 的 raw HTML 动画岛**，以及**点击播放后进入 opaque iframe 的 HTML 代码预览**。移动端也已有主 DOM 富 HTML 和 opaque iframe，但主 DOM 删除 `<script>`，只保留受限制的局部事件；完整 JavaScript 放在 `HtmlPreviewBlock`。因此不能写成“移动端不支持 HTML/JavaScript”，也不能把桌面 `animation.js` 直接复制进主 WebView 来补齐功能。

此次最值得优先接入的差异是 **CSS 语法树作用域转换**。桌面已经能够处理嵌套条件规则和选择器列表；移动仍使用正则。本次只读调用当前源码函数，确认移动在 `body .label`、`:is(.a,.b)`、`@media`、`@keyframes` 上产生结构错误或漏作用域。它影响现有静态 HTML、响应式卡片和 CSS 动画，并非必须等到新动画引擎接入才出现的问题。

| 优先级 | 事项 | 当前判断 |
|---|---|---|
| P1 | CSS AST 作用域、根选择器语义与条件规则 | 已取得当前生产函数的具体输入输出，适合作为独立小范围改进 |
| P1 | raw HTML、HTML 围栏、完整文档三种输入的产品语义 | 两端入口不同；先固定迁移样例，不把“能执行 JS”与“主 DOM 可执行”混为一谈 |
| P1 | 现有 AI 按钮对真实发送结果的接纳与恢复 | 桌面 await 发送并失败恢复；移动事件桥直接调用，按钮状态缺少同等恢复路径 |
| P2 | raw 动画岛的 JS/Canvas/Pixi 功能接入 | 确认价值后沿现有 iframe owner 扩展；保留执行能力，不新开宿主桥、不一刀切禁脚本 |
| P2 | 流中局部交互跨帧、终态和预览切换的延续 | 两端均不承诺任意 JavaScript 状态持久化，需要按具体交互验证 |
| P2 | 动画可见性与资源回收 | 桌面已有 Pixi 配额、rAF/timer 暂停和 Three 清理；移动若接动画运行时必须连同 owner 接入 |

这里的 P1/P2 是本专项的建议顺序，不是已批准的实施计划。本次任务是整理能力，未修改产品行为。

## 2. 哪些是新增，哪些是老能力的扩展

不能把今天看见的全部 raw HTML 能力都记作“流式渲染之后新增”。Git 历史可定位到：

| 历史节点 | 可确认的变化 | 分类 |
|---|---|---|
| `2f805006`，2025-07-14 | `animation.js` 历史已有 Anime.js、Three.js 高级渲染支持 | 长期能力 |
| `728690e9`，2025-10-14 | 给 AI 气泡随机 ID，为 CSS 加作用域 | 长期能力；不等于当前 AST 实现 |
| `4d602497`，2025-11-13 | CDN 动画脚本改为本地依赖路径的历史节点 | 长期资源策略 |
| `f459a7b7`，2025-12-20 | 内联脚本 `document.write` 防护 | 老能力的稳定性修复 |
| `75634499`，2026-05-30 | `preserveDynamicStreamState`、局部 shadow document 等代码的历史引入点 | 流式/多消息兼容增强 |
| `35897e5a`，2026-08-20 | content processor / renderer presentation provider 实例隔离 | 多 Surface 所有权调整 |
| `e1dbc98d`，2026-08-24 | `cssTree.parse`、stable 阶段 `processScripts:false`、当前 iframe `allow-scripts` 等进入主干 | 本轮重点的语法与终态边界增强 |
| `aff97131`，2026-09-12 | Pixi v8、消息所有权与启动配额 | 明确的新引擎支持 |
| `c31f6af9`，2026-09-12 | 脚本执行器排除 `pre/code/HTML 预览容器/流式代码块` | 代码示例与可执行内容的边界修复 |
| `06d1331b`，2026-09-30 | AI 按钮处理排除系统控件 | 交互兼容修复 |
| `4e82b2c3` / `a903a038` / `78e92eb5`，2026-10-03 | 消息所有权结束后的高度任务清理、帧取消与重入批次保留 | 资源与布局稳定性，非新内容语法 |

历史定位使用 `git log -S` 与路径历史；“某符号第一次出现在该提交”不自动证明提交内所有行为都从那天才存在。统一时间基线以本项目总报告为准。

## 3. 桌面实际调用链

### 3.1 raw HTML / CSS / SVG / 脚本

主要链路为：助手原文 → `processAssistantScopedHtmlContent` 保护字面量域并提取 CSS → Markdown 转 HTML → `renderPostProcessedHtml` → 内容、图表、媒体、按钮后处理 → `processAnimationsInContent` → `processScripts`。

依据：

- [messageRenderer.js:1593](E:/VCPChat/modules/messageRenderer.js:1593)：助手 HTML 的保护、样式入口。
- [messageRenderer.js:3656](E:/VCPChat/modules/messageRenderer.js:3656)：后处理 owner、revision 与替换前清理。
- [messageRenderer.js:3758](E:/VCPChat/modules/messageRenderer.js:3758)：规范内容阶段的脚本执行。
- [animation.js:292](E:/VCPChat/modules/renderer/animation.js:292)：实际收集与执行脚本。

`<div>`、SVG、Canvas 等不是只允许静态展示的文本；助手 raw HTML 能进入消息 DOM。普通 `<script>` 会由执行器摘下再执行。`pre`、`code`、`.vcp-html-preview-container`、`.vcp-stream-code-block` 中即使因上游畸形 HTML 真出现 `<script>` DOM，也不升级为可执行脚本。工具结果的 HTML/CSS 则属于数据域，不走助手动画岛语义。

用户消息还有独立路径：[messageRenderer.js:4503](E:/VCPChat/modules/messageRenderer.js:4503) 会转义用户 HTML，只保留未带 `on*` 与 `javascript:` 的 `<img>` 特例。上述 raw HTML 执行能力不能外推为所有 role、工具结果和代码示例均执行。

### 3.2 流中与终态

桌面已采用“结构可增量显示，规范终态执行脚本”的分工：

1. 流中 stable block 调用 `renderPostProcessedHtml`，但显式设置 `processScripts:false`，避免脚本绑定即将被终态替换的树。见 [streamManager.js:504](E:/VCPChat/modules/renderer/streamManager.js:504)。
2. CSS 动画不依赖 JavaScript 执行器，样式有效后可以随流展示。见 [messageRenderer.js:3759](E:/VCPChat/modules/messageRenderer.js:3759)。
3. raw `<div>` 岛扫描器尊重嵌套 HTML、SVG、`script/style` rawtext；当前上限为 128 层、256 KiB。闭合的岛可推进稳定边界，未闭合岛留给 tail。见 [streamManager.js:51](E:/VCPChat/modules/renderer/streamManager.js:51)、[streamManager.js:837](E:/VCPChat/modules/renderer/streamManager.js:837)、[streamManager.js:928](E:/VCPChat/modules/renderer/streamManager.js:928)。
4. 流终态重新处理完整可见原文，并走完整 `renderPostProcessedHtml`，此时默认执行脚本。见 [streamManager.js:2580](E:/VCPChat/modules/renderer/streamManager.js:2580)。

“终态执行一次”是这次终态投影的语义，不代表消息之后重新挂载、刷新历史或内容更新时脚本永远不会重跑。代码没有持久化任意脚本堆、Canvas 内部状态或闭包的协议。

流中的 `onclick` 等原生事件也不能被笼统写成“全部到终态才有副作用”：`processScripts:false` 控制的是显式 `<script>` 执行器，并不是清除所有 raw HTML 事件属性的 sanitizer。

### 3.3 主 DOM 脚本的能力和局限

[animation.js:360](E:/VCPChat/modules/renderer/animation.js:360) 为内联脚本注入：

- `container`：本次内容根，直接捕获引用，不按消息 ID 全局反查。
- shadow `document`：`querySelector`、`querySelectorAll`、`getElementById`、`getElementsByTagName` 优先查当前消息；无局部命中时回退真实 document。
- `document.currentScript` 的虚拟对象，保留原 script 的父节点、前一兄弟和属性，以支持岛内相对定位。
- 可暂停 `requestAnimationFrame`、`setTimeout`、`setInterval`；同时替换常见 `window.*` 调用字符串。
- 消息专属 PIXI facade；anime 创建时登记实例。

执行器临时屏蔽 `document.write/open/close`，完成同步执行后恢复；它是保护 SPA 不被文档写入摧毁的兼容措施。[animation.js:347](E:/VCPChat/modules/renderer/animation.js:347)

**这不是安全沙箱。** shadow document 查不到会回退；普通属性读取和写入继续落到真实 document；真实 `window` 仍存在。不能据此宣称脚本只能操作本消息。文档代理的作用主要是同 ID 多消息兼容和运行调度。

外部脚本由 `loadScript` 插入真实 `document.head`；Three/anime/Pixi 的常见 CDN 地址映射到本地 `vendor/*.min.js`，脚本加载集合放在 `window._vcp_loaded_scripts`，跨消息共享。[animation.js:25](E:/VCPChat/modules/renderer/animation.js:25)、[animation.js:271](E:/VCPChat/modules/renderer/animation.js:271)。这并不意味着任意第三方 CDN 脚本都能加载：主聊天页的 CSP 是 `script-src 'self' 'unsafe-inline'`，未映射远程源会受其约束。[main.html:9](E:/VCPChat/main.html:9)

两个需保留的实现边界：内联脚本最终被放入普通新建 script，不能把它等同于完整原生 `type="module"` 环境；外部依赖加载及其回调是异步的，当前执行器自身没有接收内容 revision / AbortSignal，不能仅凭上层即时 `isStillValid()` 检查宣称所有迟到脚本任务均已失效。这些是源码边界，不是本次已经复现的故障。

## 4. CSS 与 SVG：本轮最实质的兼容差距

### 4.1 桌面 CSS 的当前语义

[messageRenderer.js:1518](E:/VCPChat/modules/messageRenderer.js:1518) 提取 `<style>` 后保留不可见注释占位，避免空行改变 CommonMark HTML 岛结构；样式节点按消息 scope ID 和 Surface ID 所有，流中复用同一节点并替换文本。

提取前的保护顺序覆盖工具结果、HTML 注释、工具请求及未闭合协议、桌面推送、Markdown 行内代码/可变长度围栏/未闭合代码尾。使用占位符倒序恢复嵌套区域，避免代码说明中的 `<style>` 被误当作真正样式。[messageRenderer.js:1616](E:/VCPChat/modules/messageRenderer.js:1616)

实际 `scopeCss` 使用 `css-tree`：

- 遍历每个 DOM selector rule；`@media`、`@supports`、`@container`、`@layer` 中的规则也被访问。
- 根选择器 `body` / `html` / `:root` 映射到消息根，后续组合符、class、后代仍保留。
- `from/to/百分比` keyframe 步骤不当作 DOM 选择器处理。
- `@import` / `@namespace` / `@charset` 使本段 CSS 被拒绝；生成后再次解析。

见 [contentProcessor.js:1312](E:/VCPChat/modules/renderer/contentProcessor.js:1312)、[contentProcessor.js:1327](E:/VCPChat/modules/renderer/contentProcessor.js:1327)。

它仍不是全套 CSS 名称隔离：源码没有改写 keyframe 名字与 `animation-name` 引用，也没有给字体家族建立消息命名空间。两个消息定义同名 `@keyframes pulse` 时仍应验证级联影响。raw `<link rel="stylesheet">`、内联 style 与样式表 `url()` 也不是这一函数统一接管的路径；拒绝 `@import` 不能外推成所有网络样式资源均被封锁。

SVG 作为 raw HTML 结构保留，桌面有对缩进标签的一般性处理，覆盖 `g/path/linearGradient` 等而不是固定 HTML 标签白名单。[contentProcessor.js:1435](E:/VCPChat/modules/renderer/contentProcessor.js:1435)。移动同样已有 SVG/MathML 保留和图片交互，主要差异应按样式、脚本与 DOM 更新拆解，不应登记为“新增 SVG 支持”。

### 4.2 移动当前 CSS owner

消息样式合集由稳定 blocks 与 tail CSS 分别计算，一次交给当前消息实例的 `useMessageStyleInjector`。style 节点按挂载实例拥有，卸载调用 `onScopeDispose` 移除；这部分所有权已经合理，不需要照搬桌面的全局 Set。

见 [MessageRenderer.vue:929](../../src/features/chat/MessageRenderer.vue:929)、[messageStyles.ts:13](../../src/features/chat/utils/messageStyles.ts:13)、[useMessageStyleInjector.ts:8](../../src/core/composables/useMessageStyleInjector.ts:8)。

差异集中在 [useMessageStyleInjector.ts:35](../../src/core/composables/useMessageStyleInjector.ts:35) 的正则及 `split(",")`。此外，`stableMessageStyles` 当前只收集显式 style block 和单独 raw_html style 节点，不能仅看注入器就声称所有嵌套 raw `<style>` 已经过同一路径；实现时必须同时核对 Rust 分块和 AST tail 输入，避免只修下游转换器。

### 4.3 本次只读函数执行证据

执行方式：Node 直接加载桌面 `contentProcessor.js` 的当前源码（仅把 import 路径解析为绝对 file URL）；移动当前 TypeScript 用仓库 TypeScript 转译后执行，提供最小 document/style stub 观察写入的 `textContent`，`onScopeDispose` 在本探针中不触发。没有复制一套转换逻辑，没有修改生产文件，没有发网络请求。

此证据证明**当前函数生成的 CSS 文本**，不证明浏览器布局、设备动画帧率或完整消息入口已通过验证。

| 输入 | 桌面输出 | 移动输出与影响 |
|---|---|---|
| `body .label{color:red}` | `#m1 .label{color:red}` | `[data-message-id="m1"] {color:red}`；后代 `.label` 被丢掉 |
| `:is(.a,.b) > span{color:red}` | `#m1 :is(.a,.b)>span{color:red}` | `[data-message-id="m1"]:is(.a, [data-message-id="m1"] .b) > span {color:red}`；函数选择器被按逗号拆错，第一支匹配根 |
| `@media(min-width:1px){.label{color:red}}` | `@media (min-width:1px){#m1 .label{color:red}}` | `@media(min-width:1px) {.label{color:red}}`；内层选择器未加作用域 |
| `@import url(https://example.invalid/theme.css); .label{color:red}` | 空字符串，报告 `forbidden @import` | `@import url(https://example.invalid/theme.css); .label {color:red}`；整段保留且普通规则未加作用域 |
| `@keyframes pulse{from{opacity:0}to{opacity:1}}.label{animation:pulse 1s}` | keyframes 原样、`.label` 前加 `#m1` | `@keyframes pulse {from{ [data-message-id="m1"] opacity:0}to {opacity:1} [data-message-id="m1"] }.label {animation:pulse 1s}`；声明与括号被误当选择器 |

建议复用 CSS 语法树的思路和明确的 selector 语义，保留移动当前实例 owner。是否用与桌面相同的 `css-tree` 依赖应结合移动包体、既有依赖和嵌套 CSS 范围决定；不必复制整个桌面 content processor。

## 5. HTML 代码预览与完整文档

### 5.1 桌面入口、执行与回收

代码后处理识别 `language-html`，或 code 文本以 `<!DOCTYPE html>` / `<html` 开始时加“播放/返回”。工具请求、工具结果、日记气泡内跳过；内容含特定始末标记时也跳过。见 [contentProcessor.js:761](E:/VCPChat/modules/renderer/contentProcessor.js:761)、[contentProcessor.js:773](E:/VCPChat/modules/renderer/contentProcessor.js:773)。

点击播放才建立 iframe：`sandbox="allow-scripts"`，没有 `allow-same-origin`，并有 `referrerpolicy="no-referrer"`。消息代码原文放进 `srcdoc`；内部 ResizeObserver 经 `postMessage` 发送高度，父级检查 `event.source === previewFrame.contentWindow`、type、frameId，数值限制在 100–12000 px。[contentProcessor.js:860](E:/VCPChat/modules/renderer/contentProcessor.js:860)

返回、内容替换、消息销毁时调用同一 `destroyPreview`：移除 window message 监听、清空 srcdoc、导航 about:blank、尝试 stop、移除 iframe。容器 `_vcpCleanup` 是资源收据；延迟隐藏代码的 timer 也由 owner 清理。[contentProcessor.js:821](E:/VCPChat/modules/renderer/contentProcessor.js:821)、[contentProcessor.js:1489](E:/VCPChat/modules/renderer/contentProcessor.js:1489)

这里保留 JavaScript，但父应用不是同源文档；它与上一节主 DOM 动画岛的能力不可互换。

### 5.2 移动已具备的能力

Rust parser 把 `html` 围栏变成 `ContentBlock::HtmlPreview`；行首完整文档入口 `<!doctype html>` / `<html...>` 也进入同一类型。普通 HTML 容器仍进入 Markdown/raw_html。见 [content_parser.rs:637](../../src-tauri/src/vcp_modules/chat/content_parser.rs:637)、[content_parser.rs:795](../../src-tauri/src/vcp_modules/chat/content_parser.rs:795)。

`HtmlPreviewBlock` 默认代码模式；流中强制回代码、关闭全屏，复制/全屏/刷新入口也检查 `isStreaming`。终态用户选择预览后使用 `sandbox="allow-scripts"` iframe，保持脚本原文，不走 DOMPurify。见 [HtmlPreviewBlock.vue:17](../../src/features/chat/blocks/HtmlPreviewBlock.vue:17)、[HtmlPreviewBlock.vue:76](../../src/features/chat/blocks/HtmlPreviewBlock.vue:76)。

相比桌面内联自动高度，移动提供固定内容区和全屏预览，并注入当前主题基础颜色、滚动条、媒体最大宽度。原文若有 `<head>` 则向其中插入基础样式和图片桥，否则构造外层文档。不能据此保证任意完整桌面页面在小屏无横向布局问题。

内联和全屏分别创建 iframe；预览切回代码、切换全屏、刷新、内容或主题更新都会涉及 iframe 销毁/新建或 generation 变化。UI tab 选择被同步，**文档运行状态不是同一个持久实例**。游戏进度、输入值或 Canvas 画面不能因“全屏/内联都能预览”就宣称能够连续保持。

### 5.3 移动已有的窄宿主桥

HTML 预览已有图片/Canvas 交付桥，不是通用 invoke 桥：

- [sandboxImageBridge.ts:2](../../src/core/utils/sandboxImageBridge.ts:2) 在 opaque 文档内捕获图片/Canvas 点击，先申请导出槽位。
- blob/data 图片在 iframe 内读成有界字节；8 MiB 上限，Canvas 最长边 2048、最大 4,194,304 像素，SVG data 图片转 Canvas；30 秒取消机制，pagehide abort。
- [RenderedImageViewer.vue:495](../../src/components/ui/RenderedImageViewer.vue:495) 检查 `origin === "null"`、真实 iframe `contentWindow`、实例 generation nonce、requestId 和已授予槽位；并检测 iframe 移除/nonce 更新。
- 父级只接受图片字节或允许的资源 URL，并进入现有图片查看/导出 owner。blob URL 不从 opaque 文档直接跨边界使用。

这条桥可以成为后续确有消费方的富内容操作范例。没有理由为动画迁移建立任意 `invoke`、任意文件访问或第二份消息发送后端；也不需要因为存在 JavaScript 就删除这个已有图片能力。

## 6. DOMPurify、URL、宿主能力的真实边界

### 6.1 桌面

本次检查的主消息链未发现 DOMPurify 调用；`renderer.js` 的 marked 配置明确 `sanitize:false`。助手 raw HTML 是产品有意支持的执行面。主窗口 `contextIsolation:true`、`nodeIntegration:false` 并不等于页面脚本无法调用 preload 已公开 API。

- [main.js:779](E:/VCPChat/main.js:779)：主窗 webPreferences；preload 因本地 require 使用 `sandbox:false`。
- [preloads/core/expose.js:66](E:/VCPChat/preloads/core/expose.js:66)：按角色公开 `chatAPI`，并有 `electronAPI` 兼容对象；不可见角色方法是隔离桩，可见方法是真实函数。
- [animation.js:416](E:/VCPChat/modules/renderer/animation.js:416)：shadow document 没有隔离真实 window 或公开 bridge。

准确表述应为“主 DOM raw HTML 与该页面的普通脚本共享可见能力；iframe HTML 预览使用 opaque origin 边界”。不要误报成 `contextIsolation` 为每个消息提供了沙箱，也不要假定每个 electronAPI 名称都可在 chat role 使用。

主聊天页 CSP 允许样式 HTTPS、图片/媒体广泛来源、连接网络，同时 `frame-src` 仅 self 和指定 loopback；因此生成 raw iframe 并不等于任意外部网站能嵌入。[main.html:9](E:/VCPChat/main.html:9)

### 6.2 移动主 DOM

移动不是 DOMPurify 全白名单，而是专门的产品内容 guard：[astRenderer.ts:25](../../src/core/utils/astRenderer.ts:25)。当前规则为：

- 移除 `script/applet/base/embed/object` 和 refresh meta。
- 保留结构、CSS、SVG、MathML、媒体及普通 HTML；不做整块转义。
- 删除明显直接引用宿主、网络、存储、动态执行或 HTML 注入能力的 `on*` 属性；例如 `this.classList.toggle(...)` 这类局部操作可保留。
- 拒绝 `javascript:` / `vbscript:`；按 tag 与 MIME 区分主动 data 文档和被动图片/音视频；普通网络、asset、blob URL 不统一删除。
- 主 DOM 中 raw iframe 的 srcdoc 再过 guard；默认 sandbox 允许脚本/表单等，但排除 `allow-same-origin` 和 top navigation。它和 `HtmlPreviewBlock` 的完整原文 iframe 不是同一路径。

源码注释明确这不是通用 sanitizer，且信任圈内容的刻意混淆 JavaScript 仍属接受风险。报告必须尊重这个产品边界，不能把正则 guard 写成完备安全证明，也不能据此提议把所有局部事件、CSS 和 SVG 删除。

guard 同时用于全量 AST 渲染和增量 raw HTML 的 seed、frozen、live 分支，避免仅最终渲染受控、流中直接灌入另一套 HTML。见 [astRenderer.ts:220](../../src/core/utils/astRenderer.ts:220)、[astExecutor.ts:387](../../src/core/utils/astExecutor.ts:387)、[astExecutor.ts:721](../../src/core/utils/astExecutor.ts:721)、[astExecutor.ts:735](../../src/core/utils/astExecutor.ts:735)、[astExecutor.ts:757](../../src/core/utils/astExecutor.ts:757)。

Tauri 配置当前 `csp:null`，不可照套桌面 `script-src` 推论；实际主动内容边界依靠 raw guard 和 preview iframe。app command 只接受 `main` WebView label，是另一个维度，不能把它写成同一个 main WebView 内每个消息都经过独立授权。[tauri.conf.json:22](../../src-tauri/tauri.conf.json:22)、[commands.rs:418](../../src-tauri/src/commands.rs:418)

## 7. 交互与状态延续

### 7.1 AI 按钮不是普通本地游戏按钮的同义词

桌面 `processInteractiveButtons` 给助手内容的普通 `<button>` 绑定发送逻辑，优先 `data-send`、否则 textContent，封装成 `[[点击按钮:...]]`，总长 500 字符；将 type 设为 button，防止 form 默认提交。配置关闭时清除已有绑定。系统工具、代码复制、Mermaid toolbar 和 `data-vcp-system-control` 被排除。[contentProcessor.js:961](E:/VCPChat/modules/renderer/contentProcessor.js:961)

点击禁用并加勾；异步发送 await 对应 Surface 的 `messageCommands.handleSendMessage`，失败恢复按钮并报错。[contentProcessor.js:1027](E:/VCPChat/modules/renderer/contentProcessor.js:1027)、[contentProcessor.js:1111](E:/VCPChat/modules/renderer/contentProcessor.js:1111)

移动复用消息根委托，只解释 `[data-message-content]` 内的按钮，不劫持 Rich Block 自己的控制栏；发送格式、data-send 优先级和 500 限长已对齐。[useMessageEvents.ts:19](../../src/core/composables/useMessageEvents.ts:19)、[useMessageEvents.ts:55](../../src/core/composables/useMessageEvents.ts:55)。但移动直接 `historyStore.sendMessage(...)`，没有在这里接纳结果并恢复已禁用的按钮，与桌面有明确实现差异。

继续追到 Store，`sendMessage` 有条件不满足时直接 return、编辑重发 catch 后自行通知、生成返回 null 后撤销乐观用户消息等路径，并未向该按钮返回明确成功/未提交结果。因此仅补 `await/catch` 也不能覆盖所有失败或未接纳情况；应沿当前聊天 owner 给这个真实消费者提供所需结果，避免重复通知。[chatHistoryStore.ts:718](../../src/core/stores/chatHistoryStore.ts:718)、[chatHistoryStore.ts:819](../../src/core/stores/chatHistoryStore.ts:819)。

两端 raw HTML 中普通 button 都可能同时承担作者局部事件和宿主发送按钮含义；要做“只在图内加一”“开始/暂停”一类交互，应验证事件传播和宿主按钮语义。opaque iframe 内的按钮不会自动进入主 DOM 事件委托，天然更适合完整独立交互文档。

### 7.2 跨帧保留范围

桌面 tail morphdom 显式保留 expanded、preview-mode、处理标记、按钮 disabled/勾选、播放中的 audio/video、成功加载的 img，并跳过某些后处理复杂子树；使用 id/data key 帮助复用。[streamManager.js:93](E:/VCPChat/modules/renderer/streamManager.js:93)、[streamManager.js:145](E:/VCPChat/modules/renderer/streamManager.js:145)、[streamManager.js:1723](E:/VCPChat/modules/renderer/streamManager.js:1723)

移动 raw HTML tail 已有 Rust 冻结前沿 + frozen/live reconciliation；已冻结子节点不逐帧重写，live 子树 morphdom 保留相同节点、complete 图片、正在播放的音视频。[astExecutor.ts:556](../../src/core/utils/astExecutor.ts:556)、[astExecutor.ts:569](../../src/core/utils/astExecutor.ts:569)、[astExecutor.ts:755](../../src/core/utils/astExecutor.ts:755)。它没有复制桌面的任意按钮状态守卫，不能以冻结前沿机制推断所有 live 按钮、输入值与焦点均稳定。

两端终态都可能重新接管内容树；局部展开和工具选择中已有专门 owner 的部分应沿其 owner 处理。不要扩成把整个 DOM/脚本状态持久化到聊天记录的通用机制，也不要用全量重渲染“修正”增量状态问题。

## 8. 动画引擎和资源所有权

| 资源 | 桌面 owner 与释放 | 移动当前含义 |
|---|---|---|
| 消息 CSS | MessageRenderer 的 owned style 集合；替换内容更新，消息/Surface 清理删除 | 每个 MessageRenderer 的 `useMessageStyleInjector`；scope dispose 删除 |
| HTML 预览 | 容器 `_vcpCleanup` + window resize message 监听；返回/替换/删除销毁 iframe | Vue 条件 iframe + generation；切换/刷新重建；图片桥槽位跟随 iframe |
| anime | 创建时注册可见性 owner；清理 `anime.remove` | raw 主 DOM 没有桌面 anime executor；preview 内由自身文档执行 |
| Three | patch WebGLRenderer；DOM 归属观察、停止脱离文档的渲染、dispose 与场景 geometry/material 清理 | 不应假设主 DOM 已有同样资源注册；完整 HTML 可在 iframe 依赖自己的库 |
| Pixi v8 | 消息 PIXI facade，构造即登记/暂停，`init({autoStart:false})` 完成后再登记，start 受配额控制 | 未发现对应主 DOM facade；属于明确增量能力 |
| rAF/timer | 注入可暂停 API；离开热区暂停，消息解除观察时取消记录 | preview 销毁即终结文档；目前没有桌面同等主 DOM 脚本 timer owner |
| 全局外部脚本 | `window._vcp_loaded_scripts`，head 中共享依赖 | 不应照搬为所有预览共享可写 window |

主要证据：[animation.js:79](E:/VCPChat/modules/renderer/animation.js:79)、[animation.js:173](E:/VCPChat/modules/renderer/animation.js:173)、[animation.js:607](E:/VCPChat/modules/renderer/animation.js:607)、[visibilityOptimizer.js:908](E:/VCPChat/modules/renderer/visibilityOptimizer.js:908)、[visibilityOptimizer.js:1045](E:/VCPChat/modules/renderer/visibilityOptimizer.js:1045)。

Pixi 当前话题根最多启动 3 个气泡；历史初始加载有效上限 1 个。离开热区主要冻结 ticker，不立即释放；额度不足时淘汰离开热区的最老实例，热区内实例不强制销毁。这是桌面性能策略，不是可以原样复制的移动设备预算。[visibilityOptimizer.js:65](E:/VCPChat/modules/renderer/visibilityOptimizer.js:65)、[visibilityOptimizer.js:449](E:/VCPChat/modules/renderer/visibilityOptimizer.js:449)

`cleanupMessageDomResources` 串起图片、Mermaid、预览、动画、附件、媒体、样式和解除可见性观察，替换内容前也调用对应清理。[messageRenderer.js:2744](E:/VCPChat/modules/messageRenderer.js:2744)、[messageRenderer.js:3688](E:/VCPChat/modules/messageRenderer.js:3688)。

这套 owner 不是任意用户脚本资源的全自动回收器：直接在 window 注册监听、绕开包装器的 timer、用户自建 Worker/Observer 等并未由 `processScripts` 统一登记。迁移时应说明实际支持的 API 及其生命周期，不宣称“支持任意 JS 并自动释放一切”。

## 9. 推荐的小型验证语料

这些语料用于下一步实现或具名 Android Debug 观察，不表示本次已做浏览器/实机验证。不要访问用户 Release 应用，也不建设 iOS 实机测试体系。

### A. CSS 根选择器、条件规则与 keyframes

```html
<style>
body .label { color: #d22; }
@media (min-width: 1px) { .label { border-left: 2px solid #27f; } }
:is(.a,.b) > span { font-weight: 700; }
@keyframes pulse { from { opacity: .4; } to { opacity: 1; } }
.label { animation: pulse 1s alternate infinite; }
</style>
<div class="a"><span class="label">本条消息</span></div>
```

同时在另一条消息放同名 `.label`，观察后代规则是否命中、跨消息是否串色、窄宽窗口下 @media 是否仍被隔离、动画能否建立。再给两消息不同的同名 keyframes，区分“selector 已隔离”与“animation 名称已隔离”。

### B. raw 岛与 HTML 围栏的执行语义

```html
<div id="sample-island"><span class="count">0</span></div>
<script>
const value = document.querySelector('#sample-island .count');
value.textContent = String(Number(value.textContent) + 1);
</script>
```

分别作为 raw 回复、`html` 围栏、完整 HTML 文档提交；桌面 raw 终态应进入执行器，移动 raw script 被删除，移动后两者经点击 preview 执行。两条消息使用相同 ID，检验桌面 document 查询的本消息优先。执行计数只用于这个样例，不据此假定所有脚本全局仅运行一次。

### C. 代码域与工具数据域保护

将“`<style>` 是样式标签”作为 inline code，随后给真实 style 岛；再把同一 HTML 放入四反引号围栏，以及未闭合工具结果文本。观察示例代码没有抽取 CSS、没有执行脚本，真实岛不被前面的示例吞掉，结束前后语义一致。

### D. 按钮局部状态与发送失败

```html
<button data-send="继续下一步">继续</button>
<input value="初始值">
<details><summary>展开</summary>局部说明</details>
```

在 tail 仍增长时点击/输入，观察跨帧、冻结、终态接管是否延续；断开真实发送条件后点击发送按钮，确认失败反馈与可重试状态。若增加只做本地状态的按钮，单独验证其是否意外触发 `[[点击按钮:...]]`。

### E. iframe 会话与图片导出

在 HTML 围栏中放一个计时文本、可编辑 input、Canvas 和普通 image。先预览，再切全屏、切代码、刷新、切换主题、离开消息；记录哪些操作重建文档。点击 Canvas 导出时立刻关闭预览，确认旧 nonce/旧 frame 不再交付图片，不泄漏 blob URL，也不把取消报成业务成功。

### F. 重资源按需启动和清理

选取 4 条最小 Pixi/Canvas 示例，明确其库来源和版本；观察流中是否只显示结构、终态启动是否受预算控制、滚出热区后的 ticker/帧行为、回到原消息的状态，以及删除消息/切话题后 GPU 与 timer 是否释放。桌面已有 3/1 配额不能代替移动实际预算选择。

## 10. 接入约束与实施建议

1. **保留现有内容能力。** SVG、MathML、普通 HTML、局部 DOM 事件、HTML preview 均已存在。修边界不应退化为整段纯文本或禁用全部脚本。
2. **先补 CSS 语义。** 用当前消息 owner 接收 AST 转换后的 CSS；修 root descendant、selector function、条件规则、keyframe。根据真实输入域处理 style 提取，不能只在最终 v-html 路径修而遗漏 AST frozen/live。
3. **动画以独立文档/岛 owner 为单位。** 若要让桌面 raw 动画在移动自然呈现，先定义何时建立可执行岛、完整内容 identity、终态执行和重建规则，优先复用 HtmlPreviewBlock 的 opaque 文档及窄桥经验；不把主 DOM guard 移除当成“兼容完成”。
4. **保留移动平台结构。** 共享 Vue/Rust 核心不增加桌面产品分支；资源和原生边界按 Android/iOS 当前实现处理。WebView 117 / Safari 26 是目标，桌面 Electron 的 API、CSP、文件 URL 与本地 vendor 假设不能直接移植。
5. **明确连续性承诺。** 消息流中 DOM 保留、终态重建、全屏切换、离屏暂停、话题卸载是不同状态边界。按用户交互价值选择保留的状态，不新增无消费方的通用状态快照系统。
6. **发送仍归聊天 owner。** 若需要从新富内容发送消息，复用现有发送能力与失败结果，不让动画运行时复制聊天状态或调用任意宿主命令。
7. **资源策略随能力进入。** 引擎本地化、异步加载失效、rAF/timer、图形 context、图片字节、消息卸载必须有清晰归属。仅展示一个 demo 不能算移动动画能力完成。

本次完成了源码与历史审计、CSS 转换函数的只读 host 探针及报告整理；未执行 `pnpm check`（没有修改跨层/核心生产代码），未运行设备旅程，未将文档与测试反向当作产品实现事实。
