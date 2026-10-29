# AGENTS.md — VCP Mobile (Project Avatar)

> 本文件记录用户决定、工作边界与代码导航，不承担实现事实。当前生产代码是实现事实的唯一来源；文档、计划和测试均不能覆盖它，测试断言不得反向定义产品行为。

---

## ⛔ 0. 工作区隔离铁律

> 工作空间包含多个项目。`G:\VCPMobile` 是当前开发目标：
>
> | 目录            | 规则                                      |
> | --------------- | ----------------------------------------- |
> | `E:\VCPMobile`  | ✅ 可读可写                                |
> | `E:\VCPToolBox` | ❌ 参考工程，禁止修改                      |
>
> **VChat 仅允许修改 `VCPDistributedServer/Plugin/VCPMobileSync/**` 与 `rust_chat_data_service/**`，严禁修改任何非同步模块。**

---

## 1. 项目概览

**VCP Mobile**（代号：Project Avatar）是 VCPChat 的移动端版本，基于 **Tauri v2 + Vue 3 + Rust**。当前生产平台为 Android `arm64-v8a`（`aarch64-linux-android`，minSdk 26）触控手机、平板与按当前窗口宽度响应的折叠屏；iPhone/iPad 原生客户端按 Phase 0–6 实施，目标为复用 Vue/Rust 核心的实验性用户自签产物。架构上为 Rust 核心层、Tauri IPC 层、Vue 3 渲染层三层隔离。

- 包名：`com.vcp.avatar`
- 架构三层：Rust 核心层 ── Tauri IPC 层 ── Vue 3 渲染层（严格物理与状态隔离）

### 1.1 平台基线与强约束

- **桌面与异构平台禁令**：不支持 Windows/macOS/Linux 桌面端、Android TV 或其他 Android ABI；桌面用户使用 VCPChat。Vite 预览、host Rust 编译与 Tauri desktop scaffold 仅用于开发/测试，不是产品入口；现有非 Android fallback 不代表 iOS 已实现。不得为“桌面兼容”增加业务分支，也不得删除移动端子命令仍依赖的通用 `pnpm tauri` 脚本。完整 UI 契约见 `docs/MOBILE_UI_COMPATIBILITY.md`。
- **Android WebView 基线**：内核须为 117 或更新版本，与 Android minSdk 26 分别管理；共享前端 JS/CSS 目标为 `chrome117` / `safari26.0`。启动后的兼容说明复用“帮助与指引”，不承担旧内核启动失败前的检测或兜底。
- **iOS 运行与构建基线**：
  - **最低系统**：iOS/iPadOS 26.0（Tauri 配置与项目 Swift manifest 同步维护，高于 26.0 的 API 须处理实际 availability 与运行条件；早期 15.0 历史记录不再约束当前产品）。
  - **实施与审计**：严格沿用 `plan/vcpmobile-ios-compatibility-research/09-Magi综合裁决与待决策清单.md` 的 D1–D19。Windows 环境执行日常源码审计，Apple 构建统一复用 `ios-g1.yml` 的 device archive 链进行按需诊断，G1 接入与 G2 集成/打包分别验收。
  - **测试与签名边界**：不建设项目 iOS 实机测试体系，正式签名与付费 Team 均不在项目范围内；Android 正式链路继续保留，双端构建结果分别独立记录。
  - **功能范围依据**：遵循 [跨平台开发规范](docs/CROSS_PLATFORM_DEVELOPMENT.md)；功能范围与权限提示以 [Phase 2 功能权限与提示审议表](plan/vcpmobile-ios-compatibility-research/phase-2/09-功能权限与提示审议表.md) 为权威依据，新增意见优先于旧延期和隐藏建议。
---

## 2. 项目结构

```
VCPMobile/
├── src/                        # Vue 3 前端源码
│   ├── AGENTS.md               # Vue 渲染层局部代码地图与交互边界
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
│   ├── components/             # 共享组件
│   │   ├── layout/             # 布局外壳
│   │   ├── settings/           # 设置页原子组件
│   │   └── ui/                 # 通用 UI 原语
│   └── assets/                 # 主题与静态资源
├── src-tauri/                  # Tauri v2 + Rust 后端
│   ├── src/
│   │   ├── AGENTS.md           # Rust 核心与 Tauri IPC 局部代码地图
│   │   ├── main.rs             # host 开发/测试 scaffold（非产品入口）
│   │   ├── lib.rs              # 启动流程与生命周期编排
│   │   ├── commands.rs         # IPC 命令注册表 + 防爆栈总闸（macro_rules!，在 lib.rs 展开）
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
│   ├── crates/voice-mp3/       # 双端共享的裁剪 LAME 编码核心（Rust/C）
│   ├── crates/config-export/   # host 开发期配置契约导出，生成 Vue 类型/默认值/字段规则
│   └── plugins/
│       └── vcp-mobile/         # Android 原生实现与分期接入的 iOS Swift 插件
│           └── AGENTS.md       # 插件局部代码地图与修改闭环
├── docs/                       # 技术文档体系（详见 §2.1）
│   ├── vue_docs/               # 前端知识库
│   ├── modules/                # Rust 模块文档
│   ├── sync/                   # 同步子系统文档
│   ├── plugins/                # 插件文档
│   ├── ANDROID_PLUGIN_MANAGEMENT.md
│   ├── MOBILE_UI_COMPATIBILITY.md
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

`AGENTS.md` 随物理目录形成作用域树。修改任何路径前，读取从仓库根到该路径最近节点的全部 `AGENTS.md`；若当前会话从仓库根启动、子节点未被自动注入，则先显式读取对应文件。一次任务涉及多个子树时合并相应节点，子节点只补充局部关系，不重复根规则。

### 2.1 文档体系三层结构

`docs/` 不是杂乱的笔记集合，而是按主题严格分层的技术知识库：

| 子目录 | 覆盖范围 | 核心关注点 | 阅读对象 |
| ------ | -------- | ---------- | -------- |
| `docs/vue_docs/` | `src/` 全部 Vue 3 / TypeScript 前端源码 | 前端状态、组件交互、Pinia 数据流 | 前端开发者、UI 调试者 |
| `docs/modules/` | `src-tauri/src/vcp_modules/` 与 `distributed/` 核心模块 | 核心业务领域状态机、后端调度逻辑 | Rust 核心开发者 |
| `docs/sync/` | 同步 V2 子系统全链路（WebSocket + HTTP + SHA-256 哈希比对） | 多端同步协议、管线生命周期与冲突解决 | 同步系统维护者 |
| `docs/plugins/` | `src-tauri/plugins/vcp-mobile/` Android 原生插件说明 | 平台权限、前台服务、原生硬件通信 | 移动端原生开发者 |
四者与 `docs/*.md` 顶层规范文档的关系：

- `docs/vue_docs/`：回答"前端状态、组件交互、Pinia 数据流是**怎么**做的"
- `ANDROID_PLUGIN_MANAGEMENT.md`：回答"**应该**怎么做"（开发规范、权限准则、选型指导）
- `docs/plugins/`：回答"代码是**怎么**做的"（接口签名、数据流、实现细节）
- `UI_LAYER_ARCHITECTURE.md`：全局 UI 层级规范，与 `src/core/constants/layers.ts` + `uno.config.ts` + `src/assets/themes.css` 三重机制对应
- `MOBILE_UI_COMPATIBILITY.md`：Android 生产边界与 iOS 实验目标、共享窗口布局、分平台 CSS/Insets 与验证边界的权威规范
- `CROSS_PLATFORM_DEVELOPMENT.md`：Vue 运行时平台分支、原生编译隔离、权限与用户提示的开发规范

### 2.2 移动端原生插件（`tauri-plugin-vcp-mobile`）

`src-tauri/plugins/vcp-mobile/` 是移动端原生能力边界。修改该子树前先读取其 [`AGENTS.md`](src-tauri/plugins/vcp-mobile/AGENTS.md)，再从当前 Rust、Kotlin、Swift、TypeScript 或 C++ 生产代码追踪实际入口与所有者。iOS 工程输入与 G1 脚本在 `src-tauri/ios/`；生成树仍可重建，阶段证据见对应 `phase-2/` 文档。

---

## 3. 构建与开发命令

```powershell
# 完整静态检查（生成契约 + ESLint + TypeScript + Rust）
pnpm check                  # tools/check.mjs 先执行 config:check，再并行执行缓存 lint、增量 vue-tsc、构建配置 tsc 与 cargo check --locked

# Rust 配置契约的显式生成与只读一致性检查
pnpm config:generate
pnpm config:check

# Android 真机调试 — USB 模式（纯数据线、ADB reverse、Agent 限流输出）
pnpm android:debug:doctor -- --json
pnpm android:debug:dev
```

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

- **模块边界**: 业务逻辑必须位于 `src-tauri/src/vcp_modules/`。`lib.rs` 仅作为启动钩子挂载点；命令注册表独立位于 `src-tauri/src/commands.rs`（macro_rules! 形式，在 lib.rs 调用点展开——伴生宏解析约束见文件头注释）。
- **错误处理**: Tauri command handler 中**严禁使用 `unwrap()` 或 `expect()`**。必须转换为 `Result<T, String>`（`map_err(|e| e.to_string())?`）。
- **异步 IO**: 所有网络、文件、数据库操作必须异步，基于 `tokio`。
- **状态共享**: 使用 Tauri `app.manage(...)` 注入单例状态。并发结构偏好 `DashMap`/`DashSet`、`tokio::sync::RwLock`、`AtomicU32`。
- **IPC 防爆栈总闸**: 所有 app 命令经 `commands.rs` 中央 `invoke_handler` 统一 offload 到 tokio worker 线程分发（JavaBridge 线程栈仅 ~1MB，禁止在其上构造业务 future，防止物理爆栈）；新增命令在 `commands.rs` 对应领域分组注册即可，**无需**评估/壳化 future 尺寸。

### 4.3 跨层与工程纪律

**以最佳认知熵把握改动尺度。** 在完整解决问题的同时，让系统的结构、职责与因果关系更清楚，为后续理解和演化减少负担。修复与重构的边界由这一目标决定：该深入时深入，该收束时收束。谨慎体现在对根因与影响的充分理解中，优秀体现在改动后的整体质量上。

- **修改后的强制检查**: 任何涉及跨层或核心逻辑的变更后，必须运行 `pnpm check`。若未通过，必须先修复静态错误再继续。
- **跨平台功能**: 涉及平台分支、原生适配、权限或差异化 UI/提示时，先读 [跨平台开发规范](docs/CROSS_PLATFORM_DEVELOPMENT.md)。Vue 默认使用统一平台入口的运行时分支；后端检查回应真实执行条件，不以假想安全风险建立重复的功能门禁。
- **现有文件修改原则**: 对于任何已存在的文件，哪怕只改 1 行，**严禁使用全文件覆盖**，必须使用原子级、外科手术式的编辑，以防幻觉灾难。编辑方式保持精确，设计尺度由问题本身决定。
- **兼容与容错扩展**: 任何新增语法兼容、自动纠错或容错兜底，必须先与用户讨论并取得明确同意再执行。
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

## 5. 验证与测试边界

**让信心的增长，受知识增长的约束。** 验证的意义，在于让一个判断接触到能够推翻它的事实。把同一份认识复制成两套彼此吻合的说法，盲点仍可能原样保留。

方法的价值存在于它回应的问题之中。围绕当前任务的真实不确定性，发挥 Agent 的知识与判断，选择有价值的验证。已知结论不因被写成脚本就增加分量，也不必因此成为门禁。

生产代码说明当前行为，用户请求决定目标。测试失败是重新理解真实调用链的线索，不应反过来支配产品；不为迁就断言改写业务接口、复制状态、保留失效路径或增加无消费方的兼容层。源码审查、静态检查、编译与运行观察各有证据范围，交付时不互相替代。

测试与 fixture 使用仓库内相对路径；只输出日志不构成回归验证。

### 5.1 常用入口

具体命令以当前 `package.json`、Cargo workspace 与 Gradle 工程为准。常用入口如下：

```powershell
pnpm check
pnpm test:run
pnpm test:integration
# 在 src-tauri/：
cargo test --locked --lib
# 在 src-tauri/gen/android/：
./gradlew :tauri-plugin-vcp-mobile:testDebugUnitTest
```

### 5.2 测试资产目录

```
VCPMobile/
├── src/tests/                                      # Vue 测试、setup、mock 与工具
├── src-tauri/src/**                                # 与生产模块共置的 Rust 内联测试
├── src-tauri/tests/                                # Rust 仓库级测试与 fixture
├── src-tauri/benches/                              # Rust benchmark
├── src-tauri/plugins/vcp-mobile/android/src/test/ # Android 插件 JVM 测试
└── tests/
    ├── AGENTS.md                                   # Android 验证工具的局部边界
    ├── e2e-android/                                # Android Debug Agent 与设备旅程
    └── perf/                                       # 本地性能采集
```

### 5.3 Android Debug 与设备证据

用户说“编译 debug”时，默认包含构建 Android Debug 包并安装到真机供用户体验测试；除非用户明确说“不安装”，或设备连接、授权等实际条件阻止安装，不得仅交付 APK 就结束任务。

Agent 对设备的日常操作必须使用 `pnpm android:debug:*`，只允许操作 `com.vcp.avatar.debug`。严禁启动、停止、覆盖、清除或卸载用户的 `com.vcp.avatar` Release；不清空全局 logcat，不批量输出 dumpsys，不自动读取多张截图。

`doctor`、`status`、有限日志或自动化结果只说明各自取得的状态。真机 UI、触控、性能与兼容性结论仍需对应的具名设备证据。

---

## 6. UI 层级管理规范（Z-Index）

项目使用语义化层级系统，禁止在任何组件中直接使用裸露的 `z-50`、`z-[999]` 等魔法数字。

1. **权威枚举**：所有全局宏观层级（从 `content: 0` 到 `gate: 110`）由 [`src/core/constants/layers.ts`](src/core/constants/layers.ts) 统一管理，详细场景规范参见 [`docs/UI_LAYER_ARCHITECTURE.md`](docs/UI_LAYER_ARCHITECTURE.md)。
2. **局部限制**：组件内部微观层级（如角标、hover 遮罩）允许使用 `z-10`/`z-20` 等常规数值，严禁创建跨组件的任意高层级。
3. **覆盖层创建**：创建新的覆盖层组件前，必须匹配现有语义层级；若确需新增宏观层级，必须同步更新 `layers.ts` 与对应 CSS 变量。
4. **页面栈动态计算**：`SlidePage` 页面栈必须使用 `overlayStore.getPageZIndex(type)` 动态分配，确保页面打开顺序与层级严格正相关。

---

## 7. 部署与发布流程

CI 与 Release 的当前行为由 `.github/workflows/ci.yml`、`.github/workflows/release.yml` 及其实际读取的配置定义。普通开发任务不展开、不修改发布流程；只有用户明确要求 CI、版本、签名或发布变更时才读取当前 workflow 并完成对应验证。未经用户明确要求不得 push、创建 Release 或执行发布动作。

---

## 8. AI 代理与贡献约定

### 8.1 工作树与单次存档

开始任务时检查 Git 状态并保留全部无关修改。同一逻辑任务最多建立一次必要的 checkpoint；干净 HEAD 或本任务已有的可恢复点已经足够，不创建空提交或重复的 `save` 提交。

只有在即将新建/删除模块、拆分 God File、移动模块边界或修改 `mod.rs`/`lib.rs`，且相关未提交工作尚无可恢复点时，才先向用户说明并建立范围明确的 checkpoint。只暂存已核对的任务路径，禁止使用 `git add .`，未经用户授权不得提交。

### 8.2 Magi 三贤者协议

在进行重大架构调整、复杂 Bug 修复或核心功能实现前，需强制进行三方思辨：

- **Melchior (逻辑与系统)**: 审查内存安全、Rust 生命周期、IPC 开销、类型完整性、OOM 防御。
- **Balthasar (直觉与美学)**: 审查移动端原生直觉、Glassmorphism 规范、微动画、交互心理学。
- **Casper (务实与交付)**: 权衡整体质量、维护负担与实现成本，辨认局部补丁留下的复杂性与扩大重构引入的复杂性，把握恰当的工程尺度。

### 8.3 破坏性 Git 操作

执行 `git checkout`、`git restore` 或其他可能抛弃修改的命令前，必须先展示并说明当前 Git 状态、精确目标和影响范围，并取得用户明确同意。存在未存档修改时优先复用本任务 checkpoint；确需新增存档仍只处理明确的任务路径。

**不得用清理工作树、切分提交或“恢复干净状态”为由**覆盖用户修改。安全来自可核对的目标和一个可恢复点，不来自更多提交。
---

## 9. 关键配置文件速查

| 文件 / 路径 | 说明 |
| ----------- | ---- |
| `package.json` | 前端依赖、pnpm 脚本 |
| `src-tauri/Cargo.toml` | Rust 依赖、Android 构建目标、Release 优化 |
| `src-tauri/src/vcp_modules/infra/settings_manager/` | SQLite 配置领域类型、规则、存储、一次搬运与备份边界 |
| `src/core/config/generated/` | Rust 显式生成的配置类型、默认值和字段元数据，勿手动编辑 |
| `src-tauri/tauri.conf.json` | Tauri 窗口、安全、Bundle、beforeDevCommand / beforeBuildCommand |
| `vite.config.ts` | Vite 插件与 Tauri 开发、构建配置 |
| `uno.config.ts` | UnoCSS 预设、主题色、快捷类、断点 |
| `tsconfig.json` | TS 严格模式、路径包含/排除规则 |
| `src-tauri/plugins/vcp-mobile/Cargo.toml` | 移动插件 Rust 依赖与构建元数据 |
| `src-tauri/plugins/vcp-mobile/android/` | Kotlin 源码与 AndroidManifest.xml |
| `src-tauri/plugins/vcp-mobile/permissions/` | Tauri v2 权限声明：default.toml + autogenerated schemas |
| `vitest.config.ts` | 前端测试运行与环境配置 |

---

*仓库目录与核心配置以物理代码为准；业务领域实现细节归入对应子树，禁止在本文件追加具体特性流水。*
