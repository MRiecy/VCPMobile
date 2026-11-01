# 09. Magi 综合审查与产品决定

> 修订日期：2026-10-09。D1–D19 未被覆盖的边界继续有效；本次按 [Phase 2 功能权限与提示审议表](./phase-2/09-功能权限与提示审议表.md) 用户第四列的最新审批修订。该表保留用户原话，本文件记录其实施含义，旧延期和旧建议不再覆盖新决定。

## 当前联合裁决

以 Tauri 原生 iPhone/iPad 客户端为目标，复用 Vue/Rust 核心，实施原生附件、共同权限引导、本 App 系统通知、设备/网络与完整 Distributed、相机/相册保存和双向分享。SSE 后台持续接收为高优先专项，其他任务按尽力后台和明确收尾设计；最终生成用户自行签名的实验性产物。开发按依赖源码、编译规则和状态推演持续推进，Apple 构建仅在阶段验收时通过 GitHub Actions 手动执行。

当前已接通原生分支、目录、生命周期/网络 Channel、同步平台身份及说明入口；Phase 3 又接入共同通知权限、本地通知/冷启动点击、设备/传感器、剪贴板与 Distributed 的源码链。Chat/Sync 的本轮具体交付及检查见阶段三记录；Apple 编译和原生运行仍未取得。P4 媒体/分享与 P5 后台资源不能据此认定完成，任务和完成条件仍见 [08](./08-分期实施路线图与验收门禁.md)。

## 三贤者独立意见

### Melchior：逻辑与系统

本次三方审查分别读取当前生产代码、锁定依赖和旧稿决定；以下结论已合入 01–08。

支持继续的资产：

- 中央 IPC offload；
- Rust 单一领域 owner；
- Chat UUID/CancellationToken/ABA lease；
- Sync generation/cancel/join/child drain；
- SQLite、附件 CAS、前台 Rust 网络。

明确的实施工作：

- 拆 iOS/host cfg，按实际 adapter 返回状态；
- 同时接通 Swift producer、插件订阅与 bootstrap 的 native Channel；
- 使用 Tauri 随包 Swift API，核对 ObjC selector、IPC 队列与 UIKit 主队列；
- 让桥接任务接收原生最终响应，业务取消按 generation 归约；
- 为 Chat 加 cancellation cause、settle receipt 与 task 绑定；
- iOS 目录映射和大文件/像素缓冲预算；
- 按真实 feature 图审计 SQLite、zstd、ring，结论见 01。

系统结论：Swift 持有 Apple 平台资源，Rust 继续拥有 Chat/Sync/DB 和终态。FFI、线程、回调与原生库的输入已能由源码具体分析，下一步是逐路径实现。

### Balthasar：直觉与交互

- 保留高密度线性布局、灰度、语义层级和现有 overlay owner；
- PermissionGate 的检查步骤、异常文案、组件挂载一起按平台选择；
- iOS 返回复用 modal/page/session 链，根动作保持应用；
- Insets 明确来源和单位，扣除 WebKit 已处理的视口缩小量；
- iPad regular width 用同一状态 owner 的 dialog/popover；
- 44×44 CSS px 命中区通过 hit slop 达成，编辑控件至少 16 CSS px；
- 文本选择优先于自定义 ancestor longpress；
- Updater 同时收口初始化、自动/手动检查和安装；
- 附件三条导入链汇入 CAS 登记，原生 Files/PHPicker/ImageIO 保持 P0；
- 大上传补 abort、generation 和清理，图片先读元数据再按预算解码。

交互结论：适配以明确坐标、状态和资源归属完成；iPhone/iPad 的窗口与交互输入逐条纳入源码路径审计。相机、相册保存、系统通知和双向分享已获批接入；语音仍未开发但保留点击说明。普通差异由共享 Vue 的运行时分支表达。

### Casper：务实与交付

交付路径：

1. 目标依赖审计与平台契约；
2. Swift/Channel/生命周期；
3. 核心业务、共同权限/通知/设备工具与原生文件媒体/分享；
4. SSE 持续接收专项、其他尽力后台和前台回退；
5. 构建脚本、device bundle 与用户签名输入；
6. 阶段性 GitHub Actions 验收与实验产物打包。

本次恢复的关键决定：

- D2/D15：continued task 属于首产物实现目标；
- D8：设备架构 `.app.zip` 与结构检查后的 `.unsigned.ipa` 供用户自行签名；
- D13：用户可以改 Bundle ID，plist/task identifier/entitlement 配套处理；
- D14/D16：日常源码审计与静态内存预算贯穿开发，Apple 构建集中在 GitHub Actions 阶段验收执行；
- D18：HTML 子路径可先联调，原生文件/图片仍属于 P0。

交付结论：构建中间件与用户设备架构产物各有明确结构。06 已从签名工具源码追到 unsigned bundle 的实际处理路径；工程应据此准备输入与说明，07/08 规定源码闭环及 G1/G2 验收节奏。

## 当前决策账本

| ID | 既定产品选择 | 当前实施解释 |
|---|---|---|
| D1 | Tauri 原生 iPhone/iPad | 同一 Vue/Rust + Swift adapter |
| D2 | 2026-11-01 用户决定移除聊天静音保活。iOS 26 持续后台任务仅用于尽力展示回复者和进度 | Swift 异步管理展示。Rust 独立读取 SSE 并直接落库，正常 EOF 即完成，读取失败时保存已收到的正文并结束，不增加回复缓存与自动重试。同步和预渲染沿用前台常亮租约 |
| D3 | 接受按平台语义分类迁移 | 以真实命令及消费者安排实现；02 的迁移分类用于理解接口差异，不定义 UI 可见性或新审批范围 |
| D4 | 最低 iOS/iPadOS 26.0；高于该版本的新 API 分层 | 2026-10-09 用户明确选定 26.0，替代 Phase 0–2 的 15.0 工程基线；Tauri/Swift 目标同步。系统权限、任务提交与资源激活仍按真实结果处理，Apple 编译证据待 G1 |
| D5 | iPhone+iPad、单 Scene、当前窗口宽度响应 | 复用组件树，按框架要求接 Scene 生命周期 |
| D6 | 不可用入口按用户逐项选择呈现，操作返回真实结果 | 更新/Root/语音保留说明；Vue 使用局部运行时分支，原生操作按实际需要接入，不恢复统一静态能力门禁 |
| D7 | 安全默认 + 用户显式受信 LAN | LAN 属于首期，细化 Rust/WebKit transport 和拒绝恢复 |
| D8 | Android Release 附属实验性 unsigned 产物，用户自签 | 06 的设备 `.app.zip` / 条件性 `.unsigned.ipa`；应用内更新归 Android |
| D9 | incoming share 已批准接入，覆盖原延期 | P4-4 复用 ticket/session 与主 App 导入 owner；扩展及自签输入交给 P6 |
| D10 | 2026-11-01 用户要求开启本应用通知才能进入 iOS 主界面，后台回复成功入库后发送本地通知 | 点击完成通知定位到对应消息，普通通知打开应用。沿用本地通知与现有冷启动点击入口 |
| D11 | 高密度线性 UI、语义层级、iPhone/iPad 基础可访问性 | 命中区、输入字号、焦点和返回，按实际 DOM/样式/事件路径审计 |
| D12 | Swift adapter，单 Rust 业务 owner | 生命周期、附件、任务通过现有 IPC/Channel 连接 |
| D13 | CI 候选 ID `com.vcp.avatar`，用户可改写 | 最终 ID、plist task 前缀和 entitlement 配套 |
| D14 | 日常源码审计，Apple 构建在 GitHub Actions 上按需手动执行。项目 iOS 实机测试、正式签名及付费 Team 在范围外 | 2026-10-16 用户允许每阶段按需诊断。现行归档入口见 [iOS 构建说明](../../docs/IOS_BUILD.md)。 |
| D15 | 首产物保留核心业务与原生附件，并纳入本轮获批系统功能及 SSE 专项 | 前台核心是中间里程碑；P3–P5 获批项逐项记录实现与未关闭问题，完整范围按 08 推进 |
| D16 | 采用静态正确性与硬上限，设备性能专项为范围外 | 审查像素、缓冲、副本和并发预算 |
| D17 | 语音仍未开发，入口保留并说明 | iOS 点击不请求麦克风，可继续文字输入；后续接入再修 MIME/扩展名契约 |
| D18 | 原生获取/staging/大图，Rust 解析与 CAS，附件 P0 | Files/PHPicker/security scope/ImageIO 与三条导入链 |
| D19 | 真实 token/字符/字节/chunk/terminal，未知总量表达为未知 | 领域计量与系统 Progress 分层，具体适配问题见 04 |

相机、PhotoKit 保存、outgoing share、设备/完整网络信息与 Distributed 的原延期已被最新审批覆盖：媒体/分享归 Phase 4，设备工具/Distributed 归 Phase 3。2026-10-17 用户又批准双端音视频附件处理：Android Media3 音频与现有视频抽帧优化、iOS 原生音频转换与视频抽帧，图片保持现状，详见 [实施记录](./phase-5/05-双端音视频附件处理.md)。语音输入和 Android 本地 CLI 的原范围未被改变。关于页集中功能说明 U1 仍是未批准建议，不自动加入产品。

### 2026-10-09 开发方式与新增任务

- 用户明确选择最低 iOS/iPadOS 26.0；该决定减少旧系统分支，不使 P5 的 SSE 候选方案、任务激活或系统中断条件自动成立。原 15.0 输入及检查结果保留为历史证据。
- 共享 Vue 默认使用运行时平台分支；仅有真实的平台依赖或模块副作用时局部处理构建差异，不全面建设两套前端构建或平台映射。
- 移除原 `platform.rs` 四字段表、Swift 静态能力查询及其启动链；系统权限和动态结果由真实操作取得，不为假想绕过增设另一层功能门禁。
- 共同权限引导在 P3-2 接入，平台没有相应 API/设置的项目不显示；电量/空间在 P3-4 复现真实查询。磁盘 API 获批不等于把 Android 的 5 GB 启动阻断搬到 iOS。
- 2026-10-17 曾采用静音音频实验，2026-11-01 改为上表 D2 的系统进度展示。源码、Apple 归档与实际设备运行分别记录。
- 新功能继续复用 Rust 领域 owner、原生资源 owner 与现有提示消费者；CSP/Keychain 等独立安全改造不属于此次批准范围。

### 2026-10-07 用户追加决定：开发与验收节奏

D14 约定项目在 Windows 审阅生产源码、锁定依赖和接口，在 GitHub Actions 上手动取得 Apple 编译、链接及归档证据。07/08 记载了当时对分期验收的规划。

### 对前次 N 表的处理

N3/N5/N6 将 D2/D15 改成前台限定和真机研发门禁；N7/N8 改写了 D8/D14；这些结论在本次修订中撤回。N4 的 15.0、N16 的具体 Xcode 版本恢复为工程选型。N9 的命令变化及 N10–N12 的能力、cfg、Channel 发现保留。历史 N 表退出施工依据，后续引用统一使用本表 D 编号。

## 具体工程待办

| 问题 | 获取信息/实施动作 | 影响范围 |
|---|---|---|
| 现有 G1 工程输入 | Phase 2 已有生成脚本、Swift product/binding 与 minimum 配置；按锁定输入等待手动 G1 的实际编译诊断 | Phase 2 / G1 |
| minimum/Xcode/SDK | 最低 iOS/iPadOS 26.0 已由用户决定；新增 adapter 对照锁定 SDK，核对高版本 API，保存实际构建输入与诊断 | 构建及 availability |
| 共同权限与通知 | P3 源码已接查询/请求/设置返回、本地发送及双来源点击；复核真实 Apple 编译/运行，APNs 未扩大 | P3-2 / P3-3，证据见阶段三 |
| 设备/网络与 Distributed | P3 源码已接真实 API 采样与逐工具执行，缺失字段报告、保留整体节点；不能以 host 验证代替原生观测 | P3-4 / P3-5，Apple 证据待 G1/G2 |
| 文件媒体与双向分享 | 闭合 picker/相机/相册/导出/预览与分享交接，确定扩展和自签输入 | P4-1–P4-4 |
| WebKit 缓存 | 明确清理类别和结果，接共享维护页及实际 WebView owner | P4-5 |
| SSE 后台持续接收 | 核对用户提出的静音音频、continued task 等候选的具体条件；形成选型、实现与未覆盖情况 | P5-1 / P5-2 |
| 未知总量进度 | 读取 Progress/BGContinuedProcessingTask 声明，完成映射与 system end 分支 | continued-task progress adapter |
| 常亮与其他后台任务 | idle timer 持有/释放及 Sync/Log/Distributed/维护的 stop/join/恢复 | P5-4 |
| 自签 Bundle ID | 主 App/扩展 ID、共享容器、各 target entitlement/profile 与 task identifier 配套改写 | P4-4 输入 / Phase 6 用户产物 |

每个问题只影响表中对应路径；其他独立任务照 08 推进。

## 决定、事实与建议的记录方式

| 类型 | 例子 | 更新依据 |
|---|---|---|
| 用户决定 | D8 自签产物、D18 附件 P0 | 用户对目标/范围的选择 |
| 源码事实 | 当前命令 45 条、Channel 的 cfg、SQLite feature 图 | 当前生产代码与锁定依赖 |
| 工程建议 | runner 组合、模板管理方式 | 具体条件分析与实现取舍；minimum 26.0 已是用户决定 |
| 执行结果 | 本地检查、GitHub Actions 构建诊断与产物结果 | 对应输入与实际记录 |

技术研究可以指出某项决定遇到的具体 API/实现问题，并提出可选方案；工作条件记录在 00，任务按源码与接口依赖组织，阶段验收按 07/08 集中执行。

## 变更规则

改变产品范围时记录：

- 触发证据和日期；
- 影响的代码 owner/章节；
- 用户选择及对应技术依据；
- Android 回归范围；
- 受影响的首产物交付物。

源码修正和执行诊断归入对应章节，产品决定留在本表。研究段落以具体发现、推导、改动位置和下一项工作结束。
