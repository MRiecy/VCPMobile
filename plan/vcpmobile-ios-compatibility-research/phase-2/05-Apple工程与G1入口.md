# Phase 2：Apple 工程与 G1 入口

> 本文记录首次施工和当时使用的工具版本。后续清理记录见 [G1 流程精简](./10-G1流程精简.md)，最低 iOS/iPadOS 随后提高到 26.0。现行工程输入和归档说明见 [iOS 构建说明](../../../docs/IOS_BUILD.md)。

> 2026-10-08。已实现可重放工程输入和独立手动 workflow；本轮未推送、未触发 Actions、未执行 Apple 编译。Windows 检查结果与待执行 G1 分开记录。

## 1. Casper 裁决与边界

G1 只完成插件接入所需的 device debug archive、符号/资源/配置检查及证据留存。维护少量生成树外的输入和一个 Python 标准库脚本；继续由 CLI 生成工程、执行一次 frontend build、驱动 Rust/Swift 与 archive。用户自签 `.app.zip`/`.unsigned.ipa`、Bundle ID 改写和 G2 release 打包留给 Phase 6。

当时的工作流只允许手动触发，只读仓库内容，同一 ref 上的新运行可取消旧运行。使用固定 revision 的官方 [checkout](https://github.com/actions/checkout/blob/34e114876b0b11c390a56381ad16ebd13914f8d5/action.yml)、[setup-node](https://github.com/actions/setup-node/blob/49933ea5288caeca8642d1e84afbd3f7d6820020/action.yml) 和 [upload-artifact](https://github.com/actions/upload-artifact/blob/ea165f8d65b6e75b540449e92b4886f43607fa02/action.yml)。

## 2. 历史固定输入记录（精简前，非当前断言清单）

| 对象 | 本阶段选择 | 实际检查 |
|---|---|---|
| runner | `macos-15` arm64 | G1 检查 host architecture，保存 ImageOS/ImageVersion 和 sw_vers |
| Xcode | `/Applications/Xcode_26.2.app/Contents/Developer`，26.2 / 17C52 | 路径、version 和 build 必须一致 |
| iPhoneOS SDK | 26.2 | `xcrun --sdk iphoneos --show-sdk-version` |
| minimum | 15.0 | Tauri config、Swift package、环境/Xcode 设置、最终 Info.plist |
| target/profile | `aarch64-apple-ios` / debug | CLI target 参数为 `aarch64`，最终 Mach-O 为 arm64 |
| Rust | 仓库 `rust-toolchain.toml` | rustup 读取仓库版本，仅额外安装 device target |
| Node/pnpm/CLI | 24.14.1 / 12.6.0 / 2.11.4 | workflow 固定安装，脚本复核实际版本 |
| XcodeGen | 2.44.1 | 当时固定官方下载包的 SHA-256，校验后解压并记录工具版本 |

官方 runner 库存在本次查阅时列出所选 Xcode、SDK 和可用 iOS runtime；GitHub 文档将 `macos-15` 列为 arm64。这是输入选择的依据，不能替代某次 job 的实际库存。[runner 库存](https://github.com/actions/runner-images/blob/main/images/macos/macos-15-arm64-Readme.md)、[runner labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)。

锁定 CLI 的 device build 仍检查已安装的 iOS runtime。因此 G1 读取 `simctl list runtimes --json` 并检查 available iOS 项，但不启动 Simulator，也不建设设备/Simulator 旅程。G1 debug 用进程内 `CARGO_PROFILE_DEV_DEBUG=1` 和 `CARGO_PROFILE_DEV_STRIP=false` 保留链接诊断；不改 Android profile。[CLI build](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-cli/src/mobile/ios/build.rs)。

## 3. 已撤销的 SwiftPM resolution 设计（历史）

初次设计依据 `swift-rs@1.0.7` 的独立 `swift build --build-path`，误把实际 root 限定为 Tauri、opener、VCP 三个，遗漏了 log；该错误及直接拒绝的复现见 08 R1。以下保存旧机制的来源，不再要求实施或修复这套集中锁定流程。

历史 `ios/Package.resolved`（现已移除）固定 SwiftRs `1.0.7`，revision `f64a4514de07f450ec5b6aa297624cd3479d9579`，已通过上游 tag 的只读 `git ls-remote` 核对。VCP manifest 同时 exact；当时 G1 Swift shim 在三个 root 分别预置该 resolution，调用真实 Swift 并附加 `--force-resolved-versions`，成功后校验 lock 中所有 pins 与实际 `checkouts/swift-rs` 的 HEAD。未知 root 或调用形状变化都失败；不改依赖 manifest。该套流程已按 10 撤下。

opener 2.5.4 的 registry 包已自带 v1 `Package.resolved`，锁 SwiftRs 1.0.6（`b5ed223fcdab165bc21219c1925dc1e77e2bef5e`）。本阶段显式将这一已审计旧输入迁移至 1.0.7：要求 root 的父目录为 `tauri-plugin-opener-2.5.4` 且旧 lock SHA-256 为 `daf0cdf825417e400e2898d47c1de6912a112265f0fb14c056f46735e728b9c8`，并保存旧 lock/hash 到该 root 的 evidence。其他已有不匹配 lock 一律失败。Tauri/VCP root 当前不自带 lock。

shim 路径通过 PATH，真实 Swift/lock/evidence 通过 `TAURI_VCP_*` 传入；锁定 CLI 的 `mobile::env_vars` 明确转发 PATH 与 `TAURI*`，`xcode_script` 恢复这些变量。G1 最后强制收齐三个 root 的 revision 证据，防止路径被 Xcode 重置后静默绕过 gate。[CLI 环境转交](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-cli/src/mobile/mod.rs)。

上述集中 lock、迁移白名单、shim 与 revision 集合检查均已删除。当前各 package 正常解析并编译；实际 SwiftPM 和 Swift 结果仍待 G1，不把旧 JSON 检查结果称为当前构建成功。

## 4. 当时采用的工程装配方式

| 源输入/owner | 生成动作 | 后置检查 |
|---|---|---|
| `tauri.ios.conf.json` | CLI 自动合并，minimum 26.0；Network/UserNotifications/CoreMotion/CoreLocation framework，Metal 由 CLI 模板提供 | minimum 与 Swift package 一致、真实编译/链接 |
| `Info.ios.plist` | CLI build 自动合并 | 单项 TaoScene/UIWindowScene/TaoSceneDelegate，multipleScenes=false；P3 新增 LAN、定位、运动用途与 local networking ATS |
| 官方 tao 0.37.1 | 已替代原 0.35.3 vendor 补丁；窗口恢复由 app `ios_window` 接入 | 06 保留旧修正来源，11–13 记录当前 R2；G1 不宣称系统回调已运行 |
| CLI `project.yml` | 保留 CLI 原始生成内容 | overlay 不改原 spec |
| 工程覆盖配置 | 在原始 spec 上追加 target 设置和隐私 resource | 检查重复装配和未知 target 格式 |
| 现有 `icons/ios/` | 生成 AppIcon catalog；旧尺寸/槽位等值断言已删除 | 保存实际编译资源供审查；完整图像/alpha 产品审计仍归 G2 |
| `ios/PrivacyInfo.xcprivacy` | 复制到 VCPResources，显式加入 resources build phase | 保存实际 bundle 文件和 privacy 供审查，不恢复静态等值门禁 |

锁定 CLI 通过 XcodeGen 构造工程；XcodeGen spec 可用 JSON、`include` 默认数组追加，因此 overlay 不复制完整上游模板，只添加本项目所需差异。[CLI 模板](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-cli/templates/mobile/ios/project.yml)、[XcodeGen include 规则](https://github.com/yonaskolb/XcodeGen/blob/2.44.1/Docs/ProjectSpec.md#include)。

Phase 2 的隐私输入声明应用容器文件元数据 `C617.1` 与计时 `35F9.1`，对应共享 Rust 文件/计时路径；当时 Swift 仅 Scene/Application、NWPathMonitor 和 Channel。P3 已新增 DiskSpace `85F4.1`，只描述本机实际样本展示，不能声称覆盖任意远程遥测或完整 App Store 隐私合规。Phase 3–6 按最终实际 API 与数据用途继续审计；真实资源装配仍待 G1/G2。[Apple API reasons](https://developer.apple.com/documentation/bundleresources/app-privacy-configuration/nsprivacyaccessedapitypes/nsprivacyaccessedapitypereasons)、[Apple 声明要求](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)。

## 5. 当时的归档流程

工作流选择 Xcode、安装 device Rust target 与 XcodeGen，并由 Tauri 生成初始工程。那一版实现再应用项目差异、生成 Xcode 工程并执行 debug device archive，收集 Info.plist、隐私清单及原生链接诊断。

## 6. 精简前本地历史证据（2026-10-08）

- 当时的本地输入检查通过，覆盖最低系统、单 Scene、隐私声明、依赖解析与 18 个图标 slot。
- 当时的 8 项 Python 回归通过，覆盖工程装配、路径约束和 Swift 锁定输入。
- `git diff --check`：通过。
- workflow YAML：使用本机已有 PyYAML 解析通过，确认仅 workflow_dispatch；未触发远端执行。
- XcodeGen 官方 `2.44.1/xcodegen.zip`：只读下载计算 SHA-256，为 `a2e905fb68446e9bb4008cdfe2e13e3f176d0cbcca828b71770f8e53fca91b73`，未执行其中二进制。

以上是该提交在 Windows 上取得的本地结果。该轮没有执行 Apple 编译。
