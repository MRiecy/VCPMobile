# VCP Mobile v1.1.4 发布说明

> 版本号：v1.1.4 · versionCode 1001004 · 发布：2026-08 · 平台：Android 8.0+（minSdk 26）· 架构：arm64-v8a
>
> 距 v1.1.3（Guardian Protocol）共 70 个提交，聚焦三大主线：**全新内置 CLI 运行时（VCPMobileCLI）**、**日记中心**、**同步 Wire 1.2 错误契约**，以及移动端性能与 UI 细节的全面打磨。

---

## ✨ 新功能

### VCP Mobile CLI（全新内置工具运行时）

- **本地 CLI 运行时**：前台运行、任务后台续跑（best-effort job survival）、作业查询/取消（`run | poll | cancel | list`）。
- **真实 PTY 终端**：Android 原生 PTY 辅助库（`libvcp_pty.so`）+ PRoot 环境，内置 Alpine 3.24.1 aarch64 rootfs，支持交互式命令会话。
- **Skill 生命周期**：受管 Skill 目录（提示词技能记忆）、离线语义召回（semantic river recall）、本地 vref 知识契约。
- **分布式适配**：可选分布式适配器，超时请求支持 `cancel_tool` 能力；通信超时提升至 30s。
- **资源边界放宽**：PRoot 子进程内存上限 512 MiB → 4 GiB，大型脚本不再被 OOM 中断。
- **前端工作台**：工具说明（manifest 面板）、Jobs/Skills/终端三页签工作台，Manifest 契约与 Rust 侧逐字节对齐。

### 日记中心

- 远端日记服务 + 移动端日记中心（列表、搜索、文件夹视图）上线。
- 暗色主题下日记样式与侧栏行为修复。

### 主题与呈现

- 主题预览（ThemeLivePreview）与消息呈现模式（presentation modes）上线。
- 可展开侧栏工具托盘，常用工具随取随用。
- 全局确认对话框：危险操作统一确认与错误处理规范。

### 同步

- **Wire 1.2 错误契约**：统一错误码体系与排障规范，前端固定展示历史错误而非原始命令失败。
- 协议 1.2 移动端持久化与传输收口；topic owner 身份精确携带。

---

## 🔧 修复与优化

- **全局设置首开提速**：设置页 6 个重量级区块（头像裁剪、模型选择器、主题选择器、同步设置等）改为按需懒加载；冷启动首次打开模块数 33 → 9，内容可见时间约 +430ms（含冷编译），不再拖拽整页首屏。
- **CLI 工具说明页修复**：Jobs / Skills 标签下点击“工具说明”此前不显示内容，现已正确展示 manifest 面板。
- **UI 细节统一**：日记中心与 CLI 工作台关闭按钮由 X 统一为 ‹；侧栏“日记中心 / 更多”按钮阴影减淡，与相邻按钮视觉一致。
- **WebView 性能**：消除常驻刷新与首屏冗余渲染。
- **设备兼容**：Android 多设备兼容与规范、权限检查/自启动/省电策略提示优化。
- **安全边界**：富 HTML 主动内容边界收敛；前端与 Tauri 供应链更新。
- **格式兼容**：DailyNote 新格式兼容。

---

## 🛠 工程与质量

- **Agent 安全调试工具链**：新增低噪声 USB/HMR Debug Agent（`pnpm android:debug:*`），PID 隔离日志、有限快照，不触碰正式安装包。
- **构建供应链加固**：Gradle 依赖严格校验（verification-metadata.xml）、CI 修复、`tauri.settings.gradle` 停止跟踪。
- **测试规模**：Rust 内联单测 320 通过；Vitest 前端测试 231 通过（38 文件）；`vue-tsc` + `cargo check` + 生产构建全绿。
- **文档**：WebView 性能兼容研究、日记架构、同步 Wire 1.2 契约、CLI 收敛 ADR 与真机验收存档。

---

## 📦 安装与升级

| 项目 | 说明 |
| --- | --- |
| APK | `VCPMobile_v1.1.4_arm64-v8a.apk`（由发布流水线自动构建并挂载至本 Release） |
| 校验 | 同名 `.sha256` 文件，下载后可用 `sha256sum -c` 校验完整性 |
| 系统要求 | Android 8.0+（minSdk 26），仅支持 arm64-v8a 设备 |
| 升级方式 | 与 v1.1.3 同签名，直接覆盖安装即可，数据无损 |
| 首次安装 | 需在系统设置中允许“安装未知应用” |

发布构建由 GitHub Actions 自动执行：签名证书与仓库固定指纹三方一致校验、仅产出签名 arm64 APK 及其 SHA-256 文件。
