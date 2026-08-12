# 06｜Magi 三方审查与综合裁决

> 审查日期：2026-08-12。  
> 复核状态：已按产品负责人实测反馈，并以官方最新 VCPToolBox/VCPChat 线性版本完成二次裁决。
> 方法：三位审查者分别只读检查 VCPChat、VCPToolBox 与 VCPMobile，再由主审按源码证据、运行态事实、用户目标和交付风险综合裁决。
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

二次复核后冻结以下事实：

1. 桌面 VCPMemo 的实际管理模型是“文件夹 → memo 文件 → 整文件内容”；没有条目切段事实。
2. 管理路由使用 Basic admin credentials；DailyNote/LightMemo Tool 使用 Bearer API Key。
3. 最新 VCPToolBox 已提供 folder/list/get/search/save/move/delete-batch/folder-delete/associative-discovery 管理路由；DailyNote 创建与 LightMemo 语义查询走 Human Tool。
4. admin save/move/delete 通过 `runExternalFileMutation` 串行提交文件变化，再异步调度 SQLite/Rust 索引；HTTP 成功不等于语义索引已经追平。
5. 上游没有 ETag、revision、`If-Match` 或原子 rename endpoint；桌面运行态重命名能力由用户实测确认，不能再由单个前端 handler 的缺失反推产品无此功能。
6. 日记正文属于用户明确接受的可信 HTML 圈，允许 `marked.parse → innerHTML`；文件名、摘要、Tool 元信息和错误文本不因此获得 HTML 信任。
7. 单目录实际为数百至数千文件，正文平均数 KiB、范围约数百字节至数十 KiB，因此首版文件列表必须窗口化，同时保留高余量响应预算。
8. Agent 与 folder 相互独立；公共 folder 可被多个 Agent 和用户共同写入。分类名称长期稳定；LightMemo 语义排除服从服务端，本机隐藏是默认空、可逆且独立的附加发现性过滤。
9. VCPMobile 已有 SettingsState、reqwest 有界读取范式、SlidePage、overlayStore、ModalHistory、键盘 Insets 和 VueUse 列表能力，但没有远端 Diary service、Diary Store 或离线写队列。
10. 现有 `DiaryBlock.vue` 是聊天协议展示资产，不是远端整文件管理器。
11. 当前源码层级为 `editor=70, viewer=80`；实现应使用语义层名，避免扩散数字漂移。
12. 旧版排版研究稿的三级模型和“排除编辑”前提不能继续作为施工 SSOT。

## 3. 分角色裁决

### 3.1 Melchior 的系统否决线

以下任一设计出现，逻辑审查不通过：

- 凭据从 Vue 传入或被写入日志/Toast；
- 对未知大小正文直接 `.json()`/整包读入，没有累计上限；
- folder/file 用字符串拼路径，或 Basic Auth 跟随跨 origin redirect；
- list/read/search 只设 loading、不设 generation 与 target commit gate；
- 保存不记录 baseline，或 POST 超时后自动重试；
- 把 GET→POST hash 检查宣称为原子 CAS；
- 把“文件已提交”误报成“语义索引已同步”，或等待索引追平才允许写入成功；
- 将服务端自然语言错误直接送入 `v-html`；
- 在 Vue 中手工拼接 Human Tool 块而绕过 ESCAPE serializer；
- 用无界 `NoteKey → lock/cache` 映射制造长期状态。

Melchior 推荐的最小系统：一个 typed Rust service、一个有界 client、一个全局 Diary mutation gate、普通/语义搜索各自的 active owner，以及一个 Human Tool serializer；业务数据仍由一个前端 feature Store 管理。服务端本身也是单一外部 mutation FIFO，因此无需维护无界的按文件锁表。

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

- 把当前核心范围继续扩张到工作台、联想图谱、离线数据库与跨设备同步；
- 新增第二套路由、网络 client、同步实体、SQLite 日记库或 Android 插件命令；
- list/reader/editor 各建 Store 或全局 page type；
- 已知数千文件规模却仍渲染完整 DOM 列表，或为窗口化再引入新依赖；
- 复用通用 FullScreenEditor，却不补 target、dirty、busy、conflict；
- 为计划图美观而预建十余空组件、缓存层或抽象接口；
- 在现有 dirty worktree 中修改 `mod.rs`/`lib.rs` 前不做 checkpoint；
- 以“桌面已有”为唯一理由恢复低频能力。

Casper 推荐：P0 契约与 fixture → P1 浏览/普通搜索 → P2 编辑/重命名 → P3 创建/管理/语义搜索。P1—P3 共同构成当前里程碑，每阶段独立可验证；工作台和联想能力才允许后置。

## 4. 主要分歧与主审裁决

### 分歧 A｜reader/editor 是否是第二个全局 SlidePage

- Balthasar 初稿倾向 `DiaryCenterView → DiaryDocumentView` 两层 SlidePage，强调页面转场和阅读聚焦。
- Melchior/Casper 更强调一个 feature owner、最少全局状态和避免 page stack 重复。

**裁决**：全局只注册一个 `diaryCenter`。reader/editor 在该 SlidePage 内切换，并向 ModalHistory 注册内部返回状态。视觉上仍可用轻量横向 transition 表达“进入文档”，但不新增第二个 overlay page type。

理由：用户感知仍是列表进入文档；工程上只需一个生命周期根和一个 store，Android 返回顺序更容易证明。

### 分歧 B｜当前里程碑是否恢复管理与语义能力

- Balthasar 从“复原桌面任务完整性”出发，认为这些能力有清晰手机表达。
- Melchior 指出 create 走另一套鉴权与文本协议，delete/move 有 partial-success 和 tombstone 风险。
- Casper 初审认为它们会显著扩大首版写入、选择态和故障矩阵。

**二次裁决**：管理与 LightMemo 语义搜索是产品核心，全部进入当前里程碑。以 P1—P3 控制施工风险，而不是删除范围：P1 浏览/普通搜索，P2 编辑/重命名，P3 创建、移动、删除、批量、本机隐藏文件夹和语义搜索。

理由：工程阶段是可验证的交付顺序，不是把核心功能降级成未来愿望。P3 必须通过 partial-success、ESCAPE、最终一致索引和选择态测试，才算当前里程碑完成。

### 分歧 C｜搜索取消只做前端提交门还是 Rust 主动取消

- 最小实现可以仅用 generation 丢弃迟到结果。
- 但服务端搜索最长约 30 秒且并发槽有限，丢弃结果不能释放请求资源。

**裁决**：两层都做。Vue 用 generation/requestId 保证 UI 正确；Rust 用 active owner/cancellation 尽量释放连接和服务端任务。完成/清理必须核对 owner，避免 ABA 式误清理。

### 分歧 D｜首版是否直接使用虚拟列表

- 固定行高设计适合窗口化；VCPMobile 也已有 `useVirtualList`。
- 用户已确认单目录数百至数千文件；普通完整 DOM 列表不再是合理默认。

**二次裁决**：P1 直接复用 `@vueuse/core/useVirtualList`，固定 84px 行高并覆盖 overscan、稳定 key、返回滚动恢复；不新增依赖。L8 真机 profile 用于校准，而不是决定是否窗口化。

### 分歧 E｜阅读器是否需要沉浸式隐藏顶栏与翻页

- 旧排版研究稿参考电子书，建议中央点按呼出菜单和文件级翻页。
- 当前任务更接近高频记忆工具：用户需要搜索、复制、返回、编辑和确认文件身份。

**裁决**：使用常驻紧凑顶栏与连续滚动；不做中央点按、自动隐藏、仿书分页或左右滑切文件。平板只限制正文宽度。

### 分歧 F｜编辑预览是否实时

- 桌面每次输入整篇重渲染，反馈即时但成本高。
- 手机软键盘场景更需要稳定输入和可用底部动作。

**裁决**：当前版本默认手动切换预览；若实测确有需求，采用 300–500ms debounce。重型 Markdown 扩展按需加载，不在每次键击执行。

### 分歧 G｜是否认为桌面没有文件名重命名

- 静态检查发现最新 `handleSaveMemo()` 仍按当前 `{folder,file}` 保存，最新管理路由也没有专用 rename endpoint。
- 用户已经在当前桌面运行态确认文件名可重命名，运行事实不能被不完整的静态调用链否定。

**二次裁决**：当前版本必须提供重命名。施工前捕获桌面实际请求；若部署没有专用 endpoint，则采用“写入新文件 → 读回校验 → 删除旧文件”的兼容 transaction。目标已存在时拒绝覆盖；删除源失败时明确返回新旧并存，不伪装成原子操作。

### 分歧 H｜可信正文是否需要独立严格净化

- 初审从通用远端内容模型出发，建议独立严格 sanitizer。
- 产品负责人明确日记正文属于可信 HTML，桌面保留 raw HTML 的表达能力需要复现。

**二次裁决**：正文允许 `marked.parse → v-html/innerHTML`，最多复用项目已有的轻量 trusted-content 过滤，不建立 Diary 专用严格白名单。信任不外溢到文件名、摘要、语义元信息或错误；保存永远使用原始草稿字符串，不从 DOM 反序列化。

## 5. 综合方案

### 产品形态

~~~text
右侧栏：日记中心
  └─ DiaryCenterView（唯一全局 SlidePage）
      ├─ list：稳定分类 + 本机隐藏偏好 + 窗口化 memo 文件列表
      ├─ search：当前文件夹 / 全部文件夹普通搜索 + LightMemo 语义搜索
      ├─ reader：整文件可信 HTML、连续滚动、可选择
      ├─ editor：单栏草稿、dirty/saving/conflict/uncertain
      ├─ manager：创建、重命名、移动、删除、批量选择
      └─ preview：手动切换，复用正文可信渲染链
~~~

### 技术形态

~~~text
Vue components
  → one Composition Pinia store
  → typed Tauri commands
  → one Rust DiaryServiceState
  → bounded same-origin client + Human Tool serializer
  ├─ Basic → VCP admin_api/dailynotes
  └─ Bearer → DailyNote / LightMemo
~~~

### 当前里程碑边界

保留：

- 文件夹与文件浏览；
- 任意文件名 fallback；
- 整文件可信 HTML 阅读；
- 当前文件夹/全局普通搜索；
- LightMemo 语义搜索；
- 受保护的整文件编辑与文件名重命名；
- DailyNote 创建、移动、删除、批量选择、空文件夹与本机隐藏管理；
- 局部错误、刷新、空态和认证引导。

后置：

- 多文档工作台；
- 联想发现和图谱；
- 条目切分、Agent→folder 猜测映射、离线数据库和跨设备同步。

## 6. 综合通过条件

Magi 二次复核给出 `PASS FOR CONSTRUCTION`；以下是施工验收条件，而非缩减产品范围的理由：

1. P0 在开工时复核官方最新 ref，并用实际部署 fixture 校准 path prefix、鉴权、body budget 和错误；
2. Rust 保存明确称为 best-effort 冲突预检，不宣称 CAS，超时后不自动重写；
3. 文件 mutation 成功与语义索引追平分别建模，测量追平延迟并提供可重试状态；
4. 捕获桌面重命名运行态请求；没有原子 endpoint 时，通过兼容 transaction 的重名、校验失败和源删除失败测试；
5. 可信正文可直接渲染 raw HTML，所有非正文远端字符串仍使用文本绑定；
6. 所有读链路都有 generation + target 提交门，普通/语义搜索还有各自主动取消 owner；
7. 文件列表从 P1 起窗口化，UI 遵循线性、高密度、实色、无内容 blur 的移动表达；
8. P1—P3 全部通过才算当前里程碑完成；工作台和联想发现不得偷渡，也不得用它们拖延核心交付；
9. 若施工涉及新模块目录或修改 `mod.rs`/`lib.rs`，先按项目存档协议建立 checkpoint。

若部署 smoke 暴露契约漂移，状态记为 `BLOCKED-BY-DEPLOYMENT` 并报告具体端点或响应；不能把浏览纵切改名为“已完成首版”。

## 7. 最终一句话

> 三方二次裁决同意完整移植 VCPMemo 当前核心：用一个有界、可取消、可冲突提示的移动端日记中心，完成“找、读、写、改名、管理、语义检索”，同时不移植桌面窗口形态、工作台和关系图谱负担。
