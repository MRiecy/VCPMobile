# VCPMobile 主题预览与消息呈现模式专项

> 状态：`RESEARCH-COMPLETE / SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PARTIAL`
>
> 调研日期：2026-08-13
>
> VCPMobile 快照：`8957467c78af8466565f08d498671b122ef4211f`
>
> VCPChat 只读参考快照：`1a8018917c7ca7d2ee81ac7fb947b01f101ec049`

## 结论

本专项可以只在 Vue 前端完成，不需要新增 Rust command、Tauri IPC、SQLite 字段或同步协议。推荐把三个彼此独立的外观维度保持正交：

```text
深浅模式：light / dark / system
主题配色：13 个 TS ThemeModule 映射
消息呈现：bubble / panel / immersive
```

不要把它们组合成 `13 × 2 × 3` 套主题，也不要为三种消息呈现复制三份 `MessageRenderer`。消息仍由现有的单一组件树渲染，呈现模式只改变外层布局和消息外壳；Markdown、代码高亮、KaTeX、Mermaid、raw HTML、Tool、Thought、Diary、附件、流式 AST 与长按菜单都继续走原路径。

主题资产边界同样冻结：`src/assets/themes/*.ts` 是唯一运行时主题源，也是 Vite HMR 的更新入口。当前工作树没有逐主题 CSS 文件；代码中保留的 `.css` 名称只用于兼容旧存储值和旧命名，不得在本专项中恢复 CSS loader、复制 CSS 文件或把 `.css` 当作候选主题的规范身份。

面向用户的三个名称固定为：

| 用户名称 | 内部值 | 核心语义 |
|---|---|---|
| 气泡 | `bubble` | 保留当前左右气泡、头像和元信息 |
| 统一 | `panel` | 同一连续消息面，头像保留，细分隔线区分消息 |
| 刊物 | `immersive` | 隐藏头像，使用居中阅读栏和更舒展的正文节奏 |

交互入口冻结为：主题深浅按钮**短按继续切换深浅模式**，**长按 600 ms 打开三个呈现选项**。长按后的合成 click 必须被消费，不能顺带切换一次深浅模式。

主题设置页采用“局部草稿预览 → 明确确认 → 全局应用”：滑动主题视口只更新页面内的深色/浅色双栏实时预览，不改根 CSS 变量、不写 `localStorage`；只有点击“确认应用”才调用主题 Store 的正式提交动作。手机显示一个主卡并露出相邻卡，平板在同一横向视口显示多个卡；预览、选择器和确认按钮应在一个可用视口内，避免用户来回长距离上下滚动。

2026-08-13 的实施补充要求把同一套“气泡 / 统一 / 刊物”三选项作为主题页的第二个设置入口，顺序固定在主题预览、横向选择和确认区之后。它与聊天顶栏长按入口共享同一 Store 枚举和 setter，点选即生效；主题的“确认应用”只提交主题草稿，不包裹或延迟消息呈现偏好。桌面端同名区域中的百分比宽度滑杆仍在本专项边界之外。

## 文档导航

| 文档 | 内容 | 主要读者 |
|---|---|---|
| [00-研究契约与证据账本.md](./00-研究契约与证据账本.md) | 用户要求、快照、证据等级、已验证事实与边界 | 评审者、实施负责人 |
| [01-VCPChat现状与可迁移语义.md](./01-VCPChat现状与可迁移语义.md) | 桌面三模式、快捷入口、实时预览与确认应用链路 | 前端、产品 |
| [02-VCPMobile现状与差距.md](./02-VCPMobile现状与差距.md) | 当前 Store、主题页、消息 DOM、滚动与长按链路 | 前端、测试 |
| [03-手机与平板交互设计.md](./03-手机与平板交互设计.md) | 长按入口、响应式主题视口、实时预览线框与状态 | 产品、设计、前端 |
| [04-技术架构与状态契约.md](./04-技术架构与状态契约.md) | owner、类型、持久化、滚动锚定、CSS 与失败边界 | 实施负责人、审计者 |
| [05-分期施工与验收门禁.md](./05-分期施工与验收门禁.md) | 文件落点、阶段、测试矩阵、真机证据与 DoD | 实施与测试负责人 |
| [06-Magi三方审查与综合裁决.md](./06-Magi三方审查与综合裁决.md) | Melchior、Balthasar、Casper 审查与最终裁决 | 评审会 |

## 冻结决策

1. 三种消息呈现复用同一消息数据、同一 `MessageRenderer` 和同一富内容组件树。
2. 内部值对齐 VCPChat 的 `bubble / panel / immersive`；界面名称按用户要求显示“气泡 / 统一 / 刊物”。
3. 消息呈现模式是本设备的 UI 偏好，跟随现有主题偏好存入 `localStorage`，不进入 Rust/SQLite/同步。
4. 短按深浅切换保持原行为；长按打开复用全局 Modal History 的紧凑选择面板。
5. 长按必须防止后续 click 穿透；当前选项必须有可见选中态和 `aria-checked` 语义。
6. 模式切换保持当前位置：接近底部继续贴底，否则恢复首个可见消息的 viewport offset。
7. `panel` 与 `immersive` 不在滚动内容区使用 `backdrop-filter`；以主题实色/高不透明度 token 和 1px 分隔线实现。
8. 只移除消息的外层气泡壳；Tool、Thought、Diary、代码、Mermaid、HTML Preview 等结构化内容继续保留自己的表面与交互。
9. 主题候选使用页面内 `draftThemeKey`，以 `.ts` 模块 basename 比较；`.css` 旧值只在输入边界归一化，不参与运行时双身份比较。
10. 选择候选不调用 `applyThemeFile`；实时预览直接读取候选 TS 模块的 dark/light 变量。
11. 实时预览使用局部 style/CSS 变量，绝不注入到 `document.documentElement`，也不执行候选 `extraCss`。
12. “确认应用”是主题配色的唯一用户提交点；返回/关闭主题页丢弃未确认草稿，不弹阻断确认框。开发态 Vite HMR 继续只重载已应用的 TS 模块。
13. 主题选择器使用单一横向 snap 视口：手机一主卡加相邻预告，平板同视口多卡；不恢复纵向长列表。
14. 主题页在主题预览/选择/确认之后提供同一套消息呈现三选项；两处入口共享 `CHAT_PRESENTATION_OPTIONS` 与 `setPresentationMode`，不建立第二个状态 owner。
15. 消息呈现点选立即保存；它不参与主题草稿事务，也不迁移桌面端的自定义百分比内容宽度控件。

## 完成定义

本专项文档完成不等于功能完成。只有以下证据全部具备后，实施才能标记完成：

- 三种模式在普通消息、分条消息、流式消息与全部强渲染 fixture 上保持同一语义和交互；
- 长按入口、短按隔离、返回手势、持久化和非法值回退通过 Vitest；
- 主题页选择候选时根主题与持久化值不变，确认时只提交一次，返回时不提交；
- 旧 `.css` 主题别名可恢复到对应 TS 模块，成功提交后以 TS module key 持久化，且开发态 HMR 不退化；
- 360px 窄手机、平板窗口、分屏/旋转和字体 1.5× 下，预览、横向选择和确认按钮仍在可达布局内；
- `pnpm check`、`pnpm test:run`、`pnpm build` 通过；
- PHZ110 Android arm64 手机已记录 WebView、viewport、字体缩放、截图与核心触控结果；平板、分屏/折叠和最低 WebView 仍需独立设备证据，完成前不得升级为完整 `DEVICE-VERIFIED`。

## 2026-08-13 实施与验收账本

| 范围 | 当前证据 | 结论 |
|---|---|---|
| 双入口与单一 owner | ChatView 长按菜单和 ThemePicker 第二入口共用 `CHAT_PRESENTATION_OPTIONS` / `setPresentationMode` | 通过 |
| 三种外壳 | 一个 `MessageRenderer` + ChatView data attribute + 精确语义 CSS | 通过 |
| 阅读锚点 | near-bottom、负 offset、anchor 消失、重叠 generation、dispose 测试 | 通过 |
| 主题草稿/确认 | 候选隔离、dark/light 局部壁纸、显式确认、失败重试、unmount 丢弃测试 | 通过 |
| 强内容与分条 | 真实 MessageRenderer fixture 在三模式保持节点身份和 block 入口；同时修复分条 `v-memo` key 缺少 bubble ID 导致的重复文本 | 通过 |
| 自动化 | `pnpm test:run -- --reporter=verbose`：31 files / 174 tests | 通过 |
| 静态检查 | `pnpm check`：vue-tsc + `cargo check --locked` | 通过 |
| 生产构建 | `pnpm build`：4452 modules transformed | 通过 |
| 生成物/边界 | mode selectors 与三选项文案存在；无第二 renderer、UA 分支、内容区 blur、Rust/Android/capability diff | 通过 |
| Android 真机 | PHZ110 / Android 16 / API 36 / arm64-v8a；360×792 CSS viewport；WebView 150；完成竖屏、横屏、长短按隔离、三模式、持久化、锚点与 1.5× 字体旅程 | 手机核心旅程通过；扩展设备矩阵待补 |

### PHZ110 真机证据摘要

| 项目 | 记录 |
|---|---|
| 应用边界 | 仅测试 `com.vcp.avatar.debug` `1.1.4-debug`；未操作用户自用的 `com.vcp.avatar` Release |
| 设备 | OPPO PHZ110；Android 16 / API 36；`arm64-v8a`；手势导航 |
| 显示 | 物理 1080×2376；有效 density 480；竖屏 CSS viewport 360×792；字体 1.0× 与临时 1.5× |
| WebView | `com.google.android.webview` 150.0.7871.181 |
| Debug 安装物 | arm64 Debug APK SHA-256 `1dc7542e323af8bb0c9cc773ded5b355a436d1a775bf0b870df37fd66f6d8847`；Dev 前端通过 USB ADB reverse 加载，APK hash 不能单独代表实时 Vite 内容 |
| 核心交互 | 短按只切深浅；长按只开三选菜单；统一/刊物立即生效并跨重启恢复；恢复气泡后底部位置误差约 0.33px |
| 布局 | 刊物隐藏头像且保持消息锚点；横屏主题页无横向溢出，维持单一纵向滚动 owner |
| 1.5× 字体 | 首轮发现右侧短标签逐字竖排；响应式修正后标签保持横向、说明正常换行；测试后恢复 1.0× |
| 状态恢复 | 已恢复“熊熊假日 / 深色 / 气泡”、原自动旋转设置与字体 1.0× |
| 持久证据 | [`device-evidence/2026-08-13-PHZ110/README.md`](./device-evidence/2026-08-13-PHZ110/README.md) 与其中 6 张最小截图集 |

因此当前可标记 `SOFTWARE-VERIFIED / DEVICE-EVIDENCE-PARTIAL`，不能标记完整 `DEVICE-VERIFIED` 或完整 DoD。尚缺：平板窗口、分屏/折叠、最低 WebView、强渲染真机矩阵，以及主题候选“确认前不应用、确认后提交”的定向真机旅程；这些仍按 `05-分期施工与验收门禁.md` 第 8 节执行。

## 一句话施工准则

> 主题选择先在小预览里试穿，确认后才换装；消息模式只换外壳，不复制也不削弱消息引擎。
