# iPhone / iPad 无签名归档

VCP Mobile 的 iOS 构建产物是面向 arm64 真机的无签名 Xcode 归档，最低支持 iOS/iPadOS 26.0。仓库维护工程、语音资源和分享扩展，安装者负责授权、签名和安装。

## 构建与下载

1. 在 GitHub Actions 选择 **iOS Device Archive**，点击 **Run workflow**，选择要构建的分支。[工作流定义](../.github/workflows/ios-archive.yml)
2. 默认的 `release` 使用优化构建。需要定位调试配置下的问题时选 `debug`。两者都在 GitHub 的 macOS runner 上编译完整应用及分享扩展。
3. 成功后，从该次运行的 **Artifacts** 下载产物。附件包含 `.xcarchive.zip` 和 `manifest.json`。清单记录提交 SHA、profile、Xcode/SDK 版本及归档 SHA-256，可据此核对下载文件。解压归档后，主应用位于 `.xcarchive/Products/Applications/`，内嵌扩展位于主应用的 `PlugIns/VCPShare.appex`。
4. 构建或归档核验失败时，在运行页打开失败步骤，查看工具输出。记录提交 SHA、构建配置和该次运行的链接，便于复现。

清单分别记录主应用和分享扩展可执行文件的实际架构与 Mach-O 平台。真机 arm64 产物应显示 `arm64` 和 `IOS`，`IOSSIMULATOR` 表示模拟器。

## 修改工程

| 输入 | 维护位置 |
| --- | --- |
| 最低系统版本和 Apple framework | [Tauri iOS 配置](../src-tauri/tauri.ios.conf.json)。更改系统下限时也要核对 [Swift 插件 manifest](../src-tauri/plugins/vcp-mobile/ios/Package.swift)。 |
| 主应用用途声明、单 Scene 和分享组标识 | [主应用 Info.plist 输入](../src-tauri/Info.ios.plist) |
| XcodeGen 工程模板、分享扩展和原生资源输入 | 在基于官方 Tauri 模板的 [project.yml.hbs](../src-tauri/gen/apple/project.yml.hbs) 中维护主 target、扩展 target、资源与嵌入关系。同一次 `ios init` 根据该模板生成 Xcode 工程。 |
| 原生能力 | [Swift 插件](../src-tauri/plugins/vcp-mobile/ios/Sources/) |
| 本地语音 | [语音构建输入](../src-tauri/runtime-assets/voice/)。沿用现有 Python 构建，从锁定源码和裁剪算子清单生成 ONNX Runtime，再编译 Swift 绑定并将所需模型与许可证装入应用。 |

在仓库根目录运行 `pnpm check:ios`，在 Windows 或 CI 上检查版本管理的工程输入，并运行归档校验逻辑的 Node 测试。实际 Swift 编译、链接和产物核验由 macOS 工作流完成。更新说明时请使用对应提交的结果，分别记录本地检查、归档和设备运行情况。

## 分享扩展与自签

用户通过系统分享面板将内容保存到扩展，再打开 VCP Mobile 选择助手。扩展把交付清单与文件写入 App Group 容器，主应用读取并导入附件。扩展 bundle 包含原生入口、Info.plist 和自己的隐私清单。

重签时同时处理主应用和内嵌的 `.appex`，为每个 target 准备对应的 App ID、签名身份和 provisioning profile。仓库默认的主 bundle ID 为 `com.vcp.avatar`，扩展为 `com.vcp.avatar.share`，共享 App Group 为 `group.com.vcp.avatar`。两个 target 的 Xcode 签名配置引用同一份 [App Group entitlement 输入](../src-tauri/gen/apple/VCPShare/AppGroups.entitlements)。使用自己的标识时，确保两份 profile 都授权同一组，两个签名中的 `com.apple.security.application-groups` 与各自 Info.plist 的 `VCPShareAppGroup` 均使用该组名。先签扩展，再签主应用，并保留两者的版本对应关系、扩展入口和 `com.apple.share-services` 声明。

归档验证覆盖打包内容。签名后的授权、系统分享面板能否展示扩展，以及设备上的使用体验，需在最终签名安装后确认。
