# 06. 安全、隐私、签名与 App Store 发布

## 1. 总体判断

iOS 适配不是“编出一个 IPA”就结束。VCPMobile 当前没有 Apple 工程、用途说明、entitlements、PrivacyInfo、签名身份、App Store Connect 记录或 iOS Release workflow。产品能力、隐私声明、签名产物与实际代码必须一致，否则 Simulator 绿色也不能进入 TestFlight。

## 2. App Review 硬边界

与本项目最相关的 Apple 规则：

- 使用公开 API，遵守平台沙箱；
- App 应自包含，不下载、安装或执行改变 App 功能的代码；
- background mode 只能用于声明的用途；
- 权限、数据收集与用户说明必须真实；
- 审核人员必须能够访问核心功能和必要后端。

依据：[Apple App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)。

直接结论：

- `download_update/install_update` 的 APK 自安装链在 iOS 必须禁用；
- 不从 GitHub 下载并安装 IPA；
- 不使用私有 API 实现 Root、其他 App 通知读取或进程常驻；
- 不滥用 audio/location/remote notification background mode 保 SSE；
- Android mock/fake success 不能进入 iOS 审核包。

## 3. Bundle ID、版本与最低系统

当前 identifier 是 `com.vcp.avatar`。施工前必须确认：

1. Apple Developer Team 是否能注册该 Bundle ID；
2. App Store Connect 是否已有同名/同 ID 记录；
3. Share Extension 是否使用独立 ID，例如 `com.vcp.avatar.share`；
4. marketing version 与 `CFBundleVersion` 如何单调递增；
5. 最低 iOS/iPadOS 版本如何显式固定。

当前锁定的 Tauri CLI schema 给 iOS `minimumSystemVersion` 默认值 14.0，但项目不应依赖隐式默认。最低版本必须根据用户分布、WebKit、Swift API 和测试设备明确写入配置。默认值不是产品决策，也不是兼容证据。

## 4. Info.plist 用途说明

只添加实际使用且能解释的权限。候选清单：

| Key | 何时需要 | 本项目用途 |
|---|---|---|
| `NSCameraUsageDescription` | 相机输入 | 拍摄聊天附件 |
| `NSMicrophoneUsageDescription` | 录音/STT | 语音附件与转写 |
| `NSPhotoLibraryAddUsageDescription` | 写入照片库 | 保存生成或聊天图片 |
| `NSPhotoLibraryUsageDescription` | 直接读取照片库时 | 仅在实际采用 PhotoKit 读取路径时添加 |
| `NSLocationWhenInUseUsageDescription` | location tool | 用户启用且仅前台 |
| `NSMotionUsageDescription` | Motion/步态/气压等 | 用户启用设备传感器工具 |
| `NSSpeechRecognitionUsageDescription` | 使用原生 Speech framework 时 | 仅在采用原生识别时添加 |
| `NSLocalNetworkUsageDescription` | LAN HTTP/WS | 连接用户配置的本地 VCP 服务 |

要求：

- 文案用用户语言解释“为什么现在需要”，不写空泛“改善体验”；
- 权限按功能触发，不在首次启动一次索取全部；
- denied/restricted/notDetermined 分开表达；
- Local Network 由真实访问触发，不伪造预查询；
- 未进入产品范围的传感器不要提前声明。

## 5. Capabilities、Info.plist 后台模式与运行时 API

这三类机制正交，不能统称为 entitlement：

### 5.1 签名 capability / entitlement

| 能力 | 典型签名事实 | 进入时机 |
|---|---|---|
| App Groups | host 与 Share Extension 都声明同一 group value | incoming share 获批后 |
| Push Notifications | profile/签名中包含与环境匹配的 `aps-environment` | APNs slice 获批后 |
| Associated Domains | target entitlement 与服务端 association 文件匹配 | universal link 正式采用后 |
| Multicast Networking | 受 Apple 管理的 multicast entitlement | 产品证明必须依赖 multicast 后再申请；当前不默认申请 |

### 5.2 Info.plist 声明

`UIBackgroundModes` 是 Info.plist 声明，不是独立 entitlement。`fetch`、`remote-notification`、`processing` 等值只有在代码确有相应公开 API 和真实业务用途时才添加；它们不授予常驻执行权，也不保证 silent push 必达。采用 `BGTaskScheduler` 时，还要维护实际提交 task identifier 对应的 plist allowlist。

### 5.3 运行时后台机制

- `beginBackgroundTask`：只申请有限收尾时间，必须有 expiration handler 和 exact-once end receipt；
- `BGTaskScheduler`：由系统决定调度时机，适合可延迟维护，不适合持续 SSE；
- background `URLSession`：由系统接管符合条件的文件传输和 delegate 恢复，不是 entitlement，也不是任意 Rust socket 的后台许可；
- APNs：通知/提示结果可用，但 delivery 和执行预算都不能作为业务必达承诺。

每个 Extension 有独立 target、Bundle ID、App ID 和 provisioning profile。host 与 Extension 必须属于同一 Team，并在需要共享时声明同一个 App Group value；这不意味着二者复用同一 profile。

不要预先开启一堆“将来可能用”的 capability 或 background mode；这会扩大攻击面、审核解释和签名漂移。

## 6. Local Network 与 ATS

当前 Android Release 可在显式 `VCP_TRUSTED_LAN_MODE=enabled` 时允许受信 LAN 明文流量。iOS 需要重新定义，而不是把 Android manifest 配置翻译成全局 ATS 关闭。

先按实际传输 owner 拆开：WKWebView/Apple URL Loading System 路径会受到 ATS 配置影响；Rust `reqwest` + `rustls` 路径不应未经验证就假定同一 plist 例外一定生效。Local Network 隐私提示也必须按 WebView、Rust socket、Bonjour/Extension 等真实发起路径分别在真机验证。

建议顺序：

1. 默认 HTTPS/WSS；
2. 用户显式输入 LAN host 时触发 Local Network；
3. 如必须支持 HTTP/WS，先判断请求究竟由 WebView/Apple URL Loading 还是 Rust 网络栈发出，再使用可证明有效的最窄配置；
4. UI 标记“受信局域网明文连接”，不允许任意 internet cleartext；
5. 记录 endpoint scheme/host，但日志不记录 API key；
6. 真机验证 allowed/denied、IP、`.local`、IPv6-only、VPN/热点；
7. 动态用户输入的 LAN endpoint 另由应用层 allowlist、scheme/host 校验和显式 trusted-LAN 开关约束，不能假定 ATS domain exception 能表达任意 IP/CIDR；
8. App Review notes 解释业务场景、用户控制和安全边界。

依据：[TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy) 与 [ATS](https://developer.apple.com/documentation/security/preventing-insecure-network-connections)。

## 7. PrivacyInfo.xcprivacy 与 Required Reason API

仓库当前没有 `PrivacyInfo.xcprivacy`。Apple 要求 App 和相关 SDK 按规则声明隐私清单与 required-reason API，参见 [Privacy Manifest Files](https://developer.apple.com/documentation/bundleresources/privacy-manifest-files)。

不能现在凭 Rust/JS 依赖名编造清单。正确流程：

1. 生成 Apple 工程并实际构建；
2. 审计 App、Swift Package、CocoaPods 和嵌入 framework；
3. 从最终 binary/static analysis 与 Apple 报告识别 required-reason APIs；
4. 为每项选择与真实用途一致的 reason；
5. 检查第三方 SDK 是否自带有效 privacy manifest；
6. CI 验证最终 archive 内 manifest 存在且内容不漂移；
7. App Privacy 营养标签与代码/隐私政策一致。

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

- 按平台拆 capability；iOS 不授权 Root/helper/autostart/listener/wake lock；
- 每条高风险命令使用最小参数 scope；
- Tauri capability 只约束 host 中的 Tauri window/WebView command 可达面；额外 WebView 各自最小权限；
- 原生 Share Extension 不运行主 App 的 Tauri capability，它受自己的 extension sandbox、entitlements、App Group 与系统 extension API 约束；
- 继续保护高保真 raw HTML，但收窄 executable danger、跨 frame messaging 和 Tauri invoke 可达性；
- 复核 `iframe.srcdoc`、sandbox、postMessage origin/source、asset protocol 路径；
- 在能保持 LAN/富 HTML 功能的前提下设计 CSP，而不是简单关闭 raw HTML；
- 权限 schema 必须以最终 iOS generated tree 验证。

## 10. 签名资产

Tauri 官方要求 Apple Developer enrollment、已注册 Bundle ID、签名证书与关联的 provisioning profile。参见 [Tauri iOS Code Signing](https://v2.tauri.app/distribute/sign/ios/)。

### 自动签名 CI

- `APPLE_API_ISSUER`
- `APPLE_API_KEY`
- `APPLE_API_KEY_PATH`

### 手动签名

- `IOS_CERTIFICATE`
- `IOS_CERTIFICATE_PASSWORD`
- `IOS_MOBILE_PROVISION`

上述 Tauri 变量只能作为当前 host target 的已知入口。若加入 Share Extension，自动签名必须逐 target 解析正确 identity/profile；手动签名则需要按各 Bundle ID 提供 profile 映射或冻结的 Xcode export 配置，不能把一个 `IOS_MOBILE_PROVISION` 复制给两个 App ID。

### 上传认证

Tauri 当前签名文档中的 `APPLE_API_KEY` 表示自动签名所用的 App Store Connect Key ID；当前 App Store 上传示例则使用 `APPLE_API_KEY_ID`，并要求私钥按上传工具约定放置。CI 设计时必须把“构建/签名认证”和“上传认证”分别按实际 CLI 版本冻结并做 secret presence check，不能仅凭相似变量名复用。共同字段是 `APPLE_API_ISSUER`，私钥文件均不得进入仓库。

APNs provider 认证是第四类独立资产：可能使用 APNs signing key 或 provider certificate，并有自己的 Key ID/Team/environment 与轮换 owner。不得因为文件扩展名同为 `.p8` 就把 APNs provider key、App Store Connect API key 和代码签名资产视为同一个 secret。

签名治理要求：

- secrets 只进入受保护 macOS Release environment；
- PR/Simulator job 不持有 distribution secrets；
- profile 的 App ID、Team、entitlements 与 archive 实际值一致；
- Share Extension 使用独立 profile；
- CI 结束后安全清理 temporary keychain/profile/private key；
- 输出验证包括 `codesign`、embedded provisioning、architectures、Info.plist、entitlements、PrivacyInfo；
- 不在日志中打印 secret、certificate 私钥或完整 profile。

## 11. Release 产物与渠道

当前 [release.yml](../../.github/workflows/release.yml) 只构建并公开上传 arm64 APK。iOS 应使用独立 macOS workflow：

```text
protected commit/tag candidate
  -> require same-commit iOS CI
  -> restore signing identity/profile/upload API key
  -> pnpm tauri ios build --export-method app-store-connect
  -> verify archive/IPA identity and entitlements
  -> upload App Store Connect
  -> record uploaded -> processed -> internal beta -> external beta states
  -> App Review approved / release
  -> optionally publish matching GitHub Release notes
  -> cleanup signing material in every path
```

Tauri 官方 build 路径参见 [App Store Distribution](https://v2.tauri.app/distribute/app-store/)。

建议：

- IPA 不作为公开 GitHub Release 资产；
- GitHub Release 可保留 release notes，但 iOS CTA 指向 App Store/TestFlight；
- Android 和 iOS 可以同一 marketing version，但 build number/versionCode 分别治理；
- 不把当前“GitHub Release published”触发的 Android workflow 原样套给首次 iOS 上传，否则会形成“先发布 tag/release，才知道能否上传”的错误闭环；iOS workflow 的 tag/commit policy 必须单独冻结；
- Git tag、GitHub Release 与 App Store 发布是三个对象，必须记录映射，但不要求在 Apple 处理完成前公开 GitHub Release；
- 不让 iOS Release 依赖一个名称模糊、实际只有 Android 的 CI 结果。

## 12. App Store Connect 与审核材料

至少需要：

- Apple Developer Team owner 和 App Store Connect 管理责任人；
- App record、Bundle ID、SKU、分类、年龄分级；
- 隐私政策 URL、支持 URL、App Privacy answers；
- 截图和 iPhone/iPad metadata；
- 审核账号、可达的 VCP 测试服务和操作说明；
- Local Network/LAN、后台恢复、通知、传感器用途说明；
- 加密出口合规判断；
- 数据删除/账号删除流程（若存在账号体系）；
- Share Extension/推送等特殊能力演示；
- Simulator 可以准备候选截图，但截图必须准确反映提交 build；真机兼容说明、权限/后台/LAN 证据仍只能来自具名设备。是否把某张候选截图作为最终商店资产，应在目标尺寸与提交 build 复核后决定。

## 13. 安全与分发退出条件

### 13.1 可上传候选

- Apple target clean build；
- Info.plist/entitlements/PrivacyInfo 与该 build 的实际能力一致；
- 该 Beta manifest 内所有 `X` 能力和 APK updater 不可达；
- secrets 存储迁移已落实，或由明确 owner 对剩余风险作发布裁决；
- archive 签名、各 target profile、Bundle ID、Team、architecture 验证通过；
- App Privacy 数据账本、隐私政策、出口合规与最小商店记录可用于上传；
- 生成 `UPLOAD-CANDIDATE` 后才执行首次上传；不能要求它预先已有 TestFlight 处理成功。

### 13.2 已处理与内部 Beta

- App Store Connect 分别记录 upload accepted 与 build processed；
- 内部组在具名设备完成核心旅程；若该 build 声明 LAN、媒体、通知或 Extension，则补齐对应真机门禁；
- ATS/Local Network 只在该 capability 被选入时阻塞，但不能用 Simulator 代替。

### 13.3 外部 Beta

- 完成外部测试所需的 Beta App Review、测试说明、隐私与出口合规信息；
- 冻结的外部设备/系统范围有记录，缺陷可追溯到 build SHA。

### 13.4 App Store 发布

- 完整真机功能矩阵与审核材料通过版本审查；
- `APP-REVIEW-APPROVED` 只在 Apple 审核通过后取得；拒绝或一次审核结果都不能替代批准；
- 发布/Ready for Sale 后才能标记 `RELEASED`。此前必须使用对应的上传、处理或 Beta 状态，不能写“iOS 发布完成”。
