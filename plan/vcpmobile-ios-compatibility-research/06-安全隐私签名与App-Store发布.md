# 06. 安全、隐私与用户自签产物

> 2026-10-09 修订：沿用 D8/D13 的实验性未签名产物路线，将已批准的系统功能、分享扩展与后台专项纳入声明和用户签名输入；不扩大正式签名或付费 Team 范围。

## 实验性分发方案（2026-10-09）

当时研究了 Tauri CLI 2.11 的无签名归档支持，并拟从 release device archive 提取用户自签资产。下图保留当时评估的打包路线。现行产物和签名说明见 [iOS 构建说明](../../docs/IOS_BUILD.md)。[Tauri 发布说明](https://v2.tauri.app/release/tauri-cli/v2.11.0/)

```text
device .xcarchive / Products / Applications / <Product>.app
  → Mach-O 平台、动态库、资源与 plist 检查
  → <Product>.app.zip
  → Payload/<Product>.app → <Product>.unsigned.ipa
  → manifest + SHA-256
  → 用户配置最终 Bundle ID、身份与 profile → 签名安装
```

`.app.zip` 为必选资产，`.unsigned.ipa` 在 Payload 结构检查完成后附带。用户取得签名身份/profile 并选择外部签名工具；项目负责清晰的输入结构、所需 entitlement 清单和 Bundle ID 改写说明。

### 签名输入的源码依据

以 [zsign v1.1.2 `zsign.cpp`](https://github.com/zhlynn/zsign/blob/v1.1.2/src/zsign.cpp) 为具体研究样本：入口支持目录/ZIP，解包后调用 `SignFolder`，输出 IPA 以 Payload 为根归档。[`bundle.cpp`](https://github.com/zhlynn/zsign/blob/v1.1.2/src/bundle.cpp) 读取 Info.plist 与 executable，处理 profile、嵌套代码和资源封印；[`macho.cpp`](https://github.com/zhlynn/zsign/blob/v1.1.2/src/macho.cpp) / [`archo.cpp`](https://github.com/zhlynn/zsign/blob/v1.1.2/src/archo.cpp) 包含新增签名空间及 `LC_CODE_SIGNATURE` 的路径。这些源码给出了预编译 unsigned bundle 的实际处理机制。

Apple [TN3125](https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles)用于核对签名者、App ID、设备、期限与 entitlement 的配套；[签名格式说明](https://developer.apple.com/documentation/xcode/using-the-latest-code-signature-format)用于核对资源封印和嵌套签名。Apple 官方工具的支持范围与第三方工具的实现能力分别按对应来源记录。

具体包检查：device Mach-O 的平台标记为 iOS，CPU 为 arm64；Simulator 即使也是 arm64 也使用不同平台标记。主 executable 与 `CFBundleExecutable` 对应；嵌套 framework/dylib 的加载路径完整；压缩保留可执行权限和符号链接；Payload 下仅包含预期 app；签名凭据由用户提供。

### Bundle、最低系统与版本

- CI 候选 Bundle ID 为 `com.vcp.avatar`；沿用 D13，用户签名时可以改写。
- 改写 `CFBundleIdentifier` 与 `BGTaskSchedulerPermittedIdentifiers` 后，选择或生成匹配最终 App ID 的 profile，并按其授权设置 application-identifier、team/keychain 等 entitlement；profile 本身由 Apple 签发。Swift 从 `Bundle.main.bundleIdentifier` 构造 runtime task ID，并校验与 plist 一致。
- incoming share 已获批准：P4-4 确定扩展 target 与主 App 交接方式后，将扩展 Bundle ID、共享容器/App Group 以及各 target 的签名输入加入同一改写说明。先核对用户自签可提供的 profile 条件，不假定只有主 App 改 ID 就足够，也不自动要求项目购入付费 Team。
- Info.plist 留旧 task 前缀而只改 runtime ID，会导致注册/提交不匹配；把该情形列入打包检查与用户说明。
- minimumSystemVersion 与 Swift package 按用户 2026-10-09 决定采用 iOS/iPadOS `26.0`，G1 Apple 编译结果尚未取得；高于 26.0 的新增 API 仍核对 availability。continued API 的版本下限不再需要旧系统回退，实际任务提交、激活、拒绝和 expiration 仍须处理。
- 版本保持与 `package.json` / `Cargo.toml` 一致，build version 单独由产物脚本生成并检查格式。
- Xcode/SDK 组合从 runner 实际库存选择，记录路径与版本；框架 Scene 适配随该组合核对。

### 当前工程事实与后续事项

1. app 的 Tauri capability 已按平台区分，实际生产窗口为 `main`，未授予旧 portal 或 `vcp-mobile:allow-all`；新增插件调用按消费者配置；
2. `tauri.conf.json` 仍为 `csp: null`、asset protocol scope `["**"]`；
3. Android/iOS/host 插件分支已分开，iOS 未接通的 adapter 返回错误，不消费 host fixture；
4. 共享 Vue 使用同步平台身份处理普通差异；iOS 更新/Root/语音入口给出说明，APK 自动流程只由 Android 启动；原四字段能力表及统一静态门禁已经移除；
5. API key、sync token、admin/distributed secrets 仍整体序列化进 SQLite settings 并返回前端；这是当前存储事实，本轮没有批准另建 Keychain 或密钥 owner；
6. Distributed 的 iOS 连接和工具源码链已接入，继续复用逐工具显式授权与执行 owner；clipboard、notification、location/telemetry 分别消费真实原生结果，Apple 编译/运行仍待验证；
7. `.gitignore` 已覆盖 `*.p12`、`*.p8`、`*.mobileprovision`，本项目不收集用户签名凭据。

与当前适配直接相关的改动：

- Android/iOS 的 Tauri 命令授权随实际前端消费者接入；Rust 内部接口无需另公开给 Vue；
- 入口呈现由 Vue 的局部平台分支处理，Rust/Swift/Kotlin 按具体操作完成，不为潜在绕过另建统一平台门禁；
- host scaffold 与 iOS adapter 使用各自 cfg；
- unsigned CI 使用只读仓库权限，签名材料保留在用户自己的签名环境；
- 已有消息 DOM、受控 HTML 和附件资源继续复用；本轮不把 CSP/asset scope 收敛或 Keychain 迁移加入实施任务。若具体已批准功能确实受现有配置影响，先定位该调用链，再处理必要差异。

### Info.plist 与 entitlement 纪律

每项平台声明对应一个已规划功能及调用者，记录声明到代码的映射。

当前 `Info.ios.plist` 已包含单 Scene、LAN/定位/运动用途及 local-networking ATS 输入；对应原生操作源码在 P3 接入，最终 Apple bundle 尚未取得。声明进度如下：

- 已加入 `NSLocalNetworkUsageDescription`（P3-1 的用户 LAN 服务）；
- 已加入收窄的 `NSAllowsLocalNetworking`（只影响对应 Apple URL Loading System 场景）；
- 显式 minimum system；
- continued task 的 permitted identifiers 与 processing mode，按 P5-3 实现；P5-1 若选择其他保活手段，依据实际 API 条件添加对应声明，不能先把某后台模式写入配置就视为连续接收已经成立。

条件项：

- Location When In Use/Motion 用途已随 P3-4/P3-5 采样接入；Camera/Photo add-only 留给 P4-2；语音仍未开发，不预先添加麦克风请求；
- Share Extension 的嵌套代码、Bundle ID/profile 及实际需要的 App Group 在 P4-4 确定，P6 验证最终签名输入；
- 本 App 系统通知 P3-3 已接授权请求、本地发送与点击响应，不需要虚构通知 usage-description key；只有另行批准 APNs 时才加入其服务端、profile 和 `aps-environment`；
- 普通 TCP LAN 使用用途说明，Bonjour/multicast 随对应发现/多播功能声明；
- Document Picker 的 security-scoped 访问与应用自己的 iCloud container 分别按功能配置；
- 当前 Chat 使用网络/CPU，后台 GPU 等资源随真实计算需求另行声明。

每项功能按实际需要记录签名 entitlement、Info.plist 描述和运行时授权结果；有 API 不等于都需要这三种声明。共同权限引导消费各功能真实状态，不复制 Android 系统权限列表。

### PrivacyInfo.xcprivacy

先从源码列出文件时间、时钟、磁盘等实际调用及用途，选择对应 required reason；打包后再对照依赖 manifests、符号和 privacy report 补齐。

当前声明与其边界：

| Required Reason 类别 | 已声明/候选用途 | 核对动作 |
|---|---|---|
| File Timestamp | 现有清单 `C617.1`；app container 内 CAS、同步、缓存 | P4 外部文件访问加入后按真实调用重审 |
| System Boot Time | 现有清单 `35F9.1`；Rust/Tokio monotonic time | 以 Apple 链接产物确认 |
| Disk Space | 本轮加入 `85F4.1`，对应实际用户可见的本机容量样本 | 此理由不构成任意远程 telemetry 用途已符合 App Store 要求的声明；当前用户自签范围如实记录，不新增功能门禁或扩大渠道合规工程 |
| User Defaults / Active Keyboards | 从框架和业务调用中查找 | 找到使用方后登记 reason |

数据清单记录 Chat/Diary/Mail、附件、日志、标识符等实际传输目的地与用途；追踪字段从业务及依赖调用核对。未来进入 App Store 时复用这份清单生成渠道要求的隐私标签。

参考 [Privacy manifest files](https://developer.apple.com/documentation/bundleresources/privacy-manifest-files) 与 [required reason API](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)。

### 图标、导出合规与凭据

- iOS AppIcon 从现有高分辨率源生成，核对尺寸、alpha/background 和 asset catalog；
- 使用 `rustls` 的加密能力记录在依赖清单，未来 App Store export-compliance 按实际用途填写；
- 构建 job 使用 `contents: read`；附加 Release 资产的 job 在执行发布时单独取得最小写权限；
- 用户签名材料留在用户签名环境，工程产物包含声明与资源。

### 后续渠道

本轮目标为实验性用户自签资产。未来若选择 TestFlight/App Store，另行规划：

- Xcode/SDK 当期上传要求；
- 合法 signing/provisioning 与最终 entitlements；
- archive privacy report、nutrition label、privacy policy、账户/数据删除说明；
- 相应渠道的功能与 release-build 检查；
- App Review 对完整性、Android 文案、下载代码、远程原生工具和后台模式的审查。

当前交付说明写明产物版本、目标系统、包结构、用户签名步骤和具体已知问题，供使用者按所选签名环境处理。P6 包检查须包含 P4-4 的分享扩展及最终 ID 配套、P3 的已接入权限/通知/设备声明和 P5 实际采用的后台输入；本轮仅安排这些任务，不修改或触发发布流程。
