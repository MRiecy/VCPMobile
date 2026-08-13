# 02. VCPMobile 现状与差距

## 1. 当前主题 owner

`src/core/stores/theme.ts` 已经是移动端外观状态的事实 owner，当前负责：

- `ThemeMode = light | dark | system`；
- `currentTheme`、`currentThemeInfo`、13 个 `availableThemes`；
- dark/light CSS 变量分支注入；
- `<style id="vcp-custom-theme">` 的主题额外样式；
- 壁纸缩略图路径缓存；
- 深浅与主题 module key/兼容别名的 `localStorage` 持久化；
- 系统深浅变化和后端 `onThemeUpdated` 事件；
- Vite HMR 下重新应用当前主题。

因此三种消息呈现不需要新建第二套“外观管理器”。最小方案是在现有 theme Store 中增加一个清晰命名的正交字段 `presentationMode`，并把它和 `ThemeMode` 类型分开，避免“mode”歧义。

### 1.1 当前持久化事实

| 偏好 | 当前 owner | 存储 |
|---|---|---|
| 深浅模式 | `themeStore.mode` | `localStorage['vcp-theme-mode']` |
| 主题模块标识 | `themeStore.currentTheme` | `localStorage['vcp-theme-name']` |
| SQLite `currentThemeMode` | Rust Settings | 弱关联兼容字段，当前前端主题切换不以它为权威 |

消息呈现同属设备本地视觉偏好，应增加 `localStorage['vcp-chat-presentation-mode']`，不应顺手修复或扩大 SQLite 的弱关联主题字段。把它写入 Rust 会引入没有用户价值的跨层改造和双 owner 风险。

### 1.2 TS 主题源与 `.css` 兼容别名

当前主题运行链不是 CSS 主题加载器，而是：

```text
src/assets/themes/*.ts
  → eager import.meta.glob
  → ThemeInfo（fileName 取 TS 模块路径 basename）
  → injectVariables(dark/light) + extraCss
  → Vite HMR 重新应用当前 TS 模块
```

当前工作树共有 13 个逐主题 `.ts` 文件，没有对应的逐主题 `.css`。历史上 `1d43c328` 已把 CSS raw/inline 加载改为 TS 映射；旧 CSS 在 `b96ac858` 被移到 `docs/CSS_back`，又在 `74715f02` 删除。

代码里仍能看到 `.css`，但职责只是兼容：`DEFAULT_THEME`、`LEGACY_THEME_MAP` 和各模块 `meta.fileName` 保留旧命名，`findThemeModule()` 会把传入的 `.css` 后缀映射成 `.ts` 再查模块。`fetchThemes()` 实际给候选使用的是 TS basename，当前选择操作也会把这个 TS key 交给 `applyThemeFile()`。因此新主题预览必须直接消费 TS 模块的 `variables`，不能重建 CSS 解析、CSS 复制或 CSS 文件身份体系。

为了让草稿与已应用主题比较稳定，施工时应在输入边界把历史 `.css` 值归一化为已存在的 TS module key；运行时 draft、选中态与成功后的持久化统一使用 TS key。兼容别名仍可读，但不能让 `.css`/`.ts` 成为两个并列的当前主题。

## 2. 当前主题设置页

当前 `ThemePicker.vue` 的行为是：

```text
onMounted → fetchThemes()
点击主题卡 → themeStore.applyThemeFile(fileName)
  → currentTheme 立即改变
  → localStorage 立即写入
  → 根 CSS 变量立即替换
  → extraCss 立即替换
```

这与用户要求存在四个直接差距：

| 差距 | 当前行为 | 目标行为 |
|---|---|---|
| G-01 草稿边界 | 点击即全局应用 | 点击/滑动只改 `draftThemeKey` |
| G-02 实时预览 | 只有壁纸卡片 | 同时显示 dark/light 的局部 UI 预览 |
| G-03 提交动作 | 无确认按钮 | 只有“确认应用”写全局状态和持久化 |
| G-04 空间组织 | 2 列纵向网格，13 个主题形成长页 | 一个横向 snap 视口，按窗口宽度显示 1—3+ 个卡片 |

此外还有四个施工时应一起收口的细节：

1. `applyThemeFile` 当前先改 `currentTheme` 和 `localStorage`，后查找主题模块；若传入非法文件名，状态可能先被污染。正式提交应先 resolve/validate，再注入，最后 commit 状态与持久化。
2. `currentTheme` 可能来自旧 `.css` 存储值，也可能来自当前主题卡的 `.ts` key；选中态和 dirty 判断必须先归一化为 TS key，不能通过恢复 CSS 资源解决。
3. `themeThumbnails` 当前每个主题只缓存一个壁纸 URL，优先 dark；双栏预览需要分别解析 dark/light 壁纸，不能把同一缩略图假装成两种模式。
4. 横向滚动样式的 WebKit scrollbar selector 当前误写为 `.overflow-y-auto::-webkit-scrollbar`，没有匹配横向容器；重做主题视口时应使用语义 class，而不是继续依赖这个偶然选择器。

## 3. 当前设置页布局 owner

`SettingsView` 的主题子页嵌在统一的 `overflow-y-auto` 容器中，内部再放一个 `SettingsCard`。这会让主题数量直接转化为纵向页面长度，也无法稳定保留底部确认按钮。

目标结构应让主题子页成为一个独立的纵向 flex 区域：

```text
Settings SlidePage
└─ Theme subpage (height: 100%; min-height: 0; overflow: hidden)
   ├─ LivePreview (可压缩但不滚动)
   ├─ ThemeCarousel (唯一横向滚动 owner)
   └─ ApplyFooter (shrink-0，安全区之上)
```

正常竖屏手机和平板不产生纵向滚动。只有手机横屏、极端分屏或 1.5× 字体导致实际可用高度不足时，整个主题内容区退化为**一个**纵向滚动 owner；不能形成“外层竖滚 + 卡片竖滚 + 横向 rail”的嵌套手势竞争。

## 4. 当前消息渲染结构

Mobile 已具备适合三模式复用的单一路径：

```text
ChatView
└─ MessageRenderer (每条消息)
   ├─ MessageHeader
   │  └─ ChatAvatar / displayName
   └─ ChatBubble
      ├─ DiaryBlock
      ├─ Markdown/raw HTML block
      ├─ ToolBlock / ThoughtBlock / ToolSummaryBlock
      ├─ HtmlPreviewBlock / Mermaid viewer
      ├─ AttachmentPreview
      ├─ StreamingTag
      └─ timestamp footer
```

`MessageRenderer` 还支持一条消息分成多个 `messageBubbles`。三模式不能绕过这层分条语义，也不能改为三套 `v-if` 渲染器，否则会复制 AST frame、流式 tail、资源清理、作用域 CSS 和上下文菜单逻辑。

### 4.1 可复用的样式锚点

- `.chat-view-container`：整个聊天页作用域，可挂 `data-presentation-mode`；
- `.messages-inner-container`：消息列表连续表面；
- `.vcp-message-item`：单条消息边界和 `data-message-id`；
- `.vcp-bubble-container` / `.vcp-bubble-user` / `.vcp-bubble-agent`：外层气泡壳；
- `MessageHeader`、`ChatAvatar`：需补稳定语义 class，避免靠 UnoCSS 生成串定位；
- `ChatBubble` footer：需补时间语义 class，以便非气泡模式控制分条消息的重复时间。

实现应只为这些现有根节点补语义 class/data 属性。不要根据深层 Markdown DOM 或运行时 UnoCSS class 串写呈现选择器。

## 5. 滚动与重排差距

三模式会改变气泡宽度、正文换行、头像占位和消息间距，因此一次切换可能让当前阅读位置跳动数屏。

`useChatScroll` 已有正确的基础能力：加载旧历史前记录消息 ID 与顶部 offset，DOM 更新后恢复。差距在于这套能力当前仅供“顶部分页”私有使用，没有给“布局切换”暴露一个受控入口。

不推荐的做法：

- 只保存 `scrollTop`：上方消息高度改变后会落到不同内容；
- 无条件置底：会把正在读历史的用户强行拉走；
- 在 theme Store 查询 DOM：破坏状态与视图边界；
- 重建消息列表：会破坏流式实例、组件本地状态和性能。

推荐把锚定逻辑仍留在 `useChatScroll`，向 `ChatView` 暴露一次性 `preserveViewportAcrossLayoutChange(change)`：接近底部则恢复底部，否则恢复首个可见 `data-message-id` 的 viewport offset。

## 6. 长按入口差距

仓库已有全局 `v-longpress`：阈值 600 ms，支持 touch、mouse 和 contextmenu；也已有 `overlayStore.openContextMenu()` 和 `ContextMenuSheet`。这意味着入口无需自制计时器或另起 Overlay store。

但不能直接把下面两条同时挂到按钮就结束：

```vue
@click="themeStore.toggleTheme()"
v-longpress="openPresentationMenu"
```

触控长按释放后浏览器可能继续派发合成 click，结果会是“打开模式菜单，同时切换深浅模式”。现有 directive 不负责抑制 click，因此应在既有 `v-longpress` 上增加可选的 `.suppress-click` 修饰符：长按成功后只吞掉紧随其后的合成 click，并以短时失效门禁处理 contextmenu 重复触发。该行为必须是显式 opt-in，避免改变仓库内其他长按入口的既有语义。

## 7. Overlay 差距

`ContextMenuSheet` 已满足 Teleport、语义层级和返回手势，但 `OverlayActionItem` 目前没有 radio/selected 语义。三模式选择至少需要：

- 当前项可见的 2px Accent Bar 或 check；
- `role="radiogroup"` / `role="radio"`；
- `aria-checked`；
- 点击当前项也只关闭，不产生重复持久化和布局重排。

建议给既有 action 类型增加可选 `selected?: boolean`，由 `ContextMenuSheet` 在“存在 selected action”时切换为 radio 语义。这是对既有 owner 的窄扩展，优于新增一个只服务三个按钮的专用全局状态机。

## 8. 当前测试缺口

本轮未发现覆盖以下行为的现有专项测试：

- `ThemePicker` 点击、预览草稿与确认提交；
- theme Store 的非法主题/非法呈现值回退；
- 深浅按钮长按与 click 抑制；
- 三种模式的结构 class、选中菜单和持久化；
- 模式切换时的滚动锚定；
- 三模式下完整强渲染交互。

现有 `RichHtmlActiveContentGuard`、DailyNote、StreamRenderBackpressure、附件和 Chat 并发测试可作为回归基础，但不能因为它们绿色就宣称新呈现模式已覆盖。专项用例与真机矩阵见 `05-分期施工与验收门禁.md`。

## 9. 最小改造边界

首选实现只修改既有 owner：

- `theme.ts`：新增呈现偏好、TS module key 归一化、主题原子提交和双壁纸解析，同时保留 HMR；
- `ChatView.vue`：长按入口、模式作用域、调用滚动保护；
- `src/core/directives/longpress.ts`：增加显式 opt-in 的长按后 click 抑制；
- `useChatScroll.ts`：复用锚定能力；
- `ContextMenuSheet.vue` + overlay type：选中语义；
- `MessageHeader.vue`、`ChatAvatar.vue`、`ChatBubble.vue`：补稳定 class；
- `themes.css`：三模式主题感知样式；
- `ThemePicker.vue`、`SettingsView.vue`：草稿预览、横向视口、确认与响应式高度；
- `src/tests/unit/`：行为与治理测试；
- 对应 `docs/vue_docs/`：实现后更新长期文档。

不需要新建 Rust 模块、Tauri command、数据库 migration、路由、第二个消息 renderer 或第二个设置 store。
