# AGENTS.md — VCP Mobile Rust 核心与 Tauri IPC

> 本文件适用于 `src-tauri/src/**`。它继承仓库根规则，只补充 Tauri 启动装配、Rust 业务领域、IPC 总闸、状态所有权与异步执行边界；当前行为必须从本目录生产代码及其实际调用方恢复。

---

## 1. 定位与执行链

`src-tauri/src/` 是 Project Avatar 的 Rust 应用核心。它在 Tauri 进程中管理核心生命周期与长期状态，通过统一 IPC 总闸承接 Vue invoke，分发至领域 owner 并将 Result、事件或 Channel 返回调用方。

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

---

## 2. 代码地图

| 路径 | 唯一职责 |
| ---- | -------- |
| `main.rs` | Host 开发/测试 scaffold，仅作为启动库入口 |
| `lib.rs` | Tauri 组合根：构建器配置、启动期状态注入、插件挂载与系统事件接入 |
| `commands.rs` | App 命令中央注册表与 JavaBridge 线程防爆栈总闸 |
| `vcp_modules/agent/`、`group/`、`chat/` | 智能体、群聊调度、会话状态机、消息流与语音通信服务 |
| `vcp_modules/persistence/` | SQLite 数据库连接池、读写队列与权威消息仓储 |
| `vcp_modules/infra/` | 配置中心、文件与附件存储、媒体转换、分享收件箱及底层系统桥接 |
| `vcp_modules/sync/`、`updater/`、`cli/` | 多端数据同步流水线、应用内更新与移动端 CLI 运行时 |
| `distributed/` | 分布式节点通信、工具注册中心与设备端能力集成 |

---

## 3. 核心不变量

| # | 不变量 / 约束 | 锚点 | 破坏后果 |
| - | ------------- | ---- | -------- |
| R1 | 所有 app 级 Tauri 命令必须在 `commands.rs` 的 `app_command_handler!` 宏内统一注册并 offload，严禁在 `lib.rs` 散落注册 | `commands.rs` | 突破 JavaBridge 线程栈限制 (~1MB)，导致 Android 进程物理爆栈闪退 |
| R2 | Command handler 内部严禁使用 `unwrap()` 或 `expect()`，所有错误必须显式映射为 `Result<T, String>` | `commands.rs` | 单点参数异常或 IO 错误升级为进程级崩溃 |
| R3 | 所有网络、持久化与异步 IO 必须使用 async；CPU 密集型（LAME 编码、媒体处理、解密）与同步驱动操作必须调度至 `spawn_blocking` 或专有 worker | `chat/voice_service/` | 霸占 tokio 运行时 worker 线程，导致全应用 IPC 调用假死 |
| R4 | 业务逻辑与状态所有权必须封闭在 `vcp_modules/<domain>/` 内，`lib.rs` 仅作为生命周期挂载点，不得堆砌业务分支 | `lib.rs` | 破坏领域聚合，引发全局状态隐式依赖与竞态 |
| R5 | 数据库读写边界由 `persistence/` 独占拥有，调用层不得建立平行的持久化缓存或绕过仓储直接写库 | `persistence/` | 引发 SQLite 事务冲突、数据回退或脏数据落盘 |
| R6 | 通话、气泡朗读与试听共用统一 TTS 执行器，输入类型由 `TtsJob` 隔离，生命周期与取消彼此独立 | `chat/voice_service/` | 语音状态相互污染，导致挂断后残余音频播放或后台泄漏 |
| R7 | 配置持久化与规范化由 `settings_manager` 在 DbState 事务内原子提交，异常缺项回退至类型默认值，不破坏已有合法字段 | `infra/settings_manager/` | 用户设置静默丢失，或非法配置引发全应用启动死锁 |
| R8 | 群组发言与 JEV 状态由 `group_session.rs` 单一协调，群聊模式与本地参数独立保存，不侵入全局同步协议 | `group/group_session.rs` | 群组发言调度失序，或多端同步因字段不兼容被阻断 |
| R9 | 附件导入与 CAS 引用由 `file_manager` 统一裁定，分享收件箱跨进程槽位与消息落盘必须原子协同 | `infra/file_manager/` | 产生孤立磁盘垃圾，或分享内容在消息仓储落盘前被清理 |
| R10 | iOS 平台差异由原生桥接层隔离，Chat/Sync/SQLite/CAS 权威终态始终由 Rust 持有，不得在其他层维护终态镜像 | `vcp_modules/` | 平台间业务行为分叉，状态恢复无法达成最终一致性 |

---

## 4. 跨边界与异步边界

- **IPC 契约**: 命令名、参数 DTO 与返回类型变更必须与前端调用链保持原子同步，禁止设置未使用的向前兼容别名。
- **流式通道**: 事件与 Channel 推送必须保持有界缓冲，背压异常时显式终止，禁止在内存中无界堆积历史帧。
- **取消与恢复**: 耗时任务的取消由所属领域 owner 管理；前端连接断开时不假定后端任务已完成，重新建立连接以快照为准。

---

## 5. 修改与验证

| 修改范围 | 验证命令 |
| -------- | -------- |
| Rust 内部实现与领域逻辑 | 在 `src-tauri/` 下运行 `cargo check --locked` |
| 跨 Vue / Tauri IPC 契约或 DTO 变更 | 在仓库根目录运行 `pnpm check` |
| 涉及持久化 schema 或配置结构变更 | 运行 `cargo test --locked --lib` 验证对应单元测试 |

**完成判定**：启动装配、命令注册、领域所有权、资源清理路径保持闭环，静态类型检查无报错，未产生无主后台任务或未捕获的 Panic 隐患。
