# AGENTS.md — VCP Mobile Vue 渲染层

> 本文件适用于 `src/**`。它继承仓库根规则，只补充 Vue 应用、前端状态、覆盖层、消息渲染与移动交互边界；当前行为必须从本目录生产代码及其实际 Rust 调用方恢复。

---

## 1. 子系统定位

`src/` 是 Project Avatar 的 Vue 渲染层。它把 Tauri 返回值、事件与流式消息归约为前端状态，按当前移动端窗口宽度呈现同一套业务组件，并把触控、返回、表单和消息操作转换为现有 Store、Composable 或 IPC 调用。Android 为当前生产平台；iPhone/iPad 按根规范分期接入，平台原生差异在边界归一化，不能复制业务状态。

### 1.1 正常数据流

```text
Tauri Result / Event ──> App / Store / Composable ──> Feature / Component ──> UI
                              ^                                  |
                              └────── invoke / 用户意图 ─────────┘

Rust AST / TailFrame ──> MessageRenderer ──> AstExecutor / DOM Engine ──> Rich Blocks
```

前端只归约和呈现当前调用链实际送达的数据，不从页面可见性、按钮状态或本地动画反推 Rust 业务结果。一次操作的传输完成、前端状态变化和后端业务完成属于不同阶段，按实际生产代码逐层确认。

---

## 2. 代码结构

```
src/
├── main.ts                    # 主 WebView：Vue、Pinia、Router、全局指令与错误入口
├── App.vue                    # 生命周期、原生事件、Insets、外层工作区与全局挂载面
├── appStyles.ts               # reset、UnoCSS、主题样式的加载顺序
├── core/
│   ├── stores/                # 跨 Feature 状态与生命周期 owner
│   ├── composables/           # 浏览器、Tauri 事件和交互生命周期桥接
│   ├── dom-engine/            # 流式消息的局部 DOM 执行器
│   ├── constants/             # 前端共享常量与语义层级
│   ├── types/                 # 前端共享 DTO 与状态类型
│   └── utils/                 # 渲染、转换与无状态工具
├── features/                  # 领域视图、领域 Store 与共置交互
├── components/                # 应用外壳、设置原语与共享 UI
└── assets/                    # 主题和全局消息样式
```

目录表示代码归属，不替代调用链。新增状态、组件或工具前先定位当前消费者与 owner；只有多个 Feature 共同消费且生命周期一致的能力才进入 `core/` 或共享 `components/`。

---

## 3. 应用装配与状态所有权

### 3.1 根应用

`main.ts` 装配主应用；`App.vue` 持有进程级前端启动、原生窗口事件、分享与通知入口、Insets 桥、返回链和主工作区。`core/stores/shareStore.ts` 管理当前一次分享、草稿 revision 和消息提交，App 只转交入口事件；新分享覆盖未发送内容，已提交消息归聊天。全局监听必须在对应生命周期建立，并在卸载路径释放；不得由普通 Feature 重复注册第二套根监听。

浮动助手的独立 WebView、页面、Store 与设置入口已移除；`core/stores/assistant.ts` 仍是主应用的智能体/群组列表所有者。

前端从 [core/utils/platform.ts](core/utils/platform.ts) 同步取得 OS 插件提供的平台身份，普通差异由共享 Vue 中的运行时分支处理。`appLifecycle.bootstrap` 继续拥有启动编排；实际权限、生命周期和任务状态由各自操作取得，平台识别不再等待能力查询。具体约定见 [跨平台开发规范](../docs/CROSS_PLATFORM_DEVELOPMENT.md)。

`useAppLifecycle` 将 iOS 原生 network online 与既有浏览器 online 汇入同一个流恢复 owner；iOS 在首次真实 active snapshot 前不发起恢复。卸载释放两条原生监听，迟到安装立即释放；网络路径可用只表示可以尝试对账，不表示服务端已可达。

`useWebViewResume` 只向原生快照遮罩报告当前文档的可展示状态；按恢复 token、文档身份、首屏历史与壁纸解码配对，原生继续等待实际绘制提交。它不重跑 bootstrap、不接管 SSE、不以持续流式内容稳定作为揭幕条件。

`useIOSWindowGeometry` 在根部配对原生 bounds/键盘与 visualViewport，同一可用矩形供根、全屏和底部 sheet 消费。内部组件不重复加 iOS 键盘 padding；浮动键盘只影响当前编辑区。`attachmentStore` 的 operation ID 贯穿原生选择与 Rust 登记，外部文件没有 HTML/XHR 入口；`temporaryExport` 只接 WebView 内存产物。

`core/utils/nativeFilePicker.ts` 统一附件、头像、配置的 iOS 原生选择接纳：prepare 返回前保留取消意图，取得 reservation 后才启动 picker，结束后撤销剩余登记。Android 继续原单次选择。这个短期调用记录不替代 `attachmentStore` 的导入/CAS owner；已交付文件仍由实际消费者释放。

`useNotificationProcessor` 将实时事件和系统快照中的原生事件注册失败归为同一固定 ID 告知；`notification` Store 私有的 `oncePerWebview` 接受记忆先于原 30 秒去重，关闭 Toast、清除或淘汰历史不会重置。新 WebView 的新 Pinia 可再次接受 Rust 保留的真实注册失败；guardian/helper 继续原有通知语义。

### 3.2 Store、Composable 与组件

- `core/stores/` 持有跨 Feature 或跨挂载周期状态；`features/<domain>/` 内的 Store 持有领域状态。
- Composable 连接 Vue 生命周期、浏览器能力或 Tauri 事件；组件消费状态并保留只属于自身挂载期的瞬时交互。
- 现有调用链允许 Store、Composable 或明确的根/边界组件直接 invoke。不得为了形式统一新增无所有权价值的转发层。
- 修改并发请求、取消、恢复或异步回调时，沿代码检查当前 identity、generation、owner 与卸载边界，不从界面最终看起来正确推断竞态已经闭合。

### 3.3 导航与覆盖层

主 Router 只承载实际路由；页面式业务入口由 `overlayStore` 的 page stack、`FeatureOverlays` 和 `SlidePage` 形成虚拟导航。`layoutStore` 持有左右抽屉意图，`useModalHistory` 连接返回顺序；不得在 Feature 内复制另一套全局页面栈或返回栈。

`GlobalOverlayManager` 负责 Prompt、Confirm、ContextMenu、Editor、Toast 与共享 Viewer，并提供 Feature Teleport 目标。新增全局覆盖面前先找到现有 owner、挂载点与关闭路径；局部弹层留在局部组件，不因层级问题直接抬成全局状态。

### 3.4 设置与领域偏好

`features/chat/voice/voiceCallStore.ts` 与 `voiceConversation.ts` 提交通话操作并按 revision 呈现 Rust 快照，不再认领转写、提交消息或协调生成取消。`microphoneMonitor.ts` 接收 `vcp-voice-audio` 的有界频带；开关麦通过 Rust 单次命令完成准备、原生调用与封存，开麦意图携点击时的生命周期 epoch，防止后台恢复后执行旧意图。PCM、AEC、Silero、缓存和播放归移动插件。关麦及后台停止新采集，已录内容由 Rust 继续识别、发送和朗读；回前台不自动开麦。`voiceHistorySync.ts` 订阅已落盘消息并在应用恢复时对账，历史窗口按 ID 合并，深翻历史保留锚点；恢复不会重发转写、重播 TTS 或消费草稿、附件和编辑态。通用监听卸载只脱离视图，主动切换聊天和挂断仍结束通话。插话、挂断、手动停止模型会停止对应生成与朗读；TTS 独立停止或失败不取消模型。配置复用全局 Settings；预设保留入口，气泡朗读入口尚未接入。语音按纯文本进入现有群聊发言模式，不扩展手动 `@` 邀请。本地诊断不进入 ASR，系统 AEC 启用不代表效果已验证。

`features/settings/catalog.ts` 组织稳定页面 ID、懒加载组件与布局；`SettingsView` / `useSettingsNavigation` 只拥有当前访问、退场画面和返回桥接。身份、连接、总结页面分别通过 `editing/useSettingsPageSession` 建立字段会话，正式快照始终归 `core/stores/settings.ts`。修改字段走显式 edit；保存只提交本页 dirty patch，外部快照重基不能覆盖用户后续输入。

依赖保存的诊断、重建与导出等待 commit 的确定结果；导入由连接页冻结并 settlePending 后提交，只接受覆盖字段。`active=false` 立即释放本次会话并关闭局部 ModelSelector、裁剪和导入确认，未交付 picker 沿既有 helper 取消；隐藏不启动保存，已提交任务继续归原 owner。

`features/chat/preferences/` 拥有消息呈现、平滑动效、消息震动和流式工具折叠的类型、存储及只读 Store；`features/chat/settings/ChatPreferencesPage.vue` 只是一个入口，聊天消费者不依赖它挂载。主题草稿/确认仍归 ThemePicker 与 themeStore；自动检查更新偏好归 updateStore。聊天与更新共用 `core/preferences/localPreference.ts` 的异常捕获 IO，领域各自保留 key、编码和默认值。新增偏好应在所属领域接通 UI 与真实消费者，不把设置中心或主题 Store 扩为业务总容器。

`features/settings/developer/` 持有本机开发者模式和显式触感试听。关于页六连击解锁后，经同一设置导航进入；返回开发者页的父级仍保留 SettingsSubPage 的 modal 所有权。解锁状态与试听参数不进入备份。配置备份 v2 由 `useConfigBackup` 协调 Settings、theme、chatPreferences 各自的持久化入口；v1 缺失字段保留当前值，部分失败按实际范围报告。

---

## 4. 消息渲染与交互完整性

`MessageRenderer.vue` 消费当前消息、AST snapshot 与 TailFrame；`core/utils/astExecutor.ts`、`core/dom-engine/` 和各 Rich Block 承接局部更新及交互。修改任一环节时沿实际数据形态追踪生成者、缓存身份、DOM owner、恢复路径和卸载清理，不能用全量重渲染掩盖增量链错误。

`features/chat/composables/useMessageTailRenderer.ts` 随消息实例持有尾部 refs、DOM 已应用帧、恢复与清理；气泡移动不重置这份生命周期。稳定气泡投影与尾部更新分开计算。`features/chat/utils/messageStyles.ts` 纯计算原有作用域样式输入，消息级样式实例一次提交有序合集。`data-message-content` 仅标记生成内容的交互范围，不能用包含组件控件的 stable/tail 外壳替代。流式期间增量展示，终态完整结果一次接管；两阶段分块与 hash 不要求统一。

Markdown、代码、KaTeX、Mermaid、SVG/MathML、受控 raw HTML、Tool、Thought、Diary、附件和流式收敛共同构成现有消息能力。兼容或安全修改必须保留结构、样式、媒体与交互；隔离能够触达宿主或外部状态的主动执行面，不采用整块转义、纯文本降级或禁用全部脚本来替代边界设计。

消息内部按钮、链接、图片与引用由现有事件桥和对应 Store 处理。新增交互必须复用真实消费入口，区分 DOM 局部反馈与业务结果，不在渲染器内另建业务状态所有者。

消息根使用 `v-longpress.message`，普通正文、角色名、头像和留白共享消息菜单；Markdown/AST 容器不是独立交互所有者。代码选字、表单、链接、媒体预览及显式 `data-longpress-native` 控件保留自身交互，普通消息正文的选择样式随这个范围处理，不能影响消息外的富文本选择。

工具折叠偏好只控制默认策略，ToolBlock 的本次手动展开/收起优先。MessageRenderer 在同一消息中按工具次序和逐字段语义比较延续短交互 key，避免 Markdown 合并或终态 hash 变化丢掉手动选择；工具结构改变或组件卸载可重建默认状态，不复制大正文/base64 到 key，也不持久化该交互状态。消息震动开关共同控制原生 send 与正常 complete 意图，已提交终态和离线完成恢复通过 context.replayed 保持静默，不接管长按等界面反馈。

---

## 5. UI 与样式约定

### 5.1 移动端产品表达

- 高密度线性布局优先，列表重于卡片；避免大面积圆角和厚重投影。
- ID、UUID、状态码使用 Monospace；灰度承载主体，彩色只表达清晰状态。
- 交互反馈保持内敛，使用侧边强调、边框或透明度变化，不做大幅缩放与弹跳。
- 内容区域不使用 backdrop blur；fixed/sticky 单例若保留轻度 blur，必须对应真实移动端性能证据。

手机、平板、分屏和折叠状态共享同一组件树与业务状态，只按当前 WebView 可用宽度改变呈现。不得按品牌、机型、方向或“桌面模式”复制页面和 Store。

### 5.2 样式与层级

`components/settings/SettingsCanvas.vue` 是显式启用的自然画布设置模板；当前样板为语音设置，其他页面未迁移。它限定排版样式，并通过 `viewport` 模式承接内容滚动、底部操作槽与现有 Insets。普通 list 宿主不传 `viewport`。Section 在画布内默认无卡，Row 选择项继续使用统一底部菜单；业务页仍拥有草稿、保存和离开语义。组合规范见 `docs/vue_docs/components/21_设置原子组件与主题系统.md`。

Vue 组件使用 `<script setup>`；状态 Store 延续 Composition API。样式优先复用 UnoCSS、主题变量和现有共享类，复杂的消息块或交互动画可留在对应生产样式文件。

`appStyles.ts` 中 reset、UnoCSS 与主题样式的顺序具有实际级联含义。全局覆盖层使用 `core/constants/layers.ts` 定义的语义层级；局部 stacking context 可以使用局部数值。遇到遮挡先检查 Teleport、父级 stacking context 与实际挂载位置，不用更大的魔法数字覆盖问题。

---

## 6. 修改与完成边界

实现需求时，从当前视图或事件入口追踪到 Store、Composable、IPC 与渲染消费者，复用现有 owner 完成最小闭环。跨 Rust/Tauri 边界时同时读取对应目录的 `AGENTS.md`，并以两端生产代码核对字段和生命周期。

前端或跨层变化从仓库根运行 `pnpm check`。Android 窗口、Insets、返回、触控或性能结论分别以实际取得的构建与设备证据说明；iOS 按根规范采用源码审计及 G1/G2 阶段性 Actions 验收，不建设实机测试体系。测试只能作为额外执行结果，不能定义生产行为。

**完成**：目标行为由现有状态 owner 驱动，组件、覆盖层、IPC、渲染与清理路径保持一致，并明确说明已验证的生产代码路径、构建结果及尚未取得的设备证据。
