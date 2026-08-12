# VCP Mobile 修复后全方位复审报告

> 复审日期：2026-08-11（Asia/Shanghai）
> 原始复审基线：`0ff6f92f79881cf9d10307015b715fe29f0a11ee`
> 收口实现起点：`78dd7f8` 检查点；最终实现与本报告随本轮提交版本化
> 前序报告：`VCPMobile_Full_Audit_2026-08-10.md`
> 复审方式：Magi 三路并行只读复核 + 主审跨层交叉验证 + 静态检查、单测、构建与依赖审计
> 覆盖范围：Vue/Pinia、富文本与 RAG、Tauri IPC、Rust/SQLite、Chat、Sync V2、Distributed、Frontend/APK OTA、Android 插件与分享入口、CI/Release、依赖、文档与测试治理
> 变更声明：原始复审为只读审计；后续经产品负责人批准，已完成 P0-P2 代码与工程门禁收口。本报告保留原始证据，并以 0A/0B 处置回执和第 2、10、14 节的当前状态覆盖历史判断。

---

## 0A. 已批准首批处置回执（2026-08-11）

| ID / 范围 | 当前状态 | 处置结果 |
| --- | --- | --- |
| POST-OTA-01 | **Closed by removal** | 完整移除 Frontend OTA 资源加载器、自动 check/download/apply、6 个 IPC command 和 Release ZIP 产物；应用始终使用 APK 内嵌资源 |
| POST-WEB-01 / RAG | **Resolved** | `RagPayloadDetail` 与保留的 `AssistantMessageCard` 统一复用 `filterTrustedRichHtml`，保留富 HTML 视觉与局部 DOM 交互，阻断宿主能力和直接脚本 |
| 划词 Assistant 运行入口 | **Dormant source asset / runtime closed** | 移除路由、production Vite input、App 窗口/事件、Tauri handler、capability window、原生命令注册/权限与 `SYSTEM_ALERT_WINDOW`；bootstrap 和通用权限检查不再触碰保留实现 |
| POST-SET-01 日志子项 | **Resolved** | 删除 floating assistant 对完整 Settings 对象的日志输出；POST-SET-01 其余 cache/corrupt JSON 问题仍保持 Open |

兼容性处理：新版本对历史 `frontend_updates` 和 `frontend_update_downloads` 只执行固定直接子目录的 canonical 校验清理；拒绝 symlink、普通文件和越界路径，不读取旧 `active_version`。清理在 `spawn_blocking` 中 best-effort 执行，失败只保留永不加载的旧数据，不阻塞冷启动。

当前验证证据：

- `pnpm check`：PASS。
- `pnpm test:run`：17 files / 66 tests PASS。
- `cargo test --locked --lib`：97/97 PASS；`file_extractor_integration`：10/10 PASS。
- `cargo fmt --all -- --check` 与 `cargo clippy --locked -- -D warnings`：PASS。
- Android plugin JVM：26/26 PASS。
- Android aarch64 debug APK：构建成功；实包 `aapt dump permissions` 无 `SYSTEM_ALERT_WINDOW`。
- production `dist/`：无 `floating.html`、Assistant 专属 chunk 或 frontend OTA 资源；移动端聚合 ACL schema 无已注销命令。
- `git diff --check`：PASS。

该回执记录首批处置时点；其后完成的 P0-P2 收口以 0B 为准。

---

## 0B. P0-P2 收口回执（2026-08-11）

本轮没有引入新的总状态机，而是沿用项目已有的 owner、generation、bounded executor、CAS、事务与 temp+rename 模式，把输入身份、失败语义和终态提交补齐。

| 范围 | 当前状态 | 收口结果 |
| --- | --- | --- |
| File / Android share | **Resolved** | Picker 与 `ACTION_SEND(_MULTIPLE)` 使用一次性 owner/ticket staging；数量、单项/总量、耗时、并发和磁盘预算受限；Rust 侧重算 size/SHA-256 后原子发布到 CAS，临时缩略图也不再跨请求共享 |
| Maintenance / GC | **Fail-closed; physical GC deferred** | 查询失败不再降级为空集合；在线维护不再物理删除附件、缩略图或 cache，避免 snapshot→unlink TOCTOU。代价是孤儿文件可能积累，列入显式存储维护债 |
| Sync / DB | **Resolved in mobile** | finalizer 与 pipeline 传播错误；发送失败终止 attempt；tombstone 单调、topic delete 与业务/hash 更新使用事务；最终 ACK 精确绑定 `sessionId + attemptId + phase + nonce` 并一次性消费 |
| Sync peer compatibility | **External release gate** | Mobile 协议期望版本提升至 1.1.0，拒绝不会回显身份字段的 1.0.0 peer；VCPChat 参考仓未被修改，正式联调前必须同步服务端插件 |
| Distributed | **Resolved** | 工具配置 missing/corrupt/oversize/unknown 均 fail-closed；持久化采用临时文件、fsync、rename 后切内存；WS frame、tool args、in-flight 与重复请求均有硬预算 |
| APK updater | **Resolved** | 下载源、redirect host、容量、`.part`、regular-file/canonical 校验受限；download/install 共用 owner 锁，安装前冻结到不可变 UUID 文件，消除替换 ABA |
| Settings | **Resolved** | UI 只提交相对初始快照的 patch；generation 串行化运行时副作用并重读最新 Settings；不再记录完整秘密对象 |
| Resource / task ownership | **Resolved with bounded residual** | 文件、Sync NDJSON、Distributed、model batch、原生 executor 均建立 owner、取消与预算；恶意 ContentProvider 若无视 cancellation，仍可能占用单个有界 worker，不能扩散成无界任务 |
| Release / Android / CI | **Code gate resolved; external run pending** | Actions 固定 commit SHA；Release 校验默认分支 ancestry、同 SHA push CI、四层版本、证书与 checksum；Gradle 8.14.3 wrapper/hash、dependency verification、Cargo `--locked`、workspace tests/clippy、generated drift、备份禁用与 release signing fail-closed 均已落地 |
| Capability / LAN / dormant assets | **Accepted product boundary** | 保留宽主窗口 capability、Root 与受信 LAN HTTP/WS；Frontend OTA 已删除，Assistant/local server 生产入口已注销。release cleartext 仅由显式 trusted-LAN build mode 开启 |

当前代码层发布阻断项已关闭；候选发布仍必须满足以下外部条件：

1. VCPChat `VCPMobileSync` peer 升级至 1.1.0，并原样回显 `phase/sessionId/attemptId/nonce`。
2. 在受保护默认分支与正式 GitHub secrets/keystore 环境中完整跑一次 Release workflow，核对旧版证书指纹、APK checksum 与升级安装。
3. 使用 API 26/36 及至少一台 Android 14/OEM 设备完成真实分享 Provider、FGS、进程死亡、Sync/Distributed 丢包和长稳验收。

---

## 0. 原始审计结论（由 0A/0B 回执覆盖当前状态）

### 0.1 发布判断

**当前基线 `0ff6f92` 不应作为正式候选发布版本。**

前一阶段修复并非失效。Chat generation、ActiveRequests attempt lease、会话 epoch、Sync session cancel/join、Distributed connection generation、Android helper socket ownership、DB 恢复分类、缓存 CAS 等核心修复，经本轮重新阅读代码和运行测试后仍然成立。

但是，修复后从其他入口反向审计时发现了前序清单没有覆盖的边界：

1. **已按 0A 关闭**：原始前端 OTA 会自动下载并安装没有可信签名的 WebView 代码，且解压与版本路径可越界。
2. **已按 0A 关闭**：原始 RAG 与浮动助手存在没有复用主消息 active-content filter 的 `v-html` 路径。
3. 原生临时文件、分享文件和附件注册仍信任来自 WebView 的路径/hash；外部 `ACTION_SEND` 入口又缺少数量、体积、耗时和磁盘预算。
4. Maintenance、Sync finalizer、Distributed 工具配置在错误路径上仍会“空集继续删除”“失败仍报成功”或“配置损坏后全部启用”。
5. tombstone 保护尚未成为所有 repository/sync upsert 的统一不变量，仍存在旧数据复活与 topic 删除并发创建的窗口。
6. Release/Gradle 构建链缺少 action SHA、Gradle distribution hash、dependency verification 和发布前门禁闭环。

这意味着：**常规测试全绿证明已有测试覆盖的状态机没有明显回归，但不能证明更新包、破坏性维护、IPC 路径和错误语义安全。**

### 0.2 最务实的发布边界

短期最稳妥的处理不是立即建设完整 OTA PKI，而是：

1. **已完成**：停止生成和发布 `frontend-dist-v*.zip`，移除前端 OTA 运行时，只保留 Android 系统签名保护的 APK 更新。
2. 修复本报告列出的 P0/P1 代码边界，并补对应负向测试。
3. 固定 Release/Gradle 供应链身份，使用正式 keystore 重跑发布链。
4. 完成 API 26/36、Android 14 FGS/OEM 后台、真实分享 Provider、Sync/Distributed 丢包和长稳真机验收。

如果未来必须恢复前端 OTA，再单独实现“签名清单 + 受控下载 artifact + 有界解压 + 原子激活”，不要把它混入本轮状态机或依赖升级。

---

## 1. 复审方法与判定口径

### 1.1 多角度职责

| 视角 | 核心问题 | 本轮重点 |
| --- | --- | --- |
| Melchior｜逻辑与系统 | 错误是否真正阻止提交；终态是否单调；数据是否可恢复 | SQLite、tombstone、Sync finalizer/ACK、Maintenance、缓存、并发所有权 |
| Balthasar｜移动端与交互 | Android 外部入口是否受控；富 HTML 保真与宿主能力是否分离 | RAG/assistant renderer、文件分享、原生 IPC、权限、WebView/OTA |
| Casper｜务实与交付 | 最小可交付修复是什么；构建制品是否可追溯；门禁是否真实 | npm/Rust 供应链、Release、Gradle、CI、文档与测试漂移 |
| 主审交叉复核 | 一个域的“正常输入”是否成为另一域的“不可信输入” | OTA → 主 WebView → IPC；HTML → 文件命令；Sync → DB/cache；发布链 → 签名环境 |

### 1.2 严重度与状态

- **Critical**：可替换特权前端、越界删除/写入或形成等价应用级控制；必须阻断发布。
- **High**：可能造成有效数据批量丢失、权限能力意外开放、终态假成功、旧数据复活或可由不可信本地/远端输入触发的严重可用性问题。
- **Medium**：需要特定前置条件，或主要影响治理、隐私、资源稳定性与可维护性。
- **Accepted**：产品负责人已明确接受的设计风险；必须保留记录，不得写成“技术上消失”。
- **External acceptance**：必须由正式签名、真机/OEM、真实网络或长稳测试证明，host/JVM 单测不可替代。

### 1.3 边界声明

- 第 3-8 节保留原始只读后审计证据；0A/0B、第 2、10、14 节记录后续修复与当前判定。
- 行号对应 `0ff6f92`；后续修改应按函数名和不变量定位，不能只依赖行号。
- 未对互联网服务、GitHub 账号权限、真实签名密钥、VCP 服务端或 OEM 行为做渗透测试。
- 没有连接 Android 设备，因此设备级验收仍未完成。

---

## 2. 总表

| ID | 等级 | 类型 | 状态 | 结论 |
| --- | --- | --- | --- | --- |
| POST-OTA-01 | **Critical** | 代码/供应链 | **Closed by removal** | 原缺陷成立；现已删除 Frontend OTA 运行时、资源 loader 与 Release ZIP，只保留签名 APK 更新 |
| POST-WEB-01 | High | 代码/渲染 | Resolved | RAG/Assistant renderer 已共用 trusted-rich-HTML filter；Assistant 生产入口已休眠 |
| POST-FILE-01 | High | 代码/IPC | Resolved | native staging owner/ticket、canonical containment、自算 hash、唯一 temp 与原子 CAS 发布已统一 |
| POST-AND-01 | High | Android/资源 | Resolved with bounded residual | 外部分享与 picker 已有数量/容量/超时/并发预算和取消；不响应取消的 Provider 只会占用单个有界 worker |
| POST-DATA-01 | High | 代码/数据 | Resolved / safe degradation | DB 失败零删除；在线物理 GC 暂停，避免误删有效附件，孤儿文件积累作为已接受维护债 |
| POST-SYNC-01 | High | 代码/协议 | Resolved | finalizer、queue flush、pipeline 与关键 WS send 全部传播失败，commit 前不进入完成态 |
| POST-SYNC-02 | High | 代码/协议 | Mobile resolved / external peer gate | final ACK 绑定 session/attempt/phase/nonce 且防 replay；正式发布依赖 VCPChat peer 1.1.0 |
| POST-DIST-01 | High | 代码/隐私 | Resolved | 工具策略默认全禁用，损坏/缺失/未知 fail-closed，原子落盘后才切运行时状态 |
| POST-DB-01 | High | 代码/一致性 | Resolved | tombstone、topic delete、begin/finalize、sync queue 与 cache invalidation 已形成单调提交边界 |
| POST-REL-01 | High | 发布治理 | Code gate resolved / external release gate | action SHA、分支/CI、版本、证书、checksum、Gradle wrapper/verification 已闭环；待正式 secrets 实跑 |
| POST-CAP-01 | High（理论影响） | 产品边界 | Accepted / Amplifier | CSP 为空、asset/capability 很宽；在可信圈模型下可接受，但显著放大 OTA/DOM/路径缺陷 |
| POST-APK-01 | Medium | 代码/更新 | Resolved | 固定可信 GitHub 路径/redirect host、512 MiB 上限、part+sync、不可变 install stage 与共享锁 |
| POST-SET-01 | Medium | 代码/隐私 | Resolved | patch 写入、cache generation、串行副作用、损坏恢复与秘密日志 redaction 已落实 |
| POST-RES-01 | Medium | 代码/稳定性 | Resolved with bounded residual | 四域均有硬预算与任务 owner；Provider 不合作取消为有限外部残余 |
| POST-TXN-01 | Medium | 代码/一致性 | Core resolved / derived data accepted | 业务与 hash/终态关键写入已事务化；可重建缩略图、文本 cache 继续允许明确 best-effort |
| POST-ANDROID-02 | Medium | 发布/隐私 | Resolved with product exceptions | release signing fail-closed、备份关闭、权限按功能请求；受信 LAN cleartext 与禁设备迁移为明确产品选择 |
| POST-CI-01 | Medium | 测试治理 | Resolved | 失效脚本已移除，Cargo locked/workspace、audit、strict Gradle 与 tracked/untracked generated drift 已进门禁 |
| POST-LOCAL-01 | Low | 潜在能力 | Dormant / Unregistered | 历史 localhost server 代码保留，bootstrap 不再触碰且 Tauri handler 已注销 |

---

## 3. Critical：前端 OTA 信任链与路径边界失效

### POST-OTA-01｜Critical｜自动安装未经认证的特权 WebView 代码

> **当前处置：Closed by removal。** 以下保留的是原始漏洞证据；当前代码已无 Frontend OTA 运行时、asset loader、IPC 命令或 Release ZIP 消费链。

#### 3.1 完整可达链

1. `src-tauri/src/vcp_modules/infra/lifecycle_manager.rs:282-371`
   - 应用启动约 5 秒后自动执行 check → download → apply。
   - 正常自动路径没有独立用户确认，也没有把检查阶段得到的 asset identity 冻结成不可伪造 artifact。
2. `src-tauri/src/vcp_modules/updater/frontend_update_manager.rs:267-347`
   - 下载 command 接受任意 URL。
   - 没有强制 HTTPS、固定 GitHub repository/asset host、受控 redirect 或最大下载量。
   - GitHub API 返回的 asset size 没有成为下载提交条件；可选 `Content-Length` 也不是可靠总量边界。
3. `frontend_update_manager.rs:350-365`
   - apply command 直接接受任意 `zip_path` 和 `version`。
   - `updates_dir.join(version)` 后会对既有目录执行 `remove_dir_all`；绝对路径或父目录组件可逃逸更新根，形成越界删除/写入。
4. `frontend_update_manager.rs:367-395`
   - ZIP entry 使用 `version_dir.join(file_in_zip.name())`，没有 `enclosed_name()` 和 canonical containment。
   - entry 可包含绝对路径或 `../`，形成 Zip Slip。
   - 每个 entry 直接按声明大小分配并 `read_to_end`，没有 entry 数、单文件、总解压量或压缩比限制。
5. `frontend_update_manager.rs:145-185`
   - `manifest.json` 缺失时校验直接成功。
   - manifest 与 payload 位于同一不可信 ZIP，没有 APK 内置公钥或外部可信 hash；攻击者可自行修改文件再自行生成 hash。
   - manifest 没有强制覆盖全部文件，额外未列文件不会被拒绝。
6. `.github/workflows/release.yml:169-185` 与 `vite.config.ts:66-133`
   - 官方工作流只是压缩 `dist/` 并上传，没有生成强制 manifest、detached signature 或可信 hash。
   - 因此官方 `frontend-dist` 实际也会走“manifest 缺失仍成功”的路径。
7. `src-tauri/src/vcp_modules/updater/ota_assets.rs:14-15,30-40` 与 `src-tauri/src/lib.rs:88-119`
   - OTA 目录优先于 APK 内置资源，并继续运行在应用 Tauri origin。
   - 新前端继承主 WebView 的 Tauri IPC 能力，不是普通网页沙箱。
8. `src-tauri/src/lib.rs:356-361`
   - check/download/apply/rollback 等 command 注册在主 invoke handler，不能只依赖“正常 UI 不传恶意参数”作为安全边界。

`ota_assets.rs` 对 Tauri 已 percent-decode 的 `AssetKey` 直接 `update_dir.join(...)`，没有拒绝 ParentDir 或做 canonical containment。形如 `/%2e%2e%2f...` 的请求可避开浏览器对字面 `../` 的规范化，再在 asset handler 侧还原为父目录组件；OTA 激活后可把更新根父目录中的 app-private 文件作为同源 asset 返回。该路径读取必须与 OTA 写入一起关闭。

另有两个激活可靠性问题应随票处理：`src-tauri/src/lib.rs:90-108` 的 OTA base package 选择与 debug `.debug` 包名不一致，debug 环境可能根本不命中预期更新目录；`src/main.ts:69-75` 在 mount 后立即确认 boot，lazy/runtime 后续失败也会被记为本次 OTA 启动成功。这两项不是 Critical 的成立条件，但会削弱 rollback 机制。

#### 3.2 影响

- 删除或覆盖应用权限范围内的文件/目录。
- Zip Slip 写出版本目录。
- zip bomb 导致内存、磁盘或启动可用性丢失。
- 持久化替换完整前端；恶意 JS 可调用 settings、文件、Root、原生分享、分布式等 IPC。
- HTTPS 只能保护传输，不证明发布者身份；包内自带 SHA-256 只能发现意外损坏，不能成为信任根。

#### 3.3 立即处置

在完整签名方案完成前：

- Release 停止生成/上传 `frontend-dist-v*.zip`。
- 生命周期关闭自动 frontend apply；已下载 ZIP 不激活。
- 保留 Android 系统签名校验和用户确认保护的 APK 更新。

#### 3.4 如需恢复 OTA 的最小闭环

- APK 内置发布公钥，强制验证 detached signature 或签名清单。
- manifest 必须存在、覆盖完整文件集合并拒绝额外文件。
- check 结果生成不可伪造的一次性 artifact token；apply 不再接受任意 URL/path/version。
- 仅允许指定 GitHub repository 的 HTTPS asset，redirect 每一跳都复核 scheme/host/asset identity。
- version 严格解析 semver；路径只允许 Normal component；所有源/目标 canonical containment。
- 使用 `ZipFile::enclosed_name()`，拒绝 symlink、绝对路径和父目录。
- 流式解压到专用临时目录，限制下载量、文件数、单文件、总解压量和压缩比。
- 全量验证成功后原子 rename，再原子切换 active pointer；失败清理临时目录且不影响旧版本。

#### 3.5 必补测试

- version 为 `..`、多级父目录、绝对路径、Windows/Unix 路径变体。
- ZIP entry traversal、absolute entry、symlink、重复文件名、额外文件。
- manifest 缺失、签名错误、hash 错误、漏文件、重放旧版本。
- 无 Content-Length/chunked 超量、跨 host redirect、zip bomb、文件数耗尽。
- 断电/异常发生在 extraction、rename、active pointer 三个边界时旧版本仍可启动。

---

## 4. High：渲染、IPC 与 Android 外部输入

### POST-WEB-01｜High｜RAG 与 Assistant 绕过富文本主动能力门禁

> **当前处置：Resolved。** RAG 与保留的 Assistant renderer 已复用 `filterTrustedRichHtml`；Assistant 运行入口已从生产路由、构建、IPC、capability 和 Android 权限链注销。以下保留原始证据以便追溯。

前序 SEC-01 采用的是保真优先的 `filterTrustedRichHtml`，明确保留布局、CSS、SVG/MathML、媒体和受限局部交互，只切掉危险主动能力。本轮发现该策略没有覆盖所有 `v-html` renderer：

- `src/features/rag/RagPayloadDetail.vue:20-40`
  - `marked.parse()` 结果直接进入 `v-html`，未调用 `filterTrustedRichHtml`。
  - `isQuery=true` 只替换 `<`/`>`，不能阻止 Markdown `[x](javascript:...)`；本地使用项目当前 `marked` 实测会生成 `href="javascript:..."`。
- `src/features/rag/RagObserver.vue:703-707,836-839,973-977,1161-1168`
  - 多条 response/narrative 路径传入远端 VCPInfo payload；其中 narrative 没有传 `is-query`，raw HTML/event attribute 直接可达。
- `RagObserver.vue:179-185`
  - RAG 右栏是正常 UI 入口，不是仅供调试的隐藏组件。
- RAG payload 由 `vcp_info_service.rs:330-346` 的 WebSocket 数据进入，并通过 `ragObserver.ts:46-54` 交给前端；不是静态常量。
- `src/features/assistant/AssistantMessageCard.vue:19-43,112-118`
  - 同样是 `marked.parse()` → `v-html`，没有共享 filter。
  - `src/core/router/index.ts:4-8` 保留 `/assistant`，`App.vue:459-463` 可在主 WebView 直接进入 `#/assistant`，浮窗创建 handler 也仍存在；该路径是可达代码，不应按休眠能力降级。

**影响**：RAG narrative 已经可达；一旦 VCPInfo 数据、代理内容或中间链路携带主动 HTML，它会在主 WebView 上下文执行。Assistant 是未来重新启用时的同类放大器。

**最小修复**：两处 renderer 统一复用现有 `filterTrustedRichHtml(marked.parse(...))` 与共享 URL gate。不要引入另一套 DOMPurify allowlist，也不要关闭 raw HTML；这样不会牺牲用户要求的富 HTML 保真。

**测试**：对 RAG query/narrative、assistant 分别覆盖 event handler、`javascript:`/实体/控制字符混淆、iframe sandbox，以及 SVG/MathML/style/grid/动画/局部视觉交互保真。

### POST-FILE-01｜High｜文件能力没有单一受控 staging 边界

这是四个表象、一个根因：WebView 仍能把任意字符串当成本地路径或可信 hash 交给更高权限层。

1. `src-tauri/plugins/vcp-mobile/src/system.rs:485-505`
   - `write_temp_file(bytes, file_name)` 直接 `cache_dir.join(file_name)`，未拒绝绝对路径、分隔符、父目录，也没有输入大小上限。
2. `system.rs:508-523`
   - `delete_temp_file(file_path)` 仅做 lexical `starts_with(cache_dir)`；`cache/../...` 与 symlink 不能靠字符串前缀保证 containment。
3. `system.rs:621-656` 与 `VcpMobilePlugin.kt:1756-1876`
   - `register_shared_files` 接受 WebView 传入的 `cachePath`；Kotlin `processSharedFile` 直接构造 `File(cachePath)` 并复制到 uploads，没有验证该路径由原生 picker/share 本次产生。
4. `src-tauri/src/vcp_modules/infra/file_manager.rs:462-675`
   - `register_local_file` 的允许根过宽；`expected_hash` 只校验格式，命中已有 CAS 时不重新计算内容。
   - `thumbnail_path` 没有同等 `ensure_safe_path`，文件搬运与 DB 更新错误不能保证一致回滚。
5. `file_manager.rs:683-708`
   - `get_attachment_real_path(hash, original_name)` 直接拼接调用方提供的路径组成部分，没有只按 DB 记录解析并 canonical 限制于 attachment root。
6. `src/App.vue:94-174`
   - shared intent 使用共享可变 singleton 汇合文本与文件；快速到达的 A/B 两次 intent 可能把 B 的文本和 A 的文件组合成一次发送，缺少 per-intent owner/generation。

这些入口在正常 UI 下多半接收应用自己生成的路径，但 Tauri command 的安全边界必须按“已取得 WebView 执行能力的调用者”判断；在原始基线中，POST-OTA-01 与 POST-WEB-01 会把它们从理论风险放大为实际后续能力。两个放大器现已按 0A 回执收口，但文件 IPC 自身边界仍需独立修复。

**最小架构**：原生 picker/share 只返回一次性、不可猜的 staging ticket；Rust 在单一 staging root 内解析、canonical 校验、流式重算 SHA-256、消费一次后失效。thumbnail 走同一 gate。短期至少做到 basename 校验、绝对/ParentDir 拒绝、canonical containment、hash 永远重算、DB 错误传播与临时文件回滚。

### POST-AND-01｜High｜外部分享可造成主线程与存储耗尽

- `src-tauri/gen/android/app/src/main/AndroidManifest.xml:43-114` 暴露 `ACTION_SEND` / `ACTION_SEND_MULTIPLE` intent filter。
- `ShareIntentHandler.kt:25-47,85-103,118-159` 在处理 intent 时遍历外部 URI，使用 `input.copyTo(output)`，没有 URI 数量、单文件、总量、读取时限、取消、剩余磁盘或部分文件清理预算；本次 staging 源也没有完整生命周期清理。
- `VcpMobilePlugin.kt:1217,1254-1256` 在初始化及 `onNewIntent` 路径直接调用 handler；慢速或无限 ContentProvider 可阻塞关键启动/回调线程。

这不要求攻击者进入 VCP 圈子；同设备其他应用可发送匹配 intent 或提供异常 ContentProvider。影响主要是启动卡死、缓存/磁盘耗尽和 OOM，而不是直接代码执行。

**最小修复**：把复制放入已有有界 file executor；限制文件数、单文件和总字节，设置读取 deadline/cancellation 与 free-space 水位；文件名只取安全 basename；任何失败清理本次 staging；主线程只接收完成事件。

---

## 5. High：失败语义、数据与同步不变量

### POST-DATA-01｜High｜Maintenance DB 读取失败可触发全量有效附件删除

- `src-tauri/src/vcp_modules/infra/maintenance_manager.rs:83-110,132-203,208-270`
- `SELECT hash FROM attachments` 的错误被 `unwrap_or_default()` 变成空集合。
- 后续扫描把 CAS 附件、thumbnail 和 multimodal cache 中不在该集合的文件视为 ghost 并删除。
- 因而 SQLite BUSY、I/O error 或连接异常可能被解释为“数据库没有任何有效附件”。

**不变量**：任何 destructive cleanup 必须先取得完整、成功且一致的 live-set snapshot；任一查询失败时删除数量必须为 0。

**最小修复**：先在只读事务中获得所有 live/tombstone/index 集合；错误立即返回。更稳妥的是先移动到 quarantine，DB/文件协调完成后再延迟物理删除。

**测试**：对每个查询点故障注入，断言零文件删除、零 tombstone 扩散，并能安全重试。

### POST-SYNC-01｜High｜SyncFinalizer 吞错后仍可能报告成功

- `src-tauri/src/vcp_modules/sync/sync_finalize.rs:66-198`
  - metadata 查询使用 `if let Ok`；transaction begin 失败会跳过主要逻辑。
  - topic count、hash/bubble、pipeline 通知等多处结果被忽略。
  - 只有 commit 本身失败稳定返回 Err。
- `src-tauri/src/vcp_modules/sync/sync_service.rs:1293-1364,1553-1602`
  - `execute()` 返回 Ok 后发送最终 phase；收到 ACK 后发布完成。

这会形成“数据或派生 hash/count 未完成，但 UI 和协议进入 completed”的假成功，下一轮差异计算也可能建立在错误状态上。

**最小修复**：finalizer 中所有必需步骤返回显式 `Result` 并使用 `?`；同一事务内提交可原子化的数据；commit 成功后才清 cache/发送 final phase。通知类非关键步骤若允许 best-effort，必须在协议和日志中明确区分，不能影响数据成功定义。

### POST-SYNC-02｜High｜最终 ACK 没有绑定当前 attempt

- `sync_service.rs:1331-1352` 发送 messages phase 后只设置一个 final-ACK pending `AtomicBool`。
- `sync_service.rs:1553-1558` 在 pending 窗口内接受任意 `type == "PHASE_COMPLETED"`，没有核对 phase、attempt、session 或 nonce。
- 同连接上的迟到早期 phase、重复包或重放包可能让当前 attempt 提前完成。

**最小修复**：每个 sync attempt 生成随机 nonce/attempt ID；最终消息包含 session、attempt、phase 和 nonce，ACK 必须精确回显且只能消费一次。错误 phase、旧 attempt 和重放均忽略并保持 deadline。

### POST-DIST-01｜High｜Distributed 工具禁用配置损坏时 fail-open

- `src-tauri/src/distributed/tool_registry.rs:81-116,233-280`
  - disabled set 初始为空，含义是所有工具启用。
  - 配置存在但 open/read/JSON 失败时仍保持空集合。
  - poisoned lock 时 `is_enabled` 返回 true。
  - 保存使用 `File::create` 原地截断，崩溃/ENOSPC 可留下空文件。
- `src-tauri/src/distributed/mod.rs:57-73`
  - 更新内存后忽略持久化错误并返回成功。
- 工具表包含 `distributed/tools/mod.rs:26-48` 与 `tools/clipboard.rs:43-72` 的剪贴板读写等本机能力。

远端仍需通过现有 VCP 身份认证；因此这不是“互联网未认证 RCE”，而是**配置损坏后向已认证远端意外开放全部本机工具**，属于隐私/能力 fail-open。

**最小修复**：缺失、损坏、不可读、未知名字或锁异常一律默认全禁用；temp + fsync + rename；持久化成功后才切换内存；提供 typed recovery/重置入口。

### POST-DB-01｜High｜tombstone 尚未成为统一单调终态

#### 旧数据复活

- `src-tauri/src/vcp_modules/persistence/db_write_queue.rs:315-528`
  - agent/group/topic upsert 没有保护已删除行；message conflict 明确设置 `deleted_at = NULL`。
- `src-tauri/src/vcp_modules/sync/sync_dto.rs:277-306`
  - message pull DTO 没有 deletion/version/restore 语义，无法证明远端值比本地 tombstone 更新。
- `src-tauri/src/vcp_modules/persistence/message_repository.rs:526-562`
  - 通用 upsert 同样拥有 undelete 语义。

删除后，旧设备或迟到远端快照可以通过普通 upsert 把本地 tombstone 清掉。

#### topic delete 与 begin skeleton 的 TOCTOU

- `src-tauri/src/vcp_modules/chat/message_service.rs:793-893`
  - owner/topic live 检查发生在 begin transaction 之前。
- `src-tauri/src/vcp_modules/topic/topic_service.rs:213-249`
  - delete 可在该窗口 tombstone topic/messages 并清 active。
- 之后 begin 仍可创建 live message 与 active generation。
- `message_service.rs:897-938,1007-1042` 的 append/patch owner 参数未参与校验，底层通用 upsert又可清 tombstone。

**最小架构**：把 tombstone 规则集中到 repository/queue；普通 create/update/sync upsert永不清 tombstone。只有独立、显式、带新版本且经过认证的 restore 操作能复活。begin 使用同一事务的条件 `INSERT ... SELECT ... WHERE owner/topic deleted_at IS NULL` 并检查 affected rows；append/patch 同事务验证 owner/topic/message live。

**测试**：本地 delete 后 stale pull；delete_topic 与 begin barrier 交错；迟到 append/patch；owner mismatch；显式合法 restore 与普通 upsert 分离。

---

## 6. High：发布链与能力放大器

### POST-REL-01｜High｜Release/Gradle 制品身份未闭环

#### GitHub Actions

- `.github/workflows/release.yml:28,46,52,57,63,68,75,179` 使用可移动 tag；包括拥有 `contents: write` 的第三方 release action。
- job 级环境暴露签名密码，权限与秘密生命周期过宽。
- workflow 在 GitHub Release 已 `published` 后才构建，没有依赖同 commit 的 CI 成功，也没有重跑 check/test/audit。
- tag 只校验格式，没有核对 `package.json`、`Cargo.toml`、`tauri.conf.json`、Android versionName/versionCode。
- 证书比较仅在 keystore fingerprint 解析非空时执行；解析失败会弱化身份校验。
- APK/frontend ZIP 没有 checksum、provenance 或 SBOM。

#### Gradle

- `src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties` 使用第三方 Gradle 分发镜像，未配置 `distributionSha256Sum`。
- `src-tauri/gen/android/build.gradle.kts` 含 JitPack/镜像 repository，没有 Gradle dependency verification metadata 或 dependency lock。
- 这些依赖在持有 Android 签名秘密的 release job 中执行。

**最小修复**：所有 Actions 固定完整 commit SHA；签名 secrets 只进入必需步骤；release 必须依赖同 commit 门禁；版本源一致；两端证书指纹必须非空且精确匹配；Gradle 使用官方分发 URL + 官方 SHA-256并提交 verification metadata。不要在同一票升级 AGP/Kotlin/AndroidX。

### POST-CAP-01｜High（理论影响）｜主 WebView 能力面仍宽

- `src-tauri/tauri.conf.json:21-26`：`csp: null`，asset protocol scope 为 `["**"]`。
- `src-tauri/capabilities/default.json:5-27`：main 与 `vcp-portal-*` 仍共用较宽 capability，opener/window/protocol scope 较大；assistant window label 已按 0A 回执移除。
- `src-tauri/plugins/vcp-mobile/permissions/all.toml`：插件 all package 包含 Root、文件、截图/选择、保活等能力。

在用户明确的小圈子、富 HTML 保真优先模型下，这可以暂时作为 accepted exception，不单独阻断本轮。但它会显著放大任何 OTA、DOM 或路径缺陷，因此：

- 不得把它描述为技术风险已关闭。
- POST-OTA-01 与 POST-WEB-01 已按 0A 回执收口；POST-FILE-01 仍需独立修复。
- 后续可按窗口/功能拆最小 capability，不必重构应用状态机。

---

## 7. Medium 与治理待办

### POST-APK-01｜APK 下载器的 URL 与容量边界

`src-tauri/src/vcp_modules/updater/update_manager.rs:136-199` 接受任意 URL，只核对可选 Content-Length，没有 scheme/host/redirect 和最大实际字节数。Android installer 的签名校验与用户确认降低了代码执行风险，但仍允许本机/LAN 请求、慢响应和磁盘耗尽。

复用 frontend downloader 的可信 URL 与流式 byte budget；APK 最终安装继续交给 Android 系统。

### POST-SET-01｜Settings cache、损坏恢复与秘密日志

- `settings_manager.rs:92-197`：cache miss 的 DB await 与 writer 不共享 generation/commit gate；迟到读 A 可在写 B 后覆盖 cache，之后 partial update 可能把旧字段写回 DB。
- 已存在 JSON 解析失败时静默返回默认设置，随后保存会覆盖原损坏数据，失去恢复证据。
- `src/core/stores/floatingAssistant.ts:267` 输出完整 settings 对象；Settings 中包含 API key、sync token、管理员凭据等。
- VCP log/info 的部分 disconnect 日志输出原始 URL，而 connect 日志已有 mask，策略不一致。

修复应复用 agent/group 已采用的 generation + insert-if-current；只有“不存在行”允许 default，损坏 JSON 返回 typed recovery 并保存原文。前端只读取最小 DTO，日志统一 redaction。

### POST-RES-01｜无界资源与任务扇出

建议按现有 executor/semaphore 模式补预算，不建通用任务框架：

- `file_manager.rs:389-449`：IPC 可接收最高约 100 MiB `Vec<u8>`，同步 hash/write 位于 async 路径；并发调用放大内存与阻塞。
- `sync/sync_executor/pull_executor.rs:576-891`：NDJSON 缺单行、总响应和实体数限制；部分路径先 spawn 再获取 semaphore。
- `distributed/client.rs:627-733`：远端 tool request 缺 in-flight、参数大小和重复 request ID 门禁。
- `model_manager.rs:25-43,122-149,395-402,480-486`：batch task 只有 `Option<JoinHandle>`，旧 cleanup 可清新 handle，abort 不 await，旧 refresh 可覆盖新设置。

### POST-TXN-01｜跨事务与忽略错误的剩余一致性债

- 文件移动与 DB metadata/thumbnail 注册并非单一提交，部分 DB 错误被忽略。
- `sync_executor/delete_executor.rs` 的 topic/messages/active/hash 分段事务，hash 错误可被忽略。
- legacy migration bridge 未用单事务；中途崩溃可能留下半 seed 的 migration 表。
- agent/group/topic/message 的业务数据与 owner hash/bubble 使用第二事务；前者成功、后者失败时调用会返回错误但数据已生效。
- Sync entity notification 后台执行器存在忽略结果/读取 settings 失败回退默认的路径。

优先定义“主数据提交”和“派生缓存/hash 更新”的成功语义；派生项可重建时应进入明确 dirty/rebuild 状态，不要把半成功伪装成全部失败或全部成功。

### POST-ANDROID-02｜发布、明文网络、备份与权限 UX

- `src-tauri/gen/android/app/build.gradle.kts:73-78`：缺 release secrets 时可能回退 Debug signing；正式 release 应直接失败。
- `AndroidManifest.xml:25` 硬编码 `usesCleartextTraffic="true"`，使 Gradle placeholder 不能真正控制 release/debug 差异。
- manifest 未明确 `allowBackup=false` 或 data-extraction rules；SQLite 中存在凭据时需要明确备份策略。
- `src/core/stores/appLifecycle.ts:329-339` 与 `PermissionGate.vue` 将通知、全媒体读取、忽略电池优化、notification-listener 等能力集中为核心启动门槛；SAF picker 本身不需要全媒体权限，而 notification listener 当前 callback 是隐私保护占位实现。前端手动勾选也绕不过 bootstrap 后端实检。

LAN HTTP/WS 是产品能力，可记录为受信 LAN 模式，不要求一刀切 TLS；但应由 network security config/显式设置限定，而不是全局无条件打开。非核心权限改为 feature-triggered 请求。

### POST-CI-01｜门禁与文档漂移

- `pnpm test:integration` 指向不存在的 `src/tests/integration`，本轮实跑失败。
- CI Cargo 命令没有统一 `--locked`，也没有 npm/Rust audit 门禁。
- `tauri android init --ci` 后没有 `git diff --exit-code` 检测生成树漂移。
- Vitest 没有 coverage threshold；benchmark 只编译，E2E/soak 是采集脚本而非自动判定。
- `AGENTS.md`、`package.json` 与部分 docs 仍引用不存在的 `scripts/`、`plans/`、`build_android_release.ps1`、integration 目录或过时测试数量。
- `docs/DEPENDENCY_MANAGEMENT.md` 对精确版本策略自相矛盾，并列出部分不存在/旧版本依赖。
- 原审计报告中“无剩余 blocker/可作为候选发布基线”的结论已被本轮 OTA 发现推翻，应以本报告为当前判断。

最小修复是删除或落实失效脚本、Cargo 全加 `--locked`、CI 增加两类 audit 与生成树 diff；性能阈值等固定设备数据后再设，避免制造伪门禁。

### POST-LOCAL-01｜休眠的 localhost server

`local_server.rs:38-225` 的 localhost WebSocket/HTTP 能访问 chat/archive/settings，缺少独立认证。当前 `lifecycle_manager.rs:121-127,431-445` 强制关闭，故不是当前 release blocker。重新启用前必须增加随机 session token、bind-ready handshake 与最小 settings DTO。

---

## 8. 前序修复的再确认结果

本轮没有发现下列既有修复发生语义回退。它们应继续保留，不要因新发现推倒重做：

| 领域 | 复核结论 |
| --- | --- |
| Chat begin/finalize | finalizer 在事务中复核 active generation 与 tombstone；迟到 final 不应覆盖删除终态 |
| ActiveRequests | attempt ownership 与 token-matched cleanup 关闭旧 lease 删除新 owner 的 ABA |
| 删除/截断取消 | 先提交 tombstone，再取消活跃请求；本地 UI 迟到回调受 epoch/generation 门禁 |
| 历史分页/滚动 | batch Promise 明确结算；keyset cursor 与 500 窗口/返回最新路径存在 |
| 附件下载 | 具备 50 MiB、连接/总时长/stall timeout、hash/size 与 temp+rename |
| Render cache | content hash + schema 命中；普通与 rebuild 写均有 CAS；编译进入有界 blocking |
| Sync session owner | 唯一 SessionHandle、generation、cancel/join、attempt tracker 与 phase watchdog 已建立 |
| Distributed lifecycle | connection generation、single writer、child tracker 与 stop quiescence 基本成立 |
| Android helper | socket binding identity、conditional detach、session/global 预算、stop generation ACK 已建立 |
| Android executor | OOM guard、root、file 三执行域隔离；Rust PluginHandle 锁内 clone 后锁外等待 |
| Lifecycle/FGS | transition epoch + 单 mutex；linger 与 foreground 同 owner；Guardian generation 回滚与 screen owner OR 已覆盖 |
| DB recovery | CORRUPT/NOTADB 分类、orphan sidecar preflight、DB/WAL/SHM 归档与可查询恢复状态存在 |
| Agent/Group cache | generation + 短提交锁避免旧 read 在 sync clear 后回填 ghost/stale cache |
| High-speed upload | token、精确长度、endpoint lifetime 与错误传播已补齐 |

需要注意：Sync 的 session ownership 已闭环，不等于 POST-SYNC-01/02 的 finalizer/最终 ACK 语义也正确；Chat 的 normal finalize 已闭环，也不等于所有 generic/sync upsert 都尊重 tombstone。这正是后审计区分“同域不同提交边界”的原因。

---

## 9. 显式接受的剩余风险

以下内容可以继续存在，但必须具名记录：

1. **富 HTML 产品策略**：主消息使用保真优先的 active-capability filter，而不是完整通用 XSS sanitizer；恶意混淆脚本的剩余风险由产品负责人接受。新 renderer 必须至少复用同一 filter。
2. **CSP/capability/asset scope**：当前宽权限是产品与历史架构选择，作为纵深防御债保留；不能拿“小圈子”替代对 OTA、路径和破坏性操作的硬边界。
3. **明文 LAN**：VCP 局域网 HTTP/WS 兼容性优先；应明确标识受信网络模式并限制范围。
4. **RustSec RSA**：`rsa 0.9.10 / RUSTSEC-2023-0071` 只由未启用的 `sqlx-mysql` lock 路径命中，Android feature tree 不可达，且无已修版本；继续具名 ignore，启用 MySQL/all-databases 或升级 SQLx 时复核。
5. **Rust 维护性 warnings**：原始 cargo audit 仍有 21 warnings；它们不是本轮可利用漏洞清零，不应写成“0 findings”。
6. **SQLite 恢复**：DB/WAL/SHM 三次 rename 不具备掉电级原子性；现有策略是 fail-closed/manual recovery。
7. **有限历史窗口与 LWW**：500 条活动窗口、部分实体 last-write-wins 等是已知产品权衡。
8. **设备/OEM 行为**：Android 14 FGS、OEM 后台、rotation、进程死亡、真实 ContentProvider 与长会话仍需外部验收。
9. **在线物理 GC 暂停**：为避免 snapshot 与 unlink 之间的新引用穿越，本轮只做 fail-closed 统计/逻辑处理，不在线删除孤儿附件；存储可能缓慢增长，后续应以统一 mutation gate + quarantine/grace ticket 独立实现。
10. **ContentProvider 取消合作性**：平台 CancellationSignal 和超时已接入，但恶意 Provider 可以忽略取消；影响被限制在单个有界文件 worker，不会再无界占用主线程或线程池。
11. **Sync 1.1 peer 门禁**：移动端不会向旧 1.0.0 协议降级；在 VCPChat peer 未同步升级前，同步会明确版本不匹配而不是假完成。

---

## 10. 本轮实际验证

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| `pnpm check` | PASS | Vue typecheck + host Cargo check |
| `pnpm test:run` | PASS | 20 files / 77 tests |
| `pnpm build` | PASS | 前端生产构建成功 |
| `cargo test --locked --workspace --lib` | PASS | 插件 3/3 + 主库 135/135，共 138 tests |
| `cargo test --locked --test file_extractor_integration` | PASS | 10/10 |
| `cargo fmt --all -- --check` | PASS | Rust 全 workspace 格式门禁 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | PASS | workspace 全 target lint 门禁 |
| `cargo bench --locked --profile perf --no-run` | PASS | lib/bin/main/Criterion benchmark executable 编译成功；未运行性能阈值 |
| Android strict `testDebugUnitTest --rerun-tasks` | PASS | 32 tests / 0 failure / 0 error；dependency verification strict |
| Android `cargo check --target aarch64-linux-android` | PASS | NDK 29.0.13846066 / API 26 clang |
| Tauri Android generated drift | PASS | `android init --ci` 前后 tracked/untracked 内容指纹一致 |
| arm64 Debug APK | PASS | 36,506,390 bytes；含 `lib/arm64-v8a/libvcp_mobile_lib.so`；SHA-256 `0318d5907e3e84a965c64f2f821be1ae6b9cd1f054711089d4bce182d5503181` |
| production asset boundary | PASS | `dist/` 仅 `index.html` 入口，无 `floating.html` 或 frontend OTA loader |
| `pnpm audit --prod` | PASS | No known vulnerabilities |
| `pnpm audit` | PASS | No known vulnerabilities |
| raw `cargo audit` | PASS WITH EXCEPTION | 1 vulnerability（不可达 RSA）+ 21 warnings |
| `pnpm audit:rust` | PASS | 具名忽略 `RUSTSEC-2023-0071` 后 exit 0 |
| Android RSA feature tree | PASS | `cargo tree ... -i rsa@0.9.10` 无 Android 路径 |
| `adb devices -l` | BLOCKED | 无连接设备，未做真机 E2E/性能/升级安装 |
| `git diff --check` | PASS | 业务工作树无格式残留 |

自动测试已新增 Maintenance query failure、错误 final ACK/replay、配置截断、tombstone、资源预算、staging owner、Settings stale patch、Release/Android 治理等负向路径。它们仍不能替代正式签名环境、真实 ContentProvider、OEM 后台和跨端 1.1.0 peer 联调。

---

## 11. 分阶段收口清单

### P0｜立即阻断风险，不扩架构

1. **已完成**：移除 frontend OTA 自动 apply 与整个运行时，停止发布 frontend ZIP。
2. **已完成**：RAG/Assistant renderer 复用现有 `filterTrustedRichHtml` 与 URL gate，Assistant 生产入口休眠。
3. **已完成**：Maintenance 查询失败零删除；在线物理 GC 安全降级为不 unlink，并补故障测试。
4. **已完成**：SyncFinalizer/queue/pipeline 全错误传播；最终 ACK 绑定 attempt/session/phase/nonce。
5. **已完成**：Distributed 配置损坏全禁用，原子持久化后切内存，并加入 frame/request 预算。
6. **已完成**：文件 IPC 统一 staging owner、canonical containment、自算 hash、原子 CAS；分享/Picker 使用有界 executor 与预算。
7. **已完成**：repository/DbWriteQueue tombstone 单调，begin/append/patch/topic delete 事务校验 live owner。

### P1｜正式发布身份与稳定性

1. **已完成代码门禁**：Actions SHA、默认分支 ancestry/同 SHA push CI、版本/证书/checksum 校验；待正式 secrets 实跑。
2. **已完成**：Gradle 官方 distribution SHA + dependency verification metadata。
3. **已完成**：APK downloader 可信 URL、redirect、byte budget 与 immutable install owner。
4. **已完成**：Settings generation/patch、typed corrupt recovery、秘密日志 redaction与备份关闭。
5. **已完成**：ACTION_SEND、NDJSON、tool request、model task 的有限预算与 owner ID。
6. **已完成**：release 缺签名直接失败；cleartext 仅显式 trusted-LAN release mode 开启。

### P2｜治理与后续纵深

1. **已完成**：移除失效脚本，Cargo CI `--locked --workspace`，加入 audit、strict Gradle 与 Android generated drift。
2. **已完成**：Dependency Management、OTA、Sync、插件、前端和测试文档按当前代码更新。
3. **明确接受**：本轮不拆宽 capability，不与业务状态机重构捆绑；Frontend OTA 长期移除。
4. **已完成核心项**：业务/hash/终态关键写入事务化；可重建派生内容保留明确 best-effort。
5. **后续独立维护**：物理 GC 使用 mutation gate + quarantine/grace；不在本轮恢复危险 unlink。

---

## 12. Magi 讨论与统一架构判断

### Melchior：错误必须成为协议，不是日志

Maintenance、SyncFinalizer、tool config 和 tombstone 的共同问题不是缺少复杂状态机，而是失败被转换为空集合、默认值、best-effort 或普通 upsert。系统必须在 destructive action、terminal success 和 undelete 三个点 fail-closed。

### Balthasar：保真与安全边界并不冲突

无需禁用富 HTML，也无需让 RAG 退化为纯文本。正确做法是让所有 renderer 复用同一个主动能力门禁。Android 分享和文件选择也应保留原生体验，但路径不能作为跨层身份；一次性 staging ticket 比继续传裸路径更符合移动端直觉。

### Casper：先停止危险交付路径，再建设长期能力

当前最快、最可靠的发布策略是关闭 frontend OTA，而不是匆忙发明一套密码学协议。Release/Gradle 只需固定现有依赖和执行身份，不应顺带升级 Tauri、AGP、Kotlin、SQLx 或 AndroidX。

### 统一结论

本轮需要强化四个既有边界，不需要 mega-state-machine：

```text
更新 artifact
  = 可信发布身份 + 冻结 asset + 路径 containment + 资源预算 + 原子激活

文件 ingest
  = native staging owner + 一次性 ticket + 自算 hash + 单根 containment

破坏性操作
  = 完整成功 snapshot + fail closed + 可恢复/可重试删除

终态提交
  = 当前 owner/attempt + 同事务不变量 + commit 成功 + 精确 ACK
```

这四条可直接复用项目已经验证过的 generation、owner、bounded executor、CAS、temp+rename 模式，既不会过度设计，也不会因为“保持简单”继续留下隐患。

---

## 13. 正式发布前的外部验收

代码修复和自动门禁通过后，仍需完成：

- 使用正式 keystore 构建，确认与上一正式 APK 的 signing certificate fingerprint 精确一致。
- 在已安装旧正式版的设备上完成升级安装；验证数据、DB/WAL、附件、设置与前端资源不丢失。
- API 26 与 API 36 各至少一台；Android 14 FGS 拒绝/恢复；OEM 后台 5/15/30 分钟；Activity pause/rotation；主/helper 进程分别死亡。
- 外部单/多文件分享：大量 URI、超大文件、慢 Provider、Provider 取消、磁盘接近满、重复 intent。
- Sync/Distributed：丢包、半开连接、旧 ACK、重复 ACK、DB busy/ENOSPC、配置损坏与重启。
- VCPChat `VCPMobileSync` 插件升级至 1.1.0，`PHASE_ACK` 原样回显 `phase/sessionId/attemptId/nonce`；确认移动端拒绝旧 1.0.0 且不会降级。
- 500/1000/3000 条复杂消息；RAG/HTML/Mermaid/MathML/SVG 保真；前后台恢复与长稳 soak。
- 正式 release workflow 产物的证书、checksum/provenance、版本一致性和升级旅程。

---

## 14. 最终判定

`0ff6f92` 是本报告的原始后审计基线；其后 P0-P2 收口已经把文件输入、破坏性维护、Sync/DB 终态、Distributed policy、APK/Settings、资源预算和 Release/Gradle 身份纳入现有 owner/generation/CAS/事务边界。Frontend OTA 被完整移除，Assistant 保留源码但生产运行时关闭，没有为追求“架构完善”再引入第二套状态机。

当前判定为：

> **CODE-LEVEL POST-AUDIT BLOCKERS CLOSED — READY FOR EXTERNAL RELEASE ACCEPTANCE.**

这不等于“已完成正式发布”。在 VCPChat peer 1.1.0 联调、正式 keystore/受保护分支 Release workflow 实跑以及 API 26/36 + Android 14/OEM 真机验收完成前，不应给公开制品打最终发布标签。宽 capability、受信 LAN cleartext、关闭备份迁移、RustSec feature-inactive RSA、维护性 warnings、暂停在线物理 GC 与合作性 Provider 取消均是具名残余，不能在后续文档中写成风险清零。
