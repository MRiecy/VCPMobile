# Android 功能分包与资源所有权

本文记录 Android 原生插件的代码导航与迁移约束。当前生产源码是实现事实；本文不替代调用链、构建或设备证据。

## 1. 单插件与功能包

Android 保留 `tauri-plugin-vcp-mobile` 一个 Gradle library。所有 Kotlin 功能包位于 `android/src/main/java/com/vcp/mobile/`，目录与 package 一致。

| 包/入口 | 职责 |
|---|---|
| `VcpMobilePlugin.kt` | Tauri 命令与系统回调的薄入口，Activity/WebView 建立与销毁装配 |
| `ScreenKeepOnArbiter.kt` | manual、Guardian 与前后台状态的唯一屏幕常亮仲裁；固定 JNI 类名 |
| `runtime/` | root、file IO、OOM 调度隔离；任务接纳、拒绝与销毁 |
| `files/` | 系统选择/相机、公共导出、外部查看与分享暂存 |
| `sharing/` | 最新外部 Intent 接收、provider 读取与一次性文件交接 |
| `permissions/` | 权限快照、别名请求及 OEM 设置导航 |
| `lifecycle/` | 进程生命周期、原生 Channel、网络观察与初始快照 |
| `device/` | 电池/CPU/GPU/网络快照，以及有界传感器采样 |
| `media/` | 图片用途派生、音视频处理与统一原生取消/清理 |
| `webview/` | Insets、窗口截图、旧画面恢复握手与遮罩视图 |
| `notifications/`、`haptics/`、`clipboard/` | 通知、触感、剪贴板具体操作 |
| `root/`、`updater/` | Root 操作、APK 校验/安装条件与更新租约 |
| `cli/` | 运行时准备、进程/PTY、输出与进程组操作 |
| `sse/` | helper TCP、会话、离线邮箱、通知与锁资源 |
| `service/` | Manifest 注册组件及主进程 ForegroundGuardian 租约协调 |

参数 DTO 随所属功能共置。普通能力不建立接口/实现双层；只有真实替换点使用窄回调或可注入依赖。

## 2. 接入契约

`src/lib.rs` 仍注册 `com.vcp.mobile.VcpMobilePlugin`。锁定 Tauri 的 `PluginHandle` 扫描插件实例类及其父类，按方法名索引 `@Command`、`@ActivityCallback`、`@PermissionCallback`；普通组合对象及 Kotlin extension 不会被扫描。这三类入口留在插件并委托具体功能。

`@Command` 不代表已向 Vue 公开。前端命令以 Rust 注册表、`build.rs`、permissions、capabilities 与真实消费者为准。Rust 内部媒体、CLI 等调用继续核对各自的 `run_mobile_plugin` 字符串。

固定跨层符号：

- `screen.rs` 调用 `com/vcp/mobile/ScreenKeepOnArbiter.setManualRequested(Activity, boolean)`；保持 `@JvmStatic`。
- `cli.CliPtyNative` 与 `vcp_pty.cpp` 的静态 JNI 导出配对；文件拆分不改变包/类/方法 ABI。
- `service.StreamKeepaliveService`、`service.SseProxyService`、`service.VcpNotificationListenerService` 与 Manifest 配对。
- 主 Activity 的 `vcp-hardware-back` 继续交给 Vue 的覆盖层/页面返回链。

字段默认值、可空性、事件名和取消/失败表达随真实消费者核对，不建立无消费者的别名或兼容入口。

## 3. 文件操作与交接

| 操作 | 唯一所有者 | 结束与资源交接 |
|---|---|---|
| picker / 相机 | `files/NativeFilePicker` | 单槽、相机文件、取消与复制归同一操作。取消可先结束等待，但外部 chooser 回调前不释放槽位；成功交付后才移交输出 |
| 外部 Intent | `sharing/ShareIntentHandler` | 新 Intent 作废旧接收，持有 pending payload、取消与未交付 staging |
| 分享文件接纳 | `sharing/SharedFileIngestor` | 从 Handler 一次性 claim，校验/哈希后交给 Rust；不复制 current owner 或 staging registry |
| Downloads / Gallery | `files/NativeFileExports`、`PublicFileWriter` | 权限等待归导出；MediaStore pending 提交或失败删除，旧路径只回滚自己创建的目标 |
| 外部查看 / 分享 | `files/NativeOutgoingFiles` | 接收方没有可靠完成回调，暂存副本按原保留期与容量清理，独立于公共导出和 picker |

线程域与原有预算保持在实际操作附近。文件队列保持有界和串行；Root 或 OOM 调度不能占住文件执行器。权限工具不再维护另一套 pending Invoke。

Android 分享事件只唤醒读取最新快照；READY/回前台也主动拉取，Rust 持久化相应阶段后按 owner 与 payload revision 确认。物理 staging 名只包含有界机器标识，展示名独立保存。Rust 接手 uploads 后负责失败原件与部分目标清理，已发布 ready 的文件不随迟到错误删除。相册和下载共用 legacy 写权限等待器，API 26–28 授权后继续原保存操作；相册以有界 `ftyp` 品牌识别 HEIC/HEIF/AVIF，并按实际编码保存原始字节和后缀。

## 4. CLI 进程闭环

`CliProcessHost` 持有 prepared runtime、generation、进程注册表、控制/查询/取消执行器，以及进程终态。`CliPtyHost` 通过 Host 的 `terminalLaunchSpec` 取得同一运行时，不建立第二份准备状态。

- `CliProcessModels`：IPC 参数。
- `CliRuntimeInstaller`：资产校验、原子安装与目录准备。
- `CliLaunchSpec`：命令、环境、指纹与握手解析。
- `CliArtifactOutput`：共享 stdout/stderr 预算与输出 drain；达到保存预算后仍读取到 EOF。
- `OwnedProcessGroup`：身份检查、进程组信号与有界等待，无独立进程注册表。

进程组消失、读取线程结束、产物关闭、临时目录回收与租约释放由 Host 协调同一次终态。Activity 销毁不关闭进程级 CLI owner。

## 5. SSE helper

`service/SseProxyService` 保持 Manifest 的 `:helper` 进程边界，只承担前台服务与 Android 生命周期接线。`sse/SseSessionCoordinator` 持有 listener、发现端口、admission、会话注册表、上游、恢复/控制与终态交接。

- `ClientConnection` 持有有界写队列、写超时和 socket；关闭先中断 socket，避免被缓冲 flush 阻塞。
- `StreamSession` 持有正文、generation、当前消费者、上游、终态及交接任务。
- `SseProtocol` 负责 framing、正文切块与有限通知预览。
- `CompletedMailbox` 负责原子提交离线结果，不自行调度重试。
- `RequestBodyFile` 原子认领 Rust 准备的请求正文，分块上传并在已进入的写入结束后释放；请求正文不再嵌入控制 JSON。最终 HTTP 请求预算为 128 MiB，控制/响应帧的限额独立管理。
- `SseNotifications`、`SsePowerLocks` 处理原生资源，不维护会话副本。

恢复时替换消费者并排入快照与实时追加保持同一个 session monitor。ACK、落盘与移除保持既有序列；端口发布、停止接纳与关闭使用同一生命周期 owner。

保持原有 8 路接收、完整正文、5 秒 ACK 和每 generation 两次发布重试，不新增正文配额、截断或容量驱逐。helper 的接收/交接锁与主进程 Guardian 的消费者租约分别管理。

## 6. 窗口与进程事件

`LifecycleBridge` 使用 `ProcessLifecycleOwner`，通过窄 sink 发送 phase/nativeSequence/snapshot，序列跨插件实例延续。`NativeEvents` 安装唯一原生 Channel 并回放 lifecycle/network，再完成注册；网络监听与注销也归此处。

Insets、分享、权限、文件进度和 Android 通知点击保留各自 DOM CustomEvent。它们不与 Rust Channel 合成统一事件队列。

`NativeWebViewResume` 仍独占 attachment/cycle/token、截图与绘制屏障、显隐决策和清理。`ResumeCoverView` 仅负责原生遮罩呈现。Activity 时序与进程生命周期不同，不合并其状态机。

## 7. 已删除的停用路径

旧浮窗、独立页面/Store/设置组件、本地 HTTP/WebSocket 服务、临时助手对话/归档桥及无消费方配置字段已移除。主应用 `core/stores/assistant.ts` 继续负责智能体/群组列表。

旧 `processImage/MediaBridge`、仅测试引用的 picker staging 命名 helper、无消费者的插件静态实例访问已移除。当前图片派生仍经 `deriveImage`，分享仍进入主应用，测试不要求保留已失效路径。

## 8. 验证范围

核心/跨层变化运行 `pnpm check`；插件 Rust 修改运行对应 Cargo 检查。Kotlin 变化编译实际 `:tauri-plugin-vcp-mobile:compileDebugKotlin`，执行相关 `testDebugUnitTest`；JNI/C++ 修改还需要 arm64 编译/链接。

回归聚焦迟到回调、取消/销毁、队列拒绝、文件交接/回滚、输出预算耗尽后 drain、消费者替换、ACK/落盘竞争和端口关闭。既有测试可迁到新的真实 owner，不为维持反射路径暴露旧状态。

JVM 测试不能证明 Android 真实进程信号、PTY、OEM 前台服务或画面上屏。设备操作只使用现有 `pnpm android:debug:*` 与 `com.vcp.avatar.debug`，分别记录实际取得的证据。

### 2026-10-18 本次执行结果

- `pnpm check`：通过 Vue/TypeScript 与 host Rust 检查。
- `cargo check --locked -p tauri-plugin-vcp-mobile`：通过插件 host 检查。
- Android 插件 `compileDebugKotlin` 与 `testDebugUnitTest`：通过，12 个 suite、77 项测试，零失败/跳过。最终运行使用临时参数 `'-Pkotlin.incremental=false'`，避开 Windows 跨盘依赖的 Kotlin 增量缓存问题，未修改 Gradle 配置。
- 浮窗清理关联的 HTML/RAG 渲染与设置/配置备份测试：4 个文件、42 项通过。
- `git diff --check`：通过。命令/回调、DTO、JNI 和 CLI/SSE 所有权另经源码交叉审查；新增弱目标 sink 保持原 Activity 释放语义。

未进行真机、APK 安装或 Apple 构建；这些结果不由 host/JVM 检查替代。

## 9. 2026-10-18 全链路复审与修复

本次重新对照重构前 `12adb6cf`、锁定 Tauri Android 实现及 Rust/Vue 消费链，检查跨层命令、生命周期、取消和资源交付。下列问题在该基线中已经存在，分包后仍保留；修复不改变命令/DTO/JNI 名称、现有文件预算或 SSE 正文策略。

| 边界 | 发现与修复 |
|---|---|
| Activity 与插件交接 | Tauri 向全部插件转发 Activity 销毁，也会替换同名插件。插件按实际 Activity 身份幂等释放；原实例通过自己的 Application 回调完成收尾。Guardian 只解绑对应 listener，旧实例不能清掉新实例注册；某个 owner 释放失败不阻断其余清理 |
| 重挂与迟到工作 | 新 WebView/文档重放 Insets，不沿用旧文档去重。延迟 detach 不移除新生命周期订阅；关闭后的 load/Intent、传感器采样和截图不再重新启动资源 |
| picker 与 provider | 回调核对原 Invoke；普通取消仍占 chooser 槽直到真实回调。provider 取消和输入关闭独立在后台执行，避免一个阻塞另一个；销毁清理相机文件，迟到相机结果再次回收本次路径 |
| 分享所有权 | 销毁使 owner 失效并取消读取，claim 与清理同步裁定文件归属。初始 Intent 只接纳一次；真实新 Intent 仍产生新 owner。启动目录清理在进程首次使用前串行完成，迟到初始化和替换 Handler 不得删除已经持有的 staging |
| 公共导出与外发 | EOF/提交前检查中断。外发在主线程最终确认 Activity 可用并 dispatch，失败回收未交付目录；成功 dispatch 后不因旧响应通道失效删除接收方副本 |
| 队列和结果交付 | runtime/CLI 关闭时结算尚未执行的请求；失效响应通道不阻断其他清理、不触发第二个终态。picker 未能交付的文件继续归本次操作回收 |
| CLI 失败与终态 | 只清理本次独占创建的输出，幂等指纹纳入实际影响环境的 sessionId。租约等待中断释放未交回租约；清理异常记录并附加到原失败，不能跳过后续资源或使已满足进程/reader终态的完成通知永久悬挂 |
| SSE 接纳与关闭 | Coordinator 持有全部已 accept Socket，包含尚未读取完整命令的连接；先登记完成清理再启动协程，销毁在锁外关闭。正常退出等待控制 ACK 交付，客户端登记与闲置定时器配对 |
| Bitmap 与 JNI | 截图 draw/encode 失败也在 finally 回收 Bitmap，编码失败不返回假成功。PTY 在创建 fd/子进程前分配 JVM 返回数组，避免分配失败后无法向 Kotlin 交付 native handle |

回归使用真实 loopback Socket、受控阻塞输入、Activity/Application 回调、真实 Window Insets 遍历及失效 Invoke 发送端；CLI 通过公开 Host 入口模拟安装结果和失败条件，路径遵循生产安装器的 canonicalFile 契约。涉及终态资源异常的个别测试直接触达私有边界，生产代码没有为测试添加兼容状态或反射入口。

源码核对涵盖 Rust 媒体取消后等待原生终态、文件源持有至导出返回、CLI attempt/generation、SSE 同代 ACK 与离线邮箱二次读取。未发现本轮分包导致的活动命令或 DTO 映射断裂。JVM 回归仍不代表真实 Android provider、编解码、PRoot/信号或绘制上屏结果。

本次复审最终验证：`pnpm check` 通过；Android 插件 Kotlin 编译通过；22 个 JVM suite、115 项测试通过，零失败/错误/跳过；`externalNativeBuildDebug` 的 arm64-v8a NDK 编译/链接通过；`git diff --check` 通过。最终编译产物已覆盖所有 Kotlin 源码和测试修改。没有执行真机安装、设备交互或 Apple 构建。
