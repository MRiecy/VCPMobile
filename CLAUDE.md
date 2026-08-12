# AGENTS.md — VCP Mobile (Project Avatar)

> 本文件面向中文 AI 编程代理。阅读者被假设对该项目一无所知。所有信息均基于项目实际内容，未经验证的假设已被剔除。

---

## ⛔ 0. 工作区隔离铁律

> 工作空间包含多个项目，但**只有 `G:\VCPMobile` 是当前开发目标**：
>
> | 目录            | 规则                 |
> | --------------- | -------------------- |
> | `G:\VCPMobile`  | ✅ 可读可写           |
> | `G:\VCPToolBox` | ❌ 参考工程，禁止修改 |
> | `G:\VCPChat`    | ❌ 参考工程，禁止修改 |
>
> 参考目录中的代码再有问题也不要动。你的世界只有 `G:\VCPMobile`。

---

## 1. 项目概览

**VCP Mobile**（代号：Project Avatar）是 VCPChat 的移动端版本，基于 **Tauri v2 + Vue 3 + Rust**，生产目标仅为 Android `arm64-v8a`（`aarch64-linux-android`，minSdk 26）触控手机、平板与按当前窗口宽度响应的折叠屏。架构上为 Rust 核心层、Tauri IPC 层、Vue 3 渲染层三层隔离。

**平台边界（强约束）**：不支持 Windows/macOS/Linux 桌面端、iOS、Android TV 或其他 ABI；桌面用户使用 VCPChat。Vite 预览、host Rust 编译、非 Android fallback 和 Tauri desktop scaffold 仅用于开发/测试，不是产品入口。不得为“桌面兼容”增加业务分支，也不得删除 Android 子命令仍依赖的通用 `pnpm tauri` 脚本。完整 UI 契约见 `docs/ANDROID_UI_COMPATIBILITY.md`。

**Web 生成契约**：`vite.config.ts` 的 `build.target` 与 `cssTarget` 固定为 `chrome87`，防止工具链升级静默抬高语法门槛；该值不是最低受支持 WebView 的替代声明，关键 CSS 仍须 fallback，最终以具名 Android 设备证据为准。

- 版本：`1.1.4`
- 包名：`com.vcp.avatar`

---

## 2. 项目结构

```
VCPMobile/
├── src/                        # Vue 3 前端源码
│   ├── main.ts                 # 应用入口
│   ├── App.vue                 # 根布局
│   ├── core/                   # 基础设施
│   │   ├── stores/             # Pinia Store
│   │   ├── composables/        # 全局组合式函数
│   │   ├── router/             # Hash 路由
│   │   ├── directives/         # 自定义指令
│   │   ├── types/              # 共享类型
│   │   ├── constants/          # 常量（如 Z-Index）
│   │   └── utils/              # 工具函数
│   ├── features/               # 按领域划分的功能模块
│   │   ├── chat/               # 对话引擎
│   │   ├── agent/              # 智能体管理
│   │   ├── topic/              # 话题管理
│   │   ├── notification/       # 通知与 Toast
│   │   ├── settings/           # 全局设置
│   │   ├── sync/               # 同步前端
│   │   ├── distributed/        # 分布式能力
│   │   ├── rag/                # RAG 灵视中心
│   │   └── assistant/          # 浮动助手
│   ├── components/             # 共享组件
│   │   ├── layout/             # 布局外壳
│   │   ├── settings/           # 设置页原子组件
│   │   └── ui/                 # 通用 UI 原语
│   └── assets/                 # 主题与静态资源
├── src-tauri/                  # Tauri v2 + Rust 后端
│   ├── src/
│   │   ├── main.rs             # host 开发/测试 scaffold（非产品入口）
│   │   ├── lib.rs              # 命令注册与启动流程
│   │   ├── vcp_modules/        # 业务逻辑（按领域组织）
│   │   │   ├── agent/          # 智能体领域
│   │   │   ├── chat/           # 对话领域
│   │   │   ├── group/          # 群组领域
│   │   │   ├── infra/          # 基础设施
│   │   │   ├── persistence/    # 持久化
│   │   │   ├── sync/           # 同步
│   │   │   └── updater/        # 更新
│   │   └── distributed/        # 分布式计算与设备工具
│   │       ├── client.rs, tool_registry.rs, types.rs
│   │       └── tools/          # 设备能力工具
│   ├── Cargo.toml              # Rust 依赖与构建配置
│   ├── tauri.conf.json         # Tauri 配置
│   └── plugins/
│       └── vcp-mobile/         # Android 原生插件（详见 §2.2）
│           ├── src/            # 插件 Rust 侧
│           ├── android/        # 插件 Kotlin 侧
│           ├── guest-js/       # 前端调用封装
│           └── permissions/    # Tauri v2 权限声明
├── docs/                       # 技术文档体系（详见 §2.1）
│   ├── vue_docs/               # 前端知识库
│   ├── modules/                # Rust 模块文档
│   ├── sync/                   # 同步子系统文档
│   ├── plugins/                # 插件文档
│   ├── ANDROID_PLUGIN_MANAGEMENT.md
│   ├── ANDROID_UI_COMPATIBILITY.md
│   ├── UI_LAYER_ARCHITECTURE.md
│   ├── DEPENDENCY_MANAGEMENT.md
│   └── SYNC_ARCHITECTURE.md
├── .github/workflows/          # CI / Release
│   ├── ci.yml
│   └── release.yml
├── package.json                # 前端依赖与脚本
├── vite.config.ts              # Vite 配置
├── uno.config.ts               # UnoCSS 配置
└── tsconfig.json               # TypeScript 配置
```

### 2.1 文档体系三层结构

`docs/` 不是杂乱的笔记集合，而是按主题严格分层的技术知识库：

| 子目录           | 覆盖范围                                                     |           文档数            | 阅读对象                      |
| ---------------- | ------------------------------------------------------------ | :-------------------------: | ----------------------------- |
| `docs/vue_docs/` | `src/` 全部 Vue 3 / TypeScript 前端源码                      |             27              | 前端开发者、UI 调试者         |
| `docs/modules/`  | `src-tauri/src/vcp_modules/` + `src-tauri/src/distributed/` 中**长期稳定、低修改频率**的核心模块 | 31（含 ast-diff 专栏 6 份） | 新成员快速了解基础设施        |
| `docs/sync/`     | 同步 V2 子系统全链路（WebSocket + HTTP + SHA-256 Hash 差异） |             20              | 同步功能开发者                |
| `docs/plugins/`  | `src-tauri/plugins/vcp-mobile/` 全部功能模块的**代码级**说明 |             13              | 维护 Android 原生插件的开发者 |

四者与 `docs/*.md` 顶层规范文档的关系：

- `docs/vue_docs/`：回答"前端状态、组件交互、Pinia 数据流是**怎么**做的"
- `ANDROID_PLUGIN_MANAGEMENT.md`：回答"**应该**怎么做"（开发规范、权限准则、选型指导）
- `docs/plugins/`：回答"代码是**怎么**做的"（接口签名、数据流、实现细节）
- `UI_LAYER_ARCHITECTURE.md`：全局 UI 层级规范，与 `src/core/constants/layers.ts` + `uno.config.ts` + `src/assets/themes.css` 三重机制对应
- `ANDROID_UI_COMPATIBILITY.md`：Android arm64 产品边界、窗口宽度响应式、CSS/Insets 与设备证据的权威规范

### 2.2 Android 原生插件（`tauri-plugin-vcp-mobile`）

项目用**单一自定义插件** `tauri-plugin-vcp-mobile` 统一管理全部 Android 原生能力，源码位于 `src-tauri/plugins/vcp-mobile/`。

#### 插件入口与命令注册

- `src/lib.rs` 是插件唯一入口，注册全部 Tauri 命令（当前 41 条，详见 `docs/plugins/01_插件初始化与命令路由.md` §4）。划词助手的 `request_overlay_permission` / `toggle_floating_ball` 实现作为技术资产保留，但不再注册为可调用命令。
- 命令按领域分为三个 Rust 模块：
  - `screen.rs`：屏幕常亮（2 条）
  - `stream.rs`：流式保活、前台锁、分布式保活、`:helper` 进程 SSE 代理（5 条以上）
  - `system.rs`：权限、系统控制、硬件状态、文件选择、Root 等（30 条以上）
- `setup` 中注册 Android 插件 `com.vcp.mobile / VcpMobilePlugin`，将 `PluginHandle` 注入 `VcpMobileState`，供后续 `run_mobile_plugin` 调用 Kotlin 方法。
- **新增命令必须四重注册**：`lib.rs` 的 `invoke_handler` → `build.rs` 的 `COMMANDS` 数组 → `permissions/*.toml` → `guest-js/index.ts`。参见 §2.3。

#### 通信方式分层

前端与原生层有四条独立通道：

| 通道                                                         | 典型功能                                                     | 数据流                                                       | 前端接收方式                                         |
| ------------------------------------------------------------ | ------------------------------------------------------------ | ------------------------------------------------------------ | ---------------------------------------------------- |
| **Tauri invoke → Rust command → `run_mobile_plugin` → Kotlin** | 权限请求、流式保活、电池/网络/CPU 状态、文件选择、Root 命令等 | `invoke('plugin:vcp-mobile\|xxx')` → `lib.rs` 路由 → `screen.rs`/`stream.rs`/`system.rs` → `PluginHandle.run_mobile_plugin("method", payload)` → `VcpMobilePlugin.kt` | `invoke` 返回值 / `Promise`                          |
| **Raw JNI**                                                  | 屏幕常亮                                                     | `screen.rs` 直接通过 `jni` crate 调用 `activity.getWindow().add/clearFlags(FLAG_KEEP_SCREEN_ON)` | `invoke` 返回值                                      |
| **`evaluateJavascript` → `window.CustomEvent`**              | 键盘 Insets                                                  | `KeyboardInsetsManager` 监听 `WindowInsets` 后注入 `vcp-keyboard-inset` 事件 | `window.addEventListener('vcp-keyboard-inset', ...)` |
| **`plugin.trigger("lifecycle")` → Rust → `app.emit`**        | 前后台生命周期                                               | `LifecycleBridge` 进程级观察 → Rust 监听内部通道 → 发射 `vcp-lifecycle-changed` | `listen('vcp-lifecycle-changed', ...)`               |

> **注意**：生命周期事件 v1.1.2 后已改为 Tauri 事件通道，不再走 `evaluateJavascript`。旧版 2.2 中"生命周期事件用 evaluateJavascript"的描述已过时。

#### 核心组件职责

| 组件                             | 位置                                                    | 职责                                                         |
| -------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------ |
| `VcpMobileState`                 | `src/lib.rs`                                            | 持有 `PluginHandle`；非 Android host fallback 用 `PhantomData` 占位（仅开发/测试） |
| `VcpMobilePlugin`                | `android/.../VcpMobilePlugin.kt`                        | Kotlin 侧命令路由；持有 Battery/CPU/GPU/Network/Sensor/FloatingWindow/ShareIntent 等管理器 |
| `ForegroundGuardian`             | `android/.../service/ForegroundGuardian.kt`             | 进程级单例；统一调度 WakeLock + WifiLock + 前台服务；四级优先级（SYNC=40 / PRERENDER=30 / STREAM=20 / DISTRIBUTED=10） |
| `StreamKeepaliveService`         | `android/.../service/StreamKeepaliveService.kt`         | 前台服务"通知壳"，只负责 `startForeground()`；锁管理已移交 ForegroundGuardian |
| `SseProxyService`                | `android/.../service/SseProxyService.kt`                | 运行在独立 `:helper` 进程；本地 TCP + SSE 代理 + 断线内存缓存；自行管理双锁 |
| `KeyboardInsetsManager`          | `android/.../KeyboardInsetsManager.kt`                  | 监听软键盘 Insets，推送 `vcp-keyboard-inset` 事件            |
| `LifecycleBridge`                | `android/.../LifecycleBridge.kt`                        | 通过 `ProcessLifecycleOwner` 进程级观察生命周期，经 Rust 中转给前端 |
| 硬件状态管理器                   | `android/.../BatteryStatusManager.kt` 等                | 采集电量、CPU 热状态、GPU、网络、传感器等                    |
| `VcpNotificationListenerService` | `android/.../service/VcpNotificationListenerService.kt` | 通知栏监听占位服务（当前不做实际处理，保护隐私）             |

#### 关键设计决策

- **所有主进程侧保活统一走 `ForegroundGuardian`**：通过 `acquire(tag, priority, label, screenKeepOn)` / `release(tag)` 申请与释放。`StreamKeepaliveService` 不再自行管理锁。
- **`ForegroundGuardian` 是进程级单例**：同一 tag 重复 `acquire` 幂等覆盖；最后一个消费者退出时自动释放物理锁并停止前台服务；支持超时自动释放（默认 10–120 分钟）。
- **屏幕常亮与 `ForegroundGuardian` 解耦**：屏幕常亮由前端 `useScreenKeeper` 引用计数 + Raw JNI 独立管理；`ForegroundGuardian` 只在消费者 `screenKeepOn=true` 时通过 Activity 生命周期协同添加 Window flag。
- **`StreamKeepaliveService.stopWithTask=true`**：进程被用户划掉后服务随 Task 停止，避免僵尸通知。`:helper` 进程的 `SseProxyService` 因自行管理锁，保留 `stopWithTask=false`。
- **权限/保活/硬件命令走 `run_mobile_plugin`**；屏幕常亮走 Raw JNI；键盘 Insets 走 `evaluateJavascript`；生命周期走 `plugin.trigger` → Rust → Tauri Event。选择通信方式时先看事件频率与可靠性要求。

#### 排查参考

- 命令注册表与数据流：`docs/plugins/01_插件初始化与命令路由.md`
- 屏幕常亮：`docs/plugins/02_屏幕常亮控制.md`
- 流式保活与 `ForegroundGuardian`：`docs/plugins/03_流式前台保活服务.md`、`docs/plugins/11_ForegroundGuardian_前台守护者.md`
- 键盘 Insets：`docs/plugins/04_键盘Insets管理.md`
- 生命周期桥接：`docs/plugins/05_生命周期桥接.md`
- 权限与系统控制：`docs/plugins/06_权限与系统控制.md`
- Guest JS API：`docs/plugins/07_Guest JS API.md`
- 总览与导航：`docs/plugins/00_总览与导航.md`

### 2.3 插件权限声明（`permissions/`）

Tauri v2 要求每个 command 被 capability 显式授权。新增插件命令时，必须同时更新以下三处，否则前端调用会报 `not allowed`：

1. `src-tauri/plugins/vcp-mobile/src/lib.rs` 的 `invoke_handler`
2. `src-tauri/plugins/vcp-mobile/permissions/default.toml` 和 `all.toml` 的 `commands.allow`
3. `src-tauri/plugins/vcp-mobile/build.rs` 的 `COMMANDS` 数组

修改后执行 `pnpm check` 重新生成 autogenerated 文件和 schema。若从 `lib.rs` 移除命令，应手动删除对应的 `permissions/autogenerated/commands/<命令名>.toml`，避免残留僵尸权限。

> 本项目 capability 引用的是 `vcp-mobile:allow-all`，运行时真正生效的是 `all.toml`。`default.toml` 仅作为插件默认权限包存在。

---

## 3. 构建与开发命令

所有命令均在 Windows PowerShell 5.1 环境下执行。项目使用 **pnpm** 作为包管理器。

```powershell
# 完整静态检查（TypeScript + Rust）
pnpm check                  # 等价于 vue-tsc --noEmit && cd src-tauri && cargo check --locked

# Android 真机调试 — USB 模式（纯数据线，无需 WiFi/热点）
pnpm tauri android dev
```

### Rust 发布优化

`src-tauri/Cargo.toml` 的 `[profile.release]` 启用了极度激进的体积与性能优化：

- `opt-level = 3`
- `lto = true`
- `codegen-units = 1`
- `panic = "abort"`
- `strip = true`

---

## 4. 代码风格与约定

### 4.1 前端（Vue / TypeScript）

- **语法**: 强制使用 `<script setup>`。
- **样式**: **优先**使用 **UnoCSS** 原子类；复杂场景允许传统 CSS。全局消息块样式位于 `src/assets/message-blocks.css`；交互型组件（如 `ToolBlock.vue`、`ThoughtBlock.vue`）允许使用 `<style scoped>` 管理自身视觉与动画。UnoCSS 快捷类（`glass-panel`, `card`, `btn`）仍推荐使用。
- **组件命名**: PascalCase（`ChatView.vue`, `VcpAvatar.vue`）。
- **组合式函数**: `use` + PascalCase（`useContentProcessor.ts`）。
- **状态管理**: Pinia，全部使用 **Composition API 风格**（`defineStore('id', () => { ... })`），而非 Options API。
- **目录组织**: 功能共置（Feature Co-location）。组件、组合式函数、类型、工具函数放在同一 `features/<domain>/` 目录下。
- **路由**: 本质上是单页应用，Hash 模式。大部分“导航”通过抽屉、Overlay 和 Teleport 实现，而非路由切换。

### 4.2 后端（Rust）

- **模块边界**: 业务逻辑必须位于 `src-tauri/src/vcp_modules/`。`lib.rs` 仅作为命令路由与启动钩子挂载点。
- **错误处理**: Tauri command handler 中**严禁使用 `unwrap()` 或 `expect()`**。必须转换为 `Result<T, String>`（`map_err(|e| e.to_string())?`）。
- **异步 IO**: 所有网络、文件、数据库操作必须异步，基于 `tokio`。
- **状态共享**: 使用 Tauri `app.manage(...)` 注入单例状态。并发结构偏好 `DashMap`/`DashSet`、`tokio::sync::RwLock`、`AtomicU32`。

### 4.3 跨层与工程纪律

- **修改后的强制检查**: 任何涉及跨层或核心逻辑的变更后，必须运行 `pnpm check`。若未通过，必须先修复静态错误再继续。
- **现有文件修改原则**: 对于任何已存在的文件，哪怕只改 1 行，**严禁使用全文件覆盖**，必须使用原子级、外科手术式的编辑（如 `replace`），以防幻觉灾难。
- **Shell 安全（Windows）**:
  - 禁止使用 Unix 风格链式操作符（`&&`, `||`）。使用 `;` 分隔无关命令。
  - 禁止使用 PowerShell/CMD 重定向（`>`, `>>`）写入 UTF-8 内容。请使用自带的代码编辑工具。

### 4.4 UI 美学宪法（生产力优先极简主义）

项目对 UI 有极其明确的约束，并非普通移动端应用：

- **高密度线性布局**: 列表重于卡片。严禁大面积圆角（>1.5rem）与厚重投影。
- **技术精确感**: ID、UUID、状态码强制使用 Monospace 字体。灰度优先，彩色仅用于状态指示（黄=收藏、蓝=选中、红=错误）。
- **内敛交互**: 禁止大幅度缩放与弹跳。使用 2px 高亮侧边条（Accent Bar）或透明度变化作为反馈。
- **禁止毛玻璃 / backdrop-blur**: 严禁在滚动容器、卡片、列表项或任何内容区域使用 `backdrop-filter: blur()`（含 `-webkit-backdrop-filter`）。毛玻璃在移动端 GPU 上的开销极大。**仅允许**在 fixed/sticky 单例元素（如顶部导航 Floating Island）上使用轻度 blur（≤ 12px），且必须在 DevTools Performance 验证无 Composite Layer 尖峰后方可保留。

---

## 5. 测试策略

本项目已建立覆盖 Rust 后端、Tauri 插件、Vue 前端、Android E2E 与性能诊断的分层测试体系。全局架构约束文件见 `docs/Test_Architecture_Constraints.md`。新增前端、Android JVM 与仓库级集成测试应归入既有测试目录；既有 Rust `#[cfg(test)]` 内联单测保留原位。禁止以"方便测试"为由修改业务接口。

### 5.1 测试分层（L1-L8）

| 层级 | 名称                  | 覆盖范围                                     | 触发频率        | 主要工具                           |
| ---- | --------------------- | -------------------------------------------- | --------------- | ---------------------------------- |
| L1   | Rust 内联单测         | 纯函数/算法/DTO/状态机                       | 每次 PR         | cargo test                         |
| L2   | Rust 集成测试         | 文件提取与固定 fixture 边界                  | 每次 PR         | cargo test, tempfile              |
| L3   | Android 插件 JVM 单测 | Kotlin 纯逻辑 / Robolectric Shadow           | 每次 PR         | JUnit 4, Robolectric, MockK        |
| L4   | 前端组件/Store 测试   | Vue 原子组件, Pinia Store                    | 每次 PR         | Vitest, @vue/test-utils, happy-dom |
| L5   | 契约测试              | Rust 命令↔TS 调用 / 权限声明 / Kotlin 方法名 | 每次 PR         | 文本/反射快照测试                  |
| L6   | Android 仪器测试      | Service/Activity/权限生命周期                | 手工按需        | AndroidX Test                      |
| L7   | Android E2E Smoke     | 真机关键旅程与多窗口 UI                      | release 前具名验收 | Node.js + adb、人工触控/截图     |
| L8   | 性能/稳定性           | 启动/APK 体积/Criterion 基准/长稳 soak       | 手工按需        | cargo bench, adb scripts           |

### 5.2 Rust 后端测试（workspace lib 测试 + 集成目标测试 + Criterion 基准）

**内联单元测试**（`src-tauri/src/vcp_modules/` 内 `#[cfg(test)] mod tests`）：

- 当前测试覆盖解析、并发 owner/epoch、生命周期、同步、持久化、更新与文件边界；总数以 `cargo test --locked --lib -- --list` 为准。
- 纯函数优先；fixture 使用 `include_str!` / `include_bytes!` 编译期内嵌，**严禁绝对路径**
- 新增测试优先补纯逻辑模块（如 `sync_hash`、`sync_types`、`sync_dto`、`file_extractor` helper），而非需要 mock AppHandle/DB/网络的模块

**集成测试**（`src-tauri/tests/`）：

- `file_extractor_integration.rs`：DOCX/XLSX/PDF/PPTX 真实样本提取 + BOM 编码 + OOM 防护
- fixture 样本：`src-tauri/tests/fixtures/file_extractor/sample.{docx,xlsx,pdf,pptx}`（仓库内固定二进制样本）

**Criterion 基准**（`src-tauri/benches/ast_tail_bench.rs`）：

- 4 组：单帧全链路 / Syntect 高亮 / 累计流式开销 / 端到端 AuroraBuffer
- 运行：`cargo bench --locked --profile perf`（`profile.perf` 继承 release 优化等级，`panic=unwind`）
- 不加自动回归门禁（阈值需随机器调整），退化检测通过人工看报告

**铁律**：测试文件禁止绝对路径（如 `G:\VCPMobile\...`）；禁止无断言的纯 `println`"诊断测试"。

### 5.3 Vue 前端测试

**基础设施**：Vitest + happy-dom + `@vue/test-utils` + `@pinia/testing`

- 配置：`vitest.config.ts`（独立配置），`@/` alias 对齐 `tsconfig.json` paths
- setup：`src/tests/setup.ts` + `src/tests/mocks/tauri.ts` + `src/tests/mocks/browser.ts`
- Tauri API 已统一 mock：`invoke`（命令路由式）、`listen`（事件注册表）、`Channel`（手动 emit）、`convertFileSrc`、plugin guest-js
- 浏览器 API fallback：ResizeObserver / IntersectionObserver / matchMedia / rAF / clipboard / URL.createObjectURL / visualViewport

**现有测试**（`src/tests/unit/`）：覆盖设置/UI 原语、Chat/Topic/Sync/Distributed 并发边界、富 HTML、分享 Intent，以及 Release/Android 治理契约；总数以 `pnpm test:run -- --reporter=verbose` 为准。

**测试原则**：

- 不断言 UnoCSS 完整 className 串；只断言语义关键类（如 `z-dialog`、`disabled`、`text-red-500`）
- 动态渲染（KaTeX / Mermaid / 代码高亮 / HTML 预览）不断言白盒 DOM
- 安全边界（HtmlPreviewBlock / ToolBlock / astRenderer raw_html）需单独覆盖

### 5.4 Android 插件测试

**目录**：`src-tauri/plugins/vcp-mobile/android/src/test/java/com/vcp/mobile/`

**依赖**：JUnit 4, Robolectric 4.13, MockK, `android-all:9-robolectric-4913185-2`（显式声明避免运行时下载）

**现有测试**：覆盖插件契约、执行器域、分享边界、ForegroundGuardian 与 SSE socket 所有权；总数以 Gradle 测试报告为准。

**运行**：`cd src-tauri/gen/android; ./gradlew --dependency-verification strict :tauri-plugin-vcp-mobile:testDebugUnitTest`

### 5.5 Android E2E Smoke 脚本

目录：`tests/e2e-android/scripts/`

详见目录README.md

仅依赖 Node.js + adb，不引入 Maestro/Appium。

### 5.6 性能脚本

目录：`tests/perf/scripts/`

详见目录README.md

现有性能脚本与 Criterion benchmark 作为人工诊断/报告型资产保留。性能 Phase 1 已恢复为独立的 report-only A/B 轨道；不得把 Debug/HMR 描述为签名 Release、能耗结论或固定阈值门禁。真机性能实验只允许使用 `com.vcp.avatar.debug`，不得覆盖用户安装的正式 `com.vcp.avatar`。

### 5.7 测试命令速查

```powershell
pnpm check                  # 完整静态检查（vue-tsc + cargo check）
pnpm test:run               # 前端 Vitest 单测
pnpm test:unit              # 仅 src/tests/unit
pnpm test:integration       # Rust file_extractor 集成测试（不是 Vitest 目录）
cargo test --locked --lib   # Rust 内联单元测试
cargo test --locked --test file_extractor_integration  # Rust 集成测试
cargo bench --locked --profile perf  # Rust Criterion 性能基准
# Android 插件测试（在 src-tauri/gen/android/ 下执行）：
./gradlew --dependency-verification strict :tauri-plugin-vcp-mobile:testDebugUnitTest
# E2E / 性能（需连接 Android 设备）：
pnpm e2e:android:smoke      # adb 启动并采集状态
pnpm perf:apk-size          # APK 体积报告
pnpm perf:startup           # 冷启动采样
pnpm perf:collect           # dumpsys/logcat 快照
pnpm perf:rust-bench        # 运行 Rust 基准并归档
```

### 5.8 测试代码组织

```
VCPMobile/
├── src/tests/                         # 前端测试
│   ├── setup.ts                       # 全局 setup
│   ├── mocks/{tauri,browser}.ts       # 统一 mock
│   ├── utils/{mount,flush}.ts         # 测试工具
│   └── unit/components/{settings,ui}/ # 原子组件测试
├── src-tauri/tests/                   # Rust 集成测试
│   ├── fixtures/file_extractor/       # 二进制测试样本
│   └── file_extractor_integration.rs
├── src-tauri/benches/                 # Criterion 基准
│   └── ast_tail_bench.rs
├── src-tauri/plugins/vcp-mobile/android/src/test/  # Kotlin 插件测试
├── tests/e2e-android/scripts/         # adb smoke 脚本
└── tests/perf/scripts/               # 性能采集脚本
```

---

## 6. UI 层级管理规范（Z-Index）

项目使用语义化层级系统，禁止在任何组件中直接使用裸露的 `z-50`、`z-[999]` 等魔法数字。

### 6.1 层级总表

| 语义名    | 数值 | 用途                                              | UnoCSS 类   | CSS 变量          | TS 常量           |
| --------- | ---- | ------------------------------------------------- | ----------- | ----------------- | ----------------- |
| `content` | 0    | 页面内容默认层                                    | `z-content` | `--layer-content` | `LAYER_CONTENT`   |
| `local`   | 10   | 页面内局部悬浮（置底按钮、角标）                  | `z-local`   | `--layer-local`   | `LAYER_LOCAL`     |
| `drawer`  | 20   | 左右抽屉 + 遮罩                                   | `z-drawer`  | `--layer-drawer`  | `LAYER_DRAWER`    |
| `overlay` | 30   | 全局覆盖容器                                      | `z-overlay` | `--layer-overlay` | `LAYER_OVERLAY`   |
| `page`    | 40+  | SlidePage 页面栈（40 + index）                    | `z-page`    | `--layer-page`    | `LAYER_PAGE_BASE` |
| `sheet`   | 50   | BottomSheet、ModelSelector                        | `z-sheet`   | `--layer-sheet`   | `LAYER_SHEET`     |
| `dialog`  | 60   | Prompt、ContextMenu、UpdatePrompt                 | `z-dialog`  | `--layer-dialog`  | `LAYER_DIALOG`    |
| `editor`  | 70   | HtmlPreviewBlock（全屏HTML）                      | `z-editor`  | `--layer-editor`  | `LAYER_EDITOR`    |
| `viewer`  | 80   | AttachmentViewer、FullScreenEditor、AvatarCropper | `z-viewer`  | `--layer-viewer`  | `LAYER_VIEWER`    |
| `toast`   | 90   | Toast 通知                                        | `z-toast`   | `--layer-toast`   | `LAYER_TOAST`     |
| `boot`    | 100  | BootScreen（启动屏）                              | `z-boot`    | `--layer-boot`    | `LAYER_BOOT`      |
| `gate`    | 110  | PermissionGate（权限引导页）                      | `z-gate`    | `--layer-gate`    | `LAYER_GATE`      |

### 6.2 规则

1. **全局宏观层级必须使用语义化命名**；局部微观层级（组件内部角标、hover 覆盖等）可使用 `z-10`/`z-20` 等常规数值。
2. **创建新的覆盖层组件前**，先检查现有层级表，若无法归入已有层级再提议新增。
3. **SlidePage 页面栈** 使用 `overlayStore.getPageZIndex(type)` 动态计算，确保页面打开顺序与层级正相关。

---

## 7. 安全注意事项

- **路径遍历防护**: `file_manager.rs` 中的 `ensure_safe_path()` 确保所有文件访问限制在 `app_config_dir` 下。
- **内存限制**: WebView IPC 文件上传 `store_file` 限制 2 MB；大文件必须走受控 staging/高速链路；`read_local_file_base64` 限制 50 MB，防止 OOM。
- **密钥管理**: Release 签名信息只能通过本地环境变量或 GitHub Actions secrets 注入；四项签名输入任一缺失时构建必须失败，密钥库与密码不得进入版本控制。
- **Tauri 安全**: `tauri.conf.json` 中 `csp: null`，asset protocol 范围设为 `["**"]`（本地文件服务）。请确保仅在受控内容中使用此范围。
- **数据库**: SQLite 启用 WAL（Write-Ahead Logging）模式，降低移动端并发写入的锁竞争。
- **网络**: HTTP 客户端使用 `rustls-tls`，禁用原生 TLS；受信 LAN 的 HTTP/WS 是产品能力，Release 仅在显式 `VCP_TRUSTED_LAN_MODE=enabled` 时允许明文流量。

---

## 8. 部署与发布流程

### CI/CD（GitHub Actions）

- **`.github/workflows/ci.yml`**（PR CI）:
  - 触发条件：`push` / `pull_request` 到 `main` / `master`。
  - 所有第三方 Actions 固定完整 commit SHA；pnpm 使用 frozen lockfile，Cargo 命令统一 `--locked`。
  - 步骤：类型检查、Vitest 与生产 `pnpm build` → Rust fmt/test/integration/clippy/bench compile → `tauri android init --ci` 生成树漂移检查 → Gradle strict dependency verification/JVM 测试 → pnpm/cargo audit。
  - 需要 Java 17 + Android SDK + Tauri Linux 依赖。
- **`.github/workflows/release.yml`**（Release）:
  - 触发条件：GitHub Release 被 `published`。
  - 环境：Node 22, pnpm 10, Java 17 (temurin), Android NDK `29.0.13846066`。
  - Release tag、checkout HEAD 与 event SHA 必须一致，并要求同 commit 的 CI Check 已成功；四处版本源与 Android versionCode 必须一致。
  - 从 step 级 secrets 恢复密钥库；APK 证书、keystore 证书与公开固定的 `ANDROID_RELEASE_CERT_SHA256` 必须三方一致。
  - 构建启用 strict Gradle dependency verification 与显式受信 LAN 模式；只上传签名 arm64 APK 及其 SHA-256 文件，不发布独立前端 ZIP。

### 本地发布

仓库不提供携带秘密的本地发布脚本。先在当前 shell 注入 `ANDROID_KEYSTORE_PATH`、`ANDROID_KEY_ALIAS`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_PASSWORD`；确需兼容受信 LAN HTTP/WS 时再显式设置 `VCP_TRUSTED_LAN_MODE=enabled`，然后执行：

```powershell
pnpm tauri android build --apk --target aarch64
```

`src-tauri/gen/android/gradle/verification-metadata.xml` 会让 Gradle 默认启用 strict dependency verification；`tauri android build -- <args>` 的尾随参数会传给 Cargo runner，不能把 Gradle 的 `--dependency-verification` 放在该位置。

---

## 9. AI 代理与贡献约定

### 9.1 知识治理（当前仓库未启用 `plans/`）

当前仓库没有 `plans/` 与根 `scripts/` 目录，也没有 `memory:refresh`/`io:*` 脚本。不得假设这些本地框架存在，也不要为满足旧文档而创建空壳目录；架构与模块事实以 tracked `docs/`、代码、测试和本文件为准。若未来正式引入知识治理框架，必须连同实现、命令与 CI 契约一次落地。

### 9.2 Magi 三贤者协议

在进行重大架构调整、复杂 Bug 修复或核心功能实现前，需强制进行三方思辨：

- **Melchior (逻辑与系统)**: 审查内存安全、Rust 生命周期、IPC 开销、类型完整性、OOM 防御。
- **Balthasar (直觉与美学)**: 审查移动端原生直觉、Glassmorphism 规范、微动画、交互心理学。
- **Casper (务实与交付)**: 审查工程复杂度、维护成本、实现周期，拒绝过度设计。

### 9.3 编码盾（再次强调）

- **优先级 1**: 优先使用原生文件读写工具（`read_file`, `write_file`, `replace`）。
- **绝对禁止**: 对已存在文件使用全文件覆盖进行小修改；禁止在 Shell 中用 `>` / `>>` 写 UTF-8；禁止 `&&` / `||` 链式命令。

### 9.4 重构前强制存档协议（血训）

> 2025-05-11 教训：在未存档的情况下对 `sync_service.rs`（1658 行）进行大规模全文拆分重构，因 `StrReplaceFile` 对 Rust `format!` 宏中的 `{}` 处理失败，导致文件被反复写坏。用户紧急 `git checkout` 后丢失了两天的未提交工作区修改。

**任何涉及以下操作之前，必须先执行 `git add . && git commit -m "save"`（或等效存档）：**

- 新建/删除模块目录或文件
- 拆分超过 500 行的 God File
- 移动类型/函数到新的模块边界
- 修改 `mod.rs` 或 `lib.rs` 的模块声明

**理由**: 工作区修改（Working Tree）不受 `git checkout --` 保护。一旦全文覆盖出错，没有干净的回退点。git commit 是唯一可靠的时光机。

### 9.5 每次 checkout / restore 前强制看 git 状态协议（2026-05-24 血训）

> 2026-05-24 教训：在进行 PDF Fallback 乱码测试时，Agent 未进行 git commit，也未展示 git 状态就擅自调用了 `git checkout --`。由于当前完美版本（DOCX/XLSX 单遍状态机核心代码）仅处于工作区修改且暂存区（Index）中是旧正则版，导致 checkout 瞬间用旧暂存区覆盖了工作区，完美代码惨遭全部抹杀。最终 Agent 通过重写上下文中的缓存代码惊险复原。

**绝对铁律：任何 AI 代理在执行 `git checkout`、`git restore` 或任何可能会抛弃工作区修改的 Git 命令前，必须强制遵循以下三条红线：**

1. **必须首先调用 `git status` 向用户展示并汇报当前的修改状态。**
2. **严禁在没有得到用户明确的同意和核准之前，擅自执行任何 `git checkout` 或 `git restore` 抛弃修改的命令。**
3. **极力建议在执行任何清洗/覆写命令前，先执行 `git add . && git commit -m "pre-checkout-save"` 将当前工作区彻底锁死在 Git 历史中。**

**理由**: 工作区不受 checkout 保护，只有 commit 是唯一可靠的时光机。不看 git 状态就 checkout 约等于直接抹杀代码。

---

## 10. 关键配置文件速查

| 文件                                             | 说明                                                         |
| ------------------------------------------------ | ------------------------------------------------------------ |
| `package.json`                                   | 前端依赖、pnpm 脚本                                          |
| `src-tauri/Cargo.toml`                           | Rust 依赖、Android 构建目标、Release 优化                    |
| `src-tauri/tauri.conf.json`                      | Tauri 窗口、安全、Bundle、beforeDevCommand/beforeBuildCommand |
| `vite.config.ts`                                 | Vite 插件、Tauri 感知端口（1420/1421）                       |
| `uno.config.ts`                                  | UnoCSS 预设、主题色、快捷类、断点                            |
| `tsconfig.json`                                  | TS 严格模式、路径包含/排除规则                               |
| `docs/SYNC_ARCHITECTURE.md`                      | 三阶段增量同步协议的完整规范（WebSocket + HTTP + SHA-256 Hash 差异） |
| `docs/vue_docs/`                                 | 前端 Vue/TS 知识库（27 份，含架构总览、Store 全景、Feature 详解） |
| `docs/modules/`                                  | 稳定 Rust 模块技术文档集（31 份，含 AST Diff 专栏与快速决策树） |
| `docs/sync/`                                     | 同步 V2 子系统文档集（20 份）                                |
| `docs/plugins/`                                  | Android 原生插件代码文档集（13 份）                          |
| `docs/ANDROID_PLUGIN_MANAGEMENT.md`              | Android 插件管理规范：权限血训、前台服务、通知渠道           |
| `docs/ANDROID_UI_COMPATIBILITY.md`               | Android arm64 支持范围、响应式布局、CSS/Insets 与设备证据规范 |
| `docs/UI_LAYER_ARCHITECTURE.md`                  | UI 层级架构规范：12 级语义化 Z-Index 体系                    |
| `src-tauri/plugins/vcp-mobile/Cargo.toml`        | 插件 Rust 依赖：`tauri = 2.11.1`, `jni = 0.21`（Android only） |
| `src-tauri/plugins/vcp-mobile/android/`          | Kotlin 源码与 AndroidManifest.xml                            |
| `src-tauri/plugins/vcp-mobile/permissions/`      | Tauri v2 权限声明：default.toml + autogenerated schemas      |
| `vitest.config.ts`                               | 前端测试配置（Vitest + happy-dom + @vue/test-utils）         |
| `src/tests/`                                     | 前端测试代码（setup/mocks/utils/unit）                       |
| `src-tauri/tests/`                               | Rust 仓库级集成测试（file_extractor）                        |
| `src-tauri/benches/`                             | Rust Criterion 性能基准（ast_tail_bench）                    |
| `src-tauri/plugins/vcp-mobile/android/src/test/` | Android 插件单元测试（Kotlin JVM + Robolectric）             |
| `tests/e2e-android/scripts/`                     | adb smoke 脚本（安装/授权/启动/信息采集）                    |
| `tests/perf/scripts/`                            | 本地性能脚本（APK体积/启动/bench/快照），不进入 CI Release   |
| `docs/Test_Architecture_Constraints.md`          | 测试体系架构约束文档（全局约束）                             |

---

*最后更新：2026-08-13 | VCP Mobile v1.1.4。若项目结构发生重大变化，请同步更新本文件。*
