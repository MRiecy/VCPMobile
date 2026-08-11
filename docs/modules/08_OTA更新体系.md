---
title: APK 更新体系
id: module-08
version: "1.1.3"
description: GitHub Release 检查、APK 下载与 Android 系统签名安装
tags: [ota, update, apk, semver]
related_modules:
  - module-09
  - module-10
created_at: "2026-05-13"
updated_at: "2026-08-11"
---

# 08. APK 更新体系

## 1. 当前边界

VCP Mobile 只发布并安装完整 APK。HTML、CSS、JavaScript 等前端资源随 APK
一同签名和发布，运行时始终由 Tauri 提供 APK 内嵌资源。

旧版本曾支持独立的前端资源热更新。该机制现已完全停用：

- 不再检查、下载或应用 `frontend-dist-*.zip`；
- 不再从 `frontend_updates/<version>/` 读取 WebView 资源；
- 不再注册前端资源更新相关 Tauri command；
- GitHub Release 只上传 APK；
- 启动时仅在固定应用私有目录通过 canonical 校验后，尽力清理遗留文件。

即使遗留目录因校验或文件系统错误未被删除，应用也不会读取其中内容。

## 2. 模块与命令

实现位于：

`src-tauri/src/vcp_modules/updater/update_manager.rs`

对外保留三个 Tauri command：

| 命令 | 职责 |
|------|------|
| `check_for_update` | 查询 GitHub Release，比较 APK 版本并返回下载信息 |
| `download_update` | 将 APK 流式写入应用缓存并上报进度 |
| `install_update` | 交给 Android 系统安装器处理安装 |

## 3. 更新流程

```text
check_for_update
  -> GitHub /releases/latest
  -> 404 时回退到 /releases?per_page=1
  -> semver 比较远端 tag 与当前 APK 版本
  -> 查找 *arm64-v8a.apk

download_update
  -> app_cache_dir/update.apk
  -> 流式写入并发送 downloaded/total
  -> Content-Length 存在时核对实际字节数

install_update
  -> Android: 原生文件打开命令
  -> Desktop 调试: tauri-plugin-opener
  -> Android 系统安装器校验包名与 APK 签名
```

### 3.1 Release 查询

固定查询仓库 `MRiecy/VCPMobile`：

1. 优先请求 `/releases/latest`；
2. 若返回 404，查询 `/releases?per_page=1`；
3. 去掉 tag 的 `v` 前缀后与 `app.package_info().version` 比较；
4. 只选择名称包含 `arm64-v8a.apk` 的 Release asset。

若发现新版本但 Release 中没有匹配 APK，返回错误并引导用户前往 Release
页面，而不是尝试安装其他附件。

### 3.2 下载与安装边界

APK 下载使用 `reqwest` 字节流，避免把整个安装包缓存在内存中。初始 URL 必须是
`https://github.com/MRiecy/VCPMobile/releases/download/.../*.apk`，重定向只允许 GitHub
官方资产域且最多 5 次。响应即使没有 Content-Length，实际流量也受 512 MiB 硬上限控制。

下载先写入 `app_cache_dir/update.apk.part`，完整校验、flush 与 fsync 成功后才替换固定的
`update.apk`；失败会清理本次临时文件。`install_update` 会 canonical 校验调用方路径，
只允许打开这个固定缓存 APK。最终发布者身份仍由 Android 系统安装器的包名与签名校验、
以及 Release workflow 固定的证书 SHA-256 共同承担。

## 4. 遗留前端 OTA 清理

`cleanup_legacy_frontend_ota` 在 Tauri setup 阶段通过 blocking worker 异步处理以下旧目录，
不会让历史大目录的删除阻塞应用冷启动：

| 基础目录 | 固定子目录 |
|----------|------------|
| `app_config_dir` | `frontend_updates` |
| `app_cache_dir` | `frontend_update_downloads` |

清理规则：

1. 子目录名是编译期常量，不接收 IPC 或配置输入；
2. 拒绝符号链接和非目录对象；
3. canonical 后必须确认它是 canonical 基础目录的直接子目录；
4. 只有全部检查通过才执行 `remove_dir_all`；
5. 任一检查失败仅记录 warning，应用继续使用 APK 内嵌资源。

旧 `active_version` 位于 `frontend_updates` 内。启动流程不再读取该文件，因此清理
失败不会重新激活旧资源。

## 5. 发布约束

`.github/workflows/release.yml` 只上传签名 APK 及其 checksum：

- `VCPMobile_v{VERSION}_arm64-v8a.apk`
- `VCPMobile_v{VERSION}_arm64-v8a.apk.sha256`

Release 前必须验证：

- tag 与应用版本一致；
- APK 签名有效且证书与既有正式版本一致；
- 从旧正式版本覆盖安装成功；
- GitHub Release 中不存在独立的前端可执行资源包。

前端变更和 Rust/Android 变更采用同一 APK 发布、签名和回滚边界，不再维护第二套
运行时代码更新协议。
