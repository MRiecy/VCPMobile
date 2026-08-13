# 09. Magi 综合裁决与已冻结决策清单

## 1. 联合裁决

三方结论一致：VCPMobile 具备建设 iPhone/iPad 实验性兼容客户端的技术基础，但不具备 Android 功能对等迁移条件，也不以正式 Apple 分发为目标。2026-08-13 用户已冻结产品边界，当前状态为：

> **`GO-EXPERIMENTAL / DECISIONS-FROZEN`：施工可进入平台中立契约与 GitHub macOS/Simulator 尖峰；目标产物是附加在 Android GitHub Release 上、需用户自行重签的未签名 iPhone/iPad artifact。**

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

**裁决**：建立一层 typed platform facade，Swift 只上报事实；所有 unsupported fail closed；不新增第二套业务状态机。Rust 并不自动证明 iOS 内存/性能无风险；按用户决策不建立性能门禁，状态固定写作 `NOT-EVALUATED`。

### Balthasar：直觉与交互

**判断**：现有按 viewport 宽度响应的 UI 骨架、safe-area 变量和语义 z-index 可复用，但这些不能证明 WebKit、iPhone 或 iPad 可用。

关键事实：

- 当前键盘主路径来自 Android WindowInsets，生产路径未用 `visualViewport`；
- 全屏任意位置横滑开抽屉已会与选择文本和横向内容竞争；容器是否另有原生 edge-back 必须由 Wry/Tauri spike 证明；
- double-back-exit 与 `move_task_to_back` 是 Android 语义；history dummy 仍可作为跨平台 modal/router owner；
- 多个核心命中区只有 32/36/40 px，输入字号为 10–15 px，同时 viewport 和全局 `touch-action` 都限制缩放；
- iPad 宽屏仍使用全宽 bottom sheet，应由现有 overlay owner 按当前 CSS viewport/window 宽度选择 sheet/popover/dialog，而非创建 native size-class 状态机；
- 录音虽可能生成 `audio/mp4`，上传却固定 `.webm`；该功能当前 Android 也不可用，因此 iOS 直接隐藏且不请求麦克风权限；
- Vite 只锁 Chrome 87，不是 Safari/WebKit 构建合同。

**裁决**：保留高密度线性布局、灰度和语义层级，不为“iOS 化”引入玻璃视觉；优先修键盘、手势、44pt 命中、输入缩放和 iPad presentation。

### Casper：务实与交付

**判断**：按冻结后的 iOS 产品语义，41 条公开命令中 `R=4`、`S=6`、`L=19`、`X=12`。最大成本不在 Swift 方法数量，而在 capability 真值、Apple 构建、附件 staging 与 continued-task 生命周期。

关键事实：

- 当前 Linux 无 Xcode、Simulator、Swift、CocoaPods，`cargo tree` 成功仅能证明依赖图解析；
- CI 和 Release 都是 Ubuntu/Android，当前只发布签名 arm64 APK；
- iOS/iPadOS 26 的 `BGContinuedProcessingTask` 可以尽力延续用户生成，但系统可拒绝/终止，服务端不改意味着断开不可补取；
- 文档附件解析主要已有 Rust owner；iOS 重点是 Files/PHPicker/security-scoped staging 和大图 ImageIO，而不是寻找另一个“通用编码器”；
- Share Extension、APNs、系统通知、语音、更新、设备遥测和正式签名全部退出首期；
- 首次 Apple 平台集成与 GitHub runner/Xcode 差异使任何静态工期都应保留显著误差。

**裁决**：Phase 0 决策已完成；之后按平台契约/CI、前台核心、P0 附件、iOS 26 尽力后台、未签名 GitHub asset 分期，不做大爆炸迁移，也不让 iOS job 阻断 Android Release。

## 3. 已冻结的 P0 决策

| ID | 决策 | 冻结值 | 工程后果 |
|---|---|---|---|
| D1 | 产品路线 | Tauri 原生 iPhone/iPad；无 PWA、macOS、Catalyst、桌面 | 同一 Vue/Rust owner + Swift adapter |
| D2 | 后台生成 | iOS/iPadOS 26+ continued task 尽力续跑；其他情况主界面前台常亮 | 不承诺必达；服务端不改；断开即明确 interrupted |
| D3 | 功能对等 | 接受 `R4/S6/L19/X12` | `X` 稳定态不授权/不展示；旧调用 fail-closed |
| D4 | 系统分层 | 最低可编译版本由 Phase 1 固定；continued API 从 26 availability guard 启用 | 不为新 API 强制把整个 minimum 直接抬到 26 |
| D5 | 设备范围 | iPhone + iPad，按当前 viewport 响应；首期单 Scene | Simulator 覆盖手机/平板；无桌面产品分支 |
| D6 | 不可用 UI/API | 隐藏或给明确原因，不返回 mock success | Settings/Distributed/插件共享 capability 真源 |
| D7 | LAN | 保持现有“安全默认 + 显式受信 LAN”产品语义 | Apple transport/Local Network 实现细节在 Phase 1 固定，真机未验证 |
| D8 | 分发/更新 | Android Release 附属的实验性未签名 asset；用户自签；无 iOS updater | 无项目证书、TestFlight/App Store、第三方签名工具支持 |
| D9 | incoming share | 不进入当前范围 | 不生成 Extension/App Group/第二 target |
| D10 | 通知 | 系统 local notification、APNs 均不进入首期 | 不请求通知权限；保留 App 内 Toast/通知中心 |
| D11 | UI 基线 | 保留高密度线性、语义 z-index、no-blur；iPhone/iPad 基础可访问性走 Simulator | 不以项目真机作为门禁，不引入第二 overlay 状态机 |
| D12 | 原生策略 | 平台 adapter；Swift 不复制 Chat/Sync/Distributed | typed facade + 单一 Rust owner |
| D13 | Bundle ID | CI 候选 `com.vcp.avatar`；用户重签后可被工具改写 | runtime 不假定安装后 ID/Info.plist/entitlement 与 CI 相同；task identifier 必须匹配最终 ID |
| D14 | Apple 资源 | 仅要求 GitHub macOS/Xcode runner；无项目真机、正式签名或付费 Team | compile/Simulator 是项目证据；设备由社区自愿报告 |
| D15 | 首个 artifact | 核心 Chat/Diary/Settings/SQLite/Sync + 文档/文本/图片附件 + iOS 26 尽力后台 | P0 必须关闭或从 artifact 下架，不能书面延期 |
| D16 | 性能/内存 | 不建立 iOS benchmark、soak 或设备性能门禁 | 沿用硬上限与静态正确性；状态 `PERFORMANCE-NOT-EVALUATED` |
| D17 | 语音 | 当前 Android 也不可用，iOS 不移植 | 隐藏入口，不请求 microphone，不测试音频 |
| D18 | 附件 | P0；原生负责获取/staging/大图，Rust 负责 Office/PDF/text 解析与 CAS | 不寻找“Apple 通用编码器”；视频延期 |
| D19 | 进度 | 只报告真实 token/字符/字节/chunk/terminal | `max_output_tokens` 只是上限；无 token 计数时 indeterminate，禁止伪百分比 |

这些值若以后改变，必须作为新的产品变更记录，不允许施工阶段通过“顺手兼容”扩大范围。

## 4. P1 产品细节决策

这些不会改变已冻结的产品范围，可在 Phase 1–5 用构建证据选择：

| ID | 问题 | 推荐方向 | 需要的事实 |
|---|---|---|---|
| P1-1 | minimum system/Xcode | 选当期 Tauri 支持且 runner 可固定的最低组合；continued 仍从 26 分层 | Phase 1 clean build 与 runtime 库存 |
| P1-2 | continued submission strategy | 优先能立即得到拒绝并回退前台的策略 | 当期 API 与 Simulator 行为 |
| P1-3 | 大图 staging 参数 | 沿用既有字节上限，单独冻结解码像素/输出格式 | ImageIO fixture 与服务端接受格式 |
| P1-4 | iPad presentation | 复用 overlay owner，按 viewport 选择 sheet/popover/dialog | 关键弹层清单与 Simulator |
| P1-5 | LAN | 首期手工配置；冻结 Rust/WebKit 各自最窄 cleartext/Local Network 策略 | 实际请求 owner 与 build 配置 |
| P1-6 | 敏感配置存储 | 评估 Keychain/Stronghold；若未迁移，limitations 明确剩余风险 | 现有迁移兼容、登出/备份策略 |
| P1-7 | 错误文案 | 一条推荐下一步 + 稳定 reason code；完整诊断另存 | capability 与客服日志 |
| P1-8 | artifact 结构 | `.app.zip` 必选；只有结构/自签输入校验通过才附 `.unsigned.ipa` | 当期 Tauri/Xcode 输出 |

## 5. 不需要再讨论的技术底线

以下并非偏好选项，而是平台或工程边界：

- 不使用私有 API、越狱、Root shell 或伪装 background mode；
- 不在 iOS 尝试启动 Android `:helper`、Foreground Service 或物理 Wake/Wi-Fi Lock；旧 lock facade 只能映射真实 continued/foreground logical lease；
- 不监听其他 App 通知，不提供 OEM 自启动/忽略电池优化入口；
- 应用内不下载、重签、打开或静默安装 IPA 来复刻 APK updater；用户在应用外自行处理 GitHub artifact；
- 不调用 `process::exit` 或程序化退后台作为正常导航；
- 不以 `permissions=true`、固定电量/网络/传感器值或 no-op success 代表兼容；
- 不让 Swift 成为第二个 Chat/Sync/Distributed owner；
- 不以 Linux、Safari 桌面或 Simulator 结果冒充真机；当前没有审核范围；
- 不为 iOS 视觉模仿引入内容区 backdrop blur、大圆角卡片或第二套层级系统；
- 不修改 `/home/dudu/VCPToolBox` 或 `/home/dudu/VCPChat` 参考工程。

## 6. 分阶段资源清单

下表最后一列是 blocker tag，不是 evidence state；资源到位只能解除 blocker，不能自动产生编译、设备或 artifact 证据。

| 资源 | 最低要求 | 最晚需要 | 缺失时只阻塞 |
|---|---|---|---|
| GitHub macOS 构建环境 | 固定 macOS image/Xcode/runtime；CocoaPods、Rust Apple targets | Phase 1 | `MACOS-COMPILED`/Simulator 证据 |
| GitHub Release 权限 | 独立 iOS job 最小 `contents: write`，失败不影响 Android | Phase 5 | artifact attach |
| Bundle ID | CI 候选 ID 与 manifest 一致；允许用户重签改写 | Phase 1 | 构建/运行 capability，不代表用户最终 ID |
| iPhone/iPad 真机 | 项目不提供；社区自愿 | 非门禁 | 只产生可选 `COMMUNITY-DEVICE-REPORTED` |
| 服务端 owner | 明确不可改 | 已冻结 | 断开不可恢复，UI/terminal 必须诚实 |
| 签名/Team/App record | 当前不需要 | 范围外 | 不阻塞任何 Phase |
| 隐私/artifact owner | 用途说明、PrivacyInfo、limitations 与 manifest 一致 | Phase 1/5 | 实验产物安全边界 |
| Android 回归环境 | 保持现有 arm64 生产链、Gradle 插件测试与关键旅程 | 首次产品改动前 | Android 回归门禁 |

## 7. No-Go 与 Hold 判定

### `NO-GO`

- 产品要求 12 条 `X` 与 Android 等价；
- 产品重新要求设备后台持续 SSE 必达，而不接受 system-managed 尽力续跑/前台回退；
- 必须使用私有 API、越狱或伪后台模式才能满足核心旅程；
- iOS 只能通过复制整个业务核心和第二套状态机才能工作。

### `HOLD`（只保持在被阻塞的证据层，不冻结可独立前序工作）

- GitHub macOS runner/Xcode 尚未可用；
- LAN 明文或敏感数据存储实现细节仍未决；
- macOS spike 暴露关键依赖不支持 Apple target；
- Android 回归不可控或用户工作区没有安全存档条件。

### `GO`

- P0 决策冻结；
- 当前准备进入的阶段所需 GitHub runner/仓库权限可用；
- 允许以 Phase 1 最小 spike 验证未知，而不是直接承诺发布日期；
- 接受按 evidence state 逐级对外声明。

## 8. 决策记录与后续变更模板

本轮决定已落盘；后续只有范围发生变化时才新增一行，不再把冻结项改回“待定”：

| 决策 | 选择 | Owner | 截止点 | 依据/备注 |
|---|---|---|---|---|
| 2026-08-13 D1–D19 | 已冻结，详见第 3 节 | 用户 | Phase 0 | `GO-EXPERIMENTAL` |
| 后续变更 ID | 新选择 | 变更批准者 | 生效 Phase/版本 | 对 P0、Android Release、capability 与文档的影响 |

当前正确结束状态是“研究与产品决策完成，等待获批后进入 Phase 1 产品施工”。本轮仍未创建 Apple 工程或改动生产代码。
