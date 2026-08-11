# VCP Mobile 技术文档

> 版本：1.1.3 | 最后更新：2026-07-04 | 文档总数：87+ 份 | 总行数：~42,000 行

## 文档体系

VCP Mobile 技术文档按架构层级分为 5 个知识库：

| 知识库 | 路径 | 文档数 | 覆盖范围 | 入口 |
|--------|------|--------|----------|------|
| 顶层架构规范 | `docs/` | 5 份 | 跨层架构约定、依赖管理、UI 层级宪法、Android 存储 | -- |
| Rust 后端模块 | `docs/modules/` | 23 份 | `vcp_modules/` 全部稳定模块 + `distributed/` 分布式节点 | [总览](modules/00_总览与导航.md) |
| Android 插件 | `docs/plugins/` | 12 份 | `tauri-plugin-vcp-mobile` 原生 Android 子系统（v1.1.2 新增 ForegroundGuardian） | [总览](plugins/00_总览与导航.md) |
| 同步子系统 | `docs/sync/` | 20 份 | Sync V2 全链路协议（Rust + Node.js 双端） | [总览](sync/00_总览与导航.md) |
| Vue 前端 | `docs/vue_docs/` | 25 份 | Vue 3 + TypeScript 前端完整源码覆盖 | [总览](vue_docs/00_总览与导航.md) |

## 快速导航（按角色）

### Rust 后端开发者
1. [Rust 模块总览](modules/00_总览与导航.md)
2. [VCP 请求客户端](modules/09_VCP请求客户端.md) -- 核心网络层
3. [Agent 领域总览](modules/13_Agent领域总览.md)
4. [Persistence 领域总览](modules/14_Persistence领域总览.md)
5. [分布式节点能力](modules/15_分布式节点能力.md)
6. [本地服务器与浮动助手](modules/22_本地服务器与浮动助手.md) -- v1.0.3 新增，v1.1.3 浮动助手功能已暂停使用

### Vue 前端开发者
1. [前端总览](vue_docs/00_总览与导航.md)
2. [应用架构与生命周期](vue_docs/architecture/01_应用架构与生命周期.md)
3. [状态管理总览](vue_docs/core/02_状态管理总览与Store全景图.md)
4. [对话引擎总览](vue_docs/features/chat/08_对话引擎总览.md)
5. [浮动助手系统](vue_docs/features/assistant/24_浮动助手系统.md) -- 历史技术资产；当前运行入口、构建产物与权限均已注销

### Android 原生开发者
1. [插件总览](plugins/00_总览与导航.md)
2. [插件初始化与命令路由](plugins/01_插件初始化与命令路由.md)
3. [浮动窗口与会话共享](plugins/10_浮动窗口与会话共享.md) -- v1.0.3 新增，v1.1.3 浮动助手功能已暂停使用

### 同步功能开发者
1. [同步总览](sync/00_总览与导航.md)
2. [架构总览与设计理念](sync/01_架构总览与设计理念.md)
3. [同步协议详解](sync/04_同步协议详解.md)

## v1.0.3 文档更新日志

| 变更类型 | 内容 | 涉及文档 |
|----------|------|----------|
| 新增 | 浮动助手系统（local_server + floatingAssistant + FloatingWindowManager + ShareIntentHandler） | modules/22, vue_docs/24, plugins/10 |
| 新增 | 基础设施工具模块（infra/utils.rs） | modules/21 |
| 新增 | 分布式能力前端面板（DistributedView.vue） | vue_docs/19 |
| 重构 | 传感器采集从 Web API 迁移到 Android 原生（SensorStatusManager.kt） | modules/15, vue_docs/19, plugins/06 |
| 删除 | ffmpeg 二进制依赖替换为降级策略 | modules/11 |
| 删除 | frontend_bridge.rs 移除，改为 Plugin IPC 通道 | modules/15, vue_docs/19 |
| 重命名 | context_assembler_utils.rs --> context_assembler.rs | modules/03, 13, 16, 17 |
| 版本 | 全局版本号 0.9.14 --> 1.0.3 | 全部 55+ 份文档 |

## v1.1.2 文档更新日志（2026-06-24）

| 变更类型 | 内容 | 涉及文档 |
|----------|------|----------|
| **新增** | ForegroundGuardian 前台守护者（WakeLock + WifiLock + FGS 统一引用计数与四级优先级调度） | plugins/11 (NEW) |
| **重构** | StreamKeepaliveService 锁管理移交 ForegroundGuardian，降级为通知壳 | plugins/03, plugins/01, CLAUDE.md |
| **新增** | OEM 自启动权限引导链（6 厂商 Intent） + AppOps 反射检测 | plugins/06 |
| **新增** | OEM 电源管理权限引导链（5 厂商链 + 兜底） | plugins/06 |
| **新增** | 存储空间检测命令（StatFs on /data） | plugins/06 |
| **新增** | OomScoreGuard 进程保护（Root 设备 /proc/pid/oom_score_adj 写入） | plugins/06 |
| **升级** | LifecycleBridge：Activity 级 → 进程级（ProcessLifecycleOwner + lifecycle-process:2.7.0） | plugins/05, CLAUDE.md |
| **新增** | 前台感知事件丢弃（APP_IN_FOREGROUND AtomicBool）+ 后台 Linger 定时器（10min/5min） | modules/21, modules/23 |
| **新增** | spawn_linger_task 可取消延迟任务工具函数 | modules/21 |
| **重构** | Agent/Group 设置面板：watch 自动保存 → saveOnClose 关闭时保存 + 失败 Toast | vue_docs/13, vue_docs/17 |
| **重构** | PermissionGate：2 步 → 3 步（OEM 厂商卡片 + 存储空间验证条） | vue_docs/04 |
| **新增** | App.vue 桥接 set_app_foreground_state 通知 Rust 后台 linger | vue_docs/01, plugins/05 |
| **重构** | 分布式保活：set_keepalive_mode → set_keepalive_mode_inner（委托 ForegroundGuardian） | modules/15 |
| **修复** | 插件文档重复 09 编号：硬件状态/Root 重编号为 11 | plugins/00 |
| **版本** | 全局版本号 1.1.1 → 1.1.2 | 全部 17+ 份文档 |

## 文档约定

- **双语术语**：所有技术术语提供中英文对照
- **YAML 前页**：每份文档头部包含 `id`、`title`、`version`、`date` 元数据
- **交叉引用**：`[显示文本](relative/path.md)` 格式，含章节引用
- **ASCII 图表**：架构图与数据流图使用手绘 ASCII art
- **版本号**：与 Cargo.toml 中的 `version` 字段同步
