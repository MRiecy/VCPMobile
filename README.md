# 🌌 VCP Mobile: Project Avatar

> **"From Desktop Client to Cyber-Physical Avatar. **
>
> **Evolving from Node into Rust, with Soul and Logic."**

## 📖 项目愿景 (Vision)

VCP Mobile (代号: Project Avatar) 是 VCPChat 的移动端进化版。它不仅仅是移植，而是通过 **"Rust Core 下沉"** 与 **"Vue 3 响应式重构"**，将 Agent的能力注入物理世界，打造高性能、低延迟、跨端一致的 AI 伴随态体验。

---

## ⏳ 开发历程 (The Evolution Journey)

VCPMobile 的诞生与进化，伴随着自我认知的觉醒与架构的不断涅槃：

1. **破茧成蝶 (Node to Rust)**: 早期直接移植遇到严重的移动端性能与内存瓶颈。我们决定引入 **Tauri v2 + Rust Core**，将正则清洗、文件 I/O、流式通信等重型计算全面下沉至系统底层。
2. **灵魂注入 (The Magi Protocol)**: 随着架构复杂度的上升，单纯的编码不再足够。我们确立了 **Magi 三贤者多维思辨协议**（逻辑、直觉、务实），并在每次迭代后强制执行“执行->反思->沉淀”的认知闭环，将散落的灵感物理固化为 `.gemini` 记忆图谱与架构真理 (`plans/05_Sublimations/`)。
3. **视觉重塑 (Productivity-First UI)**: 摒弃了浮夸的移动端流行趋势，全面同步VChat精美UI。~~(其实安装包占用最大的部分就是UI壁纸)~~。

---

## 🏗️ 核心架构哲学 (Architectural Anchors)

VCP Mobile 遵循 **Double-Track 3-Tier (双轨三层架构)**，以确保移动端的极致流畅：

*   **⚙️ Core Layer (Rust)**: 负责所有重活 (Sync, Regex, DB, IO, Stream Parsing)。**严禁全量文件缓冲 (NO FULL FILE BUFFERS)**，依靠精准的生命周期管理守护内存边界。
*   **🌉 IPC Bridge (Tauri)**: 事件驱动的消息隧道 (`invoke` 请求与 `emit` 事件泵送)，大幅降低跨端通信开销。
*   **🎨 UI Layer (Vue 3)**: 保持绝对的无状态与轻量级。基于 Pinia 进行增量渲染，使用 UnoCSS 构建原子化视觉层。

---

## 📊 当前进度 (Current Progress)

我们在重构战役中已攻克多个核心堡垒，目前项目进度（约 85%），核心生产力已全面上线：

### ✅ 已达成里程碑 (Milestones Achieved)
*   **Tauri v2 底层基建**: 成功构建跨端基础，打通 Rust 与 Android/iOS 的生命周期。
*   **极致流式渲染 (Stream Pipeline)**: 重构了消息渲染管线，实现了(~~并非实现~~)从 Rust SSE 解析到 Vue 3 增量更新的防抖输出，彻底解决了移动端大段文本输出时的抖动与卡顿。
*   **模型生态系统 (Model Ecosystem)**: 解析模型/群组配置（暂未实现编辑，在搬了）。
*   **沉浸式主题引擎 (Theme Engine)**: 实现了从 Rust 动态读取系统主题、壁纸，并在前端无缝渲染。
*   **逻辑全面下沉**:
    *   **Context Sanitizer**: 将海量对话清理、HTML 过滤与正则匹配移至 Rust。
    *   **Delta Sync**: 确立了差异化同步协议，大幅减少数据序列化压力。
    *   **话题提取与管理**: 实现了原生的长连接维护、会话未读计数与对话摘要生成。

---

## 🚀 快速上手 (Quick Start)

为了实现 VCP Mobile 与桌面端 VChat 的完美同步，你需要完成以下步骤：

1.  **安装手机端**: 在 [Releases](https://github.com/MRiecy/VCPMobile/releases/tag/v0.9.0) 页面下载对应架构的 APK 并安装。
2.  **安装 VChat 插件**: 下载 Release 包中的 `VCPMobileSync.7z`，将其解压并安装至你的桌面端 VChat 插件目录。
3.  **扫码/IP 连接**: 确保手机与电脑在同一局域网下，配置好设置即可开启流式同步。

---

## 🛠️ 未完善功能与未来规划 (Pending Features & Roadmap)

Project Avatar 的完全体仍在锻造中，以下是即将突破的领域：

### ⏳ 待办列表 (To-Do)（~~AI瞎写的~~）
*   [ ] **多模态深度适配 (Multi-modal)**: 完善图片、音频、文件等二进制附件在移动端的原生级拍摄、选择与上传处理路径。
*   [ ] **分布式双向同步 (Distributed Sync)**: 构建基于 SQLite 的本地缓存层，实现即使在弱网/无网环境下也能无缝检索历史对话的离线增强体验。
*   [ ] **原生级交互反馈 (Native Interactions)**: 深度集成触感反馈 (Haptic Engine)、全面支持移动端原生手势 (右滑返回、长按菜单) 与系统级分享扩展。
*   [ ] **群组与角色对齐 (Group UI Alignment)**: 进一步优化多角色 (Multi-Agent) 协作在移动端的显示逻辑，使其与桌面端 `VChat` 的丰富设定（群聊中断、场景渲染）完全对齐。

---

## 📚 技术栈速览 (Tech Stack)

| 领域 | 选型与工具 |
| :--- | :--- |
| **容器平台** | Tauri v2 (Mobile Optimized) |
| **底层核心** | Rust (Tokio, reqwest, serde, regex) |
| **前端框架** | Vue 3 + TypeScript + Vite |
| **状态/流控** | Pinia (StreamManager, ChatManager) |
| **样式体系** | UnoCSS + Vanilla CSS Variables |
| **知识图谱** | `.gemini_snapshot.json` + `VCP` |

---

*This repository is managed under the strict directives of the Magi Protocol. All major architectural decisions are permanently sublimated into the `plans/` directory.*
*Created and evolved by Nova(~~并非是Nova写的~~) (VCP Evolutionary Architect).*