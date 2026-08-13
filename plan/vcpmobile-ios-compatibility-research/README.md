# VCPMobile iOS 兼容适配专项研究

> 已取得证据：`RESEARCHED / STATIC-AUDITED / DEPENDENCY-GRAPH-RESOLVED`
>
> 下一层 blocker：`MACOS-BUILD-PENDING / SIMULATOR-PENDING / DEVICE-PENDING / DISTRIBUTION-PENDING`
>
> 研究日期：2026-08-13
>
> 仓库基线：`6298fcd`
>
> 本轮范围：只读架构审计、官方资料核验和实施方案；未生成 Apple 工程，未修改产品代码，也未触碰工作区既有的 `SlidePage` 改动。

## 结论

VCPMobile 可以建设有实际价值的 iOS/iPadOS 版本，但前提是将目标定义为**前台优先的核心客户端，并对平台能力诚实降级**。Tauri 2、Vue 3 和多数 Rust 业务模块从静态调用链看是复用候选；在 Apple compile/link 前不得量化为“已复用”。当前主要阻断在 Android 专属原生插件、错误的非 Android fallback、后台常驻假设、APK 更新、Apple 工程与签名发布链。

本专项不支持以下结论：

- “增加一个 Rust target 就能发布 iOS”；
- “iOS 可以等价复刻 Android Foreground Service、`:helper` SSE 代理、Wake/Wi-Fi Lock”；
- “Root、跨应用通知监听、OEM 自启动白名单、程序化退后台存在 App Store 安全等价”；
- “Linux 静态检查、Safari 或 Simulator 通过即可证明真机兼容”；
- “当前 `not(android)` 分支可以直接充当 iOS 实现”。

推荐的唯一主线是：

```text
同一 Vue UI / Pinia 状态
        ↓
稳定、平台中立的 Tauri IPC
        ↓
既有 Rust 业务 owner（生命周期、Chat、Sync、Distributed、文件、更新）
        ↓
PlatformCapabilities + MobilePlatform adapter
        ├── Android Kotlin
        └── iOS Swift
```

Swift 只负责采集平台事实、请求权限、选择或转码文件和发送原生事件；不得建立第二套 Chat、Sync、Distributed 或恢复状态机。

## 产品路线裁决

| 路线 | 结论 | 原因 |
|---|---|---|
| Tauri 原生 iOS | **推荐** | 最大化复用现有 Vue/Rust/Tauri IPC；可以接入 Swift 原生能力与 App Store 发布链 |
| PWA | 不作为本项目首期 fallback | 仓库没有 Manifest/Service Worker，前端广泛依赖 Tauri IPC；若做 PWA，它是独立架构项目 |
| Android 等价迁移 | `NO-GO` | iOS 平台不允许或不保证后台常驻、Root、跨应用通知监听、APK/IPA 自安装等语义 |
| 前台核心客户端 | `GO-WITH-DECISIONS` | 聊天、日记、设置、数据库、前台同步、文件/照片等可按阶段落地 |

## 41 条插件命令审计结果

| 类别 | 数量 | 含义 |
|---|---:|---|
| `R` | 4 | 现有 Rust 或已引入的跨平台插件路径基本可复用 |
| `S` | 6 | 需要新增 Swift，有公开 API 可实现接近原语义 |
| `L` | 16 | 只能部分实现，必须修改 DTO、UI 或恢复策略 |
| `X` | 15 | 当前命令语义在 App Store iOS 无安全等价，必须退出正常能力面 |
| **合计** | **41** | 与插件 `build.rs`、Rust handler、权限清单一致 |

完整逐命令结论见 [02-平台能力与41条命令兼容矩阵.md](./02-平台能力与41条命令兼容矩阵.md)。

## 文档导航

| 文档 | 内容 | 主要用途 |
|---|---|---|
| [00-研究契约与证据账本.md](./00-研究契约与证据账本.md) | 研究范围、证据等级、当前工具链、官方来源 | 防止把理论结论升级成实现或验收结论 |
| [01-架构现状与跨平台改造边界.md](./01-架构现状与跨平台改造边界.md) | Rust/Tauri 复用面、cfg/fallback、单一 owner 架构 | 决定应该改哪里、坚决不重写什么 |
| [02-平台能力与41条命令兼容矩阵.md](./02-平台能力与41条命令兼容矩阵.md) | 41 条命令逐项分类、隐藏原生面、Capability DTO | 产品功能范围和原生插件范围的主账本 |
| [03-WebKit与iPhone-iPad交互适配.md](./03-WebKit与iPhone-iPad交互适配.md) | safe area、键盘、手势、弹层、可访问性、强渲染 | 前端与 UI/UX 适配依据 |
| [04-生命周期后台网络与同步策略.md](./04-生命周期后台网络与同步策略.md) | 前后台、SSE、Sync、Distributed、APNs、LAN/ATS | 关闭“后台等价迁移”误区 |
| [05-文件媒体分享通知与设备能力.md](./05-文件媒体分享通知与设备能力.md) | Files/PhotoKit/Share Extension/录音/传感器 | 平台服务分期依据 |
| [06-安全隐私签名与App-Store发布.md](./06-安全隐私签名与App-Store发布.md) | Info.plist、entitlements、PrivacyInfo、签名、审核 | 发布前合规与责任清单 |
| [07-CI测试矩阵与无设备验证边界.md](./07-CI测试矩阵与无设备验证边界.md) | Linux、macOS、Simulator、真机、TestFlight 分层 | 证据门禁和 CI 设计 |
| [08-分期实施路线图与验收门禁.md](./08-分期实施路线图与验收门禁.md) | Phase 0–5、退出条件、粗估与回滚边界 | 获批后的施工路线 |
| [09-Magi综合裁决与待决策清单.md](./09-Magi综合裁决与待决策清单.md) | 三贤者裁决、优先决策包、No-Go 条件 | 下一轮共同讨论入口 |

## 当前证据与 blocker

| 类型 | 标签 | 当前状态/含义 |
|---|---|---|
| evidence | `RESEARCHED` | 完成；已核验 Tauri/Apple 官方公开约束 |
| evidence | `STATIC-AUDITED` | 完成；已核对当前代码调用链、cfg、插件命令、CI 与发布配置 |
| evidence | `DEPENDENCY-GRAPH-RESOLVED` | 完成；`cargo tree --locked --target aarch64-apple-ios` 可解析，**不代表编译** |
| blocker | `MACOS-BUILD-PENDING` | 尚未执行 `tauri ios init`、Apple target check 或 Swift 编译 |
| blocker | `SIMULATOR-PENDING` | 尚未启动 iPhone/iPad Simulator |
| blocker | `DEVICE-PENDING` | 尚无相机、键盘、后台、局域网、内存等真机证据 |
| blocker | `DISTRIBUTION-PENDING` | 尚无上传、处理、TestFlight、App Review 或发布证据；子状态按 07 的 canonical 序列记录 |

在 `DEVICE-PENDING` 关闭前，任何文档、Issue、PR 或发布说明都不得使用“已兼容 iOS”“后台可靠”或“设备验证通过”。

## 建议首发定义

若后续决策通过，建议将首个可交付里程碑定义为：

> **VCPMobile iOS 前台核心 Beta 基线：支持冻结设备范围内的 iPhone/iPad 当前窗口宽度响应、核心聊天/日记/设置/数据库、现有 App 内 Toast/通知中心与前台同步；后台切换保证确定性收口，有已验证的服务端 cursor 才恢复，否则明确终止；Android 专属能力显式不可用。Document Picker、camera、PHPicker、PhotoKit 保存、外部打开/outgoing share、录音、系统 local notification、Distributed 等只有被 Phase 0 的版本化 MVP manifest 逐项选中并通过各自门禁时，才并入该 Beta。**

不应将 Share Extension、APNs 服务端恢复、长期后台增强、完整设备遥测或所有 Android 命令对等塞入首个里程碑。

## 下一轮先讨论什么

下一轮不应先讨论 Swift 文件怎么写，而应先冻结以下产品决策：

1. 是否接受“前台优先 + 确定性收口 + 有协议才恢复”，而非后台 SSE 常驻；
2. 首发设备和最低系统版本；
3. 版本化 MVP manifest：核心必选、按决策并入、默认延期，以及 15 条 `X` 能力的退出策略；
4. LAN HTTP/WS 是否为硬需求，还是可以要求 TLS/WSS；
5. incoming Share Extension、APNs 是否进入首发；
6. Apple Team、Bundle ID、Mac runner、真机和发布责任人；
7. 可缩放、44pt 命中区、iPad 多窗口/外接输入的验收边界。

专项的完整建议与默认选项见 [09-Magi综合裁决与待决策清单.md](./09-Magi综合裁决与待决策清单.md)。
