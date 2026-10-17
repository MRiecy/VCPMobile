# AGENTS.md — VCP Mobile 移动端原生插件

> 本文件适用于 `src-tauri/plugins/vcp-mobile/**`。它继承仓库根规则，只补充移动端原生能力边界内的代码结构、通信路径与修改闭环。

---

## 1. 子系统定位

`tauri-plugin-vcp-mobile` 实现 VCP Mobile 的 Android 原生能力，并接入 iOS Swift 生命周期、通知、设备、传感器、文件媒体和 WebKit。它接收 Vue 或 Rust 核心发出的设备意图，经 Rust 插件边界进入对应原生 owner，再把调用结果或原生事件送回实际消费者。后台 adapter 继续分期实施，需与 host 开发分支显式区分；非 Android fallback 不代表 iOS 可用，也不建立桌面产品能力。

Android、iOS、host 的插件分支显式隔离。尚未接通的原生调用如有实际消费者，应返回符合当前原因的不可用结果；不为形式完整增设占位接口。固定状态与无副作用成功仅用于排除 Android/iOS 的 host 开发 fixture，不能借作 iOS 实现。

### 1.1 正常调用链

```text
Guest JS 包装 ─┐
Vue 直接 invoke ┴─> Rust 注册命令 ─┐
Rust 核心内部调用 ─> Rust 公开入口 ─┼─> PluginHandle / JNI
                                    v
                 VcpMobilePlugin ─> Manager / Service / CLI Host ─> Android
                                    |
                                    └─> Result / trigger / CustomEvent
```

### 1.2 代码结构

| 路径 | 职责 |
| ---- | ---- |
| `src/lib.rs` | 组合插件、注册前端可调用命令、注入 `VcpMobileState` |
| `src/events.rs` | Rust 原生 Channel 订阅、生命周期/网络事件类型、后台服务失败通知与快照收集；真实 Events 注册失败保留至进程结束，Guardian/Helper 取走 |
| `src/ios.rs` | Swift 调用与事件注册；原生 receiver 持有至最终结果。授权等长等待和工具查询由调用者移入 blocking worker，事件订阅使用独立异步接收任务 |
| `ios/Package.swift`、`ios/Sources/VcpMobilePlugin.swift` | Swift product/初始化符号、主队列 app/Scene 观察与 NWPathMonitor；不拥有业务状态 |
| `ios/Sources/NativeNotifications.swift`、`NativeNotificationSceneBridge.swift` | 真实通知授权、发送、前台展示和点击快照；在插件初始化时接入锁定 Tao Scene 入口并完整转发原窗口实现 |
| `ios/Sources/NativeDevice.swift`、`NativeSensors.swift` | 设备/电量/空间/网络快照、单 owner 有界传感器采样；缺失字段保留原因，退后台停止采样 |
| `ios/Sources/NativeClipboard.swift` | 前台系统剪贴板读写，UIKit 操作在主队列执行 |
| `ios/Sources/NativeBackgroundExecution.swift` | 进程级任务租约；Stream 尽力静音音频、Sync/Prerender 前台常亮，音频中断不产生业务终态 |
| `ios/Sources/NativePresentation.swift`、`NativeFiles.swift` | 主 Scene 单操作呈现；picker 按调用 WebView/Scene 先登记 reservation 再启动，取消可撤销未启动登记；Files/PHPicker、Quick Look、导出与活动面板的文件持有至原生终态 |
| `ios/Sources/NativeImages.swift`、`NativeCamera.swift` | 串行 ImageIO 下采样、SDR 派生与真实编码，PhotoKit add-only、AVCapturePhotoOutput 有界静态拍照 |
| `android/src/main/.../NativeFileImages.kt` | 按用途预算下采样、EXIF 方向和 JPEG/PNG 编码；外部输入保持原件，头像工作副本单独派生 |
| `ios/Sources/NativeWebKit.swift`、`NativeShareInbox.swift` | WebView 几何/浏览缓存操作，及主 App 的 App Group 收件位置桥接 |
| `src/screen.rs` | 经 WebView JNI 进入 `ScreenKeepOnArbiter` |
| `src/stream.rs` | 请求独立的前台保活租约与 helper service 的 Rust 边界 |
| `src/system.rs` | 承接权限、文件、设备状态、更新与系统能力调用 |
| `src/webview.rs`、`android/src/main/.../NativeWebViewResume.kt`、`ios/Sources/NativeWebViewResume.swift` | 双端视觉恢复握手；原生持有单份旧画面遮罩并等待绘制提交，Vue 只确认当前文档布局就绪，不改变业务生命周期或主题底色 |
| `src/cli.rs` | 承接 Rust 核心到 Android CLI process / PTY 的异步桥接 |
| `android/src/main/.../VcpMobilePlugin.kt` | Kotlin 命令入口与插件 attach/destroy 编排 |
| `android/src/main/.../{service,cli}/` 与各 Manager | 执行独立的原生生命周期、后台服务和平台操作 |
| `guest-js/index.ts` | 提供被显式导出的前端包级调用 |
| `build.rs`、`permissions/`、`AndroidManifest.xml` | 声明命令生成、Tauri 授权和 Android 组件连接 |

---

## 2. 命令与通信约定

### 2.1 前端可调用命令

新增或删除前端 `plugin:vcp-mobile|...` 命令时，以 `src/lib.rs` 的实际注册入口为起点，同步 `build.rs`、相应 permission package 与所需 Kotlin 方法。`guest-js/index.ts` 只为确实使用包级 API 的调用方提供包装，不要求镜像全部注册命令。

Kotlin 中存在 `@Command` 只表示它可被原生插件桥调用，不代表已经向 Vue 公开；不得据此扩张前端 API。

应用授权另由 `src-tauri/capabilities/` 决定。`android-main.json` 逐项授予实际前端消费者需要的插件命令；`ios-main.json` 随已实现的 adapter 增补。`main` 以外当前没有生产窗口消费者，不授予遗留 `vcp-portal-*` 插件权限。Rust 内部调用不经过前端 ACL，仍必须保持自身的平台分支与资源所有者。

### 2.2 Rust 内部入口

Rust 核心可以直接调用 `screen`、`stream`、`system` 或 `cli` 暴露的函数。修改前先从实际调用者追踪到 `PluginHandle.run_mobile_plugin`、异步桥或 JNI 终点；内部能力不得为了形式一致而注册成前端命令。

### 2.3 原生事件与生命周期

`PluginHandle`、JNI、`plugin.trigger` 与 `evaluateJavascript` 是不同通道。事件变化必须同时追踪生产者和真实接收者；插件生命周期变化必须覆盖 `init`、`load`、`onDestroy` 以及对应 Manager、Service 或 Host 的建立与释放。

---

## 3. 修改与完成边界

跨层字段、方法名、返回值或事件必须沿生产调用链一次改完，并保留一个活动入口和一个实际所有者。长生命周期锁、Service、进程或 PTY 状态归当前持有它的原生组件管理，调用层只提交意图并消费结果，不复制第二套状态。

Rust 边界变化在 `src-tauri/` 运行 `cargo check --locked -p tauri-plugin-vcp-mobile`；涉及前端类型或跨层调用时从仓库根运行 `pnpm check`。Kotlin、Manifest 或 Service 变化还需编译实际 Android 插件目标。测试只能作为额外执行结果，不能定义生产代码应有行为。

Swift 接入时同步核对 `ios_path`、SwiftPM product、初始化 binding、selector、DTO、Channel 与资源归属；UI 操作显式在主队列执行，领域状态仍归 Rust。Apple 编译/链接按根规范集中在手动 Actions G1/G2，日常源码审计记录具体目标分支与待办。

`NativeNotificationSceneBridge` 依赖锁定 Tao 的 `TaoSceneDelegate` 与 `scene:willConnectToSession:options:`。升级框架时核对类注册先于插件初始化、Objective-C ABI 和原 IMP 转发；不能把 hook 移到 WebView load 或 READY 后，那里会遗漏冷启动参数。通知 pending 是最近一次点击快照，读取不表示 WebView 已接收，Vue 按原生 sequence 去重。

当前 iOS 只在 Rust bootstrap 安装一次长期 Channel；Swift WebView 替换复用同一 Channel 并更换观察者 generation。锁定 Tauri 的原生 Channel 注册表没有删除入口，不能通过反复创建 Channel 实现重试。事件注册最多一个在途，由 READY 后的独立订阅任务等待最终 ack，迟到 reject、解析或协议错误仍报告真实失败，不阻塞核心启动。

两平台真实 Events 注册失败没有进程内重试入口，由 `ServiceFailureNotices` 留存并经 `collect_service_failure_notices` 重放给新 WebView；Guardian/Helper 仍按原语义取走。当前 WebView 的一次告知由前端通知 Store 管理。真正订阅失败需完全关闭应用后重新打开；未来增加注册恢复成功路径时，必须同步调整故障留存期限。

**完成**：实际消费者、Rust 边界、Kotlin 所有者、平台声明及返回通道保持一致，并分别说明已取得的 host、Android 构建或设备证据。
