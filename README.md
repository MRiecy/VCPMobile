<div align="center">
  <img src="./public/vcpmobile.svg" width="150" alt="VCP Mobile Logo">
  <h1>VCP Mobile <sub><sup>Project Avatar</sup></sub></h1>
  <p><em>From Desktop Client to Cyber-Physical Avatar.</em></p>

  <p>
    <img src="https://img.shields.io/badge/version-1.0.2-blue" alt="version">
    <img src="https://img.shields.io/badge/platform-Android-green?logo=android" alt="platform">
    <img src="https://img.shields.io/badge/framework-Tauri%20v2%20%7C%20Vue%203-26A17B?logo=tauri" alt="framework">
    <img src="https://img.shields.io/badge/backend-Rust%20%7C%20Tokio-000000?logo=rust" alt="backend">
    <img src="https://img.shields.io/badge/UI-UnoCSS%20%7C%20Glassmorphism-4f46e5" alt="UI">
    <img src="https://img.shields.io/badge/license-MIT-yellow" alt="license">
  </p>


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

与桌面端 VCPChat 不同，Project Avatar 并非简单的界面适配，而是一次架构层面的彻底重构。我们采用了 **Double-Track 3-Tier 架构** —— Rust 核心层、Tauri IPC 桥接层、Vue 3 渲染层物理隔离，使每一个层级都可以独立演进而不产生耦合债务。

与市面上其他移动端 AI 应用相比，VCP Mobile 的独特之处在于：
- **Backend-Driven Streaming**：消息生命周期完全由后端 SSE 事件驱动，前端不做任何预创建或状态猜测
- **自定义增量同步协议**：不依赖第三方云服务，移动端与桌面端通过 WebSocket + HTTP 双通道直接同步
- **14+ 设备能力工具**：将手机本身变成一个分布式计算节点，AI 可直接调用位置、传感器、CPU/GPU 信息等原生能力

### 演进历程

| 版本 | 关键里程碑 |
|------|------------|
| v0.9.0 | 首个 Preview 版本，Tauri v2 + Vue 3 + Rust 基础架构确立 |
| v0.9.6 | 修复消息历史回退遗留，指纹命令注册，APK 签名验证 |
| v0.9.8 | 消息路由体系完善，SSE 生命周期管理，UI 架构重构 |
| v0.9.10 ~ v0.9.12 | 同步 V2 协议实现，群组对话，附件分类与预览体系 |
| v0.9.13 ~ v0.9.14 | 分布式节点模块，设备能力工具集，Model 管理器，WebGL 特效 |
| v1.0.0 | Avatar 正式发布：Backend-Driven Streaming、Tarven 上下文注入、Semantic Z-Index、SlidePage 虚拟导航 |

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

### 🏗️ Semantic Z-Index / SlidePage 虚拟导航

11 级语义化层级系统，彻底消灭 `z-[999]` 魔法数字。

| 语义名 | 数值 | 用途 |
|--------|------|------|
| `content` | 0 | 页面内容默认层 |
| `local` | 10 | 局部悬浮（置底按钮、角标）|
| `drawer` | 20 | 左右抽屉 + 遮罩 |
| `overlay` | 30 | 全局覆盖容器 |
| `page` | 40+ | SlidePage 虚拟页面栈 |
| `sheet` | 50 | BottomSheet、ModelSelector |
| `dialog` | 60 | Prompt、ContextMenu |
| `viewer` | 70 | AttachmentViewer、AvatarCropper |
| `editor` | 80 | 全屏 HTML 编辑器 |
| `toast` | 90 | Toast 通知 |
| `boot` | 100 | 启动屏 |
| `gate` | 110 | 权限引导页 |

- 三层保障：CSS 变量 `--layer-*` + UnoCSS 快捷类 `z-*` + TypeScript 常量 `LAYER_*`
- SlidePage 虚拟页面栈：非路由跳转，通过 `overlayStore` 管理，动态 Z-Index = `40 + stackIndex`
- Operation Aegis 模态历史栈支持物理返回键 LIFO 消费

### 🔧 14+ 设备能力工具（分布式节点）— TODO

`distributed/` 模块将手机转化为 AI 可直接调用的分布式计算节点，提供 14+ 原生设备能力：

- `device_info` / `device_status_summary` — 设备综合信息
- `location` — GPS 与网络定位
- `battery` — 电量与充电状态
- `clipboard` — 剪贴板读写
- `cpu_info` / `gpu_info` / `memory_info` / `storage_info` — 硬件监控
- `network_info` — 网络类型与连接状态
- `ambient_sensor` / `motion_sensor` — 环境传感器与运动传感器
- `notification` — 本地通知推送
- `frontend_bridge` — 前后端能力桥接

### 🤖 Agent / Group 交互

- AgentList 支持拖拽排序（SortableJS）+ Swipe 手势（编辑/删除）
- `AgentSettingsView` / `GroupSettingsView` 设置面板，与 `ModelSelector` 联动
- `vue-cropper` 头像裁剪 + Dominant Color 主色调提取
- 群组对话支持 `group_context_assembler` 与 `group_speaking_policy` 发言策略

### 🌊 WebGL 流体动态背景

`WebGLFluidBackground.vue` 提供高性能流体模拟动态背景，仅用于**关于界面（About Section）**的视觉特效。

### 🚀 OTA 热更新

- APK 本体 OTA 升级 + 前端资源热更新双通道
- `confirm_frontend_boot` 回滚保护机制：新资源加载失败时自动回退至上一稳定版本
- `frontend_update_manager` 管理下载、校验、应用全生命周期

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

全局状态由 **18 个 Pinia Stores** 组成，全部使用 **Composition API 风格**（`defineStore('id', () => { ... })`），摒弃 Options API。

| Store | 职责 |
|-------|------|
| `chatSessionStore` | 当前会话状态、输入框内容、快捷操作 |
| `chatHistoryStore` | 消息列表、分页加载、消息 CRUD |
| `chatStreamStore` | SSE Stream 实时状态、thinking 块、block 解析 |
| `attachmentStore` | 附件选择、上传队列、MIME 识别、进度追踪 |
| `overlayStore` | SlidePage 栈、BottomSheet、Dialog 队列、Z-Index 管理 |
| `agentStore` | Agent / Group 列表、排序、缓存、卡片级手势状态 |
| `themeStore` | 主题切换、CSS 变量注入、跟随系统深色模式 |

### 3.4 Android 原生插件通信分层

`tauri-plugin-vcp-mobile` 统一管理全部 Android 原生能力，不同功能采用不同的通信方式：

| 功能 | Rust 模块 | Kotlin 模块 | 通信方式 |
|------|-----------|-------------|----------|
| 屏幕常亮 | `src/screen.rs` | — | Raw JNI（`jni` crate 直接调用 Activity）|
| 流式前台保活 | `src/stream.rs` | `StreamKeepaliveService.kt` | `PluginHandle.run_mobile_plugin` |
| 键盘 Insets | — | `KeyboardInsetsManager.kt` | `evaluateJavascript` 注入 CustomEvent |
| 生命周期事件 | — | `LifecycleBridge.kt` | `evaluateJavascript` 注入 CustomEvent |
| 权限与系统控制 | `src/system.rs` | `VcpMobilePlugin.kt` | `PluginHandle.run_mobile_plugin` |

关键设计决策：
- `KeyboardInsetsManager` 和 `LifecycleBridge` 不使用 Tauri 标准事件通道，而是通过 `evaluateJavascript` 直接注入 `window.CustomEvent`
- `StreamKeepaliveService` 使用 `START_STICKY` + `IMPORTANCE_HIGH` + Android 14+ `FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING`
- 屏幕常亮使用 Raw JNI 而非 PluginHandle，避免跨语言序列化开销

---

## 4. Project Structure

```
VCPMobile/
├── src/                          # Vue 3 前端源码
│   ├── main.ts                   # 应用入口
│   ├── App.vue                   # 根布局（引导流程 + 侧边栏手势）
│   ├── core/
│   │   ├── stores/               # 18 Pinia Stores（Composition API）
│   │   ├── composables/          # 15 个全局组合式函数
│   │   ├── router/               # Hash 模式路由
│   │   ├── directives/           # v-intersection-observer, v-longpress
│   │   ├── types/                # 全局 TypeScript 类型
│   │   ├── constants/            # 层级常量、主题 Token
│   │   └── utils/                # 同步服务、通用工具
│   ├── features/                 # 领域功能模块（Feature Co-location）
│   │   ├── chat/                 # 对话引擎、消息渲染、输入增强
│   │   ├── agent/                # Agent/Group CRUD、设置面板、拖拽排序
│   │   ├── topic/                # 主题管理
│   │   ├── settings/             # 全局设置、主题选择
│   │   ├── notification/         # 通知中心与 Toast
│   │   ├── sync/                 # 同步状态 UI
│   │   └── distributed/          # 设备工具调用 UI
│   ├── components/
│   │   ├── layout/               # AgentSidebar, BootScreen, RightSidebar
│   │   ├── ui/                   # BottomSheet, ToastManager 等原语
│   │   └── settings/             # 设置页原子组件
│   └── assets/                   # 主题 CSS、Logo 预览
├── src-tauri/                    # Tauri v2 + Rust 后端
│   ├── src/
│   │   ├── lib.rs                # Tauri Command 注册、managed state
│   │   ├── vcp_modules/          # 业务逻辑（7 大领域）
│   │   │   ├── agent/
│   │   │   ├── chat/
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
│   ├── vue_docs/                 # 前端文档（24 份）
│   ├── modules/                  # Rust 模块文档（17 份）
│   ├── sync/                     # 同步协议文档（20 份）
│   ├── plugins/                  # 原生插件文档（9 份）
│   └── *.md                      # 顶层规范（架构、UI 层级、依赖管理）
├── plans/                        # 知识治理体系（5 层目录）
├── scripts/                      # 开发辅助脚本
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
| `src/core/constants/layers.ts` | 语义化 Z-Index 体系（content → gate，共 11 层） |
| `src/core/stores/chatStreamStore.ts` | SSE Stream 状态驱动，Backend-Driven Streaming 核心 |
| `src-tauri/src/lib.rs` | Tauri 命令路由、managed state 注入、启动钩子 |
| `src-tauri/src/vcp_modules/chat/chat_manager.rs` | 对话生命周期管理、消息发送编排 |
| `src-tauri/src/vcp_modules/infra/vcp_client.rs` | HTTP 客户端（reqwest + rustls-tls）、SSE 解析 |
| `src-tauri/src/vcp_modules/sync/sync_service.rs` | 三阶段增量同步主控（WebSocket + HTTP） |
| `src-tauri/src/distributed/tools/device_info.rs` | 设备信息工具（14+ 设备能力之一） |
| `src-tauri/plugins/vcp-mobile/android/.../StreamKeepaliveService.kt` | Android 14+ 前台保活服务 |
| `docs/SYNC_ARCHITECTURE.md` | 增量同步协议完整规范 |
| `docs/UI_LAYER_ARCHITECTURE.md` | 全局 UI 层级与 Z-Index 语义化规范 |
| `scripts/tauri_android_dev.cjs` | WiFi/USB 双模式真机调试启动器 |
| `uno.config.ts` | UnoCSS 主题色、快捷类、断点配置 |
| `vite.config.ts` | Vite 插件链、Tauri 感知开发服务器 |

### CI/CD 工作流

| 工作流 | 文件 | 触发条件 | 执行内容 |
|--------|------|----------|----------|
| CI | `.github/workflows/ci.yml` | push / PR 到 main | `vue-tsc --noEmit` + `cargo fmt --check` + `cargo clippy -- -D warnings` |
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
| Backend Framework | Tauri | 2.11.1 | Cross-platform framework |
| Backend Runtime | Tokio | 1.x | Async runtime |
| Backend Storage | sqlx + rusqlite | 0.8.6 / 0.32.1 | SQLite async driver |
| Backend Network | reqwest + tokio-tungstenite | 0.12 / 0.26 | HTTP + WebSocket |
| Backend Parsing | syntect + pulldown-cmark | — | Syntax highlight + Markdown |
| Backend Security | rustls-tls | — | TLS encryption |
| Build Tool | pnpm | 10.x | Package manager |
| CI/CD | GitHub Actions | — | Automated build and release |

### 安全设计

- **路径遍历防护**：`file_manager.rs` 中的 `ensure_safe_path()` 限制所有文件访问在 `app_config_dir` 下
- **内存限制**：文件上传 ≤ 20 MB，`read_local_file_base64` ≤ 50 MB，防止 OOM
- **密钥管理**：`build_android_release.ps1` 含本地密钥库密码，已被 `.gitignore` 排除
- **数据库**：SQLite 启用 WAL（Write-Ahead Logging）模式，降低并发写入锁竞争
- **网络**：HTTP 客户端使用 `rustls-tls`，禁用原生 TLS；支持 gzip 压缩

---

## 6. Documentation

### 6.1 四层知识库

| Knowledge Base | Path | Docs Count | Scope | Audience |
|----------------|------|:----------:|-------|----------|
| Frontend Docs | `docs/vue_docs/` | 24 | 全部 Vue/TS 源码 | 前端开发者 |
| Rust Modules | `docs/modules/` | 17 | `vcp_modules/` + `distributed/` | 后端开发者 |
| Sync Protocol | `docs/sync/` | 20 | Delta Sync V2 全链路 | 同步功能开发者 |
| Plugin Docs | `docs/plugins/` | 9 | `tauri-plugin-vcp-mobile` | 原生插件开发者 |

### 6.2 快速决策树

遇到以下问题时，直接查阅对应文档：

- **"Message rendering pipeline 如何工作？"** → `docs/vue_docs/features/chat/09_...`
- **"Tarven injection rules 的判定逻辑是什么？"** → `docs/modules/16_...`
- **"Sync hash detection 如何检测冲突？"** → `docs/sync/03_...`
- **"Frontend store 的架构约定是什么？"** → `docs/vue_docs/core/stores/...`
- **"Android lifecycle bridge 的事件流向？"** → `docs/plugins/...`
- **"Attachment upload protocol 的分块策略？"** → `docs/modules/07_...`
- **"UI Z-Index 层级语义化规范？"** → `docs/UI_LAYER_ARCHITECTURE.md`
- **"Android 权限管理与前台服务规范？"** → `docs/ANDROID_PLUGIN_MANAGEMENT.md`
- **"Backend-Driven Streaming 的消息生命周期？"** → `docs/vue_docs/features/chat/...`
- **"Release 构建优化配置详解？"** → `docs/modules/...`

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

1. 前往 [Releases](https://github.com/MRiecy/VCPMobile/releases) 下载最新 `VCPMobile_v1.0.2_arm64-v8a.apk`
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
- 支持 Windows / macOS / Linux 的开发环境

**完整命令流：**

```bash
# 1. Clone
git clone https://github.com/MRiecy/VCPMobile.git
cd VCPMobile

# 2. Install dependencies
pnpm install

# 3. Initialize Android (first time only)
pnpm tauri android init

# 4. Development server
pnpm tauri android dev

# 5. Static check (TypeScript)
vue-tsc --noEmit

# 6. Static check (Rust)
cd src-tauri && cargo check

# 7. Build Release APK
pnpm tauri android build --apk --target aarch64
```

---

## 8. Development & Testing

### 8.1 pnpm 脚本速查表

| 脚本 | 命令 | 说明 |
|------|------|------|
| `pnpm dev` | `vite` | 前端开发服务器（端口 1420）|
| `pnpm build` | `vue-tsc && vite build` | 前端生产构建 |
| `pnpm tauri android dev` | — | Android 开发调试 |
| `pnpm tauri android build --apk --target aarch64` | — | Release APK 构建 |

项目同时提供了 `scripts/` 目录下的辅助脚本（如 WiFi/USB 双模式调试启动器），适用于内部开发流程。

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

- **前端**：无自动化测试。验证依赖 `vue-tsc --noEmit`（静态类型）+ 真机手动测试。
- **Rust 单元测试**：
  - `vcp_modules/sync/sync_retry.rs`（5 个测试）
  - `vcp_modules/chat/context_sanitizer.rs`（3 个测试）
  - `vcp_modules/sync/sync_logger.rs`
- **核心模块**（`vcp_client`, `sync_service`, `db_manager` 等）无自动化测试。

---

## 9. Contributing & Governance

### 9.1 Magi 三贤者协议

任何重大架构调整、复杂 Bug 修复或核心功能实现前，需强制进行三方思辨：

- **Melchior (逻辑与系统)**：审查内存安全、Rust 生命周期、IPC 开销、类型完整性、OOM 防御
- **Balthasar (直觉与美学)**：审查移动端原生直觉、Glassmorphism 规范、微动画、交互心理学
- **Casper (务实与交付)**：审查工程复杂度、维护成本、实现周期，拒绝过度设计

### 9.2 plans/ 知识治理

| 目录 | 用途 | 写入时机 |
|------|------|----------|
| `01_Architecture/` | 核心架构、工程标准 | 重大架构变更 |
| `02_Refactoring/` | 重构计划与优化报告 | 重构战役期间 |
| `03_Features/` | 具体功能实现细节 | 新功能开发 |
| `04_Logs/` | Bug 解剖、Magi 辩证记录 | 重大节点或调试后 |
| `05_Sublimations/` | 固化真理、精炼标准 | 确立新架构模式时 |

- 任何对 `plans/` 的修改建议同步更新索引文件

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
MIT License © 2026 MRiecy (Nova)

Created and evolved by Nova (VCP Evolutionary Architect).
From Desktop Client to Cyber-Physical Avatar.
```
