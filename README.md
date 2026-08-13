<div align="center">
  <img src="./public/vcpmobile.svg" width="150" alt="VCP Mobile Logo">
  <h1>VCP Mobile <sub><sup>Project Avatar</sup></sub></h1>
  <p><em>From Desktop Client to Cyber-Physical Avatar.</em></p>

  <p>
    <img src="https://img.shields.io/badge/version-1.1.4-blue" alt="version">
    <img src="https://img.shields.io/badge/platform-Android-green?logo=android" alt="platform">
    <img src="https://img.shields.io/badge/framework-Tauri%202.11.2%20%7C%20Vue%203.5.33-26A17B?logo=tauri" alt="framework">
    <img src="https://img.shields.io/badge/backend-Rust%20%7C%20Tokio-000000?logo=rust" alt="backend">
    <img src="https://img.shields.io/badge/UI-UnoCSS%20%7C%20Minimalist-4f46e5" alt="UI">
    <img src="https://img.shields.io/badge/license-CC%20BY--NC--SA%204.0-lightgrey" alt="license">
  </p>
</div>

---

## Table of Contents

1. [What is VCP Mobile](#1-what-is-vcp-mobile)
2. [Key Features](#2-key-features)
3. [Architecture](#3-architecture)
4. [Project Structure](#4-project-structure)
5. [Tech Stack](#5-tech-stack)
6. [Documentation](#6-documentation)
7. [Quick Start](#7-quick-start)
8. [Development & Testing](#8-development-and-testing)
9. [Contributing & Governance](#9-contributing-and-governance)
10. [FAQ & Troubleshooting](#10-faq-and-troubleshooting)
11. [License & Credits](#11-license-and-credits)

---

## 1. What is VCP Mobile

**VCP Mobile**（代号 Project Avatar）是 [VCPChat](https://github.com/MRiecy/VCPChat) 的移动端进化版，一个基于 **Tauri v2 + Vue 3 + Rust** 构建的 Android 原生应用。核心目标是将 AI Agent 的交互能力以低延迟、高内存安全性的方式带入物理移动端。

> **支持范围**：生产运行、构建、发布与兼容性承诺仅覆盖 Android `arm64-v8a` 触控手机、平板，以及按当前窗口宽度响应的折叠屏。不支持 Windows/macOS/Linux 桌面端、iOS、Android TV 或其他 ABI；桌面用户请使用 VCPChat。仓库中的 Vite 预览、host Rust 编译、非 Android fallback 与 Tauri desktop scaffold 仅用于开发和测试，详见 [`docs/ANDROID_UI_COMPATIBILITY.md`](docs/ANDROID_UI_COMPATIBILITY.md)。

Web 生成目标显式冻结为 Chrome/WebView 87 时代语法（JS/CSS `chrome87`），用于防止构建工具升级静默抬高语法门槛；它不是最低 WebView 支持承诺，真实兼容结论仍需目标 Android 设备证据。

与桌面端 VCPChat 不同，Project Avatar 并非简单的界面适配，而是一次架构层面的彻底重构。我们采用了 **Double-Track 3-Tier 架构** —— Rust 核心层、Tauri IPC 桥接层、Vue 3 渲染层物理隔离，使每一个层级都可以独立演进而不产生耦合债务。

与市面上其他移动端 AI 应用相比，VCP Mobile 的独特之处在于：
- **Backend-Driven Streaming**：消息生命周期完全由后端 SSE 事件驱动，前端不做任何预创建或状态猜测
- **自定义增量同步协议**：不依赖第三方云服务，移动端与桌面端通过 WebSocket + HTTP 双通道直接同步
- **14 设备能力工具**：将手机本身变成一个分布式计算节点，AI 可直接调用位置、传感器、CPU/GPU 信息等原生能力

### 演进历程

| 版本 | 关键里程碑 |
|------|------------|
| v0.9.0 | 首个 Preview 版本，Tauri v2 + Vue 3 + Rust 基础架构确立 |
| v0.9.6 | 修复消息历史回退遗留，指纹命令注册，APK 签名验证 |
| v0.9.8 | 消息路由体系完善，SSE 生命周期管理，UI 架构重构 |
| v0.9.10 ~ v0.9.12 | 同步 V2 协议实现，群组对话，附件分类与预览体系 |
| v0.9.13 ~ v0.9.14 | 分布式节点模块，设备能力工具集，Model 管理器，WebGL 特效 |
| v1.0.0 | Avatar 正式发布：Backend-Driven Streaming、Tarven 上下文注入、Semantic Z-Index、SlidePage 虚拟导航 |
| v1.0.3 | 分布式节点系统、Root 访问支持、CPU/GPU/Network 硬件状态、WakeLock/WifiLock 双锁保活 |
| v1.1.0 Aurora Genesis | 增量 AST Diff 渲染引擎、Epoch/Revision 双级时序、ToolCallSummary、工具审批系统、聊天历史分页 |
| v1.1.1 | AST 节点树合并剪枝、正则算法兼容性优化、动态帧率降级、虚拟按键兼容 |
| v1.1.2 | RAG 灵视中心（认知广播观察器） |
| v1.1.3 Guardian Protocol | ForegroundGuardian 进程级锁调度、StreamKeepaliveService 降级为通知壳、SSE 代理 `:helper` 进程、ProcessLifecycleOwner、前端 vitest 测试框架 |

项目从首个 commit 起即采用 Tauri v2 + Vue 3 + Rust 栈，不存在 Node.js / Electron 或 Tauri v1 的历史阶段。Rust 的所有权模型和零成本抽象为移动端提供了编译期内存安全保障，Tokio 异步运行时确保网络 IO 不阻塞主线程，这对流式聊天体验至关重要。

---

## 2. Key Features

### ⚡ Backend-Driven Streaming

流式聊天的消息生命周期已全面转为后端 SSE 事件驱动，前端不再承担消息生命周期管理。

- 后端通过 `StreamEvent`（`thinking` / `content` / `blocks` / `end` / `error`）逐事件下发
- 前端 `chatStreamStore` 仅做状态映射，不做任何消息预创建或内容猜测
- 显著简化 `chatHistoryStore`，消除前后端状态不一致的隐患
- 支持 `LinesCodec` 流式解析，降低移动端内存峰值

```
┌─────────────┐     SSE Stream      ┌─────────────────┐
│   Backend   │ ──StreamEvent─────► │  chatStreamStore│
│  (Rust)     │  thinking/content   │   (Vue/Pinia)   │
└─────────────┘   /blocks/end       └─────────────────┘
```

### 🧠 Tarven 上下文注入规则

结构化提示词注入规则引擎，支持在对话流的任意节点精确插入外部上下文。

- **`system_suffix`**：在系统提示词前/后追加内容
- **`user_suffix`**：在用户消息前/后追加内容
- **`context_inject`**：在对话历史指定深度注入自定义角色消息
- 规则支持 `scope`（`global` / `agent` / `group`）分级生效
- `sort_order` 排序机制确保多规则冲突时可预期
- WYSIWYG 实时预览，所见即所得

### 🔄 分布式增量同步（Delta Sync V2）

移动端与桌面端通过自定义三阶段协议保持实时同步，无需第三方云服务。

| 阶段 | 动作 | 传输通道 |
|------|------|----------|
| 1. Metadata 指纹交换 | 对比 SHA-256 Hash 列表 | WebSocket |
| 2. Content Diff | Hash 不匹配时执行 PULL / PUSH | HTTP |
| 3. Message Stream | 增量拉取缺失消息 | WebSocket |

- 基于 SHA-256 Hash 的差异检测，避免全量传输
- WebSocket + HTTP 双通道设计：控制面走 WebSocket，数据面走 HTTP
- 冲突解决采用逻辑时钟与 `updated_at` 策略，保证最终一致性
- WAL（Write-Ahead Logging）模式 SQLite，降低移动端并发写入锁竞争

### 📎 多模态附件引擎 2.0

插件化附件系统，支持从图像到文档的全类型处理。

- **`AttachmentRegistry`** 注册表 + **`AttachmentFactory`** 工厂 + **`AttachmentClassifier`** 分类器
- 支持 8 种类型：`Image` / `Video` / `Audio` / `Document` / `Code` / `Text` / `Other`
- 双轨上传策略：
  - Android 原生 File Picker（常规文件）
  - 高速 TCP 通道（大文件分块上传）
- `AttachmentViewer` 全屏查看器，挂载于语义化 `z-viewer` 层级
- 文件上传：无大小限制，系统级 `rename` + SHA-256 流式哈希计算
- 文本提取：50 MB 文件硬上限防 OOM，提取结果按 1000 万字符截断
- 视频帧提取：Base64 累计 18 MB 动态截断，确保请求体在 20 MB 以内
- 音频提取：3500 秒（约 58 分钟）时长硬截断

### 🎯 Model 管理与选择器

- `modelStore` 集中管理模型列表、收藏状态、热门排行
- 10 分钟 TTL 缓存 + 锁频防护，避免重复请求
- `ModelSelector` BottomSheet 弹层：收藏 > 热门 > 字母序三级排序
- 与 `AgentSettingsView` 深度联动，切换模型即时生效

### 🌌 RAG 灵视中心

v1.1.2 引入的认知广播观察面板，让 AI 在 RAG 检索、工具调用、长链推理时的中间状态可视化。

- `ragObserverStore` 订阅后端认知广播事件，实时展示检索阶段、来源文档、置信度
- `rag/` 功能模块提供认知广播面板 + 载荷详情浮层
- 与 `chatStreamStore` 联动，将隐式的 RAG 过程显式映射到对话上下文

### 🧑‍🚀 浮动助手（Floating Assistant）

`assistant/` 模块提供全局悬浮窗快捷对话入口，可在任何界面快速唤起轻量级 AI 交互。

- `floatingAssistantStore` 管理悬浮窗位置、折叠状态、会话上下文
- 与主应用共享 `agentStore` / `modelStore`，保证模型与智能体选择一致
- 独立的渲染入口 `floating-main.ts`，生命周期与主窗口解耦

### 🔔 通知中心

`notification/` 模块集中管理本地通知、Toast、权限状态与系统通知监听。

- `notificationStore` 统一处理本地通知创建、点击恢复、待处理数据
- 与 Android `VcpNotificationListenerService` 联动，支持通知权限引导
- 级联逻辑删除同步：代理/群组/话题删除时同步标记相关消息

### 🏗️ Semantic Z-Index / SlidePage 虚拟导航

12 级语义化层级系统，彻底消灭 `z-[999]` 魔法数字。

| 语义名 | 数值 | 用途 |
|--------|------|------|
| `content` | 0 | 页面内容默认层 |
| `local` | 10 | 局部悬浮（置底按钮、角标）|
| `drawer` | 20 | 左右抽屉 + 遮罩 |
| `overlay` | 30 | 全局覆盖容器 |
| `page` | 40+ | SlidePage 虚拟页面栈 |
| `sheet` | 50 | BottomSheet、ModelSelector |
| `dialog` | 60 | Prompt、ContextMenu |
| `editor` | 70 | HtmlPreviewBlock 全屏 HTML 预览 |
| `viewer` | 80 | AttachmentViewer、FullScreenEditor、AvatarCropper |
| `toast` | 90 | Toast 通知 |
| `boot` | 100 | 启动屏 |
| `gate` | 110 | 权限引导页 |

- 三层保障：CSS 变量 `--layer-*` + UnoCSS 快捷类 `z-*` + TypeScript 常量 `LAYER_*`
- SlidePage 虚拟页面栈：非路由跳转，通过 `overlayStore` 管理，动态 Z-Index = `40 + stackIndex`
- Operation Aegis 模态历史栈支持物理返回键 LIFO 消费

### 🔧 14 设备能力工具（分布式节点）

`distributed/` 模块将手机转化为 AI 可直接调用的分布式计算节点，提供 14 个原生设备能力：

- `device_info` / `device_status_summary` — 设备综合信息
- `location` — GPS 与网络定位
- `battery` — 电量与充电状态
- `clipboard` — 剪贴板读写
- `cpu_info` / `gpu_info` / `memory_info` / `storage_info` — 硬件监控
- `network_info` — 网络类型与连接状态
- `ambient_sensor` / `motion_sensor` — 环境传感器与运动传感器
- `notification` — 本地通知推送
- `frontend_bridge` — 前后端能力桥接
- `telemetry_center` — 分布式遥测聚合

### 🤖 Agent / Group 交互

- AgentList 支持拖拽排序（SortableJS）+ Swipe 手势（编辑/删除）
- `AgentSettingsView` / `GroupSettingsView` 设置面板，与 `ModelSelector` 联动
- `vue-cropper` 头像裁剪 + Dominant Color 主色调提取
- 群组对话支持 `group_context_assembler` 与 `group_speaking_policy` 发言策略

### 🌊 WebGL 流体动态背景

`WebGLFluidBackground.vue` 提供高性能流体模拟动态背景，仅用于**关于界面（About Section）**的视觉特效。

### 🚀 APK 更新

- 通过 GitHub Release 检查并下载 arm64 APK
- 前端资源随 APK 一同签名发布，运行时只加载 APK 内嵌 assets
- 安装交给 Android 系统安装器完成包名与发布证书校验

---

## 3. Architecture

VCP Mobile 采用 **Double-Track 3-Tier（双轨三层）架构**，将 UI 渲染层、IPC 桥接层、Rust 核心层物理隔离，并向下延伸为 Android 原生插件层，实现端到端的类型安全与内存安全。

### 3.1 分层概念

```
┌───────────────────────────────────────────────────────────────┐
│                    Rendering Layer                             │
│  Vue 3.5 + Pinia 3 + UnoCSS                                   │
│  src/ —— 组件、Store、Composable、Directive                    │
├───────────────────────────────────────────────────────────────┤
│                    IPC Bridge Layer                            │
│  Tauri v2 —— invoke / listen / Channel                        │
│  前端调用 Rust command，后端通过 Channel 推送 SSE Stream       │
├───────────────────────────────────────────────────────────────┤
│                    Core Layer                                  │
│  Rust + Tokio                                                 │
│  src-tauri/src/vcp_modules/ —— 7 大领域：                     │
│    agent / chat / group / infra / persistence / sync / updater │
│  src-tauri/src/distributed/ —— 14+ 设备能力工具               │
├───────────────────────────────────────────────────────────────┤
│                    Native Layer                                │
│  Kotlin Android Plugin (tauri-plugin-vcp-mobile)              │
│  屏幕常亮、前台保活、键盘 Insets、生命周期桥接                 │
└───────────────────────────────────────────────────────────────┘
```

**Double-Track** 指两条独立的数据通道：
- **Request-Response Track**：Vue `invoke` → Rust Command → 返回 `Result<T, String>`，用于配置读写、CRUD 操作。
- **Streaming Track**：Rust `Channel` → 前端 `listen` / `EventSource`，用于 SSE Stream、WebSocket 消息、进度推送。

### 3.2 典型数据流

**发送消息（Send Message）**：
```
Vue Input → chatSessionStore
         → invoke("send_chat_message", payload)
         → Rust chat service → VCP API
         ← SSE Stream
         → Channel.emit() → Vue chatStreamStore
         → chatHistoryStore 追加消息块
```

**增量同步（Delta Sync）**：
```
Vue → invoke("start_sync") → Rust sync service
  → WebSocket 握手 → Delta Sync 协议
  → SHA-256 Hash compare（本地 vs 远端）
  → 生成增量 patch → SQLite WAL 写入
  → 前端 syncStore 更新进度与状态
```

### 3.3 状态管理

全局与功能状态由 **21 个 Pinia Stores** 与 **16 个 Composables** 组成，全部使用 **Composition API 风格**（`defineStore('id', () => { ... })` / `useXxx()`），摒弃 Options API。

| Store | 职责 |
|-------|------|
| `chatSessionStore` | 当前会话状态、输入框内容、快捷操作 |
| `chatHistoryStore` | 消息列表、分页加载、消息 CRUD |
| `chatStreamStore` | SSE Stream 实时状态、thinking 块、block 解析 |
| `attachmentStore` | 附件选择、上传队列、MIME 识别、进度追踪 |
| `overlayStore` | SlidePage 栈、BottomSheet、Dialog 队列、Z-Index 管理 |
| `agentStore` | Agent / Group 列表、排序、缓存、卡片级手势状态 |
| `modelStore` | 模型列表、收藏状态、热门排行、TTL 缓存 |
| `ragObserverStore` | RAG 认知广播观察、检索阶段与载荷详情 |
| `floatingAssistantStore` | 浮动助手位置、折叠状态、独立会话上下文 |
| `notificationStore` | 本地通知、Toast、权限状态、点击恢复 |
| `themeStore` | 主题切换、CSS 变量注入、跟随系统深色模式 |
| `diaryStore` | 日记文件夹/文件、普通与语义搜索、编辑基线、创建和批量管理 |

### 3.4 Android 原生插件通信分层

`tauri-plugin-vcp-mobile` 统一管理全部 Android 原生能力，不同功能采用不同的通信方式：

| 功能 | Rust 模块 | Kotlin 模块 | 通信方式 |
|------|-----------|-------------|----------|
| 屏幕常亮 | `src/screen.rs` | — | Raw JNI（`jni` crate 直接调用 Activity）|
| 前台锁协同 / 流式保活 | `src/stream.rs` | `ForegroundGuardian.kt` / `StreamKeepaliveService.kt` | `PluginHandle.run_mobile_plugin` |
| 键盘 Insets | — | `KeyboardInsetsManager.kt` | `evaluateJavascript` 注入 CustomEvent |
| 生命周期事件 | — | `LifecycleBridge.kt` | `PluginHandle.run_mobile_plugin` → `listen_any` → Tauri Event |
| 权限与系统控制 | `src/system.rs` | `VcpMobilePlugin.kt` | `PluginHandle.run_mobile_plugin` |
| SSE 代理 | `src/stream.rs` | `SseProxyService.kt` | `PluginHandle.run_mobile_plugin`；主/`:helper` 进程通过 127.0.0.1 TCP 通信 |
| 硬件/传感器状态 | `src/system.rs` | `BatteryStatusManager.kt` / `NetworkStatusManager.kt` 等 | `PluginHandle.run_mobile_plugin` |

关键设计决策：
- `ForegroundGuardian` 是进程级 Kotlin 单例，统一调度 WakeLock + WifiLock + 前台服务引用计数，支持四级优先级（`SYNC=40 / PRERENDER=30 / STREAM=20 / DISTRIBUTED=10`）
- `StreamKeepaliveService` 在 v1.1.3 降级为纯粹的“通知壳”，锁管理已移交 `ForegroundGuardian`；`stopWithTask="true"` 确保进程被划掉后通知随之消亡
- `LifecycleBridge` v1.1.3 升级为 `ProcessLifecycleOwner` 进程级观察，并通过 `plugin.trigger("lifecycle")` → Rust `listen_any` → `app.emit("vcp-lifecycle-changed")` 转发
- `SseProxyService` 运行在独立 `:helper` 进程，通过本地 TCP 套接字与主进程通信，支持 SSE 断线缓存与动态锁控
- `KeyboardInsetsManager` 不使用 Tauri 标准事件通道，而是通过 `evaluateJavascript` 直接注入 `window.CustomEvent`
- 屏幕常亮使用 Raw JNI 而非 PluginHandle，避免跨语言序列化开销

---

## 4. Project Structure

```
VCPMobile/
├── src/                          # Vue 3 前端源码
│   ├── main.ts                   # 应用入口
│   ├── App.vue                   # 根布局（引导流程 + 侧边栏手势）
│   ├── core/
│   │   ├── stores/               # 20 全局 Pinia Stores（Composition API）
│   │   ├── composables/          # 16 个全局组合式函数
│   │   ├── router/               # Hash 模式路由
│   │   ├── directives/           # v-intersection-observer, v-longpress
│   │   ├── types/                # 全局 TypeScript 类型
│   │   ├── constants/            # 层级常量、主题 Token
│   │   └── utils/                # 同步服务、通用工具
│   ├── features/                 # 领域功能模块（Feature Co-location）
│   │   ├── agent/                # Agent/Group CRUD、设置面板、拖拽排序
│   │   ├── assistant/            # 浮动助手（全局悬浮窗快捷对话）
│   │   ├── chat/                 # 对话引擎、消息渲染、输入增强
│   │   ├── diary/                # 1 个功能 Store + 日记中心页面
│   │   ├── distributed/          # 设备工具调用 UI
│   │   ├── notification/         # 通知中心与 Toast
│   │   ├── rag/                  # RAG 灵视中心（认知广播面板 + 载荷详情）
│   │   ├── settings/             # 全局设置、主题选择
│   │   ├── sync/                 # 同步状态 UI
│   │   └── topic/                # 主题管理
│   ├── components/
│   │   ├── layout/               # AgentSidebar, BootScreen, RightSidebar
│   │   ├── ui/                   # BottomSheet, ToastManager 等原语
│   │   └── settings/             # 设置页原子组件
│   └── assets/                   # 主题 CSS、Logo 预览
├── src-tauri/                    # Tauri v2 + Rust 后端
│   ├── src/
│   │   ├── lib.rs                # Tauri Command 注册、managed state
│   │   ├── vcp_modules/          # 业务逻辑（8 大领域）
│   │   │   ├── agent/
│   │   │   ├── chat/
│   │   │   ├── diary/
│   │   │   ├── group/
│   │   │   ├── infra/
│   │   │   ├── persistence/
│   │   │   ├── sync/
│   │   │   └── updater/
│   │   └── distributed/          # 设备能力工具（14+ tools）
│   ├── plugins/vcp-mobile/       # Android 原生插件
│   │   ├── src/                  # Rust 侧（screen / stream / system）
│   │   ├── android/              # Kotlin 侧（Service / Bridge / Manager）
│   │   ├── guest-js/             # 前端 TS 调用封装
│   │   └── permissions/          # Tauri v2 权限声明
│   └── Cargo.toml                # Rust 依赖与 Release 优化配置
├── docs/                         # 四层技术文档体系
│   ├── vue_docs/                 # 前端文档（27 份）
│   ├── modules/                  # Rust 模块文档（31 份）
│   ├── sync/                     # 同步协议文档（20 份）
│   ├── plugins/                  # 原生插件文档（13 份）
│   └── *.md                      # 顶层规范（架构、UI 层级、依赖管理）
├── .github/workflows/            # CI/CD（类型检查 + Release APK）
├── package.json                  # pnpm 依赖与脚本
├── vite.config.ts                # Vite 配置（端口 1420/1421）
├── uno.config.ts                 # UnoCSS 预设与主题色
└── tsconfig.json                 # TS 严格模式配置
```

### 关键文件速查

| 文件 | 说明 |
|------|------|
| `src/main.ts` | Vue/Pinia/Router 实例创建，全局指令注册，初始化监听 |
| `src/App.vue` | 根布局：BootScreen 引导、侧边栏手势、全局事件监听 |
| `src/core/constants/layers.ts` | 语义化 Z-Index 体系（content → gate，共 12 层） |
| `src/core/stores/chatStreamStore.ts` | SSE Stream 状态驱动，Backend-Driven Streaming 核心 |
| `src-tauri/src/lib.rs` | Tauri 命令路由、managed state 注入、启动钩子 |
| `src-tauri/src/vcp_modules/chat/chat_manager.rs` | 对话生命周期管理、消息发送编排 |
| `src-tauri/src/vcp_modules/infra/vcp_client.rs` | HTTP 客户端（reqwest + rustls-tls）、SSE 解析 |
| `src-tauri/src/vcp_modules/sync/sync_service.rs` | 三阶段增量同步主控（WebSocket + HTTP） |
| `src-tauri/src/distributed/tools/device_info.rs` | 设备信息工具（14 设备能力之一） |
| `src-tauri/plugins/vcp-mobile/android/.../ForegroundGuardian.kt` | v1.1.3 进程级 WakeLock/WifiLock 调度单例 |
| `docs/ARCHIVED_SYNC_ARCHITECTURE.md` | 增量同步协议历史规范 |
| `docs/sync/00_总览与导航.md` | 同步 V2 子文档导航 |
| `docs/ANDROID_UI_COMPATIBILITY.md` | Android arm64 平台边界、窗口宽度响应式与设备证据规范 |
| `docs/UI_LAYER_ARCHITECTURE.md` | 全局 UI 层级与 Z-Index 语义化规范 |
| `uno.config.ts` | UnoCSS 主题色、快捷类、断点配置 |
| `vite.config.ts` | Vite 插件链、Tauri 感知开发服务器 |

### CI/CD 工作流

| 工作流 | 文件 | 触发条件 | 执行内容 |
|--------|------|----------|----------|
| CI | `.github/workflows/ci.yml` | push / PR 到 main/master | 前端类型、Vitest 与生产 build；Rust fmt/test/integration/clippy；Android 生成树与插件 JVM 测试；benchmark 编译与依赖审计 |
| Release | `.github/workflows/release.yml` | GitHub Release published | 构建 `aarch64` Release APK 并上传 |

Release 工作流环境：Node 22, pnpm 10, Java 17 (temurin), Android NDK `29.0.13846066`。APK 自动重命名为 `VCPMobile_v{VERSION}_arm64-v8a.apk`。

---

## 5. Tech Stack

| Layer | Tech | Version | Purpose |
|-------|------|---------|---------|
| Frontend Framework | Vue | 3.5.33 | Reactive UI |
| Frontend Framework | Vue Router | 5.0.6 | Hash routing |
| Frontend State | Pinia | 3.0.4 | State management |
| Frontend State | pinia-plugin-persistedstate | 4.7.1 | State persistence |
| Frontend Style | UnoCSS | 66.6.8 | Atomic CSS |
| Frontend Build | Vite | 6.4.2 | Build tool |
| Frontend Type | TypeScript | ~5.6.3 | Type system |
| Frontend Test | Vitest | 4.1.9 | Unit / integration tests |
| Backend Framework | Tauri | 2.11.2 | Cross-platform framework |
| Backend Runtime | Tokio | 1.x | Async runtime |
| Backend Storage | sqlx + rusqlite | 0.8.6 / 0.32.1 | SQLite async driver |
| Backend Network | reqwest + tokio-tungstenite | 0.12 / 0.26 | HTTP + WebSocket |
| Backend Parsing | syntect + pulldown-cmark | — | Syntax highlight + Markdown |
| Backend Security | rustls-tls | — | TLS encryption |
| Build Tool | pnpm | 10.x | Package manager |
| CI/CD | GitHub Actions | — | Automated build and release |

### 安全设计

- **路径遍历防护**：`file_manager.rs` 中的 `ensure_safe_path()` 限制所有文件访问在 `app_config_dir` 下
- **内存限制**：IPC `store_file` ≤ 2 MB，`read_local_file_base64` ≤ 50 MB，防止 OOM
- **密钥管理**：Release 签名信息仅通过环境变量或 GitHub Actions secrets 注入，缺少任一签名输入时构建直接失败
- **数据库**：SQLite 启用 WAL（Write-Ahead Logging）模式，降低并发写入锁竞争
- **网络**：HTTP 客户端使用 `rustls-tls`，禁用原生 TLS；支持 gzip 压缩

---

## 6. Documentation

### 6.1 四层知识库

| Knowledge Base | Path | Docs Count | Scope | Audience |
|----------------|------|:----------:|-------|----------|
| Frontend Docs | `docs/vue_docs/` | 27 | 全部 Vue/TS 源码 | 前端开发者 |
| Rust Modules | `docs/modules/` | 31 | `vcp_modules/` + `distributed/` | 后端开发者 |
| Sync Protocol | `docs/sync/` | 20 | Delta Sync V2 全链路 | 同步功能开发者 |
| Plugin Docs | `docs/plugins/` | 13 | `tauri-plugin-vcp-mobile` | 原生插件开发者 |

### 6.2 快速决策树

遇到以下问题时，直接查阅对应文档：

- **"Message rendering pipeline 如何工作？"** → `docs/vue_docs/features/chat/09_...`
- **"Tarven injection rules 的判定逻辑是什么？"** → `docs/modules/16_...`
- **"Sync hash detection 如何检测冲突？"** → `docs/sync/03_哈希体系与变更检测.md`
- **"Frontend store 的架构约定是什么？"** → `docs/vue_docs/core/stores/...`
- **"Android lifecycle bridge 的事件流向？"** → `docs/plugins/...`
- **"Attachment upload protocol 的分块策略？"** → `docs/modules/07_...`
- **"UI Z-Index 层级语义化规范？"** → `docs/UI_LAYER_ARCHITECTURE.md`
- **"Android 手机/平板/折叠屏支持到什么范围？"** → `docs/ANDROID_UI_COMPATIBILITY.md`
- **"Android 权限管理与前台服务规范？"** → `docs/ANDROID_PLUGIN_MANAGEMENT.md`
- **"Backend-Driven Streaming 的消息生命周期？"** → `docs/vue_docs/features/chat/...`
- **"Release 构建优化配置详解？"** → `docs/modules/...`
- **"同步协议历史规范？"** → `docs/ARCHIVED_SYNC_ARCHITECTURE.md` / `docs/sync/00_总览与导航.md`

### 6.3 前后端交叉引用

文档体系并非单向分层，而是存在显式的**前后端交叉引用**：

| 前端文档 | ↔ | 后端文档 |
|---------|---|---------|
| Frontend Tarven rule system | ↔ | Backend Tarven injection engine (`vcp_modules/chat/`) |
| Frontend StreamStore 状态机 | ↔ | Backend SSE Stream parser + Channel emitter |
| Frontend sync progress UI | ↔ | Backend sync executor + sync pipeline |
| Frontend attachment preview | ↔ | Backend file_manager + media_processor |
| Frontend agent settings panel | ↔ | Backend agent_service + avatar_service |

这种映射关系确保任何跨层变更都能快速定位到对端实现。

---

## 7. Quick Start

### 7.1 用户安装（普通用户）

1. 前往 [Releases](https://github.com/MRiecy/VCPMobile/releases) 下载最新 `VCPMobile_v1.1.4_arm64-v8a.apk`
2. 安装到 Android 设备（minSdk 26，推荐 Android 10+）
3. 启动应用，完成权限引导（通知、存储、电池优化白名单）
4. 配置 VCP 服务器地址与 API Key
5. 开始对话

### 7.2 开发者环境

**Prerequisites：**

- Rust (Latest Stable, Edition 2021)
- Node.js (v22+) & pnpm (10.x)
- Android Studio & Android NDK (`29.0.13846066`)
- Java 17 (temurin)
- Windows / macOS / Linux host 开发环境（仅用于开发与测试，不是 VCP Mobile 产品支持平台）

**完整命令流：**

```bash
# 1. Clone
git clone https://github.com/MRiecy/VCPMobile.git
cd VCPMobile

# 2. Install dependencies
pnpm install

# 3. Initialize Android (first time only)
pnpm tauri android init

# 4. Android USB Debug (Agent-safe, proxy/TUN independent)
pnpm android:debug:doctor -- --json
pnpm android:debug:dev

# 5. Static check (TypeScript)
vue-tsc --noEmit

# 6. Static check (Rust)
cd src-tauri; cargo check --locked

# 7. Build Release APK
pnpm tauri android build --apk --target aarch64 -- --dependency-verification strict
```

---

## 8. Development & Testing

### 8.1 pnpm 脚本速查表

| 脚本 | 命令 | 说明 |
|------|------|------|
| `pnpm dev` | `vite` | 前端开发服务器（端口 1420）|
| `pnpm build` | `vue-tsc && vite build` | 前端生产构建 |
| `pnpm check` | `vue-tsc --noEmit && cd src-tauri && cargo check --locked` | 全量静态检查（前端类型 + Rust 锁定依赖编译）|
| `pnpm test` | `vitest` | 前端 Vitest（交互式）|
| `pnpm test:run` | `vitest run` | 前端测试一次性运行 |
| `pnpm test:integration` | `cargo test --locked --manifest-path src-tauri/Cargo.toml --test file_extractor_integration` | Rust 文件提取集成测试 |
| `pnpm android:debug:dev` | 统一 Debug CLI | USB + ADB reverse Android 开发调试，控制台限流 |
| `pnpm android:debug:status -- --json` | 统一 Debug CLI | 有界设备/WebView/包/进程状态 |
| `pnpm android:debug:logs -- --lines 80` | 统一 Debug CLI | Debug PID 日志，不清空全局 logcat |
| `pnpm android:debug:snapshot -- --screenshot` | 统一 Debug CLI | 状态、有限日志和可选单张截图落盘 |
| `pnpm tauri android build --apk --target aarch64 -- --dependency-verification strict` | — | Release APK 构建（需四项签名环境变量）|

当前仓库不提供根 `scripts/` 目录。Android Debug 统一使用 tracked
`tests/e2e-android/scripts/android-debug-agent.cjs`，完整规范见
`docs/ANDROID_AGENT_DEBUGGING.md`；Release 构建仍直接使用 Tauri CLI。

### 8.2 Rust Release 优化

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

### 8.3 测试策略

- **前端单元/契约测试**：Vitest 覆盖组件、Store 并发、富文本、分享 Intent 与 Release/Android 治理契约
  - `src/tests/unit/chat/...`
  - `src/tests/unit/components/ui/...`
  - `src/tests/unit/components/settings/...`
- **Android E2E**：`tests/e2e-android/` 使用仓库内 Node.js + adb 环境/权限/冒烟脚本；当前没有 Maestro 或 Playwright 流程
- **性能资产**：`tests/perf/` 与 Criterion benchmark 保留为人工诊断/报告型资产，不是当前兼容专项或 Release 的自动性能门禁
- **Rust workspace 测试**：覆盖 Chat/Sync/Distributed/DB/Updater/文件边界及 Android 插件 Rust 侧；数量以 `cargo test --locked --workspace --lib -- --list` 为准。
- **Rust 集成测试**：`file_extractor_integration` 使用仓库内固定 DOCX/XLSX/PDF/PPTX fixture。

---

## 9. Contributing & Governance

### 9.1 Magi 三贤者协议

任何重大架构调整、复杂 Bug 修复或核心功能实现前，需强制进行三方思辨：

- **Melchior (逻辑与系统)**：审查内存安全、Rust 生命周期、IPC 开销、类型完整性、OOM 防御
- **Balthasar (直觉与美学)**：审查移动端原生直觉、Glassmorphism 规范、微动画、交互心理学
- **Casper (务实与交付)**：审查工程复杂度、维护成本、实现周期，拒绝过度设计

### 9.2 知识治理

当前仓库以 tracked `docs/`、代码、测试和本 README 作为工程知识来源，未启用
`plans/`、`memory:refresh` 或根 `scripts/` 自动编译框架。架构变更应同步更新对应
文档与契约测试，不能只留下未被版本控制的本地笔记。

### 9.3 编码规范（精简版）

- **前端**：`<script setup>` 强制、UnoCSS 优先、PascalCase 组件、Feature Co-location
- **后端**：业务逻辑必须在 `vcp_modules/`；`lib.rs` 仅做路由；禁止 `unwrap()` / `expect()`；异步 IO 基于 Tokio
- **跨层**：修改后必须运行静态检查；严禁全文件覆盖小修改

### 9.4 Pull Request 流程

1. **设计思辨**：重大变更前进行技术评审，记录决策过程
2. **静态检查**：提交前确保 `vue-tsc --noEmit` 与 `cargo check` 无错误
3. **文档同步**：若修改跨层接口或新增模块，同步更新 `docs/` 对应文档
4. **提交信息**：使用中文描述变更意图，重大变更附带设计文档链接



---

## 10. FAQ & Troubleshooting

**Q: 按返回键为什么先关闭 BottomSheet 而不是退出应用？**

A: 采用 Operation Aegis 模态历史栈，返回键按 LIFO 顺序消费：Modal Stack → 重置会话 → 双击退出到后台。

**Q: Agent 设置在哪？**

A: 在主界面侧边栏长按任意 Agent 卡片，或左滑 Agent 卡片点击「编辑」图标，即可进入 AgentSettingsView。群组设置同理。

**Q: 如何切换主题或壁纸？**

A: 进入 Settings → ThemePicker，选择主题即可实时切换。壁纸从 `public/wallpaper/` 自动加载，支持明暗双模式。

**Q: 同步失败如何排查？**

A: 检查 1) 手机与电脑是否同一局域网；2) 桌面端 VCPChat 是否启用同步插件；3) 查看 `docs/sync/15_开发指南与FAQ.md`。

**Q: 上下文注入规则在哪里设置？**

A: 进入 Agent 或群组设置页面，找到「上下文注入」选项卡，可添加 `system_suffix`、`user_suffix`、`context_inject` 三种规则，支持 scope 分级与 sort_order 排序。

**Q: Agent 排序如何改变？**

A: 在 Agent 侧边栏长按并拖拽 Agent 卡片即可调整顺序。排序状态通过 `update_settings` 增量保存到后端 SQLite。

**Q: 语音模式有哪些模式？**

A: 输入栏提供三种语音交互方式：
- **语音模式**（点击语音图标切换）：显示「按住 说话」大条，按住录音后作为音频附件发送
- **STT 语音转文字**（在语音模式下按住说话）：实时识别为文字输入到文本框
- **长按快速录音**（在非语音模式下长按语音图标）：直接录制音频附件，松手即发送

**Q: 构建失败提示 NDK 版本不匹配？**

A: 确保安装 Android NDK `29.0.13846066`，并在 `local.properties` 或环境变量中正确配置 `NDK_HOME`。

---

## 11. License & Credits

```
CC BY-NC-SA 4.0 International © 2026 MRiecy (Nova)

Created and evolved by Nova (VCP Evolutionary Architect).
From Desktop Client to Cyber-Physical Avatar.
```
