# Android 存储与目录架构规范

> 文档类型：架构规范  
> 版本：v1.0  
> 日期：2026-05-24  
> 关联变更：物理存储去重机制与自定义 Android 文件选择插件（VcpMobilePlugin）落地

---

## 1. 背景与动机

在 VCP Mobile（Project Avatar）双轨三层（Double-Track 3-Tier）物理隔离架构的落地过程中，应用需要高频处理海量的 AI 智能体/群组聊天消息、OCR 文本提取、大文件分片上传以及图片缩略图的即时渲染。

在 Android 系统沙盒环境中，存储空间的使用和路径解析面临着极高的技术挑战：
- **安全沙箱隔离**：核心 SQLite 数据库与用户敏感聊天配置必须存放在高安全性、防泄露的内部私有沙盒中。
- **重资产容量爆发**：海量的图片、PDF、音视频等大文件附件不能过度积压在受限的内部存储中，需自动分流至外部专属沙盒。
- **Scoped Storage 限制**：Android 10+ 的分区存储限制导致应用无法直接用物理路径访问用户选中的文件，只能获得 `content://` URI。
- **物理冗余防范**：大文件跨会话传输时，必须在物理层面实现“内容寻址去重”，避免手机闪存被重复附件撑爆。

本规范旨在全面整理 VCP Mobile 的源码目录结构、Android 物理存储拓扑以及核心附件上传管道，为后续开发者与 AI 协同编程提供标准规范。

---

## 2. 源码开发目录全景

VCP Mobile 的开发目录严格奉行前后端物理隔离、特征共置（Feature Co-location）及物理记忆治理等原则：

| 目录路径 | 核心定位（有什么用） | 主要存放内容与设计规约 |
| :--- | :--- | :--- |
| **`src/`** | **前端渲染层源码** (Vue 3 + TS) | 前端核心。UI 呈现、Pinia 状态机与底层 UnoCSS 样式。 |
| ├── `src/core/` | 全局基础设施层 | 全局 Pinia stores、Hash 路由（仅一条 `/chat`）、全局自定义指令（长按、视口监听）、全局工具类等。 |
| ├── `src/features/` | 业务特征域 (领域物理隔离) | 按领域物理收拢的特征模块（如 `chat`、`agent`、`topic` 等）。**高内聚低耦合**，每个特征文件夹内自含 UI、Store 和 Composables。 |
| └── `src/components/ui/` | 物理共享原子 UI 组件 | **无状态、高复用**的纯粹 UI 容器（如 SlidePage, BottomSheet）。统一遵循语义化 Z-Index 规范。 |
| **`src-tauri/`** | **后端 Rust 与平台原生层源码** | Tauri 2 后端底座。Rust 核心逻辑、系统命令与 Android 插件层。 |
| ├── `src-tauri/src/vcp_modules/` | 核心 Rust 业务领域包 | 消息流式状态机、WAL 数据库事务、三阶段增量同步协议核心。`lib.rs` 仅作入口挂载，全部逻辑下沉至此。 |
| ├── `src-tauri/src/distributed/` | 分布式设备集成层 | 统一管理并注册 14 个 Android 底层物理设备能力（如电池、剪贴板、传感器获取工具）。 |
| └── `src-tauri/plugins/vcp-mobile/` | 自定义 Android 原生插件 | **项目内唯一自定义插件**（`tauri-plugin-vcp-mobile`），托管 Kotlin 原生代码。实现保活前台服务与 Scoped Storage 穿透。 |
| **`docs/`** | **高稳定性工程知识规范库** | 沉淀项目核心设计思想，包含 Android 插件管理、Z-Index UI 规范、增量同步体系等。 |
| **`plans/`** | **AI 协同物理记忆体系** | 专为 AI 代理设计的“物理大脑”。严格划分架构、重构、日志（Magi 会话）、真理固化五大分区。 |
| **`scripts/`** | **工程保障与自动化脚本** | 包含 WiFi/USB 双调试模式启动器、安全文件 IO 工具、Memory 自动编译脚本。 |

---

## 3. Android 运行时存储拓扑

### 运行时存储层级明细表

| 存储分类 | 物理路径 | Rust API 映射 | 主要存放内容与设计考量 |
| :--- | :--- | :--- | :--- |
| **内部私有沙盒** | `/data/user/0/com.vcp.avatar/files/` | `app_handle.path().app_config_dir()` | **SQLite 核心数据库**（`vcp_avatar.db`）。开启 WAL 模式所产生的临时共享内存与事务日志文件（`-wal`, `-shm`）。提供最高的数据防窥安全性。 |
| **内部私有缓存** | `/data/user/0/com.vcp.avatar/cache/uploads/` | `app_handle.path().app_cache_dir()` 的 `uploads` 子目录 | **运行时临时与分片上传缓存**（如拍照 `camera_*.jpg`、图库拷贝 `pick_*_temp`、分片临时文件）。由 Kotlin 插件物理重定向收拢生成于此，便于开机一锅端自愈清扫。 |
| **外部专属沙盒** | `.../Android/data/com.vcp.avatar/files/attachments/` | 自定义 `get_attachments_root_dir()` | **物理附件寻址库**。存储图片、PDF、视频等重资产。采用 **内容寻址存储（CAS）**，以 `SHA-256` 摘要哈希重命名去重，防止闪存重复冗余。 |
| **外部缩略缓存** | `.../Android/data/com.vcp.avatar/files/thumbnails/` | 自定义 `get_thumbnails_root_dir()` | **原生硬解缩略图库**（短边 200px）。供前端快速加载预览，防范 WebView 大图软解所产生的 OOM（内存溢出）崩溃。 |
| **系统附赠目录** | `.../Android/data/com.vcp.avatar/files/Documents/` (或 `Pictures/`) | `app_handle.path().document_dir()` | **空目录**。由于系统 API 查询行为自动创建（详见下文 §4 揭秘）。 |

---

## 4. 深度机制揭秘（一）：`Documents` / `Pictures` 自动创建之谜

### 4.1 现象描述
在新装应用的外部沙盒目录中，尚未进行任何附件下载或文档保存，物理磁盘上就自动多出了空文件夹 `Documents` 和 `Pictures`。

### 4.2 底层成因
在 Rust 层 `file_manager.rs` 定位附件目录时，使用了如下 fallback 路径解析：
```rust
if let Ok(mut path) = app_handle.path().document_dir() {
    path.pop(); // 弹出 documents，回到 /files/
    path.push("attachments");
    return Ok(path);
}
```
* **跨平台 JNI 调用**：Tauri 底层的 `document_dir()` 最终会映射到 Android 原生系统 API 的调用：`Context.getExternalFilesDir(Environment.DIRECTORY_DOCUMENTS)`。
* **Android 原生行为**：根据 Android SDK 规范，`Context.getExternalFilesDir(String type)` 在被查询时，只要传入了具体的标准媒体/文档常量（如 `DIRECTORY_DOCUMENTS` 或 `DIRECTORY_PICTURES`），**Android 系统的底层实现就会默认该应用接下来要使用这一空间，进而在物理上强制创建这个文件夹**。
* **架构演进结论**：虽然 Rust 层拿到该路径后立刻进行了 `.pop()` 弹出动作以定位到 `/files/attachments`，但**“只要查询发生，创建已在底层完成”**。这是 Android 系统的标准行为，不会影响应用性能或空间占用。

---

## 5. 深度机制揭秘（二）：附件上传“内部 cache 缓冲 ➔ 最终 rename 归档”管道

为了安全、丝滑地跨越 Scoped Storage（分区存储限制）并保护内存不泄露，VCP Mobile 构建了一条极具工业美感的数据接收归档管道：

```
[Android 系统文件选择器]
         │ 
         ▼ (1) 返回 content:// URI
[VcpMobilePlugin.kt]
         │
         ├──► (2) contentResolver.openInputStream(uri) 流式读取
         ├──► (3) 64KB 物理片流式拷贝至 cacheDir/uploads 专属目录
         ├──► (4) 同步对输入流执行 SHA-256 MessageDigest 摘要计算
         ├──► (5) 拷贝完成，对临时文件重命名为以哈希命名的正式缓存 (/uploads/{hash}.ext)
         ├──► (6) 如果是图片，物理触发 Native 硬件级 createImageThumbnail 缩略图生成
         │
         ▼ (7) 穿透 JNI 断裂层，将 cache 绝对路径、Hash、原始名返回给 Rust
[Rust file_manager.rs]
         │
         ├──► (8) 核心物理去重校验：检查 attachments/ 下是否已存在 `{hash}.ext`
         │         ├───► [若已存在]：瞬间物理删除内部 cache/ 中的文件，实现去重；
         │         └───► [若不存在]：瞬间执行 fs::rename(&cache_file, &dest_file)。
         │
         ▼ (9) 闭环落库，通知前端渲染
[SQLite vcp_avatar.db] ➔ 前端直连物理路径渲染 (无内存积压与零拷贝中转)
```

### 5.1 Scoped Storage 的 URI 突破
由于 Android 10+ 的分区存储规范，应用无法直接使用 C 语言风格的文件操作去读取外部存储的文件路径，只能拿到 `content://` 开头的 URI。  
自定义 Kotlin 插件 `VcpMobilePlugin` 的 `pickFile` 指令承担了“破壁人”的角色，通过 `contentResolver` 建立通道，将数据流式拉回到应用专属沙盒内。

### 5.2 内部 `cacheDir/uploads` 的安全缓冲定位
* **物理重定向与收拢**：应用原生拍照（`camera_*.jpg`）、图库流式拷贝（`pick_*_temp`）及分片缓存，统一重定向生成在 `/cache/uploads/` 专属目录下。
* **避免产生物理碎脏文件**：如果将尚未校验、尚未计算哈希的文件直接流式复制到正式的 `attachments/` 目录，一旦拷贝中断、哈希冲突或拷贝失败，外部附件库就会留下大量的未注册垃圾碎片文件。
* **缓存作为安全区**：内部 `uploads` 目录扮演了完美的“缓冲计算站”。文件在此处静默完成组装、哈希指纹提取以及原生硬解缩略图。

### 5.3 物理指针秒级转移（Rename）
* **原理**：因为内部 `cache` 和外部专属 `files/` 共享同一个 Android 存储分区挂载，当 Rust 层执行 `fs::rename` 时，底层 Linux 内核仅仅修改了文件系统的索引节点（Inode）指针，而**完全不需要在物理闪存中进行任何的数据流读写复制**。
* **优势**：无论是 10KB 的音频还是 500MB 的视频，从私有 `cache` 归档到外部 `attachments` 都在 **0.001 秒** 内瞬间物理跃迁完成，极大地节省了 CPU 开销并防范了 OOM。

---

## 6. 深度机制揭秘（三）：缓存大扫除与三级维护架构（Level 1/2/3）

为了从根本上治愈 Android WebView 运行后静态缓存顽固积压以及大容量物理附件垃圾堆积问题，系统设计了科学的“三层阶梯自愈式”维护架构：

```
                    ┌────────────────────────┐
                    │  系统缓存三级自愈体系   │
                    └───────────┬────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
┌───────────────┐       ┌───────────────┐       ┌───────────────┐
│    Level 1    │       │    Level 2    │       │    Level 3    │
│  WebView 缓存 │       │ 孤儿附件 GC   │       │ 系统物理重建  │
└───────┬───────┘       └───────┬───────┘       └───────┬───────┘
        │                       │                       │
 物理抹除HTTP Cache     双向"幽灵"磁盘校验       物理清除V8字节码
 释放WebView页面内存    CPU协作式时间片挂起     SQLite PRAGMA碎片整理
                        保障WebView满帧渲染     (绝对不伤sync_logs)
```

### 6.1 Level 1：网页状态与物理 HTTP 缓存清扫
* **Tauri 接口标准清理**：调用 Tauri 原生 `clear_all_browsing_data()`，释放 WebView 当前运行占用的页面缓存与图片缓存。
* **物理 HTTP 缓存一剪梅**：由于 Android 底层 Chromium 机制限制，原生 API 无法立即释放磁盘上的 HTTP Cache。Level 1 管道通过强行物理定位并删除 `/cache/WebView/Default/HTTP Cache` 目录，一举粉碎顽固积压的网页静态数据，彻底治愈存储死结。

### 6.2 Level 2：数据库-磁盘双向“幽灵附件”GC 与 CPU 时间片挂起机制
* **第一阶段（基于数据库引用的孤儿清理）**：扫描 `attachments` 索引表，对比 `message_attachments` 被删除或无效的哈希，将失去所有消息引用的附件与缩略图从磁盘上物理抹除，并从库中移除。
* **第二阶段（双向磁盘“幽灵文件”扫盲清扫）**：针对历史老版本残留、闪退或时序冲突留下的在库无记录的“物理幽灵文件”及缩略图，逆向遍历 `/attachments/` 与 `/thumbnails/`。
  * **64位十六进制哈希强校验**：基于指纹长度与 ASCII 字符强校验，保障删除安全性，绝不误杀。
* **核心亮点：CPU 协作时间片挂起机制 (Yield-Now Control)**
  > [!IMPORTANT]
  > **为什么需要 CPU 时间片协作挂起？**
  >
  > 在低端 Android 物理机型上，当用户聊天附件积累到上万个时，如果使用 Rust `tokio` 强力线程一次性无锁无挂起地遍历并校验所有磁盘物理文件，会导致 CPU 瞬间被打满到 100%。这将剥夺主线程 WebView 渲染和 Tauri IPC 通道的时间片，造成极严重的掉帧、卡顿（从 60 帧骤降至 5~10 帧）乃至 ANR（应用无响应）崩溃。
  
  为了完美解决这一痛点，Level 2 大规模清扫实现了**协作式 CPU 时间片出让控制**：
  * **出让频率**：系统将该机制抽象为通用状态机控制器 `YieldCounter`（位于 `file_manager.rs`）。在双向磁盘扫描及大小统计循环中，通过 `yield_ctrl.tick().await` 推进计数，当达到 200 阈值时自动挂起当前协程，将时间片安全出让给高优先级的前台渲染或 IPC 帧率保障任务。
  * **性能收益**：100% 杜绝了超大规模扫描时的掉帧卡顿。通过物理真机测试，在 20000+ 附件规模下清扫时，WebView 依旧以 60FPS 满帧率丝滑渲染，体验无感。

### 6.3 Level 3：系统物理重建与空间整理
* **重构背景**：处理 Chromium 在运行期强行独占锁定的 V8 `code_cache` 字节码编译缓存。该重建操作由静默冷启动或热升级等绝对安全期调度，安全闭环。
* **物理重构**：强力清除 `/cache/code_cache`；同时，对 SQLite 数据库运行增量真空物理整理：
  ```sql
  PRAGMA incremental_vacuum(500);
  PRAGMA optimize;
  ```
  通过分批回收 500 个 Page，避免造成单次大 Vacuum 导致长时间的 I/O 阻塞与锁竞争，重新规划索引，让数据库读取速度达到巅峰。
* **红线纪律**：**任何级别的清理操作均不允许误伤并清扫 `sync_logs`（多端差分同步日志）目录下的任何诊断数据**。

---

## 7. 深度机制揭秘（四）：Kotlin 原生实时自愈与冷启动极速清零

为了封死运行期间的每一处物理泄漏漏网之鱼，系统落地了“运行时实时擦除 ➔ 开机首航一锅端 ➔ 历史幽灵平滑自愈”的多重防御机制：

### 7.1 Kotlin 侧运行期实时物理自愈
* **常态释放**：当用户通过 Android 文件选择器进行流式拷贝时，如果用户在发送前主动点击了“取消”，或者图库流式读取发生了未知异常，Kotlin 侧 `VcpMobilePlugin` 会在 `catch` 异常分支及流中断分支中立即执行 `currentTempFile.delete()`。
* **成效**：实现物理临时文件运行时的瞬时自愈物理擦除，将任何泄漏扼杀在摇篮中。

### 7.2 开机首航“一锅端”极速清扫
* **极速归零**：每次应用冷启动（开机首航）时，Rust 侧 `clear_upload_cache` 会在后台静默执行。由于所有的临时文件（拍照、图库拷贝、分片）在 Kotlin 侧都已统一物理收拢至 `/cache/uploads/` 专有目录，Rust 侧无需进行昂贵的逐个扫描，直接通过 `fs::remove_dir_all` **一铲子物理清空 uploads 目录**。
* **优势**：0.001s 极速物理清零，完全不增加冷启动开机耗时。

### 7.3 老版本（1.0.1）历史幽灵垃圾自愈清除
* **平滑过渡**：由于老版本（1.0.1）将拍照和图库拷贝直接裸露存放在 `/cache/` 根目录下，老用户在升级到新版本后，根目录中仍积压着大量的垃圾文件。
* **自愈扫描**：在开机首航的清理逻辑中，系统会自动启动一次对于 `/cache/` 根目录的针对性扫描，精准剔除并抹除包含历史特征命名的 `camera_*.jpg` 以及 `pick_*_temp` 垃圾文件。
* **结果**：老版本积压账单被一次性精准出清，旧账彻底清零，实现物理存储无感平滑迁移！

---

## 8. 缓存治理架构与自动清理策略汇总

| 触发场景 | 作用级别 | 核心执行内容 | 执行时机与防时序冲突策略 |
| :--- | :--- | :--- | :--- |
| **应用冷启动** | 极速首航自愈 | 物理一铲子抹除 `/cache/uploads` 目录；<br>针对性扫描并剔除老版本遗留在 `/cache` 根目录下的历史垃圾。 | **开机静默执行**，瞬间完成，不增加首屏编译阻塞。 |
| **暂存文件发送** | 5分钟安全锁 | 校验 hash 后，如果创建时间在 300 秒内，即使未在消息中引用也跳过 targeted GC。 | **防时序冲突**。避免因网络握手稍慢，导致前台刚生成的暂存附件被后台 GC 线程误杀。 |
| **静默自动维护** | 3天周期自动 | 调用 Webview 标准清理；<br>SQLite `PRAGMA incremental_vacuum(100)` 分批页回收；<br>自愈清理 `message_attachments` 已删消息的关联；<br>运行 `PRAGMA optimize` 优化查询规划器。 | **距离上次清理超 3 天**在启动时自动挂载，静默无感。 |
| **前端手动维护** | Level 1 & Level 2 | Level 1：物理抹除 HTTP 缓存；<br>Level 2：进行超大规模孤儿/幽灵双向大清扫，开启 **CPU 200限额出让挂起** 机制。 | **用户手动触发**，保障前台 WebView 满帧率（60FPS）渲染，绝不卡死掉帧。 |

---

## 9. 后续维护与架构防范指南

1. **绝对路径安全性检查**：  
   后端 `file_manager.rs` 中的 `ensure_safe_path` 实行严格的白名单路径遍历防护。任何新增的文件写入/读取逻辑，必须确保绝对路径位于 `config_dir`、`cache_dir`、`attachments_dir` 或 `thumbnails_dir` 内部，否则会被拒绝并抛出安全错误。
2. **CPU 协作挂起阈值红线**：  
   在任何涉及大批量文件扫描（如文件查找、批量指纹读取、数据同步大清扫）的循环体中，**必须严格限制每处理 150 - 200 个文件主动执行一次 `tokio::task::yield_now().await` 协作挂起**，以捍卫移动物理端主线程的 60FPS 帧率红线。
3. **文本提取下沉**：  
   对于 PDF、Word（DOCX）等文件的纯文本提取（extracted_text）完全在后端 Rust 的后台任务中静默入库，**严禁将大段提取文本在中转时通过 JNI 或 WebView 参数回传给前端**，防范大文本回传产生的 WebView 卡顿与内存崩溃。
4. **日志诊断红线**：  
   任何级别的缓存清扫（手动、自动或冷启动）均**绝对禁止触碰、清扫或误伤 `sync_logs`（差分同步诊断日志）目录**，所有日志清扫策略由 lifecycle/updater 独立包治理。
