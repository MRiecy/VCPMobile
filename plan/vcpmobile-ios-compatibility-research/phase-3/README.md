# Phase 3：前台核心、共同权限与设备能力

> 工作区状态：2026-10-09 从 `8207ec70` 开始本轮源码实施，P3-1–P3-5 的授权源码链已接入并完成第二轮独立审查；Vue/host 检查、Android Rust 目标复验与已执行定向验证见下表。用户批准来源是 [功能审批第四列](../phase-2/09-功能权限与提示审议表.md)。G1 Apple 构建仍未执行，尚无 Swift/iOS 编译或运行结果，不能将源码完成当作全部阶段验收完成。

## 输入

2026-10-17 证据更新：本文件下文的“G1 未执行”属于阶段实施时点。2026-10-16 已有 [Phase 3/debug 成功归档](https://github.com/MRiecy/VCPMobile/actions/runs/38884164944)，对应 `a8482b60`，实际 Xcode 16.4 / SDK 18.5。工具链已改为固定 Xcode 26.2，后续 Phase 4/5 和当前源码尚未取得对应 Apple 构建结果；不能将旧归档扩展为最新提交的验收。

现有目录与生命周期 DTO、Phase 2 Swift 事件订阅及单窗口恢复；平台身份来自 `src/core/utils/platform.ts`，不再输入全局能力表。G1 尚未执行，取得的具体编译诊断按依赖处理，不虚构其已闭环，也不阻止独立源码工作。

## 既定范围

启动 READY、Chat/Diary/Settings/SQLite/手动 Sync；cancel/stop/join/flush、前后台恢复及应用内返回；共同权限引导、本 App 系统通知、设备/网络数据和完整 Distributed 兼容。

产品输入以 [D1–D19 更新账本](../09-Magi综合裁决与待决策清单.md) 和最新功能审批为准，依赖顺序见 [总路线](../08-分期实施路线图与验收门禁.md)。原系统通知/完整设备工具延期已被本轮批准替代；APNs 服务及正式签名未因此自动纳入。

## 获批任务与依赖

| 任务 | 入口与实施内容 | 依赖与完成条件 |
|---|---|---|
| P3-1 核心与 LAN | 现有 Rust Chat/Sync/Settings/Diary/SQLite owner；受信 LAN、启动、错误和恢复；检查新 WebView 与仍运行请求的输出接管 | 复用 Phase 2 phase/epoch。区分新建请求与重接已有请求，不能以 `already_running` 证明新 JS Channel 已接到输出；持久化和有界收尾须有实际路径 |
| P3-2 共同权限引导 | 复用引导外壳，按已接入 API 列出真实权限；检查、请求、拒绝、返回设置后刷新 | 不展示 iOS 不存在的 OEM 自启动/通知监听/电池豁免；没有已接入项目时不伪造步骤或授权，不阻断无关功能；Phase 4/5 增加 API 时沿同一入口扩展 |
| P3-3 本 App 系统通知 | 原生发送、授权和点击返回现有页面/会话 owner；App 内通知继续独立工作 | 用途与点击 DTO、前后台呈现和待消费意图明确；拒绝通知不阻断聊天。远程 APNs 的服务端、凭据与签名依赖另行明确，不按本 App 通知批准推定已实现 |
| P3-4 设备与网络结果 | 电量/低电量模式、可用空间、网络与其余设备字段由实际原生来源提供，连接变化 Channel 保持原用途 | Sync 最小电量消费优先；字段不可取要表达缺失/原因，不能填模拟零值。空间值说明卷/容器和单位，不照搬启动 5 GB 门槛；NWPath 不替代服务器可达性或授权 |
| P3-5 Distributed 兼容 | 接通现有 client、catalog、配置、执行与遥测消费者，逐工具处理 Apple 可取得字段与权限 | 已从 Phase 2 浏览/保存配置扩展到连接、执行和采样源码链，仍需区分本地证据与 Apple 验证。Root/Android CLI 没有 iOS 实现时按工具报告真实不可用，不能因此禁止整个节点或把目录展示当作执行完成 |

系统资源由原生层持有，业务仍归原 Rust owner；用户工具启用偏好、有效连接参数与任务执行条件保留，不重建跨平台能力矩阵或防绕过门禁。

## 本轮实施与交接

本轮闭合 P3-1–P3-5 的授权源码链，并取得与改动对应的本地证据；P4 原生媒体/分享和 P5 SSE 后台保活不在本轮实施。任务分配到现有生产所有者，不建立新的全局能力表。下表保留逐路径 owner、输入输出和完成边界，实际检查及剩余证据在后文登记。

| 任务 | 生产所有者与施工分工 | 输入与结果 | 异常、取消与资源边界 | 本轮证据目标 |
|---|---|---|---|---|
| P3-1 Chat 接管 | `infra/vcp_client.rs`、Chat 消息提交路径、`core/stores/chatStreamStore.ts`；Chat 代理实施 | 既有 message/attempt 与活动请求 → 新 WebView 的完整快照、后续增量与最终持久结果；不新发同一 HTTP 请求 | 请求 lease、取消 token、流 buffer 和持久提交仍归原 owner；快照先于增量，旧接收端与迟到终态不能串入新 attempt。中止命令取消原 token 后最多五秒等真实终态，超限保持 cancelling，不 abort 写入；非流式 body 等待也可取消 | 接管时已有内容、接管与新 chunk 竞争、无 chunk 等待、用户取消、持久化失败与唯一终态；durable end 与命令 receipt 使用实际已提交正文，不以 `already_running` 返回代替输出已接管 |
| P3-1 Sync/LAN 与生命周期 | `sync/`、`lifecycle_controller/manager`、Log/Info 服务；Sync 代理实施 | 用户保存的 Sync WS/HTTP 地址及 session → 实际连接/传输、停止收尾、状态；iOS 同 Android 拒绝远程 Sync 地址误填手机 loopback | 保留 WebView 上传自用 `127.0.0.1`；按现有 session/generation 取消子任务、处理在途写入和释放；网络失败不能一概命名为权限拒绝 | mobile 端点校验、stop/join/写入收尾与前台恢复；沿现有 Chat/Sync owner 留 P5 接点，不因 P5 未实施取消所有 iOS SSE |
| P3-1 SQLite/Settings/Diary | `db_manager`/写事务、`settings_manager`、`DiaryServiceState`/`diaryStore`；根代理审查原实现 | iOS Application Support 的 `vcp_avatar.db` 与既有迁移/WAL；Settings 提交后缓存；Diary 共享 Rust HTTP 请求及真实修改结果 | 数据库取消后保留物理写租约至 commit/rollback；Diary 请求 identity/cancel、响应预算和 mutation gate，含 Save/CreateUncertain；Vue 丢弃旧搜索回调并按保存 owner 处理关闭 | 当前调用链未发现必须另建 iOS 分支的缺口；host 数据库/Diary 检查单独记录，不能宣称 iOS 已运行或验证了设备重启持久化 |
| P3-2 共同权限引导 | `PermissionGate.vue`、`appLifecycle` 与插件通知权限命令；前端、Swift、插件 Rust 代理协同 | 首次共同引导仅增加本 App 通知可选项目；通知中心读取/请求/系统设置返回后刷新。定位/运动权限由 P3-4 的工具采样触发，不列入首屏引导 | 不列 Android OEM/监听/电池豁免；未询问、拒绝、不可用分别由实际 API 表达；可选通知权限不阻断普通聊天；UI 操作主线程，迟到回调不复活已释放请求 | 读取、主动请求、拒绝继续、返回设置刷新与首次启动；引导已阅偏好不作为系统授权，不由获批范围推断成功 |
| P3-3 本 App 系统通知 | Swift `VcpMobilePlugin` 通知所有者 → 插件 Rust/ACL → App 原通知/导航消费者；Swift、插件 Rust、前端代理协同 | 通知内容及导航意图 → 真实发送结果；系统点击 → 冷启动待取/运行期事件，再由原导航 owner 消费 | 通知委托需在实际启动时间点安装；同时处理通知点击先于 Channel/READY、重复取快照与事件、设置拒绝；不建立 APNs 服务或跨 App 通知监听 | selector/DTO/队列及委托安装时点源码审计；发送失败和冷/热启动点击合流的行为回归。单独记录尚未获得的 Apple 运行证据 |
| P3-4 设备、网络与传感器 | Swift 设备/传感器 owner → 插件 Rust DTO → Sync/工具消费者；设备与插件 Rust 代理实施 | 电量/省电、卷容量、设备/热状态、CPU/内存/GPU、真实 NWPath/interface 地址、定位/运动/气压 → 带单位和采样语义的实际字段 | 首次消费后由进程持有电池监控，首个未知读数保持缺失；传感器请求有在途/期限/停止路径，迟到回调忽略；首个 CPU 样本、无硬件、未授权或不可取字段如实缺失，不伪造零值；网络路径不表示服务端可达 | Swift availability/类型、Rust 编解码、单位/范围和缺字段消费；原生资源取得及释放沿正常/错误/期限分支核对；Apple 编译仍待 G1 |
| P3-5 完整 Distributed | `distributed/`、现有 `reconcile_distributed_node`、Distributed Vue 页面；Distributed 代理实施 | 有效连接参数与用户 enabled-tool 选择 → 注册、工具执行/采样/聚合、状态与停止结果 | 复用唯一 client/registry/telemetry；Root/Android CLI 不迁移；单项缺失不能关闭整节点，不能重写用户授权偏好；本轮不扩 P5 后台保证 | 实际连接/停止调和、逐工具映射及部分缺失、授权拒绝、前台恢复；源码可执行链与原生设备观测分开记录 |

根代理负责 `lib.rs`、命令注册、Cargo、Info.plist/privacy/config 集成以及统一静态检查；独立审计代理维护本文件、核对 Apple 声明和锁定框架，并在实现落盘后做第二轮只读审查。各实现代理提供真实验证结果和遗留项，不将规划表作为测试事实。

### 工程与 transport 接入审查

施工初始状态：iOS minimum 为 15.0，Swift 包亦为 iOS 15；`Info.ios.plist` 当时仅含单 Scene 输入，框架配置只有 `Network`，隐私清单尚无磁盘容量理由。用户在本轮明确将最低 iOS/iPadOS 改为 26.0，Tauri config 与 Swift package 同步更新；现行依据为 D4，旧 15.0 保留为历史输入。用途声明随真实新增消费者集成，由根代理修改。

- Rust Chat/Sync/Log/Info/Distributed 网络主要使用 `reqwest/rustls` 与 `tokio-tungstenite`；WebKit 的本地上传 XHR 是独立路径。ATS、局域网隐私和 URL/证书实际错误不能混写为一项权限。
- LAN 用途声明与 local networking ATS 输入需按 Apple 文档核对；保留用户主动配置的服务地址，不额外创造受信主机状态表或用 NWPath 当权限探针。
- 新增定位/运动、通知和磁盘查询要逐一核对 Info.plist、framework、required-reason 的真实适用范围。自签目标不等于已经完成 App Store 隐私声明；尤其磁盘容量结果被远端工具消费时，不能把一个本地用途理由写成覆盖全部遥测用途。
- `vite.config.ts` 初始仅指定 `chrome87`；Safari 语法与 CSS 输入仍须记录，P3 transport/原生接通不自动完成 P4 WebKit 交互与渲染适配。

独立源码审计的具体依据：

1. Apple 区分 [ATS 的 URL Loading System 范围](https://developer.apple.com/documentation/security/preventing-insecure-network-connections)与 [局域网隐私](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)。[NSAllowsLocalNetworking](https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking) 的当前文档包含 IP 地址及系统版本差异；不能用旧资料中的“IP 默认不受 ATS 限制”替代现行输入。Rust TLS 网络、WebKit 本地 XHR、远程 Sync 误填 loopback 分别追踪。
2. [UNUserNotificationCenter.delegate](https://developer.apple.com/documentation/usernotifications/unusernotificationcenter/delegate) 要求在启动完成前安装。锁定 Tauri 2.12.1 `app.rs` 先创建 Runtime、再 `initialize_plugins`；tao 0.37.1 `EventLoop::new` 已注册 `TaoSceneDelegate`，`EventLoop::run` 才进入 `UIApplicationMain`。因此在插件构造中同步安装可满足该时点，异步延后安装不能得到相同结论。
3. Apple 明确将 Scene 因通知建立的响应放在 [connectionOptions.notificationResponse](https://developer.apple.com/documentation/uikit/uiscene/connectionoptions/notificationresponse)；锁定 tao 的首 Scene 不发 `SceneRequested`。本轮 `NativeNotificationSceneBridge.swift` 已在插件初始化时对现有 `TaoSceneDelegate` 的 `scene:willConnectToSession:options:` 接入一次精确转发：先将响应交给原通知 owner 暂存，再完整调用原 IMP，窗口建立仍由 tao 持有。冷/热两条来源使用同一去重与保留的最新快照，不复制 Scene owner。
4. 上述 hook 的 ABI 按 [Apple Objective-C runtime 声明](https://github.com/apple-oss-distributions/objc4/blob/main/runtime/runtime.h)核对：原 IMP 的 C 函数参数含 receiver、selector 和三个对象参数；`imp_implementationWithBlock` 的 block 仅含 receiver 与三个对象参数，没有 selector。安装必须保留原 IMP、每次转发原方法且保持进程内一次安装；实现落盘后复核实际错误与释放分支。此处为源码设计依据，不代表冷启动通知已经在 iOS 执行验证。

5. 工程输入已由根代理接入：LAN、定位、运动用途声明；配置中的 Network/UserNotifications/CoreMotion/CoreLocation 四个 framework（Metal 已由锁定 CLI 模板提供）；DiskSpace `85F4.1`。锁定 [CLI build](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/mobile/ios/build.rs)在 archive 前合并 `Info.ios.plist`，[CLI 配置](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/mobile/ios/mod.rs)将 framework 列表送入生成模板，现有 `g1.py::overlay` 复制隐私文件并加入 resources；未改 workflow。这里只确认生成链有实际消费者，最终 bundle 装配仍待 Apple 构建。

### 独立复审发现与处理

| 问题 | 真实触发与处理 | 当前证据 |
|---|---|---|
| 冷启动通知不能只等首 Scene 的 Tauri event | tao 首 Scene 不发 `SceneRequested`；UN delegate 同步早装，并补上述 Scene 参数转发到同通知 owner | 锁定启动顺序与 Apple Scene API 声明已核对；代码路径已接入，未取得 iOS 冷启动运行证据 |
| Scene hook 返回类型指针误作 Optional | Apple `method_copyReturnType` 返回 `_Nonnull`，原 `guard let` 不符合 Swift 导入类型；改普通绑定并 `defer free`；`method_copyArgumentType` 确为 Nullable，保留条件绑定 | 已按官方 runtime.h 逐声明复审并修正；不是 Apple 编译结果 |
| 通知设置 Swift 常量的最低版本混淆 | `UIApplication.openNotificationSettingsURLString` 是 iOS 16.0 引入，15.4 指旧全局常量；最初 15.4 guard 错误已修正。用户随后将 minimum 提高为 26.0，该成员改为直接使用，不再保留旧系统分支 | 已直接核 [Apple 成员声明](https://developer.apple.com/documentation/uikit/uiapplication/opennotificationsettingsurlstring)的 DocC 平台元数据；保留此次审计发现，不将旧部署目标的错误继续列为当前阻断 |
| 同步原生等待误认为 blocking 池 | 锁定 Tauri `#[command(async)]` 对同步函数只搬到普通 async worker，不自动 `spawn_blocking`；新通知查询/请求/设置三命令已显式进入 blocking worker，传感器和 Distributed 同步采样继续由其已有阻塞调用边界持有 receiver | 锁定 macro/IPC 实现与实际调用者复核；Swift `resolve()` 的 nil 经 Tauri 变为 JSON null，与 Rust `()` 一致 |
| 旧 WebView 消费冷启动待取结果 | `getPendingNotification` 不再取走最新点击；Scene/UN delegate 与前端由 identity/sequence 合流，避免旧 WebView 尚未消费就丢失唯一快照 | 通知 owner 代码已修；新 WebView 重放和重复/迟到处理由前端行为验证覆盖，不宣称建立全量可靠队列 |
| 电池首读与字段缺失 | 启用监控后读数可能尚未刷新，逐次恢复关闭会让后续读取反复未知；改首次实际消费后进程持有监控，未知仍返回 nil/原因 | DTO 区分数值 0 与不可得；不拿设备预算、热状态等级或首 CPU 样本伪造全机字段 |
| 大致定位与统一新鲜度筛选 | Apple [iOS 14 定位说明](https://developer.apple.com/videos/play/wwdc2020/10660/)指出 reducedAccuracy 更新频率约四次/小时；统一三十秒筛选会拒绝合法近似位置。现按实际 accuracyAuthorization 区分：精确定位保留新鲜度要求，近似定位返回有效区域中心、系统时间和真实年龄，标明非实时 | 已回读 Swift 及位置工具消费者；十五分钟前的有效近似位置保留年龄说明，同龄精确位置被筛出。不额外申请 fullAccuracy；这是分支源码推演，非设备定位结果 |
| 启动未取得真实 phase 就启动网络服务 | iOS 原 foreground 默认 true，若冷启动在 bootstrap 完成前退后台，可能先启动网络再被快照关闭；改 iOS 初值 false，首次真实 active 才调和 Log/Info/Distributed，inactive 不触发业务取消 | LifecycleState、controller fallback 与首 active 路径已回扫；不添加另一个权限或能力状态表 |
| 仅 mobile 编译的通知事件不满足 emit 约束 | `NativeNotificationClick` 在 mobile 分支转发 `Emitter::emit`，但原 payload/Kind 缺 `Clone`；host 未编译该分支，Android arm64 实际 target check 才报错。根代理已给 DTO 与 kind 补齐 Clone | 首次目标检查的实际失败与修复已取得，Android 复验结果下表登记；即使 Android 通过也不代表 Swift/iOS 已编译 |
| 群聊接力和跨 Channel 迟到数据 | 仅替换当前消息 Channel 会让下一棒回到旧 WebView，棒间也可能无 generation 行；现先为活动群回合更换共享输出，再扫消息。新消费者带 revision，旧 Channel 增量、rAF、编译和恢复回执不能覆盖新快照/终态 | Chat、插件与前端代理交叉复审后修正；群消息继续沿原历史回调维护去重、排序、500 条窗口与摘要，不复制另一套历史 owner |
| Sync 等待超限被当作真实结束 | 五秒只限制调用者等待，真实 child/DB drain 仍由原 JoinHandle 持有；超时保留 stopping，真实结束才清 logger/path/状态。前端共享 stop/reload promise，pending 不提前刷新，关闭后重开不丢收尾 owner | Sync 与 Distributed 交叉复审后回扫，延迟 stop/reopen、pending/retry 的前端行为回归与 Rust 有界等待用例覆盖实际竞态 |
| 连接重启带回旧队列或覆盖后台停止 | Log 改为每个 socket 独立发送队列，连接配置保留 revision；iOS 停止与 Settings 使用同一调和锁，避免快停快开与后台设置写入恢复旧连接 | 连接发送有期限、缓存有数量/编码预算，错误保留真实诊断；不将 stop 意图发布等同于取得了 iOS 后台执行时间 |

NativeDevice/Sensors 的第二轮源码审查覆盖了 Mach 计数/指针容量、CPU 回绕、NWPath 首回调与两秒期限、接口枚举释放；三轴各 40 个样本上限、单 `CMMotionManager`、定位授权后五秒与总二十秒期限，以及所有结束路径停止三轴/气压/定位并释放观察者。Apple 明确 [Scene 退后台同时发送 UIApplication 与 UIScene 通知](https://developer.apple.com/documentation/uikit/uiscenedelegate/scenedidenterbackground(_:))；重复到达由单次终态消化。定位新鲜度等具体发现以上表处理状态为准，没有 Apple SDK 编译结果。

### 执行与验证记录

| 项目 | 当前记录 |
|---|---|
| 施工基线 | `8207ec70`；本轮开始时工作树干净；随后各代理按范围并行编辑 |
| 源码复审 | 已完成 transport、Swift Channel/Scene hook、设备/传感器、生命周期/Sync/Chat 输出所有权与工程输入的第二轮检查；具体发现和修正见上表，API 声明与源码推演不替代 Apple 编译 |
| Vue/Rust 静态检查 | 根代理在最终 Chat 整体等待/对象复用及 Clone 修正后执行 `pnpm check` 通过；此前 TS union 错误已修。最终 `pnpm build` 复验通过，转换 4582 modules；这两项不编译 Swift/iOS |
| 插件 Rust 定向验证 | 插件代理执行 `cargo check --locked -p tauri-plugin-vcp-mobile` 通过，`cargo test --locked -p tauri-plugin-vcp-mobile --lib` 14 项通过；覆盖新增 DTO 的真实 0/缺失原因以及原 receiver 保活等，范围为 host |
| app Rust 定向验证 | 根代理在 host 按筛选执行通过：`distributed::` 36 项、`sync_service::tests` 28 项、`vcp_log_service::tests` 2 项、`lifecycle_` 11 项、`db_manager::tests` 12 项、`diary_service::tests` 25 项；其中 `lifecycle_` 包含消息终态提交/回滚 7 项、controller 1 项、state 2 项及 distributed lifecycle 1 项。Chat owner 执行 `active_request_tests` 19 项通过，含空闲 SSE 接管不重 POST、群聊当前/下棒接管和 headers 后 body 等待时取消。筛选可能重叠，不合计独立总数，不称为全仓 Rust 测试通过；额外 `message_service::tests` 筛选匹配 0 项，不计验证证据 |
| 定向行为验证 | Distributed 的 `DistributedViewAuthorization.test.ts` / `UseDistributedConcurrency.test.ts` 两文件 20 项；Sync 的 `SyncSessionConcurrency.test.ts` 34 项；通知/引导的 `PermissionGate.test.ts` / `AppBootstrap.test.ts` / `NotificationClickRouting.test.ts` 三文件 24 项；Chat 的 `ChatConcurrencyGuards.test.ts` / `StreamRenderBackpressure.test.ts` / `MessagePresentationRenderer.test.ts` 三文件 39 项。均由对应 owner 实际执行通过，不引用上一阶段结果 |
| Apple/iOS | G1/G2 均未执行；未取得本轮 Swift/iOS 编译、链接或原生运行结果 |
| Android | 首次实际目标检查发现上述 mobile-only Clone 编译错误；修复并完成最终 Chat 改动后，`cargo check --locked --offline --target aarch64-linux-android` 复验通过，NDK `29.0.14206865` / API 26。只提供该 Rust 目标及 mobile 共享分支编译证据，未执行 Gradle/Kotlin/APK 或设备验证 |

本轮文档和代码差异检查通过。没有修改 workflow、Cargo/package 依赖或锁文件，未推送或运行 G1/G2。Phase 2 审议表仅更新顶部历史快照说明，全部表格行和用户审批原文保持不变。

## 完成条件

成功/取消/错误 terminal、旧 generation、取消后写入和持久化有明确 owner 与证据；Chat/手动 Sync 不在恢复前台时擅自重发。P3-2 至 P3-5 的真实授权、部分数据缺失、通知点击、节点停止和无原生 API 工具的结果均进入现有消费者；源码完成、host 检查和 Apple 构建分别记录。

只登记实际执行的结果和仍待处理的问题；Apple 构建集中在 G1/G2 手动 Actions。进入下一阶段依据具体依赖和问题归属，不因独立待办阻断其他工作。
