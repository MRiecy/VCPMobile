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

`core/stores/assistant.ts` 是智能体/群组列表 owner。前端从 [core/utils/platform.ts](core/utils/platform.ts) 同步取得平台身份，普通差异在共享 Vue 中运行时分支，约定见 [跨平台开发规范](../docs/CROSS_PLATFORM_DEVELOPMENT.md)；`appLifecycle.bootstrap` 拥有启动编排，权限与任务状态由各自操作取得。

- `useAppLifecycle`：唯一的流恢复 owner，汇入浏览器与 iOS 原生 online；网络可用只表示可尝试对账。
- `useWebViewResume`：只向原生快照遮罩报告当前文档可展示，不重跑 bootstrap、不接管 SSE。
- `useIOSWindowGeometry`：根部提供唯一可用矩形，供根、全屏和底部 sheet 消费；内部组件不重复处理键盘 padding。
- `core/utils/nativeFilePicker.ts`：iOS 原生选择的预约与取消接纳；导入与 CAS 仍归 `attachmentStore`，外部文件不走 HTML/XHR，`temporaryExport` 只接 WebView 内存产物。
- `useNotificationProcessor`：原生事件注册失败以固定 ID 在每个 WebView 内只告知一次。

### 3.2 Store、Composable 与组件

- `core/stores/` 持有跨 Feature 或跨挂载周期状态；`features/<domain>/` 内的 Store 持有领域状态。
- Composable 连接 Vue 生命周期、浏览器能力或 Tauri 事件；组件消费状态并保留只属于自身挂载期的瞬时交互。
- 现有调用链允许 Store、Composable 或明确的根/边界组件直接 invoke。不得为了形式统一新增无所有权价值的转发层。
- 修改并发请求、取消、恢复或异步回调时，沿代码检查当前 identity、generation、owner 与卸载边界，不从界面最终看起来正确推断竞态已经闭合。

### 3.3 导航与覆盖层

主 Router 只承载实际路由；页面式业务入口由 `overlayStore` 的 page stack、`FeatureOverlays` 和 `SlidePage` 形成虚拟导航。`layoutStore` 持有左右抽屉意图，`useModalHistory` 连接返回顺序；不得在 Feature 内复制另一套全局页面栈或返回栈。

`GlobalOverlayManager` 负责 Prompt、Confirm、ContextMenu、Editor、Toast 与共享 Viewer，并提供 Feature Teleport 目标。新增全局覆盖面前先找到现有 owner、挂载点与关闭路径；局部弹层留在局部组件，不因层级问题直接抬成全局状态。

### 3.4 设置与领域偏好

设置中心只提供导航与编辑会话骨架，字段和偏好归各自领域。新增设置项在所属领域接通 UI 与真实消费者，不把设置中心或主题 Store 扩为业务总容器。

- **导航与编辑会话**：`features/settings/catalog.ts` 定义页面 ID、懒加载与布局；`SettingsView` / `useSettingsNavigation` 持有访问、退场画面与返回桥接。配置页经 `editing/useSettingsPageSession` 建立叶路径会话，正式 `ConfigSnapshot` 归 `core/stores/settings.ts`，按领域 revision 接纳 IPC 与 `vcp-config-changed`。提交携 base values；失败或冲突保留草稿，迟到结果与旧 ACK 不覆盖新状态，不新增自动备份。依赖保存的诊断、导出与导入先等待提交结果。`active=false` 立即释放会话与局部弹层，已提交任务仍归原 owner。`core/config/generated/` 由 Rust 生成，坏值规范化在 Rust，前端不提供修复入口。
- **配置备份**：`useConfigBackup` 协调 v2 的 domains、groupPreferences 与 theme/chat 偏好；SQLite 部分同事务提交，界面偏好交回原 localStorage owner 并分别报告结果。只保留 v1 文件结构转换，不保留旧值别名。
- **群设置**：使用 `SettingsCanvas`。`chatMode` 独立于同步 `mode` 保存、默认顺序发言，列表快照的 `mode` 才是实际模式；界面不提供跟随选项或同步实现说明。`assistant.saveGroup` 按表单 baseline 只提交变更字段。JEV 运行状态来自 `group_turn` 与重连快照，前端不调度成员。
- **语音通话**：通话、转写、消息提交与生成取消归 Rust。`voiceCallStore` / `voiceConversation` 只提交操作并按 revision 呈现快照；`microphoneMonitor` 消费 `vcp-voice-audio` 有界频带，开麦意图携生命周期 epoch；`voiceHistorySync` 对账已落盘消息，恢复不重发、不重播、不消费草稿。PCM、AEC、Silero 与播放归移动插件。后台只停止新采集，回前台不自动开麦；主动切换聊天或挂断结束通话，通用卸载只脱离视图。TTS 停止或失败不取消模型；语音按纯文本进入现有群聊发言模式，不扩展手动 `@`。
- **语音设置**：`VoiceSettingsPage`（音色模式、默认演绎指导）与 `VoiceStyleGuidancePage`（全局 LLM 注入规则、自动指导开关）复用全局 Settings，返回时保存。`VoiceTtsPreview` 使用草稿与独立任务 ID，`VoiceCloneSample` 经原生 picker 导入样本引用。
- **本机偏好**：`features/chat/preferences/` 拥有消息呈现、动效、震动与工具折叠偏好，`ChatPreferencesPage` 只是入口；主题归 ThemePicker / themeStore，自动更新归 updateStore。三者共用 `core/preferences/localPreference.ts` 的 IO，各自保留 key、编码与默认值。`features/settings/developer/` 持有开发者模式与触感试听，不进入备份。

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
