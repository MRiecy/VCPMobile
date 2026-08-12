# 06｜Magi 三方审查与综合裁决

> 审查日期：2026-08-12。  
> 方法：三位审查者分别只读检查 VCPChat 与 VCPMobile，再由主审按源码证据、用户目标和交付风险综合裁决。  
> 边界：三方均未修改 VCPChat；本报告记录意见，不以多数票替代事实。

## 1. 审查任务

### Melchior｜逻辑与系统

重点：

- VCPChat 管理 API 的真实调用链、鉴权和 DTO；
- Rust/Tauri 边界、HTTP budget、OOM 与重定向；
- list/read/search/save 的 owner、竞态和冲突；
- Markdown、错误文本和凭据安全；
- VCPMobile 可复用基础设施与跨层缺口。

### Balthasar｜直觉与美学

重点：

- 桌面信息架构与手机拇指操作的转换；
- 列表、阅读、编辑、搜索和返回心理模型；
- 字号、密度、主题、动效、触控与无障碍；
- 桌面 Glassmorphism、图谱和工作台是否值得迁移；
- 现有排版研究稿与真实产品目标的偏差。

### Casper｜务实与交付

重点：

- 首版完成定义和阶段边界；
- 已有 Store、Overlay、网络、测试与依赖的复用；
- 文件落点、维护成本、测试矩阵和施工顺序；
- 哪些功能有事实支持，哪些属于过度设计；
- 工作区存档协议与可恢复交付。

## 2. 三方共同确认的事实

三方独立审查后没有分歧的事实：

1. 桌面 VCPMemo 的实际管理模型是“文件夹 → memo 文件 → 整文件内容”；没有条目切段事实。
2. 管理路由使用 Basic admin credentials；DailyNote/LightMemo tool 使用 Bearer API Key。
3. 桌面 list/read/refresh 缺少 generation；整文件保存是 last-write-wins，没有 dirty guard 或条件写。
4. 桌面 `marked.parse → innerHTML`、宽松 CSP 和错误文本插入不可复制到 Mobile。
5. 桌面卡片网格、重度 blur、大阴影、hover/scale、持续 Canvas 与人为 800ms 延迟不适合手机。
6. VCPMobile 已有 SettingsState、reqwest 有界读取范式、SlidePage、overlayStore、ModalHistory、键盘 Insets、DOMPurify 与 VueUse 列表能力。
7. VCPMobile 没有远端 Diary service、Diary Store、离线缓存、写队列或 Agent ID→folder 契约。
8. 现有 `DiaryBlock.vue` 是聊天协议展示资产，不是远端整文件管理器。
9. 当前源码层级为 `editor=70, viewer=80`；实现应使用语义层名，避免扩散数字漂移。
10. 旧版排版研究稿的三级模型和“排除编辑”前提不能继续作为施工 SSOT。

## 3. 分角色裁决

### 3.1 Melchior 的系统否决线

以下任一设计出现，逻辑审查不通过：

- 凭据从 Vue 传入或被写入日志/Toast；
- 对未知大小正文直接 `.json()`/整包读入，没有累计上限；
- folder/file 用字符串拼路径，或 Basic Auth 跟随跨 origin redirect；
- list/read/search 只设 loading、不设 generation 与 target commit gate；
- 保存不记录 baseline，或 POST 超时后自动重试；
- 把 GET→POST hash 检查宣称为原子 CAS；
- 未证明 raw-save 的索引一致性就宣布编辑可用；
- 复用桌面 `innerHTML` 或现有 trusted-circle renderer 处理远端日记；
- 将服务端自然语言错误直接送入 `v-html`；
- 用无界 `NoteKey → lock/cache` 映射制造长期状态。

Melchior 推荐的最小系统：一个 typed Rust service、一个有界 client、一个 mutation gate、一个 active search owner；业务数据仍由一个前端 feature Store 管理。

### 3.2 Balthasar 的体验否决线

以下任一设计出现，美学与移动直觉审查不通过：

- 把文件夹做成大书架卡片，再下钻到另一层大卡片；
- 把桌面 260px 侧栏、双栏编辑器或悬浮工作台等比缩小；
- 阅读页默认打开 textarea，或标题看似可改但实际不能 rename；
- 搜索范围只用循环图标表达，用户看不出当前 scope；
- 内容区使用 backdrop blur、厚阴影、大圆角、辉光或按压缩放；
- 用点按屏幕中央隐藏 chrome、分页仿书或复杂手势增加学习成本；
- 返回键越过 dirty editor、搜索态或选择态直接关闭中心；
- 阅读正文继承根节点 `select-none`；
- 把高风险删除藏成唯一的长按秘密动作；
- 为“算力感”故意延迟真实结果。

Balthasar 推荐：右侧栏入口、当前文件夹选择层、高密度线性文件列表、连续滚动阅读、显式单栏编辑；正文 16px 左右、行高约 1.65，交互目标至少 48dp。

### 3.3 Casper 的交付否决线

以下任一设计出现，务实交付审查不通过：

- 首版同时做创建、批量管理、语义搜索、工作台、图谱、离线与同步；
- 新增第二套路由、网络 client、同步实体、SQLite 日记库或 Android 插件命令；
- list/reader/editor 各建 Store 或全局 page type；
- 未测量真实列表规模就引入新虚拟滚动依赖；
- 复用通用 FullScreenEditor，却不补 target、dirty、busy、conflict；
- 为计划图美观而预建十余空组件、缓存层或抽象接口；
- 在现有 dirty worktree 中修改 `mod.rs`/`lib.rs` 前不做 checkpoint；
- 以“桌面已有”为唯一理由恢复低频能力。

Casper 推荐：P0 契约冻结 → P1 只读纵切 → P2 普通搜索与受保护编辑。每阶段有独立退出条件，低价值能力允许永久后置。

## 4. 主要分歧与主审裁决

### 分歧 A｜reader/editor 是否是第二个全局 SlidePage

- Balthasar 初稿倾向 `DiaryCenterView → DiaryDocumentView` 两层 SlidePage，强调页面转场和阅读聚焦。
- Melchior/Casper 更强调一个 feature owner、最少全局状态和避免 page stack 重复。

**裁决**：全局只注册一个 `diaryCenter`。reader/editor 在该 SlidePage 内切换，并向 ModalHistory 注册内部返回状态。视觉上仍可用轻量横向 transition 表达“进入文档”，但不新增第二个 overlay page type。

理由：用户感知仍是列表进入文档；工程上只需一个生命周期根和一个 store，Android 返回顺序更容易证明。

### 分歧 B｜首版是否恢复创建、删除与批量移动

- Balthasar 从“复原桌面任务完整性”出发，认为这些能力有清晰手机表达。
- Melchior 指出 create 走另一套鉴权与文本协议，delete/move 有 partial-success 和 tombstone 风险。
- Casper 认为它们会显著扩大首版写入、选择态和故障矩阵。

**裁决**：Release 1 不含创建、删除、移动。P2 只保留用户明确要求的整文件编辑；管理操作进入 P3。

理由：编辑已经需要解决索引一致性与冲突，首版再叠加多种 mutation 会削弱可靠性。后置不代表永久删除，04 已为 P3 固定接口与测试前提。

### 分歧 C｜搜索取消只做前端提交门还是 Rust 主动取消

- 最小实现可以仅用 generation 丢弃迟到结果。
- 但服务端搜索最长约 30 秒且并发槽有限，丢弃结果不能释放请求资源。

**裁决**：两层都做。Vue 用 generation/requestId 保证 UI 正确；Rust 用 active owner/cancellation 尽量释放连接和服务端任务。完成/清理必须核对 owner，避免 ABA 式误清理。

### 分歧 D｜首版是否直接使用虚拟列表

- 固定行高设计适合窗口化；VCPMobile 也已有 `useVirtualList`。
- 但真实文件数量尚未采集，窗口化会增加测量、滚动恢复和动态高度约束。

**裁决**：P1 先用普通线性列表，P0 记录规模，L8 真机 profile。若出现可复现瓶颈，再复用 `@vueuse/core/useVirtualList`，不新增依赖。

### 分歧 E｜阅读器是否需要沉浸式隐藏顶栏与翻页

- 旧排版研究稿参考电子书，建议中央点按呼出菜单和文件级翻页。
- 当前任务更接近高频记忆工具：用户需要搜索、复制、返回、编辑和确认文件身份。

**裁决**：使用常驻紧凑顶栏与连续滚动；不做中央点按、自动隐藏、仿书分页或左右滑切文件。平板只限制正文宽度。

### 分歧 F｜编辑预览是否实时

- 桌面每次输入整篇重渲染，反馈即时但成本高。
- 手机软键盘场景更需要稳定输入和可用底部动作。

**裁决**：Release 1 默认手动切换预览；若实测确有需求，采用 300–500ms debounce。重型 Markdown 扩展按需加载，不在每次键击执行。

## 5. 综合方案

### 产品形态

~~~text
右侧栏：日记中心
  └─ DiaryCenterView（唯一全局 SlidePage）
      ├─ list：当前文件夹 + 高密度 memo 文件列表
      ├─ search：当前文件夹 / 全部文件夹
      ├─ reader：整文件安全 Markdown、连续滚动、可选择
      ├─ editor：单栏草稿、dirty/saving/conflict/uncertain
      └─ preview：手动切换的严格净化预览
~~~

### 技术形态

~~~text
Vue components
  → one Composition Pinia store
  → typed Tauri commands
  → one Rust DiaryServiceState
  → bounded same-origin Basic Auth client
  → VCP admin_api/dailynotes
~~~

### 首版边界

保留：

- 文件夹与文件浏览；
- 任意文件名 fallback；
- 整文件安全阅读；
- 当前文件夹/全局普通搜索；
- 受保护的整文件编辑；
- 局部错误、刷新、空态和认证引导。

后置：

- DailyNote 创建；
- 删除、移动、批量选择和文件夹偏好；
- 语义搜索与联想发现；
- 工作台和图谱；
- 条目切分、Agent 深链、离线数据库和跨设备同步。

## 6. 综合通过条件

Magi 对方案给出 `CONDITIONAL PASS`，条件为：

1. P0 用实际部署 fixture 校准 API、path prefix、body budget 和错误；
2. 写入前确认条件写能力与 admin raw-save 的索引一致性；
3. Rust 保存协议不得把 best-effort 预检描述成 CAS；
4. 远端 Markdown 通过独立严格 sanitizer profile；
5. 所有读链路都有 generation + target 提交门，搜索还有主动取消；
6. UI 遵循线性、高密度、实色、无内容 blur 的移动表达；
7. Release 1 严守 P1/P2，不夹带 P3/P4；
8. 施工前按项目协议确认并提交工作区 checkpoint。

若 Q-006“raw-save 后索引一致性”不能通过，综合裁决自动降级为 `PASS-READ-ONLY`：可以发布可靠浏览与普通搜索，不得用不一致写入满足功能表。

## 7. 最终一句话

> 三方一致同意移植 VCPMemo 的领域能力，不移植它的桌面窗口形态和技术债；用一个有界、可取消、可冲突提示的移动端文件中心，先把“可靠地找到、读懂、谨慎修改 Agent 记忆”做完整。
