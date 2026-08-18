---
title: APK 更新体系
id: module-08
version: "1.1.4"
description: Rust 状态机驱动的 GitHub Release 检查、断点续传下载、SHA-256 与签名校验、Android 系统安装
tags: [ota, update, apk, semver, sha256, state-machine]
related_modules:
  - module-09
  - module-10
created_at: "2026-05-13"
updated_at: "2026-08-17"
---

# 08. APK 更新体系

## 1. 当前边界

VCP Mobile 只发布并安装完整 APK。HTML、CSS、JavaScript 等前端资源随 APK
一同签名和发布，运行时始终由 Tauri 提供 APK 内嵌资源。

旧版本曾支持独立的前端资源热更新。该机制现已完全停用：

- 不再检查、下载或应用 `frontend-dist-*.zip`；
- 不再从 `frontend_updates/<version>/` 读取 WebView 资源；
- GitHub Release 只上传签名 APK 与其 SHA-256 旁车文件；
- 启动时仅在固定应用私有目录通过 canonical 校验后，尽力清理遗留文件。

## 2. 架构：Rust 状态机 + 事件广播

更新状态的唯一持有者是 Rust 侧 `UpdateSession`（`app.manage()` 注入）。
前端不持有真相，只做镜像：订阅 `vcp-update://status` 事件 + 启动时
`get_update_status` 补拉。

```text
Idle → Checking → Available → Downloading → Verifying → ReadyToInstall → Installing → Idle
                      ↑              │（失败/取消）          │
                      └── Failed ←───┴──────────────────────┘
```

| 状态 | 含义 |
|------|------|
| `idle` | 无更新或流程结束 |
| `checking` | 正在查询 Release |
| `available` | 有新版本，待下载 |
| `downloading` | 下载中（含进度 `downloaded/total`） |
| `verifying` | SHA-256 校验中 |
| `readyToInstall` | 已下载且校验通过，可安装 |
| `installing` | 正在拉起系统安装器 |
| `failed` | 失败（`error.stage`: check/download/verify/install + `retryable`） |

安装类错误不回 `failed`：保持 `readyToInstall` 并写入 `error`，
用户处理问题（如授权）后可直接重试安装，无需重新下载。

## 3. 模块与命令

实现位于：

- `src-tauri/src/vcp_modules/updater/update_manager.rs`（状态机与命令层）
- `src-tauri/src/vcp_modules/updater/download.rs`（下载与校验机制）

对外五个 Tauri command：

| 命令 | 参数 | 职责 |
|------|------|------|
| `check_for_update` | 无 | 查询 Release、选定资产、下载并解析 SHA-256 旁车文件 |
| `start_update_download` | 无 | 断点续传下载 + 校验；URL 只来自 Rust 验证过的 API 响应 |
| `cancel_update_download` | 无 | 停止下载，保留 `.part` 供续传 |
| `get_update_status` | 无 | 前端补拉当前快照 |
| `install_update` | 无 | 权限/签名校验后交给 Android 系统安装器 |

> **契约红线**：前端（含设置页、弹窗、自动检查）禁止直接 invoke 这些命令，
> 必须经由 `core/stores/update.ts`；命令不接受任何 URL 或文件路径参数。

## 4. Release 查询与资产选择

固定查询仓库 `MRiecy/VCPMobile`：

1. 优先请求 `/releases/latest`；404 时降级 `/releases?per_page=1`；
2. semver 比较远端 tag 与当前 APK 版本；
3. APK 资产按 **`ends_with("arm64-v8a.apk")`** 选择——
   旁车文件 `*.apk.sha256` 永远不会误命中（1.1.4 前用 `contains` 曾导致
   选中校验文件而更新失败）；
4. 必须存在同名 `.sha256` 旁车文件，内容须为 `sha256sum` 标准格式
   （`<64hex>  <文件名>`，文件名与 APK 资产名一致），缺失或畸形即报错。

若本地 `updates/update.apk` 已存在且 SHA-256 与远端一致，直接进入
`readyToInstall`，不重复下载。

## 5. 下载与校验

- 下载目录统一为 `app_cache_dir/updates/`；启动时把旧位置（cache 根目录）
  的 `update.apk*`、`installing-*.apk` 迁移进来，缓存回收交由系统管理；
- 断点续传：`.part` 保留，重试时发送 `Range: bytes=N-`；仅接受 200（截断
  重下）或 206（校验 `Content-Range` 起点等于本地字节数）；416 或起点不符
  时删除 `.part` 从头再来；
- 停滞判死：单 chunk 30 秒无字节到达即放弃本次尝试；指数退避最多 3 次
  （取代旧版 300 秒整体超时）；
- 下载完成后流式 SHA-256 校验，不匹配则删除并允许重下；
- 保留既有防线：512 MiB 上限、GitHub 白名单域重定向（≤5 次）、flush+fsync
  后原子 rename、安装前 canonical 路径校验、`installing-<uuid>.apk`
  版本隔离暂存与 7 天陈旧清理；
- 下载期间通过插件 `acquire_ota_keepalive` 持有 ForegroundGuardian
  前台锁（`PRIORITY_OTA = 50`，60 分钟兜底超时），防止切后台被杀。

## 6. 安装链路

```text
install_update
  → 状态必须是 readyToInstall
  → can_install_packages（未授权 → 报错并引导跳系统授权页）
  → verify_apk_signature（PackageManager 提取 APK 与自签名证书 SHA-256 比对；
    不一致 → 删除安装包并拒绝安装）
  → installing-<uuid>.apk 暂存 → open_file_native → 系统安装器
```

## 7. 前端

- `core/stores/update.ts`：状态镜像与命令入口（唯一）；
- `core/composables/useAutoUpdate.ts`：READY 后延迟 2s 检查；24h 冷却
  （仅成功时写入时间戳）；"忽略此版本"（localStorage 记录版本号）；
  "自动检查更新"开关；
- `core/composables/useUpdateDownloader.ts`：通知栏进度（300ms 节流）
  与下载完成后的安装接续；
- `UpdatePrompt.vue` / `UpdateSection.vue`：状态机驱动的 UI，
  遵循 UI 层级与美学宪法。

## 8. 发布约束

`.github/workflows/release.yml` 只上传：

- `VCPMobile_v{VERSION}_arm64-v8a.apk`
- `VCPMobile_v{VERSION}_arm64-v8a.apk.sha256`

Release 前必须验证：tag 与版本一致、APK 签名有效、从旧正式版本覆盖安装成功。
