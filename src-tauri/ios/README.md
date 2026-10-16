# Apple 工程输入与阶段诊断

> 2026-10-16：[首轮 Phase 3 诊断](https://github.com/MRiecy/VCPMobile/actions/runs/38884164944)已生成 arm64 无签名归档，但实际归档使用 Xcode 16.4 / SDK 18.5，与预检记录的 26.2 不符。现固定使用 Xcode 26.2 并切换 runner 的系统选择，修正后仍待 Apple 构建复验。

此目录保存生成树之外的可重放输入；`gen/apple/` 继续由锁定的 Tauri CLI 生成并忽略。[独立手动 workflow](../../.github/workflows/ios-g1.yml) 显示为 **iOS stage diagnostics**，保留原文件名并复用 G1 链，生成 unsigned device archive。它不是发布或用户安装入口。

## 使用方式

```text
# 手动 Actions 内依次执行（workflow 已连接）
python3 src-tauri/ios/scripts/g1.py prepare
python3 src-tauri/ios/scripts/g1.py build
python3 src-tauri/ios/scripts/g1.py summary
# 修改工程生成脚本后，可在本地验证文件写入行为
python -B src-tauri/ios/scripts/test_g1.py
```

在 GitHub Actions 选择 **iOS stage diagnostics → Run workflow**，选择分支；`phase` 默认 `phase-3`，`profile` 默认 `debug`，Xcode 固定为 `g1-inputs.json` 中的 26.2，不提供临时版本覆盖输入。阶段标签用于证据归属，不按阶段裁剪编译范围，也不自动标记阶段通过；任一阶段均编译该 revision 的完整当前应用。Release 选项供优化/链接问题诊断，仍不生成用户安装包。

workflow 需要先存在于默认分支，手动运行时可选择分支；见 [GitHub 手动触发说明](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)。已安装并认证 GitHub CLI 的环境也可使用：

```text
gh workflow run ios-g1.yml --ref main -f phase=phase-3 -f profile=debug
```

流程执行真实编译、链接和归档，保留输出供人工分析；没有独立的 `check` 命令，也不重复运行源码/配置断言。本地脚本测试覆盖 overlay 文件归属、真实子进程非零退出、安装失败与进程中断后的证据恢复，不模拟 Apple 编译成功；这些测试不在诊断 workflow 中重复运行。

## 维护输入

| 文件 | 所有权 |
|---|---|
| `../../.github/toolchain.env` | CI、Android Release 与 iOS 阶段诊断共用的 Node/pnpm 精确版本；各 workflow 在安装前读取 |
| `../tauri.ios.conf.json` | 最低 iOS/iPadOS 26.0；声明网络、通知、传感器以及 PhotosUI/Photos/ImageIO/CoreGraphics/AVFoundation/QuickLook/UniformTypeIdentifiers/CryptoKit framework；Metal 由锁定 CLI 模板提供 |
| `../Info.ios.plist` | 单 Scene、LAN/定位/运动/相机/相册用途、`.vcpcfg` 类型和主 App Group 标识；Tauri build 自动合并 |
| `g1-inputs.json` | 固定 Xcode 26.2 的路径、XcodeGen 下载地址和下载完整性摘要 |
| `PrivacyInfo.xcprivacy` | 文件元数据、计时和本机磁盘容量展示的 required reasons；后续原生 API/数据使用变化需按真实用途核对 |
| `scripts/g1.py` | 初始化证据、重放工程、真实 device 构建、归档收集及结果摘要 |
| [`share/`](share/README.md) | 独立 Share Extension 源码、Info、双 target App Group entitlement 和 Phase 6 重签输入 |

Swift plugin 的源文件仍归 `../plugins/vcp-mobile/ios/`。构建直接使用 Tauri/SwiftPM 的正常依赖流程，不拦截 `swift`，不维护构建包白名单，不向 registry 预置或改写 lock。Cargo/pnpm 的正常锁文件和 VCP Swift manifest 的依赖约束继续生效。

最低运行版本由用户确定为 26.0；本项目 Swift manifest 同步使用 `.iOS("26.0")`，保留原有 Swift tools 版本。G1 的 `IPHONEOS_DEPLOYMENT_TARGET` 直接读取 Tauri 配置，Xcode 仍由 `g1-inputs.json` 单独选择；提高最低版本不等于已经取得 Apple 构建结果。

Rust `swift-rs` 与 SwiftPM `SwiftRs` 是独立依赖。各 Swift root 按自己的 manifest 和原有 lock 解析；不再宣称所有 root 被额外脚本统一到同一 revision。解析、编译或链接出现具体错误时，根据真实构建输出处理。

## 工程重放

Actions 中先安装 device Rust target，执行 `pnpm tauri ios init --ci --skip-targets-install`，再执行 overlay。overlay 新增 `project.vcp.json`，通过 XcodeGen `include` 合并 CLI 的 `project.yml`；补充隐私 resource、device family、Share Extension target/embed dependency 和 G1 诊断选项，并从现有 `icons/ios/` 重建 AppIcon catalog。两个 target 的 minimum 读取 `tauri.ios.conf.json`；扩展版本读取 Tauri 产品版本与 iOS bundleVersion。对相同输入重复 overlay 不会累加 resource，也不改写 CLI spec。

`build` 仅允许在 GitHub Actions macOS 中执行，要求 `gen/apple` 尚不存在，避免覆盖已有工程。workflow 选择 runner，Node/pnpm 从共用 `toolchain.env` 读取。脚本先用 `sudo -n xcode-select --switch` 将临时 runner 的系统 Xcode 切到配置固定的 26.2，再记录工具版本并构建；同时保留 `DEVELOPER_DIR`，使直接调用和 Tauri 过滤环境变量后的子进程使用同一套工具。切换失败即停止，日志保存为 `xcode-select.log`；选择路径、可用 Xcode 和实际工具版本写入 manifest。编译目标仍为 device arm64，使用 `--no-sign`，并清除子进程环境的签名输入。下载摘要、解压路径/符号链接和 overlay 写入边界保护保留。iOS 专用 profile 和 Apple 环境设置不影响 Android 构建。

编译成功后用 `ditto` 保存实际 `.xcarchive.zip`，manifest 记录归档摘要；同时收集 bundle 文件列表、Info.plist、已有隐私清单及 `lipo`/`nm`/`otool` 原始输出。Phase 4 对实际 `.appex` 嵌入、分享入口、最低版本以及主/扩展版本和 App Group 字段一致性进行归档检查，失败使本次诊断失败；它不证明最终签名授权。普通诊断采集失败写入 `diagnosticErrors`，仍保留已生成归档；真实构建/归档收集失败则正常失败。

manifest 的 `status=built` 仅表示构建和归档收集成功，产物内容由人工结合源码与诊断判断。阶段诊断不自动关闭 G1，也不替代 G2 的结构检查和用户打包；不生成 `.ipa`。

## 诊断结果怎么读

1. 先看 Actions Summary：checkout commit、阶段/profile、Xcode/SDK、失败步骤，以及失败时最后一份日志的末尾摘要。
2. 下载 `ios-<phase>-<profile>-<sha>-<run>-<attempt>` artifact，读取 `ios-build-manifest.json` 和完整日志。`toolchain`、`rust-target`、`project-init`、`project-overlay`、`device-build`、`archive-collection` 分别记录结果；Swift、Rust、前端或链接的具体失败仍看真实 `build.log`。
3. 依赖安装在 Apple 构建之前失败时，仍有 `pnpm-setup.log` / `dependencies.log` 和 workflow step 结果。构建超时或被终止时，已落盘的 running 阶段在摘要中标记 interrupted；摘要/上传采用 `always()` 尽力收集，runner 丢失或整体 job 强制终止仍可能无法上传。
4. `generated-project/` 保存已生成的 XcodeGen spec、pbxproj、plist、配置、scheme 和解析锁等输入；成功归档另有 zip、SHA-256 和 bundle 诊断。文件采集只提供事实，不建立手写预期值门禁。
5. 将真实失败修回对应 owner，再对同一阶段集中复验；在该阶段记录 run URL、commit、工具版本和结论。编译通过不证明通知点击、权限回调或 SSE 后台持续运行已在设备上验证。

保留期为 14 天；同分支同 profile 的新运行取消旧运行。构建 step 上限 75 分钟，job 上限 90 分钟，为失败摘要和上传留出时间。只手动触发，不增加 push/PR/定时执行。

构建后只记录实际得到的结果；除预检版本外，还应读取归档 `Info.plist` 的 `DTXcodeBuild` / `DTSDKName` 和 Mach-O 的 SDK 记录，确认实际构建工具。清理范围见 [G1 流程精简记录](../../plan/vcpmobile-ios-compatibility-research/phase-2/10-G1流程精简.md)。
