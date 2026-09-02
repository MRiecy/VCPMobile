# VCPMobile Widget 体系 功能规划草案

> 版本：v0.2（草案，含前置讨论 1/2 门禁决策）
> 定位：手机端"AI 自由创造"的核心载体。不对标桌面端形态，对标其自由度内核——**AI 给你做的东西，会活着，而且永远是你的。**
>
> 更新记录：
> - v0.1：初稿。
> - v0.2（2026-08-31，分支 `feature/widget-system`）：并入前置讨论结论——iframe 围栏统一门禁（§三.1、§六）、主 DOM 门禁最小禁区（§六）。网络策略定为完全放开。

---

## 一、背景与动机

- VCPChat desktop 依靠桌面端底层能力（独立 desktop 窗口 + JS 特权）实现完全自定义、自包含的 widget，但作品锁死在桌面环境。
- 手机端无法照搬（Vue 前端、无桌面特权），但调研了 Kimi 移动端的两种内联形态后发现：
  - **Widget 形态**：沙盒 iframe + 宿主注入设计 token + postMessage 桥，观感原生但限制严格、不可持久、AI 一次性交付后失联；
  - **HTML 代码块预览**：裸 srcdoc 渲染，自由但无桥、无主题同步、无持久化。
- 结论：两者底层同为 `sandboxed iframe + srcdoc`，差异全在"契约层"。VCPMobile 可以自造契约——骨架照抄（iframe + 桥），限制自定义（用户显式授权），并补上两者都缺的东西：**持久化与跨轮生命**。

## 二、核心理念

1. **创造物属于用户**：文件即真相，人类可读、可备份、可分享、可手动修复；
2. **Widget 是活的**：AI 跨轮次主动更新，widget 不是一次性烟花；
3. **能力松紧随用户**：默认沙盒仅保底线（见 §六），逐项能力由用户显式放行；
4. **四层递进、各司其职**：对话内出生 → 应用内居住 → 悬浮窗伸手 → 系统主屏站岗。

## 三、四层架构

### 第 1 层：对话内（出生地）

- 识别 ` ```widget ` 围栏，走独立渲染管线（区别于普通代码块）；
- 隐形 iframe：透明背景、无容器 chrome、宽度撑满、高度随内容自适应（widget 内 ResizeObserver + postMessage 报高）；
- 宿主注入：主题 CSS 变量（自动跟随深浅色）+ `sendPrompt(text)` 桥（widget → 对话意图回传）；
- 沙箱基线：**仅 `sandbox="allow-scripts"`，无 DOMPurify 清洗，网络完全放开**（决策细节见 §六）。

### 第 2 层：应用内 Widget 桌面页（居住地）

- 独立 tab，桌面隐喻：网格布局（2x2 / 4x2 格子，CSS Grid + span）、长按编辑、拖拽换位、删除；
- 每个格子是活 iframe，非图标；
- 承担全部 widget 的持久化与管理（详见第四、五节）；
- 作为第 3、4 层的数据源与选品池。

### 第 3 层：悬浮窗（伸向系统层的手）

- 定位：提醒 + 一瞥 + 轻点。**不承担完整对话闭环**（吸取悬浮助手教训：重输入、重业务场景留主 App）；
- 形态：贴边半透明小球（Agent 头像）→ 单击展开面板（显示当前活跃 widget）→ 长按拖动挪位 → 面板内可跳主 App；
- 技术：`TYPE_APPLICATION_OVERLAY` + 单 WebView 复用（切 widget = 换 srcdoc），与第 2 层同一份 widget 代码与桥协议；
- 已知边界：键盘输入受限（FLAG_NOT_FOCUSABLE 与 IME 冲突）、拖动/点击手势需阈值区分、Android 12+ 遮挡触摸限制、国产 ROM"后台弹出界面"权限适配、需 ForegroundService 保活（代价：常驻通知）；
- 能力声明中有新数据时小球呼吸动效/红点提示。

### 第 4 层：系统主屏（站岗位）

- Android 优先，**位图渲染法**：后台 WebView 离屏渲染 widget → Bitmap → 定期推送 AppWidget；交互为点击 deep link 回 App；
- 备选 **Schema 映射法**（AI 输出受限 JSON schema → Glance/原生渲染），用于高频更新 widget 的省电场景；
- iOS（WidgetKit）仅支持 schema 映射 + timeline 预生成，自由度受限，后续再做；
- 复用第 2 层的"冻结态截图"管线，基建零浪费。

## 四、Widget 存储形态（全文件 + 物化索引）

### 单 widget 记录包

```
widgets/w_7f3a2b/
├── manifest.json    # 元数据（id、标题、grid 尺寸、来源对话、版本号、capabilities、update_policy、创建/活跃时间）
├── code.html        # 自包含渲染代码（版本化存储：v1/v2/v3…）
├── state.json       # 运行时状态（高频写，串行写队列保证一致）
└── snapshot.png     # 冻结态缩略图（冻结时覆盖）
```

### 拆分理由

- code 与 state 写入频率差数量级，合存会在高频写下放大损坏面；
- manifest 独立成 JSON，launcher 无需解析 HTML 即可列表；
- 快照为二进制，与文本天然分离。

### code 约定

- 一切资源内联：图标内联 SVG，图片端上抓取后转 base64 嵌入（防外链失效，保证多年后打开原样）；
  - 注：网络已定为完全放开（§六），资源内联是**耐久性约定**而非安全强制；
- 固定入口约定：`window.__init(state, bridge)`，激活时由端上注入状态与桥对象后调用。

### 查询与索引

- **全文件为真相 + index.json 物化索引**（全部 manifest 汇总），启动只读索引，增删改时同步更新，损坏时全量重建；
- 元数据在内存索引中做列表/筛选/搜索，量级（数百个）下零压力；
- 查询层抽象为 `queryWidgets(filter)` 接口，未来若出现千级用户或全文检索需求，实现换 SQLite，上层无感。

### 版本管理

- AI 每次修改产出新版本 code，manifest 记当前版本号；
- 长按格子可"回到上一版"，兜底 AI 改崩的情况。

### 分享导出

- 记录包打 zip，自定义后缀（如 `.vcpw`），他人导入即解压入库；
- 自包含特性使单个 code.html 也可直接发给任何浏览器打开。

## 五、桌面页运行管理（三态模型）

```
休眠（dormant） ←→ 冻结（frozen） ←→ 活跃（live）
仅存代码+状态      显示快照+角标        真 iframe 运行
```

- **休眠 → 冻结**：IntersectionObserver 预载，仅展示快照位图；
- **冻结 → 活跃**：进入视口并停留，才从实例池取 iframe、注入 code + state；
- **活跃 → 冻结**：滑出视口 → 序列化 state → 截图 → 实例归还池中；
- **实例池**：约 6 个 iframe 实例复用（类 RecyclerView 思想），活跃数硬上限 8，超限按最久未见强制冻结。

### Agent 推送策略

- 活跃 widget：`evaluateJavascript` 直接灌数据；
- 非活跃：仅写 state 存储 + 打 dirty 标记（格子红点），激活时一次性灌入；
- **AI 的"活"体现在数据层而非渲染层**——性能问题的根本解法。

### 保命闸

- 单 widget 看门狗（死循环/内存暴涨 → 单独冻结标记，不波及全局）；
  - 注：Android WebView 中 iframe 与主页面共享渲染进程，`while(true)` 会冻结整个 App——**sandbox token 无法预防此类事故，看门狗是唯一解法**；
- 手动下拉强制刷新（实时性 widget）；
- OOM 兜底：进程重建后全部冷启动自休眠态，state 序列化须勤快。

## 六、门禁与能力契约（前置讨论结论，2026-08-31）

> 本节替代 v0.1 的"能力契约"章节，由两轮前置讨论（iframe 围栏重审、主 DOM 门禁重审）收敛而成。
> 总原则：**widget 的一切自由走 iframe 管线；主 DOM 富文本保持纯展示定位。**

### 6.1 iframe 围栏（widget 管线与旧 `html` 预览）

**基线规格（最小门禁）：**

```
widget iframe:  sandbox="allow-scripts"           ← 唯一常态 token
永久禁区:       allow-same-origin / allow-top-navigation*（无授权入口）
清洗:           无（删除 DOMPurify）
网络:           完全放开（不设 CSP，交给 VCP 长期记忆学习经验）
桥:             origin === "null" + frame 身份匹配 + nonce 校验（沿用 RenderedImageViewer 模式）
```

**决策要点：**

- `sandbox` 属性**永远在场**：无 sandbox 的 srcdoc iframe 与主应用同源，等价于直接暴露满血 Tauri IPC——它是沙箱存在的前提，不是限制；
- `allow-same-origin` 与 `allow-top-navigation*` 列入永久禁区：前者使沙箱失效（iframe 继承应用 origin，可触达 parent 与 Tauri IPC），后者可导航走整个 App 主 WebView（"主 Vue 崩坏"类事故）。两者对 agent 表达力零影响；
- 其余 token（`allow-modals` / `allow-popups` / `allow-forms` / `allow-downloads` / `allow-pointer-lock`）**与宿主安全无关**，默认不授予，作为 capabilities 逐项放行——纯体验取舍；
- **删除 DOMPurify**：现有配置显式保留 `<script>`、放开全部属性与未知协议，防护价值形同虚设，反而造成"看起来很安全"的误导。iframe sandbox + opaque origin 才是真边界。T2（widget 围栏）与 T1（旧 `html` 围栏预览）同步删除；
- **网络完全放开**：opaque origin 下无应用凭证可偷，跨域 fetch 受目标服务器 CORS 约束，残余风险可接受。资源内联（§四）降级为耐久性约定；
- 无 `allow-modals` 时 `alert()` 被系统静默吞掉，现有注入的 alert 补丁脚本（`HtmlPreviewBlock.vue`）可一并删除；
- 死循环/内存暴涨归 §五 保命闸看门狗，非权限问题；
- 桥协议升级为信封格式 `{ source, version, widgetId, nonce, type, payload }`，父侧统一校验器，widget 侧由 `__init(state, bridge)` 注入。

### 6.2 主 DOM 富文本门禁（`filterTrustedRichHtml`，独立管线）

**威胁模型**：主 DOM 中任何 JS 执行 = 满血 Tauri IPC（含 Root 插件命令），无中间态。因此最小禁区被唯一确定：

> **禁"脚本进主 DOM"，禁"主帧被导航"，其余全放。**

**维持现状的措施（条条命中最小禁区，无赘肉）：**

- 剥 `script` / `applet` / `base` / `embed` / `object`（脚本执行与 URL 劫持途径）；
- 剥 `meta http-equiv=refresh`（主帧导航）；
- URL 协议过滤：`javascript:` / `vbscript:` / 活动型 `data:` 文档（`text/html`、`image/svg+xml` 等）；
- 嵌套 iframe 加固：强制 sandbox、剥 `allow-same-origin`、srcdoc 递归过滤（主 DOM 内容间接执行脚本的唯一后门）；
- `target=_blank` 强制 `noopener noreferrer`（零成本）；
- 保留：style、自定义元素、表单、canvas、SVG、外链、纯本地交互 handler——VCPChat 式富文本表达力不受限。

**on* 事件处理器：决策采用白名单制（M1 落地后收紧）**

- 现状为黑名单正则（`HOST_CAPABILITY_IN_HANDLER`），对 JS 源码天然不可完备，属"信任圈接受风险"；
- **决策**：M1（widget 围栏）落地后收紧为白名单——剥掉所有 on* 属性，仅豁免 VCP 表情 handler（`__vcpFixEmoticon` / `__vcpShowEmoticon`）；
- 依据：M1 之后 agent 的交互需求一律走 widget iframe，主 DOM 富文本回归纯展示，风险与表达力各归各位。

**已知缺口（决策：暂缓处理）：**

- `<form action="https://...">` 提交可直接导航主 WebView（`useMessageEvents` 只拦截 `a[href^="http"]` 点击）；
- 非 http(s) scheme 裸链接（`intent:` / `file:` 等）点击走 WebView 默认导航；
- `<style>` 全局污染为 VCPChat 兼容特性，属已接受风险。

### 6.3 capabilities 声明与授权

- manifest 中声明所需能力：`modals / popups / forms / downloads / haptics / location / sensors …`，默认仅 `allow-scripts`；
- 用户长按格子逐项授权；"不设限制"落地为"用户显式放行"而非裸奔；
- token 类能力（modals/popups/forms/downloads）授权 = 追加 sandbox token；桥 API 类能力（haptics/location/sensors/系统分享/系统通知回拉）授权 = 桥协议开放对应命名空间 + iframe `allow` 属性（Permissions-Policy）；
- **网络不需要 capability**：默认全开（见 6.1）；
- 手机独有能力为差异化甜点：陀螺仪/加速度计、GPS、光线传感、`navigator.vibrate`、系统分享面板、系统通知回拉。

## 七、里程碑建议

| 阶段 | 内容 | 产出 |
|---|---|---|
| M1 | 第 1 层：widget 围栏识别 + 隐形 iframe + token 注入 + sendPrompt 桥；落地 §六 门禁基线（删 DOMPurify、T1/T2 统一 `allow-scripts` 基线） | 对话内可渲染 |
| M2 | 第 2 层：桌面页 + 全文件存储 + 索引 + 三态生命周期 | widget 有家了 |
| M3 | 活 widget：跨轮更新、版本管理、脏标记推送；主 DOM on* 白名单收紧（§6.2） | 差异化成立 |
| M4 | 第 3 层悬浮窗（Android） | 系统层入口 |
| M5 | 第 4 层系统主屏 widget（位图法，Android） | 终极形态 |
| M6 | 分享导出 `.vcpw`、能力授权细化、传感器甜点 | 生态闭环 |

## 八、开放问题

- 快照截图管线选型：WebView `capturePicture` vs AI 生成"静态预览模式"？
- 悬浮窗保活的常驻通知文案与可关闭策略；
- iOS 端整体路径（schema 映射到什么程度）；
- AI 侧 prompt 约定：`__init` 入口、capabilities 声明、grid 尺寸感知如何稳定输出；
- 桥 API 命名空间与版本化策略（`vibrate` / `getLocation` / 传感订阅的最小集合）；
- 主 DOM 已知缺口（form 导航、非常规 scheme 链接）的修复时机（决策暂缓，见 §6.2）。
