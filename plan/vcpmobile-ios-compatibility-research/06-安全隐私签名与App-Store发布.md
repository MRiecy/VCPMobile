# 06. 安全、隐私、未签名产物与未来 App Store 边界

## 1. 总体判断

iOS 当前定位是**实验性可选兼容**，不是正式 Apple 分发产品。项目只在 GitHub Actions 的 macOS runner 编译可供用户后续自签名的未签名 iPhone/iPad 产物，并把它作为 Android GitHub Release 的非阻塞附属资产；项目不提供官方签名、TestFlight/App Store、自动更新、用户证书托管或第三方续签工具支持。

未签名不等于无安全边界：Info.plist、后台声明、Tauri capability、PrivacyInfo、数据保护和产物清单仍必须与实际代码一致。它也不等于可直接安装；用户必须使用自己的 Apple 身份、设备注册和 provisioning profile 重签，安装资格和续签周期由其选择的工具与 Apple 账号决定。

## 2. iOS 平台硬边界

即使不走 App Store，下列操作系统与安全边界仍成立：

- 使用公开 API，遵守平台沙箱；
- App sandbox 与公开 API 不因 sideload 消失；
- background capability 只能用于声明的真实用途，系统仍可拒绝或终止；
- 权限、数据收集与用户说明必须真实；
- 签名后的 entitlements 必须由实际 provisioning profile 允许，不能靠修改 plist 获得。

直接结论：

- `download_update/install_update` 的 APK 自安装链在 iOS 必须禁用；
- 应用内不从 GitHub 下载、重签或安装 IPA；GitHub Release 只公开用户手工自签输入产物；
- 不使用私有 API 实现 Root、其他 App 通知读取或进程常驻；
- 不滥用 audio/location/remote notification background mode 保 SSE；
- Android mock/fake success 不能进入 iOS artifact。

App Store Review 只在未来另行决定正式 iOS 产品时恢复为发布门禁；当前文档不准备审核账号、商店截图或 App Store Connect 流程。

## 3. Bundle ID、版本与最低系统

当前 identifier 是 `com.vcp.avatar`。实验产物构建时必须确认：

1. CI 使用的候选 Bundle ID 与 artifact manifest 一致；用户重签工具可能改写 Bundle ID，项目不得把原 ID 当作安装后事实；
2. marketing version 与 `CFBundleVersion` 能从 Android Release/tag 和 commit SHA 追溯；
3. 最低 iOS/iPadOS 编译版本显式固定；
4. continued-processing 以 `if #available(iOS 26.0, *)` 分层，更早受支持系统始终 `foreground_required`；
5. 不生成 Share Extension/Catalyst/macOS target。

当前锁定的 Tauri CLI schema 给 iOS `minimumSystemVersion` 默认值 14.0，但项目不应依赖隐式默认。最低版本由当期 Tauri/Xcode/WebKit 编译尖峰冻结；没有必要为了 continued processing 把整个应用最低版本直接抬到 26，availability guard 可保留更早系统的前台模式。默认值不是兼容证据。

## 4. Info.plist 用途说明

只添加首个 artifact 实际使用且能解释的权限：

| Key | 何时需要 | 本项目用途 |
|---|---|---|
| `NSLocalNetworkUsageDescription` | LAN HTTP/WS | 连接用户配置的本地 VCP 服务 |
| `BGTaskSchedulerPermittedIdentifiers` | iOS 26 continued processing | 只列实际注册、符合当期 Xcode 要求的 wildcard task identifier |

要求：

- 文案用用户语言解释“为什么现在需要”，不写空泛“改善体验”；
- 权限按功能触发，不在首次启动一次索取全部；
- denied/restricted/notDetermined 分开表达；
- Local Network 由真实访问触发，不伪造预查询；
- 未进入产品范围的相机、麦克风、照片库写入、通知、定位、Motion 和 Speech 不提前声明；PHPicker 用户选择不等于申请整个照片库读取权限。

## 5. Capabilities、Info.plist 后台模式与运行时 API

这三类机制正交，不能统称为 entitlement：

### 5.1 签名 capability / entitlement

| 能力 | 典型签名事实 | 进入时机 |
|---|---|---|
| Continued-processing GPU entitlement | 当前不启用 | SSE 只需网络/CPU，不请求后台 GPU；不得无故扩大 entitlement |
| App Groups / Push / Associated Domains / Multicast | 当前不启用 | Share Extension、APNs、universal link 与 multicast 均不在首期范围 |

### 5.2 Info.plist 声明

`UIBackgroundModes` 是 Info.plist 声明，不是独立 entitlement。首期只允许为 iOS 26 continued processing 声明当期 Xcode/BackgroundTasks 文档要求的 `processing`，并在 `BGTaskSchedulerPermittedIdentifiers` 配置以最终 Bundle ID 为前缀的静态或 wildcard identifier；不添加 `audio`、`location`、`fetch` 或 `remote-notification`。声明不授予常驻执行权。

### 5.3 运行时后台机制

- `beginBackgroundTask`：只申请有限收尾时间，必须有 expiration handler 和 exact-once end receipt；
- `BGContinuedProcessingTask`（iOS/iPadOS 26+）：用户主动发起生成的尽力续跑，必须有真实 `Progress`、取消和 expiration；系统拒绝、最终 Bundle ID 与 task identifier 不匹配或配置不可用时回退前台；
- `BGTaskScheduler`：由系统决定调度时机，适合可延迟维护，不适合持续 SSE；
- background `URLSession`：由系统接管符合条件的文件传输和 delegate 恢复，不是 entitlement，也不是任意 Rust socket 的后台许可；
- APNs：当前不实现。

不要预先开启一堆“将来可能用”的 capability 或 background mode；这会扩大攻击面、重签失败面和配置漂移。用户自签若改写 Bundle ID，必须同步满足最终 task identifier/prefix 配置；应用以实际 `register/submit` 结果决定 `continued_task_accepted` 或 `foreground_required`，不能因 plist 中存在声明就返回已启用。

## 6. Local Network 与 ATS

当前 Android Release 可在显式 `VCP_TRUSTED_LAN_MODE=enabled` 时允许受信 LAN 明文流量。iOS 需要重新定义，而不是把 Android manifest 配置翻译成全局 ATS 关闭。

先按实际传输 owner 拆开：WKWebView/Apple URL Loading System 路径会受到 ATS 配置影响；Rust `reqwest` + `rustls` 路径不应未经验证就假定同一 plist 例外一定生效。Local Network 隐私提示应按 WebView 与 Rust socket 的真实发起路径分别记录；项目无真机，相关设备事实保持社区未验证。

建议顺序：

1. 默认 HTTPS/WSS；
2. 用户显式输入 LAN host 时触发 Local Network；
3. 如必须支持 HTTP/WS，先判断请求究竟由 WebView/Apple URL Loading 还是 Rust 网络栈发出，再使用可证明有效的最窄配置；
4. UI 标记“受信局域网明文连接”，不允许任意 internet cleartext；
5. 记录 endpoint scheme/host，但日志不记录 API key；
6. Simulator 先验证基础错误映射；allowed/denied、IP、`.local`、IPv6-only、VPN/热点保持社区真机未验证；
7. 动态用户输入的 LAN endpoint 另由应用层 allowlist、scheme/host 校验和显式 trusted-LAN 开关约束，不能假定 ATS domain exception 能表达任意 IP/CIDR；
8. artifact 说明解释业务场景、用户控制和安全边界。

依据：[TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy) 与 [ATS](https://developer.apple.com/documentation/security/preventing-insecure-network-connections)。

## 7. PrivacyInfo.xcprivacy 与 Required Reason API

仓库当前没有 `PrivacyInfo.xcprivacy`。Apple 对正式分发的 App 和相关 SDK 有隐私清单与 required-reason API 规则；虽然当前不提交商店，项目仍应让最终 bundle 的清单与实际调用一致，避免实验分支形成另一套隐私事实。参见 [Privacy Manifest Files](https://developer.apple.com/documentation/bundleresources/privacy-manifest-files)。

不能现在凭 Rust/JS 依赖名编造清单。正确流程：

1. 生成 Apple 工程并实际构建；
2. 审计 App、Swift Package、CocoaPods 和嵌入 framework；
3. 从最终 binary/static analysis 与 Apple 报告识别 required-reason APIs；
4. 为每项选择与真实用途一致的 reason；
5. 检查第三方 SDK 是否自带有效 privacy manifest；
6. CI 验证最终 `.app` 内 manifest 存在且内容不漂移；
7. artifact 隐私说明与代码/隐私政策一致。

应建立数据账本：聊天正文、附件、日记、API key、设备遥测、诊断日志、push token、分享文件、位置/运动数据分别说明是否采集、是否离设备、保存多久、如何删除。

## 8. 本地秘密与数据保护

当前 `vcp_api_key` 等设置被序列化到 SQLite `settings` 的 global JSON 行；仓库未使用 Keychain/Stronghold。iOS 施工前必须决策：

- API key、distributed key、log key 是否迁入 Keychain/受保护存储；
- 迁移如何保持 Android 数据兼容与失败回滚；
- 前端 `read_settings` 是否仍返回完整 secret；
- debug/log/crash report 如何脱敏；
- 文件保护级别、锁屏时数据库/附件是否可访问；
- iCloud backup 是否包含聊天、日记与附件。

推荐将“连接配置”和“秘密值”分离；UI 只获得必要的 masked/has-value 状态。此项不是为了制造 iOS 专属设置系统，而是收紧既有 owner。

## 9. Tauri capability 与 Web 内容边界

当前主窗口 capability 统一包含 `vcp-mobile:allow-all`，而 [tauri.conf.json](../../src-tauri/tauri.conf.json) 的 CSP 为 `null`、asset protocol scope 为 `**`。iOS 上不应继续把所有 Android command 暴露给 WebView。

建议：

- 按平台拆 capability；iOS 不授权 Root/helper/autostart/listener；旧 wake-lock facade 只有映射为真实 continued/foreground logical lease 时才可最小授权；
- 每条高风险命令使用最小参数 scope；
- Tauri capability 只约束 host 中的 Tauri window/WebView command 可达面；额外 WebView 各自最小权限；
- 首期不生成 Share Extension target，Tauri capability 只覆盖主 App WebView；
- 继续保护高保真 raw HTML，但收窄 executable danger、跨 frame messaging 和 Tauri invoke 可达性；
- 复核 `iframe.srcdoc`、sandbox、postMessage origin/source、asset protocol 路径；
- 在能保持 LAN/富 HTML 功能的前提下设计 CSP，而不是简单关闭 raw HTML；
- 权限 schema 必须以最终 iOS generated tree 验证。

## 10. 未签名与用户自签责任边界

项目 CI 不注入 Apple certificate、provisioning profile、App Store Connect API key 或 APNs key，也不创建临时 keychain。macOS job 以当期 Tauri/Xcode 可验证的 code-sign-disabled 方式构建 device `.app` 和 Simulator artifact；具体命令必须在 Phase 1 尖峰后冻结，不能把“编译了 Rust staticlib”冒充完整 `.app`。

公开产物不是可直接安装的软件：

- 用户必须使用自己的 Apple ID/Developer Team、设备注册与 provisioning profile 重签；
- 免费 Personal Team 的设备/App ID 有数量限制，provisioning profile 通常 7 天到期，用户需要自行重新签名/安装。参见 [Apple membership comparison](https://developer.apple.com/support/compare-memberships/)；
- AltStore、Sideloadly、TrollStore、企业证书或其他第三方工具均不由项目推荐、集成、测试或支持；它们的合法性、安全性、可用系统版本和撤销风险由用户自行判断；
- 重签可能改写 Bundle ID、application identifier、Info.plist 或 entitlement。运行时 capability 必须以最终已安装环境与 task register/submit 结果为准；continued processing 不可用时自动回退前台常亮；
- 用户不得向 Issue/日志上传 Apple ID 密码、证书私钥、完整 provisioning profile 或第三方签名凭证。

Apple code signing 的官方入口见 [Tauri iOS Code Signing](https://v2.tauri.app/distribute/sign/ios/)。本文只用它解释为什么 unsigned artifact 仍需后续签名，不把签名步骤纳入项目交付。

## 11. GitHub Release 附属产物

当前 [release.yml](../../.github/workflows/release.yml) 的 Android 签名 APK 仍是唯一正式产物。iOS 使用独立 macOS job，并复用同一个 `release published` 事件向该 GitHub Release 追加实验资产：

```text
GitHub Release published
  ├─ Android job：既有正式门禁、签名 APK、失败则 Release 工作流失败
  └─ iOS experimental job：macOS + fixed Xcode
       -> Apple device compile/link with code signing disabled
       -> iPhone/iPad Simulator smoke
       -> package unsigned self-sign input
       -> generate SHA-256 + manifest + limitations
       -> attach assets to the existing GitHub Release
       -> failure is visible but MUST NOT block or delete Android assets
```

建议产物：

- `VCPMobile-iOS-experimental-unsigned.app.zip`；只有当工具链确实生成且结构校验通过时，才可额外提供明确命名的 `*.unsigned.ipa`；
- 对每个资产提供 `.sha256`；
- `ios-artifact-manifest.json` 记录 tag、commit SHA、Tauri/Rust/Node/Xcode/runtime 版本、target architectures、Bundle ID、minimum system、iOS 26 availability、requested capabilities、实际 `codesign` 检查结果、测试层级与 `distributionSigned=false`；
- `IOS-EXPERIMENTAL-LIMITATIONS.md` 明示“不可直接安装、需自行签名、可能 7 天续签、无项目真机/性能保证、无自动更新、后台仅尽力”；
- Simulator artifact 可作为 CI 调试资产，不包装为设备安装包。

iOS job 可以通过独立 workflow/check 或 job-level `continue-on-error` 实现非阻塞，但必须保留失败日志与状态，不能静默绿色。应用内 updater 不发现、不下载这些资产；用户手工检查 GitHub Release。

## 12. App Store/TestFlight 的未来边界

当前不创建 Apple Developer 付费团队、App Store Connect record、TestFlight group、商店 metadata、审核账号或正式签名 workflow。若未来产品定位改变，必须重新立项处理签名 secret、隐私标签、截图、审核、出口合规、设备矩阵和更新路线；不能把本实验 artifact 的 E5 构建证据升级为正式分发证据。

## 13. 实验产物安全退出条件

- [ ] Apple device target 与 iPhone/iPad Simulator 完整 `.app` 编译/链接成功；
- [ ] Info.plist、requested capabilities、PrivacyInfo 与 P0 范围一致，不包含麦克风、通知、定位、Motion、Share Extension、APNs 或 iOS updater；
- [ ] 12 条 `X`、Android Root/helper/OEM/listener/APK updater 和全部 mock fallback 在 iOS fail-closed；
- [ ] 文档/文本/图片附件 fixture 与 staging contract 通过；
- [ ] iOS 26 continued task 和旧系统 foreground fallback 都有静态/Simulator 契约证据；系统拒绝、用户取消和 expiration 有明确终态；
- [ ] 未签名/自签输入资产、SHA-256、manifest 和 limitations 文件来自同一 commit；检查确认 `distributionSigned=false`、记录实际 code-sign 状态且无嵌入项目秘密；
- [ ] iOS job 失败不会阻断、修改或撤回 Android 签名 APK；
- [ ] 发布文字只称“实验性未签名 iPhone/iPad 兼容产物”，不称 App Store、正式 iOS Release、真机验证或后台可靠。

项目自有真机、iOS 性能基准/soak、TestFlight/App Review 均不在退出条件中；对应证据状态分别保持 `COMMUNITY-DEVICE-UNVERIFIED`、`PERFORMANCE-NOT-EVALUATED` 与 `OUT-OF-SCOPE`。
