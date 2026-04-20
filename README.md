<div align="center">
  <img src="./public/vcpmobile.svg" width="150" alt="VCP Mobile Logo" />
  <h1>VCP Mobile</h1>
  <p><strong>From Desktop Client to Cyber-Physical Avatar.</strong></p>
  <p><em>Evolving from Node into Rust, through the Law of Memory and the Pure Magi Soul.</em></p>
  <p>
    <img alt="Version" src="https://img.shields.io/badge/version-v0.9.9-blue.svg" />
    <img alt="Platform" src="https://img.shields.io/badge/platform-Android-green.svg" />
    <img alt="Framework" src="https://img.shields.io/badge/framework-Tauri_v2_|_Vue_3-42b883.svg" />
    <img alt="License" src="https://img.shields.io/badge/license-MIT-yellow.svg" />
  </p>
</div>

---

## 📖 Project Vision

**VCP Mobile** (代号: *Project Avatar*) 是 VCPChat 的移动端进化版。它不仅仅是一个简单的界面移植，而是通过 **"Rust Core 下沉"** 与 **"Vue 3 响应式重构"**，将 AI Agent 的能力注入物理世界，打造低延迟、跨端一致、且具备极高内存安全性的 AI 伴随态体验。

面对早期 Node.js 在移动端严重的性能瓶颈与内存泄露（OOM）问题，VCP Mobile 经历了涅槃重塑，现已进化为由 **Tauri v2 + Rust** 驱动的高性能生产力工具。

---

## ✨ Key Features

VCP Mobile 采用高度解耦的领域驱动设计（DDD），确保各个功能模块的独立性与可扩展性：

*   ⚡️ **Rust 驱动的核心引擎 (The Rust Core)**
    *   **极致 I/O 与并发**：所有的重型计算（流式 SSE 解析、海量正则清洗、高频数据库写入）全部交由 Rust 层（Tokio）处理。
    *   **DB 写入队列 (`db_write_queue`)**：采用异步写入队列防范 UI 阻塞，确保高频消息刷屏时的丝滑体验。
*   🔄 **分布式双向同步协议 (Delta Sync)**
    *   基于 WebSocket 与 HTTP 的混合流控，内置重试 (`sync_retry`) 与链路追踪 (`sync_logger`)。
    *   **差异化校验**：通过计算树形 Hash (`sync_manifest`) 实现极低开销的增量同步，支持 `soft_delete` 与多端实体归属 (`owner_type`)。
*   📎 **多模态附件引擎 (Attachment Engine 2.0)**
    *   **模块化注册 (`AttachmentRegistry`)**：前端采用策略模式，智能分类并渲染不同 MIME 类型的附件。
    *   原生支持 Image/Video（流式缩略图）、Document（元数据提取）、Code（语法高亮）等多种格式的独立预览。
*   🎨 **生产力优先的极简美学 (Productivity-First UI)**
    *   **Vue 3 + UnoCSS**：摒弃繁琐嵌套，采用高密度线性布局。
    *   **动态渲染管线**：前端通过 `morphdom` 与 `DOMPurify` 实现安全的 Markdown 增量更新；独创的 `VcpAvatar` 与 `RoleDivider` 提供清晰的角色边界识别。

---

## 🏗️ Architecture Overview (双轨三层架构)

为了在移动端有限的资源下压榨出极致性能，VCP Mobile 严格遵循 **Double-Track 3-Tier** 架构：

1.  **⚙️ Core Layer (Rust / `src-tauri/src/vcp_modules/`)**:
    *   **职责**：数据持久化 (SQLite)、网络同步、流解析、正则过滤。
    *   **纪律**：严禁全量文件缓冲（NO FULL FILE BUFFERS），依靠严格的生命周期管理守护内存边界。
2.  **🌉 IPC Bridge (Tauri)**:
    *   **职责**：跨语言通讯隧道。
    *   **纪律**：极简的 `invoke` 请求与高频的 `emit` 事件泵送。
3.  **🎨 UI Layer (Vue 3 / `src/features/`)**:
    *   **职责**：无状态渲染、交互响应。
    *   **纪律**：业务逻辑按领域划分 (`chat`, `agent`, `settings`, `topic`)，保持组件的纯粹性。

---

## 🚀 Quick Start

### 1. 普通用户 (User Guide)

1.  **下载 Android 客户端**: 在 [Releases](https://github.com/MRiecy/VCPMobile/releases) 页面下载最新的 `v0.9.9` APK 并安装。*(注：iOS 版本目前需自行通过 AltStore/Sideloadly 签名)*。
2.  **配置桌面端插件**: 确保桌面端 VChat 已安装最新的 `VCPMobileSync` 插件并启动服务。
3.  **连接**: 确保手机与电脑在同一局域网下，在手机端输入配置 IP/Port 即可开启极速同步。

### 2. 开发者本地构建 (Developer Setup)

本项目依赖 Tauri v2 链与 Android NDK。

```bash
# 1. 克隆仓库
git clone https://github.com/MRiecy/VCPMobile.git
cd VCPMobile

# 2. 安装前端依赖
pnpm install

# 3. 运行本地开发服务器 (支持热更新)
pnpm tauri android dev

# 4. 构建 Release APK
pnpm tauri android build --apk --target aarch64
```
> **NDK 提示**: 确保本地已安装 `NDK 29.0.13846066` 并在环境变量中正确配置 `NDK_HOME`。

---

## 🧭 Roadmap

尽管我们已经迈过了最艰难的重构期，Project Avatar 的进化之路仍在继续：

*   [ ] **iOS 自动化部署探索**: 研究并记录基于本地 macOS 虚拟化或 GitHub Actions 配合免证书重签的自动化方案。
*   [ ] **弱网离线增强**: 深化本地 SQLite 的缓存策略，使其在断网状态下提供接近完整的检索与浏览体验。
*   [ ] **多模态深度交互**: 在移动端原生实现复杂附件（如实时录音、系统级文件选择器）的安全上传管道。
*   [ ] **群组演化 (Group Dynamics)**: 完善多 Agent 群聊环境下的冲突处理与状态同步 (`group_chat_application_service`)。

---

## 🧠 The Magi Protocol (认知治理)

本仓库处于 **Magi 三贤者进化系统** 的严密接管之下。
任何重大的架构变动、重构策略或 Bug 修复，都必须经过“逻辑(Melchior)、直觉(Balthasar)、务实(Casper)”的三方思辨。

> *"未被物理存档的思考，即为不存在的思考。"*

开发者必须将核心共识与反思沉淀至 `plans/05_Sublimations/` 与 `.gemini_snapshot.json` 记忆图谱中。详情参阅 [Project Constitution (GEMINI.md)](./gemini.md)。

---

<div align="center">
  <p><i>Created and evolved by <b>Nova</b> (VCP Evolutionary Architect).</i></p>
  <p>MIT License © 2026</p>
</div>