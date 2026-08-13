# 01. VCPChat 现状与可迁移语义

## 1. 先纠正一个命名误区

桌面端把快捷入口写成“气泡 / 统一 / 刊物”，但设置页和内部代码的名称更精确：

| 快捷标签 | 设置页名称 | 内部值 | 实际行为 |
|---|---|---|---|
| 气泡 | 气泡模式 | `bubble` | 原有左右消息气泡 |
| 统一 | 统一磨砂模式 | `panel` | 消息共享连续全宽面，细分隔线分组 |
| 刊物 | 沉浸文本模式 | `immersive` | 隐藏头像，内容进入居中长文阅读栏 |

因此，VCPMobile 不应把三个词解释成三个颜色主题；它们是**消息呈现模式**。主题配色与深浅模式仍是独立轴。

## 2. 桌面端模式链路

当前 VCPChat 的数据流是：

```text
快捷选项 / 设置 radio
  → normalizeChatPresentationMode()
  → captureChatPresentationScrollAnchor()
  → body.classList = chat-presentation-{mode}
  → globalSettings.chatPresentationMode = mode
  → 同步快捷区和设置区选中态
  → 清理依赖宽度的测量缓存
  → 双 rAF 恢复消息滚动锚点
  → 可选 saveSettings()
  → 保存失败则回滚 previousMode
```

这里最值得迁移的不是 Electron API，而是四条设计原则：

1. 合法值集中归一化，未知值回退 `bubble`；
2. 一个 apply owner 同时服务快捷入口、设置入口和启动恢复；
3. 改布局前后保留用户阅读位置；
4. 只改同一 DOM 的呈现 class，不复制消息渲染逻辑。

桌面端还会通知 Pretext 宽度估算器清缓存，并让消息可见性优化器重新测量。VCPMobile 没有同一套 Pretext bridge，但现有 `content-visibility`、`contain-intrinsic-size` 和 `ResizeObserver` 同样意味着模式切换是一次真实重排，不能只写一个 class 就假设滚动位置自然稳定。

## 3. 三种模式的语义拆解

### 3.1 气泡 `bubble`

- 保留左右对齐；
- 保留用户与 Agent 气泡颜色、圆角、头像、名字和时间；
- 保留当前气泡宽度策略；
- 是非法值和首次升级的默认回退。

这是 VCPMobile 当前行为，实施时应把它当作基线而不是重写目标。

### 3.2 统一 `panel`

桌面语义是一个连续聊天面：

- 用户和 Agent 都改为左起的全宽消息行；
- 头像与名字仍存在；
- 外层气泡背景、圆角、边框和阴影被移除；
- 相邻消息以细分隔线区分；
- 工具、日记、代码、HTML Preview 等内部结构仍保留自己的卡片表面。

桌面使用滚动容器磨砂背景，但这一点**不能原样迁移**。VCPMobile 明确禁止内容区 `backdrop-filter`，移动版“统一”应以 `--vcp-panel-bg-90` 或等价稳定实色、高不透明度面板与 1px 主题分隔线表达，不牺牲 GPU 预算。

### 3.3 刊物 `immersive`

桌面语义是阅读型连续文本：

- 隐藏头像；
- 名字和时间变成低干扰的章节元信息；
- 外层气泡变透明；
- 正文使用更舒展的行高；
- 消息进入有最大宽度的居中阅读栏；
- 消息间用留白和细线形成篇章节奏。

移动版不能固定照抄桌面的 `880px`。窄手机使用可用宽度减安全边距，平板才启用约 `46rem` 的阅读栏上限。任何情况下内部代码块、表格、Mermaid 和附件仍需 `max-width: 100%` 与自己的横向/全屏交互。

## 4. 桌面快捷入口与移动端差异

桌面入口位于主题商店按钮附近，靠 hover/focus 展开，并实现 roving `tabIndex`、方向键和 Escape。Android 触控没有可靠 hover，用户明确要求改为长按深浅按钮，因此移动端应迁移**选项和单一 apply owner**，不迁移 hover 弹层。

移动端的等价交互是：

```text
短按主题按钮 → 只切 light/dark
长按主题按钮 → 打开 modal-history 管理的紧凑选择面板
点击 气泡/统一/刊物 → 立即应用并保存消息呈现偏好 → 面板关闭
Android 返回/遮罩 → 只关闭面板，不改变模式
```

三模式快捷切换不需要“确认”；用户要求的确认应用针对主题设置页。把两者混成一个提交事务会让长按快捷入口失去快捷意义。

## 5. 桌面主题选择链路

VCPChat 的独立主题窗口包含：主题卡片网格、实时预览和“应用并刷新”。其状态分为：

```text
themes[]        后端返回的主题集合
selectedTheme   主题窗口内候选
active theme    主窗口实际使用的主题文件
```

点击卡片只执行两件事：更新 `.selected` 和调用 `updatePreview(selectedTheme.variables)`。预览将 dark 与 light 分支分别绑定到两块局部 pane；只有底部按钮才调用 `api.applyTheme(selectedTheme.fileName)`。

这正是 Mobile 当前缺失的交互边界，但 Electron 的落盘实现不应迁移：桌面端复制 CSS 文件并刷新窗口，Mobile 已有预编译 TS 主题模块和运行时 CSS 变量注入，确认时继续使用这一机制即可。

## 6. 可迁移与不可迁移矩阵

| 桌面能力 | Mobile 决策 | 理由 |
|---|---|---|
| `bubble/panel/immersive` enum | 原值迁移 | 降低跨端协议和维护认知差异 |
| 同一消息 DOM + 外层 class | 迁移 | 保护强渲染与流式链路 |
| 切换前后恢复滚动锚点 | 迁移 | 手机视口更窄，重排影响更大 |
| 非法值回退 bubble | 迁移 | 升级与损坏偏好的安全基线 |
| 快捷入口直接保存 | 迁移 | 符合“快捷模式切换”语义 |
| hover/focus 弹出 | 不迁移 | Android 触控不具备可靠 hover |
| 内容区 16px backdrop blur | 禁止迁移 | 违反 Mobile UI 与 WebView 性能约束 |
| 固定 880px 阅读栏 | 响应式改造 | 手机、平板按当前窗口宽度响应 |
| Electron `saveSettings` | 不迁移 | Mobile 外观偏好已由 localStorage 拥有 |
| 主题双分支实时预览 | 迁移并移动化 | 直接满足用户目标 |
| 主题网格长列表 | 改为横向视口 | 避免设置页长距离纵向滚动 |
| 复制 CSS + 整窗刷新 | 不迁移 | Mobile 已能无刷新注入主题变量 |

## 7. 上游实现的风险提示

VCPChat 当前代码是行为参考，不是无条件的质量证明：

- 三模式最初历史提交标题包含“没做完”，后续当前 HEAD 已补充快捷区和布局优化；施工应以当前代码语义为准，不以单个历史提交为规范；
- 本轮检索未发现覆盖三模式的专门自动化测试；
- 桌面 CSS 依赖 `color-mix()` 和大面积 blur，不能据此声明 Android WebView 兼容；
- 桌面主题窗口默认选择主题数组第一项，而不是明确读取当前已应用主题；Mobile 应在打开时以 `themeStore.currentTheme` 初始化草稿，避免用户误确认第一个主题；
- 桌面 `applyTheme` 没有把失败反馈返回主题窗口。Mobile 正式提交应返回可判断结果，失败时保留原 applied 状态并允许重试。

## 8. 跨端一致性口径

跨端一致性只要求以下用户可见语义相同：

- 三个名称与模式含义一致；
- 气泡保留现状、统一形成连续面、刊物形成阅读栏；
- 主题卡候选先预览、确认后应用；
- 切换后仍停留在原阅读位置。

不要求像素级复制桌面磨砂、窗口标题栏、卡片尺寸、hover、固定宽度或刷新行为。移动端必须优先服从 Android 触控、Insets、内容区禁 blur 和当前窗口响应式契约。
