---
title: OTA 更新体系
id: module-08
description: APK 本体 OTA 与前端资源热更新的双轨更新机制详解
tags: [ota, update, frontend-update, rollback, semver]
related_modules:
  - module-09
  - module-10
created_at: "2026-05-13"
updated_at: "2026-05-13"
---

# 08. OTA 更新体系

## 1. 概述

VCP Mobile 采用**双轨 OTA（Over-The-Air，空中下载技术）**更新策略：

| 轨道 | 模块 | 更新对象 | 文件路径 |
|------|------|----------|----------|
| 本体轨道 | `update_manager.rs` | APK 安装包 | `src-tauri/src/vcp_modules/updater/update_manager.rs` |
| 资源轨道 | `frontend_update_manager.rs` | 前端静态资源（HTML/CSS/JS） | `src-tauri/src/vcp_modules/updater/frontend_update_manager.rs` |

### 1.1 为什么分离两种更新？

在移动端 Tauri（WebView）架构下，APK 本体与前端资源的生命周期天然解耦：

- **APK 本体**包含 Rust 运行时、Tauri 框架层、系统级权限声明（AndroidManifest.xml）和预置的 Web 资源。更新 APK 需要用户交互（系统安装器弹窗），且可能涉及权限变更、签名验证、系统级重启。
- **前端资源**仅为 Vite 构建产物（`dist/` 目录），在运行时可通过 Tauri 的自定义协议（asset protocol）从外部目录加载。更新前端资源无需重新安装 APK，用户无感知，可在应用内静默完成。

分离的核心收益：

1. **降低热修复成本**：UI Bug、文案调整、交互优化无需走完整的 APK 发布与安装流程。
2. **缩减下载体积**：前端资源包（`frontend-dist-v*.zip`）通常仅数 MB，而 APK 可达数十 MB。
3. **保留回滚能力**：前端热更新引入版本目录管理与启动失败自动回滚，避免"一次性覆写导致崩溃无法恢复"的灾难。
4. **APK 升级兜底**：当 APK 版本号提升时，旧的前端 OTA 包可能与新 Runtime 不兼容，系统自动清理，确保回退到 APK 内置版本。

### 1.2 适用场景

| 场景 | 推荐轨道 | 理由 |
|------|----------|------|
| Rust 逻辑变更、新增 Tauri 命令 | APK 本体 | 前端资源无法修改 Rust 运行时 |
| 新增 Android 权限、NDK 库升级 | APK 本体 | 需重新打包 APK 并重新签名 |
| UI 样式调整、组件 Bug 修复 | 前端资源 | 无需用户手动安装，秒级生效 |
| 前端路由变更、静态资源配置调整 | 前端资源 | 仅涉及 `dist/` 产物 |
| API 契约变更但 Rust 层已兼容 | 前端资源 | 前端可独立适配新字段 |

---

## 2. 双轨 OTA 架构对比表

| 维度 | APK 本体更新 (`update_manager.rs`) | 前端资源热更新 (`frontend_update_manager.rs`) |
|------|-----------------------------------|---------------------------------------------|
| **触发时机** | 用户手动点击"检查更新"或启动时主动检测 | 同左，用户触发检查 |
| **更新粒度** | 整包 APK（~10-50MB） | 前端产物 zip（~1-5MB） |
| **用户感知** | 高：需下载完整 APK → 系统安装器弹窗 → 确认安装 | 低：下载 zip → 解压 → 应用内重启（或下次启动生效） |
| **生效方式** | 系统级安装，覆盖旧 APK，应用进程终止 | 写入 `frontend_updates/<version>/`，切换 `active_version` 指针 |
| **回滚能力** | 无（依赖系统安装器的签名一致性，不支持自动回滚） | **有**：连续 3 次 boot 失败自动删除故障版本，回退到 APK 内置资源 |
| **校验方式** | Content-Length 大小校验 | SHA-256 manifest 逐文件校验 + Content-Length 下载校验 |
| **版本来源** | GitHub Release tag_name（如 `v0.9.13`） | Release Assets 中 `frontend-dist-v*.zip` 文件名，或 fallback 到 tag_name |
| **版本比较** | `semver::Version` 严格比较 | 同左 |
| **旧版本保留** | 不保留（仅缓存一个 `update.apk` 在 `app_cache_dir`） | 保留最近 2 个旧版本目录，其余按 semver 排序清理 |
| **APK 升级时的行为** | — | 若 APK 版本 > 活跃前端版本，自动清空全部前端 OTA 包 |
| **核心状态文件** | 无持久状态 | `active_version`（版本指针）、`boot_manifest.json`（boot 统计） |

---

## 3. APK 本体更新详解

本节基于 `src-tauri/src/vcp_modules/updater/update_manager.rs`（222 行）。

### 3.1 整体流程

```
┌─────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│  check_for_update│ ──▶ │ fetch_latest_    │ ──▶ │  semver 版本比较 │
│   (Tauri command)│     │   release()      │     │  (latest > current?)
└─────────────────┘     └──────────────────┘     └────────┬─────────┘
                                                          │
                              ┌───────────────────────────┘
                              ▼
                     ┌──────────────────┐
                     │ 查找 APK asset    │
                     │ (arm64-v8a.apk)  │
                     └────────┬─────────┘
                              │
         ┌────────────────────┼────────────────────┐
         │ asset 存在且有更新  │ asset 缺失但有更新   │
         ▼                    ▼                    ▼
┌─────────────────┐   ┌──────────────────┐   ┌──────────────────┐
│ 返回 download_   │   │ 返回错误提示用户   │   │ 返回 has_update= │
│ url + apk_size   │   │ 手动去 Release页   │   │ false            │
└─────────────────┘   └──────────────────┘   └──────────────────┘
                              │
                              ▼
┌─────────────────┐     ┌──────────────────┐     ┌──────────────────┐
│ download_update  │ ──▶ │ 流式下载到       │ ──▶ │ Content-Length   │
│ (progress via    │     │ app_cache_dir/   │     │ 完整性校验        │
│  Channel)        │     │ update.apk       │     │ 不一致则删除重试  │
└─────────────────┘     └──────────────────┘     └──────────────────┘
                              │
                              ▼
┌─────────────────┐     ┌──────────────────┐
│ install_update   │ ──▶ │ opener 打开 APK  │
│                  │     │ 触发系统安装器    │
│                  │     │ 失败则清理缓存    │
└─────────────────┘     └──────────────────┘
```

### 3.2 GitHub API 查询策略（降级机制）

`fetch_latest_release()` 实现了**两级降级查询**：

| 优先级 | 端点 | 适用场景 | 响应体 |
|--------|------|----------|--------|
| 1 | `GET /repos/MRiecy/VCPMobile/releases/latest` | 最新 Release 为正式版（非 prerelease） | 单个 `GitHubRelease` JSON |
| 2 | `GET /repos/MRiecy/VCPMobile/releases?per_page=1` | `/latest` 返回 404（最新为 prerelease） | `Vec<GitHubRelease>` 取首个 |

> 注：GitHub 的 `/releases/latest` 端点**仅返回最新的正式 Release**。如果仓库最新发布的是 prerelease，该端点会返回 404，此时降级到列表端点取第一个（即时间最新的 Release，无论是否 prerelease）。

请求头统一携带 `User-Agent: VCPMobile`，超时 15 秒。

### 3.3 semver 版本比较逻辑

```
latest_version  = release.tag_name 去掉前缀 'v'
current_version = app.package_info().version.to_string()

if semver::Version::parse 两边都成功:
    has_update = latest > current   // 严格的 semver 比较
else:
    has_update = latest != current  // 降级为字符串比较
```

- 使用 `semver` crate 进行结构化比较（支持 `major.minor.patch+pre-release`）。
- 若任一方不是合法 semver（如本地开发版本号），降级为纯字符串不等比较。

### 3.4 下载流程：流式、进度、完整性

`download_update` 接收 `Channel<DownloadProgress>` 用于向前端实时推送进度：

```
pseudo:
  apk_path = app_cache_dir / "update.apk"
  若 apk_path 存在: 先删除旧文件

  client 超时设为 300 秒
  res = client.get(url).send()
  total = res.content_length()        // HTTP Content-Length
  stream = res.bytes_stream()
  file = 创建 apk_path

  while chunk = stream.next():
      file.write_all(chunk)
      downloaded += chunk.len
      on_progress.send({ downloaded, total })

  file.flush()

  if total.is_some() && downloaded != total:
      删除 apk_path
      return Err("下载文件不完整")
```

- **流式写入**：使用 `reqwest` 的 `bytes_stream()` + `tokio::io::AsyncWriteExt`，避免将完整 APK 缓冲到内存。
- **进度推送**：每收到一个 chunk，即通过 Tauri `Channel` 向前端发送 `{ downloaded, total }`。
- **完整性校验**：以 HTTP `Content-Length` 作为预期大小，若实际下载字节数不匹配，立即删除损坏文件并报错。

### 3.5 安装触发（opener 打开 APK）

```
pseudo:
  result = app.opener().open_path(
      apk_path,
      mime_type = "application/vnd.android.package-archive"
  )
  if result 失败:
      删除 apk_path
      return Err("无法启动安装器...")
```

- 使用 `tauri-plugin-opener` 的 `OpenerExt` trait 调用系统安装器。
- MIME 类型固定为 `application/vnd.android.package-archive`，确保 Android 识别为 APK 安装意图。
- 若 opener 失败（如用户未授予"安装未知应用"权限），清理缓存文件并提示用户前往 GitHub Release 页面手动下载。

---

## 4. 前端资源热更新详解

本节基于 `src-tauri/src/vcp_modules/updater/frontend_update_manager.rs`（567 行）。

### 4.1 整体流程

```
┌─────────────────────────────┐
│  check_for_frontend_update   │
│   (Tauri command)            │
└─────────────┬───────────────┘
              ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│  get_local_baseline_version  │ ──▶│ 优先 read_active_version()  │
│                              │     │ 若无，则 fallback 到 APK    │
└─────────────┬───────────────┘     │ 内置版本号 get_apk_version()│
              │                     └─────────────────────────────┘
              ▼
┌─────────────────────────────┐
│  fetch_latest_release()      │  ← 与 APK 模块共用相同策略
│  find_frontend_asset()       │  ← 匹配 frontend-dist-v*.zip
└─────────────┬───────────────┘
              ▼
┌─────────────────────────────┐
│  extract_version_from_       │  ← 从文件名解析版本号
│    asset_name()              │    或 fallback 到 tag_name
└─────────────┬───────────────┘
              ▼
┌─────────────────────────────┐
│  semver 比较                │
└─────────────┬───────────────┘
              │
    ┌─────────┴─────────┐
    ▼                   ▼
┌────────┐        ┌──────────────┐
│有更新   │        │ 无更新        │
└───┬────┘        └──────────────┘
    ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│  download_frontend_update()  │ ──▶│ 流式下载到 cache_dir/        │
│  (progress via Channel)      │     │ frontend_update_downloads/   │
└─────────────┬───────────────┘     │ *.zip                        │
              │                     └─────────────────────────────┘
              ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│  apply_frontend_update()     │ ──▶│ 解压 zip 到                  │
│                              │     │ frontend_updates/<version>/  │
└─────────────┬───────────────┘     └─────────────────────────────┘
              │
              ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│  verify_unzipped_files()     │ ──▶│ 逐文件 SHA-256 与            │
│                              │     │ manifest.json 比对            │
└─────────────┬───────────────┘     └─────────────────────────────┘
              │
              ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│ 写入 active_version 文件      │ ──▶│ 内容为版本号字符串            │
│                              │     │ 如 "0.9.12"                  │
└─────────────┬───────────────┘     └─────────────────────────────┘
              │
              ▼
┌─────────────────────────────┐     ┌─────────────────────────────┐
│ cleanup_old_versions(keep=2) │ ──▶│ semver 排序，删除最旧的      │
└─────────────────────────────┘     └─────────────────────────────┘
```

### 4.2 版本检查：前端资源包解析

前端资源包在 GitHub Release Assets 中的命名约定：

```
frontend-dist-v{semver}.zip
例: frontend-dist-v0.9.12.zip
```

`find_frontend_asset()` 遍历 `release.assets`，匹配前缀 `frontend-dist-v` 和后缀 `.zip`。`extract_version_from_asset_name()` 从文件名中截取出 semver 版本号。若 Asset 中不存在前端包，则降级使用 `release.tag_name.trim_start_matches('v')`。

本地基线版本（`get_local_baseline_version`）的优先级：

1. 若 `frontend_updates/active_version` 文件存在且非空，其内容即为当前活跃的前端 OTA 版本。
2. 否则，使用 APK 内置版本号（`app.package_info().version`）。

### 4.3 下载与解压

下载逻辑与 APK 模块几乎一致，差异点：

| 差异 | APK 模块 | 前端资源模块 |
|------|----------|--------------|
| 缓存子目录 | `app_cache_dir/`（根目录） | `app_cache_dir/frontend_update_downloads/` |
| 文件名 | 固定 `update.apk` | 从 URL 提取原始文件名 |
| 内部函数可见性 | `#[tauri::command]` 直接暴露 | `download_frontend_update_inner()` 为 `pub(crate)`，供内部复用；外层 `#[tauri::command]` 包装 |

解压通过 `zip::ZipArchive` 实现：

```
pseudo:
  version_dir = frontend_updates / version
  若 version_dir 存在: 先删除
  创建 version_dir

  archive = ZipArchive::new(zip_file)
  for i in 0..archive.len():
      entry = archive.by_index(i)
      out_path = version_dir / entry.name()
      if entry.is_dir():  create_dir_all(out_path)
      else:
          确保 parent 目录存在
          buf = Vec::with_capacity(entry.size)
          entry.read_to_end(&mut buf)
          写入 out_path
```

### 4.4 manifest.json SHA-256 校验

解压完成后，`verify_unzipped_files()` 读取 `manifest.json`（由构建流程生成，位于 zip 根目录），执行逐文件哈希校验：

```
pseudo:
  manifest = json 解析 version_dir/manifest.json
  files = manifest["files"]  // 对象: { relative_path: expected_sha256 }

  for (path, expected_hash) in files:
      actual_path = version_dir / path.trim_start_matches('/')
      if !actual_path.exists(): return Err("缺少文件")
      content = fs::read(actual_path)
      actual_hash = sha2::Sha256::digest(content) 格式化为 hex
      if actual_hash != expected_hash: return Err("hash 不匹配")
```

- 若 zip 中不存在 `manifest.json`，**跳过校验**（向后兼容旧构建流程）。
- 使用 `sha2` crate 计算 SHA-256，结果格式化为小写 hex 字符串比对。

### 4.5 版本目录管理

前端热更新在文件系统中维护以下结构：

```
app_config_dir/
└── frontend_updates/
    ├── active_version              ← 纯文本文件，内容为当前活跃版本号（如 "0.9.12"）
    ├── boot_manifest.json          ← JSON，记录各版本的 boot 统计
    ├── 0.9.11/                     ← 旧版本目录（保留）
    │   ├── index.html
    │   └── ...
    ├── 0.9.12/                     ← 当前活跃版本目录
    │   ├── index.html
    │   ├── manifest.json
    │   └── ...
    └── 0.9.13/                     ← 可能的新版本目录
```

`cleanup_old_versions(updates_dir, keep)` 负责清理过期版本：

1. 遍历 `frontend_updates/` 下的所有子目录，排除 `active_version` 和 `boot_manifest.json`。
2. 按 semver 排序（解析失败则回退字符串比较）。
3. 仅保留最新的 `keep` 个版本（当前配置为 2），删除更旧的目录。

> 保留旧版本的目的：为回滚机制提供可回退的物理目录，同时控制磁盘占用。

---

### 4.6 回滚保护机制

前端热更新最大的风险是：推送了一个有缺陷的资源包，导致 WebView 白屏或 JavaScript 致命错误，应用陷入"启动即崩溃"的死循环。为此引入**基于启动成功计数的自动回滚**。

#### 4.6.1 状态文件：`boot_manifest.json`

```json
{
  "0.9.12": {
    "last_boot_at": 1715558400,
    "boot_count": 2,
    "boot_attempt_count": 2
  },
  "0.9.13": {
    "boot_attempt_count": 3
  }
}
```

| 字段 | 类型 | 含义 |
|------|------|------|
| `last_boot_at` | u64 (UNIX 秒) | 该版本最近一次成功启动的时间戳 |
| `boot_count` | u64 | 该版本**累计成功启动**次数（由 `confirm_frontend_boot()` 递增） |
| `boot_attempt_count` | u64 | 该版本**累计尝试启动**次数（由 `rollback_if_needed()` 递增） |

#### 4.6.2 两个关键函数的调用时机

| 函数 | 调用时机 | 作用 |
|------|----------|------|
| `rollback_if_needed()` | **Rust 启动流程中**，WebView 加载前端资源之前 | 检测活跃版本是否已连续多次启动失败，必要时删除故障版本并清除 `active_version` |
| `confirm_frontend_boot()` | **前端 Vue 应用完全挂载后**（如 `App.vue` 的 `onMounted`）通过 Tauri invoke 调用 | 向 Rust 报告"本次启动成功"，递增 `boot_count` |

#### 4.6.3 回滚判定流程（ASCII 图）

```
启动时 rollback_if_needed():
┌─────────────────────────┐
│ 读取 active_version      │
│ 例: "0.9.13"            │
└───────────┬─────────────┘
            ▼
┌─────────────────────────┐
│ 读取 boot_manifest.json  │
│ 获取该版本对应的 entry   │
└───────────┬─────────────┘
            ▼
┌─────────────────────────┐
│ boot_count = entry.      │
│   get("boot_count")      │
│   .unwrap_or(0)          │
└───────────┬─────────────┘
            ▼
┌─────────────────────────┐
│ boot_attempt_count =     │
│   entry.get("boot_       │
│   attempt_count")        │
│   .unwrap_or(0)          │
└───────────┬─────────────┘
            ▼
      ┌─────┴─────┐
      ▼           ▼
┌──────────┐  ┌──────────┐
│boot_count│  │boot_count│
│   == 0   │  │   > 0    │
└────┬─────┘  └────┬─────┘
     │             │
     ▼             ▼
┌──────────┐  ┌──────────┐
│ attempt  │  │ 正常启动 │
│ >= 3 ?   │  │ 不处理   │
└────┬─────┘  └──────────┘
     │
   是│
     ▼
┌─────────────────────────┐
│ 触发回滚：                │
│ 1. 删除 updates_dir/    │
│    0.9.13/               │
│ 2. 删除 active_version   │
│    文件                  │
│ 3. 下次启动 fallback     │
│    到 APK 内置版本       │
└─────────────────────────┘
     │
   否│
     ▼
┌─────────────────────────┐
│ boot_attempt_count += 1  │
│ 写回 boot_manifest.json  │
│ （记录本次尝试）          │
└─────────────────────────┘
```

#### 4.6.4 回滚判定表

| boot_count | boot_attempt_count | 行为 |
|------------|-------------------|------|
| 0 | 1 | 递增 attempt，继续尝试 |
| 0 | 2 | 递增 attempt，继续尝试 |
| 0 | ≥3 | **触发回滚**：删除版本目录 + 清除 `active_version` |
| ≥1 | 任意 | 视为已成功启动过，不处理（信任该版本） |

> **关键设计**：`boot_attempt_count` 在每次启动时（无论成功与否）都递增，而 `boot_count` 仅在前端成功挂载后由 `confirm_frontend_boot()` 递增。因此，若一个版本连续 3 次启动都未能走到 `confirm_frontend_boot`（如 JS 报错导致白屏），则判定为故障版本并自动清除。

### 4.7 APK 升级清理策略

当用户通过 APK 本体更新安装了新版本，APK 内置的前端资源可能已经比 OTA 热更新的版本更新。此时旧的前端 OTA 包不应再继续生效，否则可能出现：

- 新 APK 的 Rust 命令已变更，但旧前端资源仍调用旧接口。
- 新 APK 的内置前端版本更高，但 `active_version` 仍指向旧的 OTA 目录。

`clear_on_apk_upgrade()` 在每次启动时执行检测：

```
pseudo:
  apk_version  = get_apk_version()       // APK 内置版本
  active_version = read_active_version()   // 当前活跃的 OTA 版本

  if active_version 存在:
      if semver::Version::parse 两边都成功 且 apk > active:
          log::info("APK upgraded, clearing old OTA packages")
          删除整个 frontend_updates/ 目录
```

| 场景 | APK 版本 | 活跃 OTA 版本 | 行为 |
|------|----------|---------------|------|
| 首次安装 | 0.9.13 | None | 不处理 |
| APK 升级后首次启动 | 0.9.14 | 0.9.12 | 清空全部 OTA，回退到内置 0.9.14 |
| 仅前端热更新 | 0.9.13 | 0.9.14 | 不处理（OTA 版本 > APK 版本是正常情况） |
| 版本号解析失败 | dev | 0.9.13 | 不处理（semver 解析失败静默忽略） |

---

## 5. 公共基础设施

两个 OTA 模块虽然职责不同，但共享多项底层机制。

### 5.1 GitHub API 客户端

| 属性 | 值 |
|------|-----|
| User-Agent | `VCPMobile`（GitHub API 强制要求，否则可能返回 403） |
| 检查更新超时 | 15 秒 |
| 下载超时 | 300 秒 |
| TLS | `reqwest` 默认（`rustls-tls`，见 `Cargo.toml`） |
| 错误转换 | 全部通过 `map_err(|e| e.to_string())` 转为 `Result<T, String>`，符合 Tauri command 要求 |

### 5.2 下载进度结构（`DownloadProgress`）

两个模块各自定义了**同名同构**的 `DownloadProgress`：

```rust
#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}
```

- `downloaded`：已下载字节数（实时递增）。
- `total`：`Content-Length` 提供的总大小（可能为 `None`，如服务器未返回）。

前端通过 Tauri `Channel<DownloadProgress>` 接收流式事件，驱动进度条 UI。

### 5.3 semver 版本比较

两个模块均使用 `semver` crate：

```
has_update = match (parse(remote), parse(current)) {
    (Ok(r), Ok(c)) => r > c,
    _ => remote != current,  // 降级策略
}
```

- `semver::Version::parse` 支持标准语义化版本（`1.2.3`、`1.2.3-alpha`、`1.2.3+build`）。
- 当任一方解析失败（如开发版本号 `0.9.13-dev`），降级为字符串不等比较，避免阻塞更新检测。

---

## 6. 安全与可靠性考量

### 6.1 下载完整性

| 校验层级 | APK 本体 | 前端资源 |
|----------|----------|----------|
| 传输层 | HTTPS (TLS 1.2+) | 同左 |
| 大小校验 | Content-Length vs 实际写入字节 | 同左 |
| 内容校验 | 无（依赖 APK 签名） | SHA-256 manifest 逐文件校验 |

- APK 的终极完整性由 Android 系统安装器通过**数字签名**验证，模块层面无需额外哈希。
- 前端资源在解压后执行 `verify_unzipped_files()`，确保每个文件与构建时生成的 `manifest.json` 一致，防止 zip 传输损坏或被篡改单个文件。

### 6.2 路径安全

- 前端资源目录通过 `app.path().app_config_dir()` 获取，落在应用私有存储空间（`/Android/data/com.vcp.avatar/files/`），其他应用无读取权限。
- 解压时以 zip 内文件名为相对路径，拼接在 `frontend_updates/<version>/` 下，**不会逃逸出目标目录**（无 `../` 遍历风险，因为 `version_dir.join(entry.name())` 在标准路径拼接中已自然阻止绝对路径写入）。

### 6.3 回滚的可靠性边界

回滚机制基于 `boot_manifest.json` 的持久化计数，存在以下边界情况：

| 边界情况 | 影响 | 缓解措施 |
|----------|------|----------|
| `boot_manifest.json` 被手动删除 | 丢失历史统计，`boot_attempt_count` 重置为 0 | 无直接缓解；下次启动重新累计 |
| 用户连续 3 次启动时强杀进程 | `boot_attempt_count` 递增到 3，误判为故障版本并回滚 | 合理阈值（3 次）平衡容错与风险 |
| `confirm_frontend_boot` 在前端崩溃前被调用 | `boot_count` 被错误递增，故障版本不会被回滚 | 应在应用完全就绪、核心逻辑执行成功后调用 confirm |
| 磁盘空间不足导致解压失败 | `apply_frontend_update` 提前返回 Err，旧 `active_version` 不变 | 原子性：先解压到新目录，校验通过后才切换指针 |

### 6.4 并发与重入

- 两个模块的 Tauri command 均为 `async` 函数，由 Tokio 调度执行。
- 当前实现**未加显式锁**，若用户在前一次下载未完成时再次触发检查/下载，可能产生并发写入。建议前端 UI 层在下载进行期间禁用"再次检查更新"按钮。
- `apply_frontend_update` 在写入 `active_version` 之前已完成解压和校验，确保"切换版本指针"是最后一步，最大限度保证原子性。

### 6.5 清理策略的磁盘保护

- APK 缓存：`update.apk` 在每次下载前被删除，避免磁盘堆积。
- 前端 OTA：`cleanup_old_versions(keep=2)` 确保最多保留 3 个版本目录（当前活跃 + 2 个旧版）。
- APK 升级时：`clear_on_apk_upgrade` 一次性清空全部前端 OTA 目录，防止旧资源与新 Runtime 不兼容。

---

## 7. 函数接口表

以下列出两个模块的全部公开（`pub` / `#[tauri::command]`）及内部关键函数。

### 7.1 `update_manager.rs`

| 函数签名 | 可见性 | 说明 |
|----------|--------|------|
| `check_for_update(app: AppHandle) -> Result<UpdateInfo, String>` | `#[tauri::command] pub async` | 检查 APK 更新，返回版本信息、下载链接、Release Notes |
| `download_update(app: AppHandle, url: String, on_progress: Channel<DownloadProgress>) -> Result<String, String>` | `#[tauri::command] pub async` | 流式下载 APK 到缓存目录，通过 Channel 推送进度 |
| `install_update(app: AppHandle, apk_path: String) -> Result<(), String>` | `#[tauri::command] pub async` | 使用 opener 打开 APK 触发系统安装器 |
| `fetch_latest_release(client: &Client) -> Result<GitHubRelease, String>` | `async fn`（模块私有） | GitHub API 查询，带 `/latest` → `/list` 降级逻辑 |

### 7.2 `frontend_update_manager.rs`

| 函数签名 | 可见性 | 说明 |
|----------|--------|------|
| `check_for_frontend_update(app: AppHandle) -> Result<FrontendUpdateInfo, String>` | `#[tauri::command] pub async` | 检查前端资源包更新，从 Asset 文件名解析版本 |
| `download_frontend_update(app: AppHandle, url: String, on_progress: Channel<DownloadProgress>) -> Result<String, String>` | `#[tauri::command] pub async` | 流式下载前端 zip，包装 `download_frontend_update_inner` |
| `download_frontend_update_inner(app: &AppHandle, url: &str, on_progress: Option<Channel<DownloadProgress>>) -> Result<String, String>` | `pub(crate) async` | 内部可复用的下载实现，允许不传 progress Channel |
| `apply_frontend_update(app: AppHandle, zip_path: String, version: String) -> Result<(), String>` | `#[tauri::command] pub async` | 解压 zip、校验 manifest、切换 `active_version`、清理旧版本 |
| `get_active_frontend_version(app: AppHandle) -> Result<Option<String>, String>` | `#[tauri::command] pub async` | 读取当前活跃的 OTA 前端版本号 |
| `clear_frontend_updates(app: AppHandle) -> Result<(), String>` | `#[tauri::command] pub async` | 用户手动清空全部前端 OTA 数据 |
| `confirm_frontend_boot(app: AppHandle) -> Result<(), String>` | `#[tauri::command] pub async` | 前端挂载成功后调用，递增 `boot_count` |
| `rollback_if_needed(app: &AppHandle)` | `pub fn` | 启动时检测，连续 3 次 boot 失败则删除故障版本并清除 `active_version` |
| `clear_on_apk_upgrade(app: &AppHandle)` | `pub fn` | 启动时检测 APK 版本是否高于活跃 OTA 版本，是则清空全部 OTA 包 |
| `read_active_version(app: &AppHandle) -> Option<String>` | `pub fn` | 读取 `active_version` 文件内容，不存在或空则返回 None |
| `get_apk_version(app: &AppHandle) -> String` | `fn`（模块私有） | 读取 APK 内置版本号 |
| `get_local_baseline_version(app: &AppHandle) -> String` | `fn`（模块私有） | 获取本地基线版本（OTA 优先，否则 APK） |
| `fetch_latest_release(client: &Client) -> Result<GitHubRelease, String>` | `async fn`（模块私有） | 与 `update_manager.rs` 同构的 GitHub API 查询 |
| `find_frontend_asset(release: &GitHubRelease) -> Option<&GitHubAsset>` | `fn`（模块私有） | 在 Release Assets 中匹配 `frontend-dist-v*.zip` |
| `extract_version_from_asset_name(name: &str) -> Option<String>` | `fn`（模块私有） | 从 Asset 文件名提取 semver 版本号 |
| `verify_unzipped_files(update_dir: &Path) -> Result<(), String>` | `fn`（模块私有） | 基于 `manifest.json` 的 SHA-256 逐文件校验 |
| `cleanup_old_versions(updates_dir: &Path, keep: usize) -> Result<(), String>` | `fn`（模块私有） | semver 排序，保留最近 `keep` 个版本目录 |
| `clear_frontend_updates_sync(app: &AppHandle) -> Result<(), String>` | `fn`（模块私有） | `clear_frontend_updates` 的同步内部实现 |

---

## 8. 附录：数据结构定义

### 8.1 `UpdateInfo`（APK 更新信息）

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub has_update: bool,              // 是否有新版本
    pub current_version: String,       // 当前 APK 版本
    pub latest_version: String,        // 远程最新版本
    pub download_url: Option<String>,  // APK 直链（可能为 None）
    pub release_page_url: Option<String>, // GitHub Release HTML 页
    pub release_notes: Option<String>, // Release Body（Markdown）
    pub apk_size: Option<u64>,         // APK 文件大小（字节）
}
```

### 8.2 `FrontendUpdateInfo`（前端资源更新信息）

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FrontendUpdateInfo {
    pub has_update: bool,              // 是否有新版本
    pub current_version: String,       // 本地基线版本
    pub remote_version: String,        // 远程前端包版本
    pub download_url: Option<String>,  // zip 直链
    pub zip_size: Option<u64>,         // zip 文件大小
}
```

### 8.3 `DownloadProgress`（下载进度事件）

```rust
#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,               // 已下载字节数
    pub total: Option<u64>,            // 总大小（Content-Length，可能未知）
}
```

---

*文档生成基于源码：*
- `src-tauri/src/vcp_modules/updater/update_manager.rs`（222 行）
- `src-tauri/src/vcp_modules/updater/frontend_update_manager.rs`（567 行）
