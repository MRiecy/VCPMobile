# 06. Magi 三方审查与综合裁决

## 1. 审查输入

三方共同审查以下候选方案：

- 在现有 theme Store 增加 `bubble/panel/immersive`；
- 深浅按钮短按不变，长按打开现有 ContextMenu owner；
- 同一 `MessageRenderer` 通过 ChatView data attribute 改外壳；
- 模式切换复用 `useChatScroll` 的消息锚点；
- 主题页使用局部草稿、dark/light 双预览、横向 snap rail 和确认按钮；
- 不新增 Rust/IPC/SQLite/同步；
- 内容区不使用 backdrop blur。

## 2. Melchior：逻辑与系统审查

### 2.1 认可

1. **类型完整性**：三值 readonly tuple + normalizer 能把启动、菜单和持久化收敛到同一合法集合；中文标签不进入存储。
2. **单一 owner**：外观偏好继续由 theme Store 管理，草稿只存在 ThemePicker，滚动几何只存在 useChatScroll，没有双写状态机。
3. **IPC/OOM**：候选主题全是 APK 内编译 TS 模块，预览两个轻量 pane，不需要 IPC、网络或批量解码 13 个完整渲染树。
4. **消息安全**：模式只改外壳，不接触 raw HTML、AST mutation、Tool/Thought/Diary 和附件 owner，安全面不会因模式复制而扩张。
5. **生命周期**：localStorage 是同步、设备本地偏好；不引入 Rust async 生命周期或数据库并发。

### 2.2 风险与约束

| 风险 | 必须措施 |
|---|---|
| 非法存储值污染 class | 唯一 normalizer，默认 bubble |
| 长按后的 click 穿透 | opt-in directive modifier + fake timers |
| 模式重排破坏阅读位置 | message ID + viewport offset，而非 raw scrollTop |
| ResizeObserver 与恢复竞争 | layout-changing 短期 gate，恢复后刷新高度基线 |
| `content-visibility` 估算失效 | 保持节点身份，模式后触发布局重测 |
| 主题 apply 先写后验证 | validate-first、可返回结果、失败回滚 |
| candidate 泄漏到全局 | preview 只用局部 style，不调 inject/apply |
| 壁纸路径未来扩展 | basename + scheme/扩展名约束，不接受任意 URL |
| 快速重复确认 | APPLYING gate |
| 流式中切模式 | 不重建 message、不变更 stream owner/epoch |

### 2.3 Melchior 结论

`APPROVE WITH GATES`：只有在滚动事务、click 抑制、主题原子提交和强渲染 fixture 同时落地时批准。仅完成 CSS 视觉稿不足以通过。

## 3. Balthasar：直觉与美学审查

### 3.1 认可

1. 短按/长按建立在用户已熟悉的顶栏按钮上，入口紧凑，不增加永久工具栏拥挤。
2. “气泡 / 统一 / 刊物”比技术名更适合快速选择，当前项以 Accent Bar + check 表达，符合技术精确感。
3. 主题页把“看效果”和“挑主题”放在同一视口，横滑比两列长列表更符合手机拇指操作。
4. dark/light 并列预览能在确认前暴露两种模式的可读性，避免用户切主题后再回聊天来回试错。
5. 刊物模式保留发送者和时间，只隐藏头像，不会让对话失去来源感。

### 3.2 否决的桌面照搬

- 否决滚动消息区 16px 毛玻璃：违反项目 GPU 约束，也会让壁纸上的正文对比度不稳定；
- 否决桌面 880px 固定宽度：手机必须按可用 viewport，平板才有阅读栏上限；
- 否决 hover 弹出：触控没有稳定 hover；
- 否决大卡片、大阴影和选中缩放：会降低主题密度并扰动 scroll snap；
- 否决 13 个分页圆点：信息噪声高，使用 `05 / 13 + 名称` 更准确；
- 否决主题切换前把整个真实聊天临时换色：这不是预览，而是未确认的全局副作用。

### 3.3 交互风险

长按的可发现性弱于显式按钮，但这是用户指定入口。首期通过准确无障碍名称、轻触觉反馈和设置页简短说明补足，不引入一次性教学状态机。若设备验收发现 600ms 体感过慢或误触，应基于真机记录调整，不凭桌面鼠标感受决定。

低高度窗口不能为了“绝对同屏”缩小触控目标或隐藏预览。正常 portrait 追求同屏；极端 landscape/分屏允许一个可预测的纵向滚动 owner，是更诚实的移动端降级。

### 3.4 Balthasar 结论

`APPROVE`：前提是统一和刊物坚持平面、线性、轻反馈，不把桌面的 Glassmorphism 当成必须复制的视觉资产。

## 4. Casper：务实与交付审查

### 4.1 最小施工面

Casper 认可不新增后端和第二套 renderer。功能可由约 9 个既有前端 owner 的窄修改完成；测试放入既有 `src/tests/unit`。`ThemePicker` 首期保持单组件，只有在实现后实际不可维护时再提取子组件。

### 4.2 拒绝的过度设计

- 不新增 `AppearanceStore + PresentationStore + ThemeDraftStore` 三 Store；
- 不做主题/深浅/呈现的组合枚举；
- 不新增数据库 migration 或跨设备同步；
- 不做无限循环 carousel、自动播放、手势物理引擎；
- 不为三模式写三套消息模板；
- 不引入第三方 carousel、gesture 或 animation 依赖；
- 不扩大成桌面字体、气泡宽度、自定义 Agent 主题等设置迁移；
- 不新增 Maestro/Playwright 并把工具建设冒充功能交付。

### 4.3 交付顺序

1. 先完成 enum、长按、持久化和滚动锚定；
2. 再完成三种外壳并跑强渲染回归；
3. 再把 ThemePicker 改为草稿/预览/确认；
4. 最后做响应式与具名设备验收。

这样每一步都有可观察结果，也避免主题页和消息模式同时改动时难以定位回归。

### 4.4 Casper 结论

`APPROVE`：范围明确、无需跨层，属于低到中等风险的前端呈现改造。工期风险主要来自强渲染与滚动回归，不来自基础 UI 本身，不能因此跳过专项测试。

## 5. 三方争议与裁决

### 5.1 模式是否进入 SQLite

- Melchior：SQLite 可统一持久化，但会制造前端/后端 owner 选择；
- Casper：当前主题 module key/兼容别名本就由 localStorage 权威管理，新增跨层没有收益；
- 裁决：**localStorage**。以后若所有外观偏好统一迁移，再作为独立数据迁移任务处理。

### 5.2 是否复用通用 ContextMenu

- Balthasar：三项选择需要清晰当前态；
- Casper：专用 Sheet 会增加新组件和状态；
- Melchior：通用 action 增加可选 `selected` 不破坏旧调用；
- 裁决：**窄扩展 ContextMenuSheet**，增加 optional selected/radio 语义，不新建 Presentation Overlay。

### 5.3 是否完全同屏

- Balthasar：正常手机和平板必须同屏；
- Melchior：Insets、1.5× 字体和横屏高度不允许绝对保证；
- 裁决：**正常 portrait 同屏是验收目标；极端低高度允许唯一纵向滚动降级**，但确认按钮必须可达，不能多层嵌套滚动。

### 5.4 是否迁移桌面磨砂

- 三方一致否决；
- 裁决：统一模式使用稳定主题表面与细线。任何 blur 只能遵守项目 fixed/sticky 单例例外，本专项没有必要申请该例外。

### 5.5 ThemePicker 是否拆组件

- Melchior：纯预览组件可提高隔离；
- Casper：当前只有 74 行，预先拆分会扩大文件面；
- 裁决：**先在 ThemePicker 内实现清晰分区和纯 helper**；超过约 350—400 行或出现独立测试/复用压力时，再提取一个无状态 `ThemeLivePreview`，不提前建立组件族。

### 5.6 主题源继续用 TS 还是恢复 CSS

- Melchior：当前 glob、变量注入、`extraCss` 与 HMR 均由 TS `ThemeModule` 提供；双预览可直接读取结构化变量；
- Balthasar：预览需要 dark/light 两套可预测 token，TS 映射比重新解析 CSS 更稳定；
- Casper：逐主题 CSS 已从当前树删除，恢复 CSS loader 会制造第二来源和新的漂移；
- 裁决：**TS 是唯一运行时主题源**。旧 `.css` 名称只在输入边界作为兼容别名归一化到 TS module key；不恢复 CSS 文件、CSS parser 或 Electron 复制刷新链路，且必须保留开发态 TS HMR。

## 6. 最终综合裁决

方案获准进入实施，冻结为：

```text
一个以 TS ThemeModule 为唯一主题源的 theme Store
+ 一个 ChatView data mode
+ 一个已有 ContextMenu 的选中态扩展
+ 一个 useChatScroll 布局事务
+ 同一 MessageRenderer 的外壳 CSS
+ 一个 ThemePicker 局部 draft/preview/confirm
= 完整功能
```

实施不得把“本质不难”理解为只抄三段 CSS。真正的完成边界包含：长按 click 隔离、滚动锚定、主题确认事务、强渲染保护、低高度布局和 Android 真机证据。反过来，也不得借这些边界扩建后端、同步协议或新 UI 框架。

## 7. Go / No-Go 条件

### Go

- 当前工作树已安全存档；
- `themeStore` 继续作为唯一外观 owner；
- 实施者接受强渲染不退化和内容区禁 blur；
- 可以在具名 Android 设备上完成最终触控/截图验收。

### No-Go

- 计划把 VCPChat 参考目录作为修改目标；
- 计划新增第二套消息 renderer；
- 计划在候选选择时临时注入全局主题再靠返回回滚；
- 计划用 UA/机型判断手机和平板；
- 计划以 host preview 或 happy-dom 结果替代 Android WebView 验收。

## 8. 最终一句话

> Melchior 保证状态与滚动不撒谎，Balthasar 保证手机上看得懂、摸得顺，Casper 保证只改真正属于这项功能的 owner。
