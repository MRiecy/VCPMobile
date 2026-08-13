# VCPMobile iOS 兼容适配专项研究

> 已取得证据：`RESEARCHED / STATIC-AUDITED / DEPENDENCY-GRAPH-RESOLVED`
>
> 下一层 blocker：`MACOS-CI-BUILD-PENDING / SIMULATOR-PENDING / UNSIGNED-ARTIFACT-PENDING / COMMUNITY-DEVICE-UNVERIFIED`
>
> 研究日期：2026-08-13
>
> 仓库基线：`6298fcd`
>
> 本轮范围：初始架构审计、官方资料核验与研讨文档修订；未生成 Apple 工程，未修改产品代码，也未触碰工作区既有产品改动。

> 产品决策冻结：2026-08-13。本文档包后续章节中的历史候选若与本页“实验性产物定义”冲突，以冻结决策为准。

## 结论

VCPMobile 可以建设一个有实际价值的 **iPhone/iPad 实验性兼容版本**。它不是 Android 对等版，也不是正式签名、TestFlight 或 App Store 产品：GitHub Actions 的 macOS runner 负责编译可供后续自签名的未签名产物，并作为 Android GitHub Release 的非阻塞附属资产发布；用户自行完成签名、安装与临时续签，项目不接触其账号、证书或第三方签名工具。

核心能力采用“前台必达范围 + iOS/iPadOS 26 及以上尽力后台续跑”两级策略。若 `BGContinuedProcessingTask` 不可用、未获系统调度或被终止，就要求用户保持应用前台并在生成期间保持屏幕常亮；服务端不做配套改造。Tauri 2、Vue 3 和多数 Rust 业务模块从静态调用链看是复用候选，在 Apple compile/link 前仍不得量化为“已复用”。

本专项不支持以下结论：

- “增加一个 Rust target 就能发布 iOS”；
- “iOS 可以等价复刻 Android Foreground Service、`:helper` SSE 代理、Wake/Wi-Fi Lock”；
- “Root、跨应用通知监听、OEM 自启动白名单、程序化退后台存在 iOS 安全等价”；
- “Linux 静态检查、Safari 或 Simulator 通过即可证明真机兼容”；
- “当前 `not(android)` 分支可以直接充当 iOS 实现”；
- “未签名 iOS 产物可直接安装”或“项目保证第三方续签工具可用”；
- “使用 `max_output_tokens` 相除即可得到真实生成百分比”。

推荐的唯一主线是：

```text
同一 Vue UI / Pinia 状态
        ↓
稳定、平台中立的 Tauri IPC
        ↓
既有 Rust 业务 owner（生命周期、Chat、Sync、Distributed、文件）
        ↓
PlatformCapabilities + MobilePlatform adapter
        ├── Android Kotlin
        └── iOS Swift
```

Swift 只负责采集平台事实、请求权限、选择或转码文件和发送原生事件；不得建立第二套 Chat、Sync、Distributed 或恢复状态机。

## 产品路线裁决

| 路线 | 结论 | 原因 |
|---|---|---|
| Tauri 原生 iOS | **推荐** | 最大化复用现有 Vue/Rust/Tauri IPC；Swift 仅承担 iOS 平台适配 |
| PWA | 不作为本项目首期 fallback | 仓库没有 Manifest/Service Worker，前端广泛依赖 Tauri IPC；若做 PWA，它是独立架构项目 |
| Android 等价迁移 | `NO-GO` | iOS 平台不允许或不保证后台常驻、Root、跨应用通知监听、APK/IPA 自安装等语义 |
| iPhone/iPad 实验兼容产物 | `GO-EXPERIMENTAL` | 聊天、日记、设置、数据库、附件和前台同步可落地；不承诺正式签名与真机验收 |
| macOS/Catalyst/桌面端 | `OUT-OF-SCOPE` | macOS 只作为 CI 构建宿主；产品仅覆盖手机和平板 |

## 41 条插件命令审计结果

| 类别 | 数量 | 含义 |
|---|---:|---|
| `R` | 4 | 现有 Rust 或已引入的跨平台插件路径基本可复用 |
| `S` | 6 | 需要新增 Swift，有公开 API 可实现接近原语义 |
| `L` | 19 | 只能部分实现，必须修改 DTO、UI 或恢复策略 |
| `X` | 12 | 当前命令语义在 iOS 无安全等价，必须退出正常能力面 |
| **合计** | **41** | 与插件 `build.rs`、Rust handler、权限清单一致 |

完整逐命令结论见 [02-平台能力与41条命令兼容矩阵.md](./02-平台能力与41条命令兼容矩阵.md)。

## 文档导航

| 文档 | 内容 | 主要用途 |
|---|---|---|
| [00-研究契约与证据账本.md](./00-研究契约与证据账本.md) | 研究范围、证据等级、当前工具链、官方来源 | 防止把理论结论升级成实现或验收结论 |
| [01-架构现状与跨平台改造边界.md](./01-架构现状与跨平台改造边界.md) | Rust/Tauri 复用面、cfg/fallback、单一 owner 架构 | 决定应该改哪里、坚决不重写什么 |
| [02-平台能力与41条命令兼容矩阵.md](./02-平台能力与41条命令兼容矩阵.md) | 41 条命令逐项分类、隐藏原生面、Capability DTO | 产品功能范围和原生插件范围的主账本 |
| [03-WebKit与iPhone-iPad交互适配.md](./03-WebKit与iPhone-iPad交互适配.md) | safe area、键盘、手势、弹层、可访问性、强渲染 | 前端与 UI/UX 适配依据 |
| [04-生命周期后台网络与同步策略.md](./04-生命周期后台网络与同步策略.md) | 前后台、SSE、iOS 26 持续处理、真实进度、LAN/ATS | 尽力后台与前台常亮回退契约 |
| [05-文件媒体分享通知与设备能力.md](./05-文件媒体分享通知与设备能力.md) | Files、Rust 文档解析、ImageIO/PhotoKit 与明确砍项 | 附件 P0 和平台能力边界 |
| [06-安全隐私签名与App-Store发布.md](./06-安全隐私签名与App-Store发布.md) | Info.plist、entitlements、未签名产物、自签边界 | 实验性分发与安全责任清单 |
| [07-CI测试矩阵与无设备验证边界.md](./07-CI测试矩阵与无设备验证边界.md) | Linux、GitHub macOS、Simulator、社区真机分层 | 证据门禁和非阻塞 CI 设计 |
| [08-分期实施路线图与验收门禁.md](./08-分期实施路线图与验收门禁.md) | Phase 0–5、退出条件、粗估与回滚边界 | 获批后的施工路线 |
| [09-Magi综合裁决与待决策清单.md](./09-Magi综合裁决与待决策清单.md) | 三贤者裁决、D1–D19 冻结值、No-Go 条件 | 决策真源与后续变更入口 |

## 当前证据与 blocker

| 类型 | 标签 | 当前状态/含义 |
|---|---|---|
| evidence | `RESEARCHED` | 完成；已核验 Tauri/Apple 官方公开约束 |
| evidence | `STATIC-AUDITED` | 完成；已核对当前代码调用链、cfg、插件命令、CI 与发布配置 |
| evidence | `DEPENDENCY-GRAPH-RESOLVED` | 完成；`cargo tree --locked --target aarch64-apple-ios` 可解析，**不代表编译** |
| blocker | `MACOS-CI-BUILD-PENDING` | 尚未在 GitHub macOS runner 执行 `tauri ios init`、Apple target check 或 Swift 编译 |
| blocker | `SIMULATOR-PENDING` | 尚未启动 iPhone/iPad Simulator |
| blocker | `UNSIGNED-ARTIFACT-PENDING` | 尚未生成带 SHA-256 与构建清单的实验性未签名产物 |
| accepted gap | `COMMUNITY-DEVICE-UNVERIFIED` | 项目没有 iOS 设备；真机行为与性能由用户/社区自愿回报，不阻塞实验产物 |
| accepted gap | `PERFORMANCE-NOT-EVALUATED` | 不做 iOS 性能认证、基准或 soak 发布门禁；保留现有硬上限与静态正确性检查 |

任何文档、Issue、PR 或发布说明都不得使用“已完成 iOS 真机兼容”“后台可靠”“设备验证通过”或“可直接安装”。允许的准确表述是“实验性未签名 iPhone/iPad 兼容产物；需用户自签；设备与性能未由项目验证”。

## 已冻结的首个实验产物定义

> **VCPMobile iOS/iPadOS 实验性未签名基线：支持 iPhone/iPad 当前窗口宽度响应、启动与配置、核心 Chat/Diary/Settings/SQLite、前台 Sync、文档/文本/图片附件；iOS/iPadOS 26 及以上尝试使用 `BGContinuedProcessingTask` 延续用户发起的生成，系统不接纳或终止时回退为前台常亮等待。全部能力 fail-closed，Android 专属入口隐藏。CI 只证明编译/Simulator；产物需用户自签，项目不提供更新和正式设备验收。**

以下能力不进入首个产物：语音录音、视频提取、incoming Share Extension、APNs、系统通知、Root/通知监听、完整设备遥测、APK/IPA 自更新、macOS/Catalyst/桌面端。相机、相册写入、outgoing share 与 Distributed 只有后续确有需求时再独立加入。

## 下一轮施工前只需细化的非范围问题

产品范围已冻结，不再等待 Apple Team、正式签名或项目自有真机。施工时只需记录：GitHub runner/Xcode 固定版本、最低可编译系统版本、iOS 26 availability 分层、LAN HTTP/WS 的现有需求，以及 iPad 弹层/键盘的具体实现选择。完整决策记录见 [09-Magi综合裁决与待决策清单.md](./09-Magi综合裁决与待决策清单.md)。
