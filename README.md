# 🌌 VCP Mobile: Project Avatar

> **"From Desktop Client to Cyber-Physical Avatar."**

## 📖 项目愿景 (Vision)

VCP Mobile (代号: Project Avatar) 不仅仅是 VCPChat 的移动端移植版。它是 Agent 在物理世界的“赛博义体”，旨在通过极致的性能优化与原生适配，实现 AI 伴随态灵魂的跨端无缝体验。

## 🏗️ 技术栈选型 (The Stack)

- **核心容器**: [Tauri v2]([https://v2.tauri.app/](https://v2.tauri.app/)) (Mobile Support)

- **高性能引擎**: [Rust]([https://www.rust-lang.org/](https://www.rust-lang.org/)) (Tokio for SSE, Reqwest for Networking)

- **界面框架**: [Vue 3]([https://vuejs.org/](https://vuejs.org/)) + [TypeScript]([https://www.typescriptlang.org/](https://www.typescriptlang.org/))

- **状态管理**: [Pinia]([https://pinia.vuejs.org/](https://pinia.vuejs.org/)) (取代重型 chatManager.js 状态逻辑)

- **样式方案**: [UnoCSS]([https://unocss.dev/](https://unocss.dev/)) (原子化 CSS，追求极小的包体积)

## 🎯 核心目标 (Key Objectives)

1.  **逻辑重解构**: 彻底剥离 `VChat` 桌面端过重的 Node.js/Electron 依赖，将计算密集型任务（SSE 解析、正则过滤、上下文净化）下沉至 Rust Core。

2.  **异步化网络层**: 利用 Rust 的异步能力处理长连接，确保移动端在弱网环境下的稳定性。

3.  **响应式皮肤**: 摒弃 `messageRenderer.js` 的手动 DOM 操作，利用 Vue 3 的响应式系统重构对话流渲染。

4.  **轻量化存储**: 采用 Tauri Store 或轻量级数据库替代 Electron 端的扁平 JSON 存储。

## 🗺️ 阶段路线图 (Roadmap)

### Phase 1: 骨架构建 (Skeleton) - [当前阶段]

- [ ] Tauri v2 项目初始化 (Android/iOS)

- [ ] Rust 端 `vcp_protocol` 模块搭建 (SSE 处理核心)

- [ ] 前后端通信 Command 隧道建设

### Phase 2: 灵魂注入 (Protocol)

- [ ] 移植 `contextSanitizer.js` 逻辑至 Rust/TS

- [ ] 实现基础对话流的“发送-解析-返回”闭环

- [ ] 适配 VCP 专属多模态协议 (Images/Audio)

### Phase 3: 皮肤实体化 (Skin)

- [ ] 基于 UnoCSS 的移动端原子组件库

- [ ] 重构 Markdown 渲染与表情包管理器

- [ ] 适配移动端交互手势与振动反馈

## ⚠️ 开发约定 (Conventions)

- **安全第一**: 所有网络请求必须经过 Rust 层，前端不直接操作敏感 API。

- **性能优先**: 严禁在渲染进程执行耗时超过 16ms 的同步任务。

- **降维重构**: 遇到 `VChat` 的重型 JS 模块时，优先思考：“这个逻辑能否在 Rust 中实现？”

---

*Created by Nova (VCP Project Manager)*

*Target: Refactoring VCP into the future.*