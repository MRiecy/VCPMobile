# AGENTS.md — VCP Mobile Rust 核心与 Tauri IPC

> 本文件适用于 `src-tauri/src/**`。它继承仓库根规则，只补充 Tauri 启动装配、Rust 业务领域、IPC 总闸、状态所有权与异步执行边界；当前行为必须从本目录生产代码及其实际调用方恢复。

---

## 1. 子系统定位

`src-tauri/src/` 是 Project Avatar 的 Rust 应用核心。它在 Tauri 进程中建立长期状态与生命周期，把 Vue invoke 分发给对应业务领域，协调持久化、VCP 网络、同步、更新和分布式能力，并通过 Result、事件或 Channel 把结果送回调用方。

iOS 分期适配复用这些领域 owner；Android、iOS adapter 与 host 开发分支需明确区分。Swift 持有 Apple 平台资源，Chat/Sync/SQLite/CAS 与业务终态继续由 Rust 拥有；是否已实现以当前目标 cfg 和实际调用链为准。

### 1.1 正常执行链

```text
main.rs ──> vcp_mobile_lib::run()
                    |
                    v
       lib.rs setup / app.manage / bootstrap / plugin / native event
                    |
Vue invoke ──> commands.rs IPC 总闸 ──> vcp_modules / distributed owner
                                             |
                                             ├─> persistence / network / plugin
                                             └─> Result / Event / Channel ──> Vue
```

命令到达只表示进入 Rust 调用链；调用返回、事件发出、持久化完成和外部系统生效是不同阶段。判断结果时沿生产代码确认实际 owner 与提交边界，不从前端请求结束或日志文本推断业务完成。

---

## 2. 代码结构

```
src-tauri/src/
├── main.rs                     # host scaffold，只调用 library 入口
├── lib.rs                      # Tauri 组合根、启动状态、插件和原生事件装配
├── commands.rs                 # app command 注册表与 JavaBridge 防爆栈总闸
├── vcp_modules/
│   ├── agent/、group/、chat/   # 助手、群组、会话、消息与内容处理
│   ├── persistence/            # 数据库状态、写队列与消息仓储
│   ├── infra/                  # 生命周期、设置、文件、媒体与 VCP 连接设施
│   ├── cli/、sync/、updater/   # CLI 运行时、同步与更新生命周期
│   └── diary/、forum/、mail/、taskcenter/、logcenter/、agentmgr/
│                                # 面向各业务域的 Rust 服务入口
└── distributed/                # 分布式客户端、工具注册、遥测与设备工具
```

`vcp_modules/mod.rs` 声明物理领域并提供现有扁平 re-export；具体状态、算法和副作用留在所属领域。目录和 re-export 只负责可达性，不应承载第二份业务实现。

---

## 3. 启动装配与状态所有权

### 3.1 Tauri 组合根

`lib.rs` 负责建立 Builder、注册启动期 `app.manage(...)` 状态、挂载插件、启动 bootstrap、接收原生网络与生命周期事件，并在最终组合点安装 `commands::app_command_handler!()`。业务规则不得堆回 `lib.rs`；新增行为先进入实际领域 owner，再由组合根连接生命周期。

需要在 bootstrap 前即可调用的纯内存状态必须先于异步引导注册。依赖数据库、文件或外部连接的状态按其实际初始化阶段建立；不得用默认成功对象掩盖尚未完成的资源初始化。

`infra/ios_window.rs` 在 iOS RunEvent 回调中同步处理隐式退出与单主窗口恢复。它以 UIKit 唯一 connected Scene 的身份/role、Tauri 现有 `main` WebView 和 builder 的创建前同名注册检查为依据，复用原 `main` 配置并核对实际 Window session；不维护窗口镜像状态，不重跑 Rust bootstrap。绑定异常仅报告，不循环销毁重建。此处不承担运行中 Chat 输出接入新 WebView 的恢复职责。

### 3.2 领域 owner

- `vcp_modules/<domain>/` 持有对应业务状态、输入校验、生命周期和副作用。
- `chat/voice_service/` 的 `call.rs` 持有固定目标、转写提交接纳、精确回复关联与通话快照；原生 VAD 直接触发 Rust 插话取消，ASR 完成直接提交，无需 WebView 消费事件。`voice_submission.rs` 先注册真实聊天 owner，保存用户消息后复用 Agent/Group 应用服务，UI Channel 可缺席；不复制上下文或模型网络实现。关麦在 Rust 内停止原生采集并封存尾音，已接纳 ASR 继续；后台不取消后续发送与朗读，挂断才取消本通话未发送内容及对应回复。输入、模型和 TTS 取消状态独立，有限工作租约覆盖 ASR 至实际播放收尾。统一 RequestOutput/群轮按 `originUserMessageId` 供给 TTS，前端恢复只读取快照和已持久化消息，不重放业务动作。
- `persistence/` 持有数据库写入与读取边界，调用领域不复制第二套持久化状态。
- `infra/` 提供被多个领域共享的生命周期和基础能力，不反向拥有各业务领域结果。
- `infra/file_manager/operations.rs` 裁定文件取消与 CAS 提交，`resources.rs` 管理用途派生、文本预览和临时导出；`infra/share_inbox.rs` 持有当前分享记录与跨进程槽位，发送经现有消息仓储同事务落盘。启动附件回收同时读取当前分享持有的 hash。
- `distributed/` 的 client、registry 与 telemetry 由 `DistributedState` 汇合；设备工具保持自包含，不复制 CLI 或其他领域已有的运行时。

新增全局状态前先确定唯一 owner、建立时点、替换条件和释放路径。`app.manage` 只是状态注入机制，不等于状态所有权可以分散到多个调用者。

---

## 4. IPC 命令总闸

### 4.1 注册位置

app 层 Tauri command 统一在 `commands.rs` 的 `app_command_handler!` 中导入和注册，并在 `lib.rs` 调用点展开。新增命令只在对应领域实现 handler，再加入该宏的领域分组；不得把命令清单重新放回 `lib.rs`，也不得把该宏改成无法在 crate root 解析伴生命令宏的普通子模块函数。

`plugin:*` 命令属于 Tauri 插件路径，不经过 app command 清单；修改 `vcp-mobile` 插件时遵循 `../plugins/vcp-mobile/AGENTS.md`。

领域命令按真实输入、任务和系统资源处理执行条件，不复制 Vue 的产品入口决定或要求统一能力门禁。原生平台差异收在实际适配位置；读取状态、取消和清理按各自资源生命周期执行。具体约定见 [跨平台开发规范](../../docs/CROSS_PLATFORM_DEVELOPMENT.md)。

当前 app command 总闸仅接受 `main` WebView；插件/core 命令另由分平台 capability 文件配置。Android/iOS 生命周期 phase/nativeSequence/snapshot 经原 Rust owner 归约，native sequence 与 Rust transition epoch 分别排序；iOS 的 network Channel 只给现有前端恢复逻辑提供重试提示，不代表完整设备遥测可用。

### 4.2 JavaBridge 栈边界

中央 handler 在 JavaBridge 线程上只接收 `Invoke`，随后把完整命令匹配、参数反序列化与 future 构造交给 Tauri async worker。不得绕过这个总闸注册 app 命令，也不得为单个复杂命令重新增加 future 壳或尺寸判断；保持一次中央 offload 和一条响应通道。

Tauri command handler 返回 `Result<T, String>`，在领域边界把错误转换为可传递信息。命令处理路径不得使用 `unwrap()` 或 `expect()` 把可恢复输入、IO 或外部错误升级为进程崩溃。

跨 Vue/Rust 的命令名、参数、DTO、事件或 Channel 变化必须沿实际生产调用者一次修改完整；不为猜测中的旧调用方增加 alias、双读双写或第二套协议。

---

## 5. 异步、阻塞与资源边界

- 网络和可异步等待的文件、数据库操作继续使用现有 async owner，不在锁内跨越无关 await。
- 同步数据库驱动、压缩、解析、媒体处理和必要的 `std::fs` 操作可以存在，但耗时或无界工作必须离开 async worker，进入现有 `spawn_blocking`、专用 worker 或明确的同步阶段。
- 选择锁、原子或并发容器前先确认真实 owner、竞争者和提交边界；不因统一风格添加没有并发需求的共享状态。
- 文件、消息、流和批处理沿当前代码保留大小、数量与生命周期边界；不得把流式路径退化为无界全量缓冲。
- 取消、重试和恢复由持有该生命周期的领域 owner 决定；调用层不通过重复 spawn、静默 fallback 或日志文案制造第二个完成状态。

---

## 6. 修改与完成边界

实现需求时，从 Vue 调用名、Rust handler、事件或失败现象进入，沿 `commands.rs` 找到领域 owner，再追踪输入、状态、持久化或外部副作用和实际返回路径。修改横跨多个领域时仍保留一个结果 owner，并让调用者使用同一活动路径。

Rust 边界变化在 `src-tauri/` 运行 `cargo check --locked`；跨 Vue/Tauri 变化从仓库根运行 `pnpm check`。涉及 Android 插件、生成树或设备行为时分别读取对应局部 `AGENTS.md` 并取得实际 Android 构建或设备证据。测试只能作为额外执行结果，不能定义生产行为。

**完成**：启动装配、命令注册、领域 owner、状态生命周期、持久化或外部副作用与返回通道保持一致，并明确说明已验证的生产路径、构建结果及尚未取得的 Android 或外部系统证据。
