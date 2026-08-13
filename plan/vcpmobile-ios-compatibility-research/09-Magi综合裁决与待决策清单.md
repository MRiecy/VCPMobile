# 09. Magi 综合裁决与待决策清单

## 1. 联合裁决

三方结论一致：VCPMobile 具备建设 iOS/iPadOS 前台核心客户端的技术基础，但不具备 Android 功能对等迁移的条件。当前建议状态为：

> **`GO-WITH-DECISIONS`：先冻结产品边界和 Apple 资源获取计划；现有环境可启动平台中立契约，Mac 到位后再执行 macOS/Simulator 构建尖峰。**

若目标仍是 Android Foreground Service、`:helper` SSE、Root、跨 App 通知监听、OEM 自启动、Wake/Wi-Fi Lock 和 APK 自更新的等价版本，则联合裁决为 `NO-GO`。

## 2. 三贤者独立意见

### Melchior：逻辑与系统

**判断**：多数 Rust 业务模块从静态依赖与调用链看是复用候选，但 Apple compile/link 前不能标记 verified；最大系统风险是把当前非 Android fallback 当成 iOS 实现，以及为 iOS 复制生命周期/同步 owner。

关键事实：

- crate 已包含 `staticlib/cdylib/rlib`，多数网络、SQLite、Chat、Sync 逻辑不天然属于 Android；
- 自定义插件只配置 Android 路径和 Kotlin handle，仓库没有 Swift 插件、Apple 工程、Info.plist 或 entitlements；
- Rust lifecycle 已出现 iOS 条件分支，但当前没有 Swift sender，默认 foreground 不能作为平台事实；
- 权限 `true`、电池 `100`、固定北京坐标及固定 motion/ambient 等伪传感器结果、占位剪贴板、通知空操作等 fallback 会制造错误证据；
- Sync 的 generation/cancel/join 结构值得保留，Swift 不应接管它；
- `/proc`、`/sys`、Root、APK updater 和 `process::exit` 语义必须从 iOS capability 中剔除。

**裁决**：建立一层 typed platform facade，Swift 只上报事实；所有 unsupported fail closed；不新增第二套业务状态机。

### Balthasar：直觉与交互

**判断**：现有按 viewport 宽度响应的 UI 骨架、safe-area 变量和语义 z-index 可复用，但这些不能证明 WebKit、iPhone 或 iPad 可用。

关键事实：

- 当前键盘主路径来自 Android WindowInsets，生产路径未用 `visualViewport`；
- 全屏任意位置横滑开抽屉已会与选择文本和横向内容竞争；容器是否另有原生 edge-back 必须由 Wry/Tauri spike 证明；
- double-back-exit 与 `move_task_to_back` 是 Android 语义；history dummy 仍可作为跨平台 modal/router owner；
- 多个核心命中区只有 32/36/40 px，输入字号为 10–15 px，同时 viewport 和全局 `touch-action` 都限制缩放；
- iPad 宽屏仍使用全宽 bottom sheet，应由现有 overlay owner 按当前 CSS viewport/window 宽度选择 sheet/popover/dialog，而非创建 native size-class 状态机；
- 录音虽可能生成 `audio/mp4`，上传却固定 `.webm`，WebKit 链路会产生 MIME/扩展名不一致；
- Vite 只锁 Chrome 87，不是 Safari/WebKit 构建合同。

**裁决**：保留高密度线性布局、灰度和语义层级，不为“iOS 化”引入玻璃视觉；优先修键盘、手势、44pt 命中、输入缩放和 iPad presentation。

### Casper：务实与交付

**判断**：按当前命令语义，41 条公开命令中 `R=4`、`S=6`、`L=16`、`X=15`。最大成本不在 Swift 方法数量，而在产品语义、签名 target、后台恢复、测试矩阵和发布治理。

关键事实：

- 当前 Linux 无 Xcode、Simulator、Swift、CocoaPods，`cargo tree` 成功仅能证明依赖图解析；
- CI 和 Release 都是 Ubuntu/Android，当前只发布签名 arm64 APK；
- Share Extension 会新增 target、App Group、Bundle ID/profile 和数据移交协议；
- APNs 与后台恢复需要服务端协作，不能只由移动端闭环；
- 若先做前台核心 Beta，可把 incoming Share、APNs 与广泛设备遥测后置；
- 首次 Apple 平台集成、审核和设备差异使任何静态工期都应保留显著误差。

**裁决**：Phase 0 先签产品合同；Phase 1 用最小 macOS spike 消除依赖/构建未知；之后按前台核心、平台服务、恢复型后台、TestFlight 分期，不做大爆炸迁移。

## 3. 必须共同决定的 P0 项

以下项目会改变架构或发布范围，不能由施工代理自行假设。推荐默认值旨在形成可讨论基线，不代表已经替用户决定。

| ID | 决策问题 | 推荐默认值 | 若选择其他方向的影响 | 最晚冻结点 |
|---|---|---|---|---|
| D1 | 产品路线 | **Tauri 原生 iOS；不做 PWA fallback** | PWA 需要重做 IPC、存储、后台、安装和安全边界，是独立项目 | Phase 0 |
| D2 | 首发承诺 | **前台核心 + 确定性收口；有已验证 server cursor 才恢复，否则明确中断** | 若要求后台持续生成必达，必须先建设服务端 continuation/补取/APNs，仍不能保证设备常驻 | Phase 0 |
| D3 | 功能对等 | **接受 15 条当前语义为 `X`，其中 APK 下载通知不能偷换为通用 transfer** | 不接受则 iOS 项目 `NO-GO`；私有 API/越狱不进入备选 | Phase 0 |
| D4 | 最低系统 | **暂按 iOS/iPadOS 16+ 做 spike，依据真实用户分布再冻结** | 兼容 14/15 会扩大 WebKit/API/设备测试面；更高版本会缩小用户覆盖 | Phase 1 前 |
| D5 | 设备/Scene 范围 | **架构支持 iPhone/iPad 窗口尺寸；首个 Beta 不启用多窗口 Scene，但验证旋转、分屏尺寸与基础外接键盘** | 若首发多 Scene，必须先实现所有 scene 聚合与进程级 lifecycle owner，测试成本显著增加 | Phase 1 前 |
| D6 | `X` 能力 UI/API | **普通用户隐藏；诊断页只读 capability reason；稳定态不授权旧命令** | 保留禁用入口会增加噪音；若迁移期旧调用仍可达，必须临时最小授权并由 handler fail-closed | Phase 2 前 |
| D7 | 局域网传输 | **HTTPS/WSS 默认；只有明确业务硬需求时开放窄范围受信 LAN** | 明文 LAN 需要 ATS 例外、Local Network 用途说明、真机提示和审核说明 | Phase 1 前 |
| D8 | 更新渠道 | **App Store/TestFlight；应用内只提示版本，不下载/打开 IPA** | 企业/自有分发是另一套账号和合规范围，不应复用 APK updater | Phase 1 前 |
| D9 | incoming share | **不进首个 Beta，作为 Phase 4 独立项目** | 首发加入会新增 Extension、App Group、签名 profile 与幂等 staging | Phase 1 前 |
| D10 | APNs | **首个 Beta 非必需；服务端恢复稳定后再加入** | 首发加入需服务端 token、环境、用户解绑、通知深链与隐私治理 | Phase 2 前 |
| D11 | 可访问性 | **核心命中区 44pt、编辑输入至少 16px；同时处理 viewport 与全局 `touch-action` 的 blanket zoom 禁止，或证明等价系统缩放路径** | 只删 viewport 仍会被全局 touch-action 限制；保持现状无法给出完整无障碍结论 | Phase 2 前 |
| D12 | 原生实现策略 | **官方 Tauri 插件满足语义时优先；项目专属 lifecycle/insets/share 保留单一自定义插件** | 全部自研增加维护面；全部改官方插件可能丢失项目特定事件契约 | Phase 1 |
| D13 | Bundle ID | **优先保留 `com.vcp.avatar`，以 Apple Team 实际可注册性为准** | 修改会影响 deep link、App Group、Keychain access group、签名和商店身份 | Phase 1 前 |
| D14 | Apple 资源分期 | **Phase 1B 前有合规 Mac；Phase 2 真机退出前有开发签名与具名设备；Phase 5A 前有 distribution Team/App record；只在对应阶段承诺日期** | 一次性要求所有资源会无谓阻塞 1A/Simulator；反过来缺资源又不能越级声称设备或分发通过 | Phase 0 定 owner；各阶段前到位 |
| D15 | Beta MVP manifest | **核心必选、按决策并入的 Phase 3 slices、独立 Phase 4 项目、延期项四层冻结；未选中能力不阻塞该 Beta** | 若只写笼统“附件/通知/媒体”，范围、签名和测试门禁会互相冲突 | Phase 0 |

### 建议本轮优先拍板

第一次讨论只需先冻结 D1、D2、D3、D4、D5、D7、D9、D14、D15。其余可在 macOS spike 给出真实证据后细化。

## 4. P1 产品细节决策

这些不会改变“是否做 iOS”，但会明显改变 MVP 边界和测试成本：

| ID | 问题 | 推荐方向 | 需要的事实 |
|---|---|---|---|
| P1-1 | Distributed 是否进入 MVP | 仅前台、只注册真实工具，默认可后置 | 用户核心旅程是否依赖设备工具 |
| P1-2 | 系统 local notification 是否进入 MVP | 与现有 App 内 Toast/通知中心、APNs 分开；若选入则支持本 App 普通系统通知/deep link，不做高频进度通知 | 本地提醒来源、服务端事件与用户提醒需求 |
| P1-3 | 后台文件传输 | 仅在大文件业务成立时新增平台中立 transfer owner，再接 background URLSession | 文件类型、典型/最大体积、服务端上传协议 |
| P1-4 | 相机与相册 | camera、PHPicker selection、PhotoKit add-only、PhotoKit read/write 四条 capability 分开；优先私密选择与 add-only | 核心附件来源和隐私最小化要求 |
| P1-5 | 音频输出格式 | 以 WebKit 真机产物和服务端接受格式共同冻结 | MediaRecorder 支持、转码成本、Whisper API 合同 |
| P1-6 | 传感器 | 默认关闭；只为明确功能申请 Motion/Location | 功能用途、数据保留、后台必要性 |
| P1-7 | iPad presentation | 复用 overlay owner，按宽度选择 sheet/popover/dialog | 关键弹层清单与多窗口验收范围 |
| P1-8 | LAN server 发现 | 首期手工配置优先，Bonjour 另立能力 | 用户网络拓扑与服务端 mDNS 支持 |
| P1-9 | 敏感配置存储 | token/API key 迁移到 Keychain/Stronghold 类安全存储 | 现有迁移兼容、登出/备份策略 |
| P1-10 | 错误文案 | 一条推荐下一步 + 稳定 reason code；完整诊断另存 | 客服/遥测与隐私策略 |
| P1-11 | iOS 版本信号 | 冻结 App Store、受控服务或改造后的 release metadata；DTO 不沿用 `apk_size/download_url` | 现有服务端/商店可用性与隐私需求 |

## 5. 不需要再讨论的技术底线

以下并非偏好选项，而是平台或工程边界：

- 不使用私有 API、越狱、Root shell 或伪装 background mode；
- 不在 iOS 尝试启动 Android `:helper`、Foreground Service、WakeLock/Wi-Fi Lock；
- 不监听其他 App 通知，不提供 OEM 自启动/忽略电池优化入口；
- 不下载、打开或静默安装 IPA 来复刻 APK updater；
- 不调用 `process::exit` 或程序化退后台作为正常导航；
- 不以 `permissions=true`、固定电量/网络/传感器值或 no-op success 代表兼容；
- 不让 Swift 成为第二个 Chat/Sync/Distributed owner；
- 不以 Linux、Safari 桌面或单一 Simulator 结果替代真机与审核；
- 不为 iOS 视觉模仿引入内容区 backdrop blur、大圆角卡片或第二套层级系统；
- 不修改 `/home/dudu/VCPToolBox` 或 `/home/dudu/VCPChat` 参考工程。

## 6. 分阶段资源清单

下表最后一列是 blocker tag，不是 evidence state；资源到位只能解除 blocker，不能自动产生编译、设备或分发证据。

| 资源 | 最低要求 | 最晚需要 | 缺失时只阻塞 |
|---|---|---|---|
| macOS 构建环境 | 支持目标 Xcode 的受控 Mac；完整 Xcode、CocoaPods、Rust targets | Phase 1B | `MACOS-COMPILED`/Simulator 证据 |
| 开发签名 | 与目标 App ID/设备匹配的 development identity/profile | Phase 2 真机退出 | `DEVICE-VERIFIED`；不阻塞 1A/Simulator |
| Distribution Team/App record | 可创建 distribution 证书/profile 与 App Store Connect record | Phase 5A | Archive 上传/TestFlight |
| Bundle ID | Phase 1 可用候选；分发前冻结 host，Extension 另有 ID | Phase 1B/5A | 对应 target identity 或分发 |
| iPhone 真机 | 至少一个最低/当前范围内具名设备 | Phase 2 真机退出 | iPhone `DEVICE-VERIFIED` |
| iPad 验收渠道 | 自有、借测或合规测试服务；是否阻塞首 Beta 由 D5 冻结 | Phase 2 或 5B | iPad 设备范围声明 |
| 服务端 owner | 核心期确认现有协议；只有 D2 要恢复或启用 4A/4B 时再提供 continuation/APNs | Phase 0/4A/4B | 对应协议能力，不默认阻塞前台核心 |
| APNs provider 资产 | key/certificate、环境、token 运维与服务端 owner | Phase 4B | 只阻塞 APNs slice |
| 隐私/发布 owner | 用途说明 owner 先确定；商店材料在 5A/5C 完成 | Phase 0/5A/5C | 对应能力并入或分发状态 |
| Android 回归环境 | 保持现有 arm64 生产链、Gradle 插件测试与关键旅程 | 首次产品改动前 | Android 回归门禁 |

## 7. No-Go 与 Hold 判定

### `NO-GO`

- 产品要求 15 条 `X` 与 Android 等价；
- 产品要求设备后台持续 SSE 必达，但拒绝服务端恢复方案；
- 必须使用私有 API、越狱或误导审核材料才能满足核心旅程；
- iOS 只能通过复制整个业务核心和第二套状态机才能工作。

### `HOLD`（只保持在被阻塞的证据层，不冻结可独立前序工作）

- Phase 1B 尚无 Mac、Phase 2 尚无对应真机/开发签名，或 Phase 5A 尚无 distribution Team/App record；
- 服务端恢复、LAN 明文或敏感数据存储仍未决；
- macOS spike 暴露关键依赖不支持 Apple target；
- Android 回归不可控或用户工作区没有安全存档条件。

### `GO`

- P0 决策冻结；
- 当前准备进入的阶段所需资源已可用；后续阶段资源只需有 owner 与到位计划；
- 允许以 Phase 1 最小 spike 验证未知，而不是直接承诺发布日期；
- 接受按 evidence state 逐级对外声明。

## 8. 建议会议输出模板

下一轮共同讨论可直接填写下表，避免决策散落在聊天记录中：

| 决策 | 选择 | Owner | 截止点 | 依据/备注 |
|---|---|---|---|---|
| D1 产品路线 | 待定 |  | Phase 0 |  |
| D2 前后台承诺 | 待定 |  | Phase 0 |  |
| D3 15 条 `X` | 待定 |  | Phase 0 |  |
| D4 最低系统/设备 | 待定 |  | Phase 1 前 |  |
| D7 LAN 策略 | 待定 |  | Phase 1 前 |  |
| D9 incoming Share | 待定 |  | Phase 1 前 |  |
| D14 Apple 资源分期 | 待定 |  | Phase 0/各阶段前 |  |
| D15 Beta MVP manifest | 待定 |  | Phase 0 |  |

在这些项目被明确选择前，本专项的正确结束状态是“研究完成、等待决策”，而不是提前创建 Apple 工程或改动生产代码。
