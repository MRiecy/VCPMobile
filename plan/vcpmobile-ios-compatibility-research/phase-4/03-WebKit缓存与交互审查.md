# Phase 4：WebKit 缓存与交互重构方案

> 2026-10-16；审查基线 `7e4d645b`。这是 P4-5 方案草案，未实施生产代码，未执行 Apple 构建或 iOS 运行诊断。目标为最低 iOS/iPadOS 26.0、固定构建 Xcode 26.2；构建 SDK 与最低运行系统分别维护。

## 1. 目标与设计范围

本专题负责五件事：清理有明确范围的浏览缓存；让根应用与全屏浮层统一适应可见窗口；完成 iPad 展示、焦点、选择和返回；为富消息的渲染与导出建立资源所有权；把所需脚本、字体与样式完整装进应用包。Vue/Rust 继续承载共享业务，Swift 管理 WKWebView、键盘和原生展示资源。

功能范围沿用 [D11/D12/D14/D16](../09-Magi综合裁决与待决策清单.md)、[最新审批](../phase-2/09-功能权限与提示审议表.md)、[移动 UI 规范](../../../docs/MOBILE_UI_COMPATIBILITY.md)、[跨平台规范](../../../docs/CROSS_PLATFORM_DEVELOPMENT.md)。Markdown、KaTeX、Mermaid、SVG/MathML、raw HTML、Canvas、Tool/Thought/Diary、附件及流式收敛共同构成目标消息能力。实施集中于这些功能的边界和公共交互原语，不展开全站 UI 改版。

## 2. 模块与接口

| 模块 | 负责的数据和资源 | 对外契约 |
|---|---|---|
| Swift 浏览缓存操作 | 本次请求捕获的 WKWebsiteDataStore、完成回调 | `clear_webview_cache`：完成或调用失败；完成范围固定为内存/磁盘浏览缓存 |
| Swift 窗口几何采集 | 当前 WKWebView/Scene、键盘几何、观察者与实例 generation | 输出以 WebView bounds 为基准的 point 快照；加载与替换时重放 |
| 前端几何归一化 | 同一帧的原生快照、visualViewport、CSS env | 一个 `usableRect`、四边实际需消费的 safe inset、浮动键盘遮挡矩形 |
| 根应用与全屏浮层 | 可用矩形对应的布局空间 | 统一定位到 `usableRect`；内部布局只消费 safe inset 与自己的视觉间距 |
| 模态/编辑交互 | 当前顶层页面、opener、Selection、滚动容器 | 返回、关闭、焦点恢复和输入显露沿同一个操作会话收尾 |
| 富消息资源 | 图表任务、受控派生图片、导出临时文件 | 以 message/operation identity 归属，按字节、像素和并发预算释放 |

这些接口是实施契约，字段可以在编码时按实际消费者收敛。原生资源生命周期与消息、附件、会话生命周期分别归属；跨边界传递所需事实和句柄。

## 3. 浏览缓存的数据归属与完成契约

### 3.1 操作流程

维护页调用 `clear_webview_cache`；Rust 的 iOS 分支通过原生插件提交 Swift 操作。Swift 在主队列捕获当前 `webView.configuration.websiteDataStore`，清理集合固定为：

- `WKWebsiteDataTypeDiskCache`；
- `WKWebsiteDataTypeMemoryCache`。

调用 `removeData(ofTypes:modifiedSince:completionHandler:)`，时间取 epoch，completion 到达后 resolve。单次 Invoke 承接完整生命周期：准备 → 系统清理中 → 完成。WebView 不存在时返回调用失败；执行中持有本次捕获的 store，WebView 更换后仍等待这个 store 的回调。页面卸载只结束该页面的结果消费，原生回调继续释放资源。维护页在执行中禁用重复提交，Swift 对同一 store 的并发请求返回“清理进行中”。

Apple 将 WKWebsiteDataStore 定义为 WebView 的 cookie、缓存及站点数据管理对象，并提供按类别删除；当前 WebView 的配置对象才是实际数据归属。[Apple：WKWebsiteDataStore](https://developer.apple.com/documentation/webkit/wkwebsitedatastore)

完成结果含义是“系统完成了对指定两类数据的清理调用”。维护页文案为“已完成 WebKit 磁盘与内存浏览缓存清理；聊天、附件与设置保留”。API 没有删除字节数或逐文件失败清单，结果不包含释放 MB；随后加载的资源可以重新形成缓存。[Apple：removeData](https://developer.apple.com/documentation/webkit/wkwebsitedatastore/removedata(oftypes:modifiedsince:completionhandler:))

### 3.2 数据边界

浏览缓存与持久数据分开管理。Cookies、LocalStorage、SessionStorage、IndexedDB、Service Worker/Cache API 保留；聊天数据库、CAS 原件、导入临时区与 WebKit 数据存储分别归原领域管理。主题、会话选择和通知提示记忆继续持久化。预渲染刷新和 SQLite 维护拥有各自的操作入口及完成结果。

实施直接使用上述两类 WebKit 数据常量和完成回调；命令参数不向前端开放任意目录或任意数据类型。该边界由本功能的清理语义决定，不增加额外权限引导。

## 4. iOS 窗口几何与统一布局

默认采用一个清楚的布局策略：**根应用和全屏浮层共同定位到同一个可用矩形，键盘避让在这个边界消费一次。** Chat footer、Editor、Diary 等内部区域按容器剩余高度布局，只添加实际 safe inset 和自己的视觉间距。

### 4.1 原生采集与快照

Swift 为当前 WKWebView 安装 `WindowGeometry` owner，引用所属 window/Scene。采集基准是原生 `WKWebView.bounds`；WebView 本身继续占满框架分配的窗口，前端可用矩形只影响内部 CSS surface。这样采集源不会随着计算结果缩小，避免布局反馈循环。

原生快照包含：

```text
webViewGeneration       当前 WebView 实例身份
geometrySequence        本实例内递增序号
sampleId                回显前端本次采样身份
boundsPoints            WebView 的宽、高
dockedTopPoints?        停靠键盘在 WebView 内的顶边
floatingRectPoints?     浮动/分离键盘的相交矩形
```

UIKit 操作在主队列执行。窗口布局、安全区、键盘 frame、Scene 激活和 WebView 替换触发几何失效通知。屏幕坐标先经当前 window 转到 WebView 坐标，再与其 bounds 求交。停靠区域使用 `UIKeyboardLayoutGuide` 的默认 docked 跟踪语义，并结合真实键盘显示状态排除仅有 safe area 的隐藏状态；浮动矩形取键盘通知转换后的相交范围。系统给出的是边界矩形时，局部显露按这个保守范围计算，不宣称拥有分离键盘每个按键的形状。[Apple：keyboard layout guide](https://developer.apple.com/documentation/uikit/adjusting-your-layout-with-keyboard-layout-guide)

根桥安装监听后主动请求一次快照。后续原生通知和 `visualViewport.resize/scroll`、window resize 都汇入同一个 requestAnimationFrame 调度；最多一份原生采样在途，变化只标记需要下一次采样。请求携带 sampleId 和当前前端 viewport revision，回复到达后重新读取 viewport；若其宽、高、offset 或 scale 已变化，丢弃这对不一致输入并取得新采样。WebView 更换释放旧观察者，并以新 generation 开始；旧 generation 或已被更新序号取代的结果不落入布局。

初始原生几何未知时，先以 visualViewport 和 CSS env 构造可见区域，不推测键盘高度。旋转过渡中继续使用最新完整输入，下一份一致采样原子替换矩形与 safe inset；几何尚未稳定不阻塞页面加载和业务任务。

### 4.2 坐标转换与可用矩形

前端以**未受 surface 布局影响的** `document.documentElement.clientWidth`、visualViewport 和原生 bounds 为输入。`html/body` 保持整个 layout viewport 尺寸；不从 `.vcp-app-root`、聊天 footer 或已缩小的浮层反推键盘。CSSOM View 坐标以 CSS px 表达，并提供独立的 scale/offset。[CSSOM View](https://drafts.csswg.org/cssom-view/#the-visualviewport-interface)

在 viewport-fit=cover、根文档宽度匹配 WebView 的配置下，同次采样按以下关系映射：

```text
cssPerPoint = documentElement.clientWidth / boundsPoints.width / visualViewport.scale
native point (x, y) → layout CSS (offsetLeft + x × cssPerPoint,
                                offsetTop  + y × cssPerPoint)
V = visualViewport 的 offsetLeft / offsetTop / width / height
N = 原生 WebView bounds 映射后的矩形
usableRect = V ∩ N
若有停靠键盘：usableRect.bottom = min(usableRect.bottom, 映射后的 dockedTop)
```

这是选定的坐标模型；实施时核对 WKWebView content inset/page zoom 配置与上述覆盖关系，几何诊断保存输入值。不同采样的 bounds 和 viewport 不混算。键盘使 visualViewport 已缩短时，取交集/min 会自然保留那条边界；原生键盘也报告同一遮挡时，结果仍只计算一次。缩放产生的 viewport 缩小属于可见区域事实，只有真实原生键盘状态才标记“键盘打开”。

前端以 CSS 变量一次写出 rect 的 left、top、width、height，根 surface 与 Teleport 全屏 surface 使用同一个公共定位类。普通页面、sheet 和 dialog 位于所属 surface 内，通过 flex/overflow 布局。iOS 内部 footer 的额外键盘 padding 归零；键盘状态仍可用于输入交互，但不能再改变同一 surface 的底部高度。Android 使用自己的归一化输入，平台分支留在公共边界。

### 4.3 safe area 与视觉间距

CSS `env(safe-area-inset-*)` 是安全区唯一来源，在 root surface 外的轻量 probe 读取，保留原始四边 CSS 值。根据完整 WebView 边界 N 构造四条 unsafe band，再计算它们与 usableRect 的交集，得出该矩形仍需消费的 safe inset。例如底边已位于键盘上沿、没有覆盖 Home indicator 区域时，safe bottom 为 0。CSS env 与 viewport-fit=cover 的含义见 [WebKit 说明](https://webkit.org/blog/7929/designing-websites-for-iphone-x/)。

`--vcp-safe-*` 只表达这四个剩余安全边距；工具栏最低留白、按钮间距等使用独立视觉 token。每个独立 surface 在边缘消费 safe inset 一次，内部组件通过容器布局得到空间；嵌套覆盖层按其所属 surface 处理。业务组件不重新读取 CSS env，也不将固定视觉间距算进键盘或系统安全区。

### 4.4 浮动键盘与编辑区域

浮动/分离键盘保留为遮挡矩形，不缩短整个 usableRect。当前编辑 owner 在光标或输入区域与该矩形相交时，滚动自身容器使编辑位置可见；动作限定于当前活动 editor，用户已离开编辑器或发生新的焦点选择时终止。textarea 没有直接 DOM caret rect 时，以当前输入框的可见区域作保守显露目标。硬件键盘仅按真实快捷条/遮挡采样处理。

阶段性几何诊断覆盖停靠、浮动、分离、硬件键盘，输入框之间切换、覆盖层打开/关闭、旋转/分屏缩放、WebView 更换以及迟到快照。检查对象是输入坐标和最终可用矩形，不要求每个业务 footer 自行实现键盘算法。

## 5. iPad 展示、输入与导航

### 5.1 原生 presentation

P4-1/P4-3 的原生操作会话持有 presenter、请求身份和完成回调。presenter 从当前 WKWebView 所属 VC/Scene 解析；同一 Scene 一次展示一个操作。系统关闭、取消、失败或 presenter 消失时，按本次 operation identity 完成并释放引用。

iPad 分享使用当前 WebView 作为 `sourceView`。调用方有按钮锚点时，传递同一几何快照下的 CSS rect，由原生沿第四节的逆变换得到 `sourceRect`；没有按钮锚点的程序触发入口，使用当前 WebView 可见区域中的固定中心 anchor。展示期间窗口变化重新计算锚点。Quick Look 和 Files 使用各自系统 controller 的 presentation 规则。[Apple：sourceRect](https://developer.apple.com/documentation/uikit/uipopoverpresentationcontroller/sourcerect)

### 5.2 输入与可点击区域

iOS 编辑控件最小字号为 16 CSS px。textarea、提及高亮背板和占位层共用同一字体、行高及 padding；公共输入组件负责搜索、Prompt 和配置字段的同一规则。按钮拥有至少 44×44 CSS px 的实际点击区域，图形尺寸可以保持紧凑，邻接命中区域互不重叠。label、焦点轮廓和 Tab/Enter/Escape 行为随对应控件实现。

文本正文优先交给系统选择。祖先长按只处理消息非交互区域；已有 Selection、编辑控件、链接、按钮、图片、SVG/Canvas 等由其自身交互 owner 接收手势。消息操作入口位于消息头/操作菜单，选词、复制局部内容与操作整条消息各有清楚入口。

### 5.3 模态会话与焦点

应用内 modal/page owner 记录开启时的 opener 与会话身份，顶层模态管理可聚焦元素和 Escape 关闭。关闭后若 opener 仍挂载、仍属于当前页面且用户未选择新焦点，恢复该元素；否则把焦点留在当前有效上下文。原生 picker/share 完成后恢复页面交互，不主动重新弹出输入键盘。

返回统一经 modal/page/session 顺序归约；根部保持应用。每个 HTML 预览实例持有自己的 iframe ref，刷新、进入全屏和关闭都操作该实例；异步刷新以实例 generation 确认目标，卸载时取消定时器并撤销回调。

## 6. 富消息、本地媒体与有界资源

### 6.1 渲染与内容边界

消息渲染以 message identity 绑定 AST/tail、图表任务和 DOM 节点。滚动 owner 维护 following/free 与分页锚点；字体、图片或图表完成后，只在 following 状态跟随底部，free 状态保持阅读位置。离屏渲染优化不改变消息内容和交互。任务完成时同时检查 message generation、renderer 是否仍挂载、目标节点是否仍归该实例，再提交 DOM 更新。

HTML 使用两种内容边界：

| 路径 | 执行环境 | 交互契约 |
|---|---|---|
| AST rich HTML | 受信内容产品规则下的 DOM/SVG/MathML，允许局部 DOM 交互 | 宿主能力由显式消息事件桥调用；解析与增量更新使用同一内容规则 |
| HTML 代码预览 | srcdoc + `sandbox="allow-scripts"`，独立 opaque origin | 支持脚本和 Canvas；图片/导出消息检查 source window、nonce 与 origin=null，并绑定当前预览实例 |

本地资源持久化 attachment/CAS identity；在实际展示时通过 `convertFileSrc` 生成当前平台 URL。iOS 由 `asset://localhost` 经 WKURLSchemeHandler 交付。opaque iframe 所需资源由所属预览会话明确交接，文件路径和读取权限仍由主应用管理；大文件使用路径/ticket，有限图片数据按字节预算传递。

Blob URL 归创建它的 document 管理。iframe 产生的图片先在该 document 编码为有限 Blob，再通过当前预览实例的导出请求交给主应用；主应用收到最终完成/取消后通知释放，iframe 卸载也释放自身 URL。图片桥不获得通用宿主 IPC 或任意文件读取能力。

### 6.2 本地预览与派生资源

| 用途 | 数据交付 | 生命周期 |
|---|---|---|
| 文本预览 | Rust 按 attachment identity 解析实际路径，有界读取并返回 `text + truncated + decoding`；UTF-8 预览在完整字符边界截断 | 请求绑定当前 viewer identity，关闭/切换后取消读取或丢弃迟到结果 |
| 图片列表 | P4-2 图像服务按显示尺寸和像素预算生成缩略派生 | 列表持有派生使用期，空闲缓存按累计字节预算淘汰 |
| 全屏图片 | 按当前显示面积、显示密度和允许放大范围取得受控派生 | viewer 持有当前派生；重新取得更清晰版本时完成切换后释放旧版本 |
| 原件保存/导出 | 原件只读引用，或本次图形编码结果的受控临时文件 | 由系统操作持有到完成/取消，随后释放临时资源 |

图像原件存于 CAS，显示派生与原件 identity 分开。用户要求保存原图时读取原件，显示尺寸不会改变保存内容。资源参数集中见 6.5。

### 6.3 Canvas/Mermaid 导出流程

图形导出使用一个 operation owner，按导出队列执行需要像素分配的步骤：

```text
开始导出 → 核对图形尺寸/预算 → 生成受控 Canvas → toBlob
         → 检查编码字节 → 有界传输到临时导出文件
         → PhotoKit / Files / 系统分享 → 完成或取消 → 释放
```

分配前核对有限、正数宽高，按用途参数计算输出：

```text
scale = min(1, maxSide / max(width, height), sqrt(maxPixels / (width × height)))
outputWidth / outputHeight = 至少 1 像素的向下取整结果
RGBA budget = outputWidth × outputHeight × 4
```

尺寸乘法使用溢出安全核算，编码字节和同时存在的 SVG 字符串、Image、Canvas、Blob、传输缓冲另列预算。Mermaid/SVG 在交给 Image 解码前就设置目标宽高、保留 viewBox；图形本身在页面仍可缩放查看。通用 Canvas 的导出只控制应用拥有的输出缓冲和传输，用户 HTML 脚本自身分配的内存不被该预算宣称覆盖。

`toBlob` 返回空值、资源加载失败、origin-clean 限制和编码超限各自返回具体失败。外部字体、图片和 SVG foreignObject 在导出前按实际资源依赖准备；页面展示与可编码导出分别反馈。Blob 经 P4-1 的有界字节传输接口以 `purpose=temporaryExport` 落盘，Swift 接路径及实际 MIME/字节数。只有用户选择“添加为附件”时才进入 CAS 登记。

准备阶段取消会中止读取/传输并回收文件；提交系统保存或分享后，文件持有到该系统操作完成。页面离开只取消页面消费，系统仍在读取的文件由 operation owner 保持。Blob URL、Image、Canvas 和 iframe 引用在完成、失败、取消与实例卸载路径分别释放；取消后的迟到回调按 operation identity 收尾。

### 6.4 图表缓存与异步任务

Mermaid 渲染服务在消息领域维护一个按估算字符串字节计量的 LRU，key 包含源码、主题及实际渲染选项；相同 key 的在途任务合并，记录订阅它的 message identity。缓存淘汰只影响可重算 SVG，不影响原始消息。最后一个消费者离开时，尚未开始的任务撤销；已经进入库执行的任务完成后释放结果，除非仍有有效消费者。

缓存保存可实例化的 SVG 模板。插入每条消息前，由该渲染实例分配独立 ID 前缀，结构化重映射 SVG 元素 ID、marker/clipPath、href、CSS `url(#...)` 及其他内部引用；同一图表在多条消息出现时，引用始终指向各自实例。全屏查看器同样实例化自己的身份。若某类输出无法可靠完成模板化，就只缓存与实例无关的中间结果，由各实例生成成品 SVG。

异步回调仅更新仍存在且归属一致的节点。字体与图表资源完成后，滚动 owner 根据当前阅读状态处理高度变化。

### 6.5 资源参数集中表

下表区分本方案的起始工程参数与仍需收敛的参数。图片参数与 [文件与图片专题](./01-文件导入与图片处理审查.md) 统一维护；实施前结合显示目标、编码输出和同时存在的副本完成预算表。

| 参数 | 本方案取值 / 状态 | 控制位置 |
|---|---|---|
| 文本预览字节 | 建议默认 128KiB；读取缓冲另计 UTF-8 边界所需最多 4 字节 | Rust 读取前限量，返回 truncated |
| Canvas 导出活动数 | 建议 1 | 导出 operation 队列 |
| Mermaid 渲染活动数 | 建议 1，相同 key 合并在途任务 | 消息领域渲染服务 |
| Canvas maxSide / maxPixels / maxEncodedBytes | 待按输出清晰度与副本核算定值 | 分配前约束尺寸，编码后约束字节 |
| Mermaid cacheBytes / maxEntryBytes | 待按长会话资源预算定值 | 计量 LRU 插入、命中与淘汰 |
| 列表/全屏图尺寸、像素、编码字节和累计缓存 | 与 P4-2 按用途确定 | 图像服务及 viewer 使用期 |
| 临时导出字节、传输块大小和占盘 | 与 P4-1/P4-3 统一确定 | 字节接收、临时文件及系统消费期 |

应用拥有的像素缓冲、字符串、编码文件、传输块分别核算；库/系统内部开销列为不确定项。数值是静态资源控制，不代表实测设备峰值。影响用户输出清晰度和操作结果的参数在实施前说明；内部队列、分配与释放方式由实现负责。

### 6.6 应用包与渲染验证

共享前端构建同时声明 Android chrome87 与 Safari 26.0 的目标约束。实施前核对锁定 Vite/esbuild 支持的 target 语法，并检查实际生成 JS/CSS。Mermaid 懒加载 chunk、KaTeX CSS 与字体均进入应用包，首次离线打开可从本地资源加载。

渲染资产覆盖长公式/MathML、Mermaid foreignObject/中文字体/全屏/导出、受控 HTML 局部交互、opaque iframe 脚本/图片桥/Canvas、本地中文与空格文件名、图片 MIME 与旋转，以及长历史中的图片晚到、分页保锚、文本选择时继续流式输出和前后台后的同一 tail 收敛。

领域验证覆盖 iframe 实例身份、Canvas 预算、卸载后的异步回调和几何输入配对；AST 连续性与富 HTML 资产继续承担对应行为验证。happy-dom 检查状态和 DOM 契约，生产 build 检查资源打包；真实字体/SVG/CSS 几何和 WKWebView Canvas 导出结果由目标运行证据单独记录。

## 7. 实施片段与交付证据

| 片段 | 交付范围 | 复审重点 |
|---|---|---|
| W1 浏览缓存 | Swift 操作、Rust command、维护页结果 | store 归属、两类数据、等待 completion、WebView 更换与重复调用 |
| W2 窗口几何 | Swift 采集、前端快照配对、公共 surface 类、根应用和全屏浮层消费 | 原生固定基准、scale/offset、旋转乱序、safe band、停靠与浮动键盘 |
| W3 输入与模态 | 公共编辑样式、命中区、Selection、焦点和 presentation 接线 | 输入背板一致、顶层交互、opener 身份、iPad anchor、同一次关闭 |
| W4 媒体资源 | 有界文本预览、图表服务、Canvas 编码、临时导出交接 | 分配前预算、原件/派生用途、取消、系统消费期间文件存活 |
| W5 打包与目标诊断 | Safari 构建约束、离线资源、相关领域验证、Phase 4 Actions | JS/CSS/字体资源、Swift 编译链接、具体几何与导出问题的目标诊断 |

W1 可独立完成；W2 的坐标契约供 W3 的原生锚点复用；W4 与 P4-1/P4-2/P4-3 的文件和图像接口同步实施。每个片段完成跨层静态检查和针对性验证，再形成审查结果。

本轮证据为生产入口、锁定依赖源码和平台 API 文档；本方案尚未实施。iOS 26 WKWebView 的实际 geometry、popover、文本选区、Canvas 和自定义协议运行结果仍待目标诊断。后续 `pnpm check`、领域用例、生产资源检查、device archive 及按需 Actions 单项 Simulator 结果分别记录。日常采用源码审计和阶段性 Actions，不建设项目 iOS 实机测试体系。

## 8. 实施入口与一手资料

以下入口用于导航，不定义新方案必须保持的文件结构；行号对应审查基线。

| 主题 | 起始位置 |
|---|---|
| 缓存操作 | `src/features/settings/components/MaintenanceSection.vue:13`；`src-tauri/src/vcp_modules/infra/maintenance_manager.rs:274` |
| WebView 生命周期 | `src-tauri/plugins/vcp-mobile/ios/Sources/VcpMobilePlugin.swift:48/65` |
| 根几何与布局 | `src/core/composables/useKeyboardInsets.ts:117`；`src/App.vue:22/722`；`src/assets/themes.css:8`；`index.html:6` |
| 输入与全屏面 | `src/features/chat/InputEnhancer.vue:684`；`src/features/chat/ChatView.vue:179/365`；`src/components/ui/FullScreenEditor.vue:99` |
| 选择、返回与焦点 | `src/core/directives/longpress.ts`；`src/core/composables/useModalHistory.ts`；`src/components/ui/VcpPrompt.vue` |
| 富消息 | `src/features/chat/MessageRenderer.vue:613`；`src/core/utils/astRenderer.ts:23`；`src/features/chat/blocks/HtmlPreviewBlock.vue` |
| 图像和导出 | `src/features/chat/blocks/MermaidFullScreenViewer.vue:216`；`src/core/utils/renderedImage.ts:41`；`src/components/ui/RenderedImageViewer.vue:380` |
| 本地媒体 | `src/features/chat/attachment/AttachmentViewer.vue:89`；`src/features/chat/attachment/types/ImageAttachment.vue` |
| 构建资源 | `vite.config.ts:109`；`src/main.ts:13`；`src/tests/unit/chat/` |

- 锁定输入：`src-tauri/Cargo.lock:5223` Tao 0.37.1、`:5291` Tauri 2.12.1、`:7207` Wry 0.57.0。
- [Wry 0.57.0 WKWebView 实现](https://docs.rs/crate/wry/0.57.0/source/src/wkwebview/mod.rs)：本轮读取本机同版本 registry 源码，核对数据存储与 iOS WebView 配置。
- [Tauri 2.12.1 asset protocol](https://docs.rs/crate/tauri/2.12.1/source/src/protocol/asset.rs)：本轮读取本机同版本源码，核对资源来源、请求与响应边界。
- [Apple WKWebsiteDataTypeDiskCache](https://developer.apple.com/documentation/webkit/wkwebsitedatatypediskcache)、[MemoryCache](https://developer.apple.com/documentation/webkit/wkwebsitedatatypememorycache)。
- [Apple UIKeyboardLayoutGuide](https://developer.apple.com/documentation/uikit/uikeyboardlayoutguide)、[followsUndockedKeyboard](https://developer.apple.com/documentation/uikit/uikeyboardlayoutguide/followsundockedkeyboard)。
- [WebKit Visual Viewport API](https://webkit.org/blog/9674/new-webkit-features-in-safari-13/)、[CSSOM View 坐标定义](https://drafts.csswg.org/cssom-view/#the-visualviewport-interface)。
- [Apple UIPopoverPresentationController sourceRect](https://developer.apple.com/documentation/uikit/uipopoverpresentationcontroller/sourcerect)。

这些资料说明 API 与框架行为；本方案的接口、预算和布局策略是工程设计，实施后仍须取得各自范围内的验证证据。
