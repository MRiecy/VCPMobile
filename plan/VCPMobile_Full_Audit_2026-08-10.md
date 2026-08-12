# VCP Mobile 全面审计与修复交接报告

> 审计日期：2026-08-10（Asia/Shanghai）  
> 审计基线：`main` / `a4e071e`（本地相对 `origin/main` 领先 3 个提交）  
> 修复收口：`0ff6f92`（2026-08-11，本地提交，未推送）  
> 审计方式：Magi 三路并行只读审计 + 主审交叉复核 + 静态检查/单测/构建验证  
> 审计范围：Vue/Pinia 消息链路、Rust/Tauri 后端、SQLite 持久化、Sync V2、Android 原生插件、`:helper` SSE 代理、生命周期/保活、distributed、依赖与测试治理  
> 变更声明：原始审计阶段未修改业务代码；2026-08-10 至 2026-08-11 已完成对应修复、对抗复核与验证回填。

## 0. 给接手 Agent 的使用说明

1. 本文中的行号对应上述审计基线。代码发生变化后，请优先按文件、函数名和关键字段重新定位，不要只依赖行号。
2. “确认”表示源码时序足以证明问题；“高可信风险”表示源码已具备危险条件，但最终调度窗口、OEM 行为或性能阈值仍需真机复现。
3. 不建议先拆分 God File。应先用外科手术式修改建立状态不变量和回归测试。若确需拆分超过 500 行的文件或移动模块，必须先按 `AGENTS.md` 建立 Git 存档提交。
4. 跨层或核心逻辑修改后必须运行 `pnpm check`；涉及对应领域时继续运行本文末尾列出的定向测试。
5. 修复时不要为 chat、sync、lifecycle 各造一套复杂框架。它们需要的是同一个轻量模式：单调 `generation/epoch`、唯一 owner、可取消任务、提交前代次复核。

---

## 1. 执行摘要

### 1.1 发布判断

**最终软件复核（2026-08-11）**：P0 与后续 P1/P2 清单已在代码边界内闭环；三路 Magi 对抗复核未发现剩余代码 blocker。前端、Rust host、Android target、Kotlin JVM、构建与供应链门禁均通过。当前可以进入提交与候选发布阶段，但本地没有 Android 设备，Android 14 FGS/OEM 后台、Activity 旋转、API 26/36 真机旅程与长会话性能仍是 release 前设备验收，不得用 JVM/host 结果替代。

- ~~主 WebView 原始 HTML 缺少安全过滤~~：已增加可信圈 active-capability filter；严格对抗场景的剩余风险由产品责任人接受。
- ~~thinking 骨架与 terminal 无序覆盖~~：后端 `begin/finalize` 事务化，消息生命周期只能单向提交。
- ~~历史 Channel Promise 悬挂与 `loading-top` 死锁~~：改为小批量 request/response，滚动由显式 Promise 完成协议收敛。
- ~~同消息正常/恢复请求 ABA~~：attempt lease 拒绝重复 owner，清理仅能释放自己的代次。
- ~~Sync `stop -> start` 叠加会话~~：唯一 SessionHandle + generation + cancel/join 建立互斥所有权；reconnect attempt、派生任务、阶段命令、watchdog 与最终 ACK 均绑定当前 owner。
- ~~DB 误判损坏并丢弃 WAL~~：只有 CORRUPT/NOTADB 可启动恢复，DB/WAL/SHM 整体归档，孤立旁车在 SQLite open 前 fail-closed，恢复路径持久到系统快照。
- ~~Android helper socket ABA 与 root executor 饥饿~~：连接引用同一性清理，OOM/root/file 三执行域隔离；helper 增加 session/global 预算、有界 writer、generation stop ACK 与按需生命周期。
- ~~Distributed stop 后 Connected 复活~~：生命周期互斥锁 + generation 提交门禁关闭状态复活；session child tracker、单 writer 与 deadline 确保 stop 返回时旧 I/O 已静默。
- ~~历史 OFFSET 漂移与无界 DOM 风险~~：改为 `(timestamp,msg_id)` keyset；内存/DOM 窗口上限 500，若顶部翻页淘汰新端，置底/发送/输入聚焦会显式重载最新页。
- ~~缓存与同步落盘假成功~~：render cache 以 content hash + schema 校验并做 CAS；DbWriteQueue 把 batch 错误传播给 Flush，Finalize 与桌面最终 ACK 成功后才发布完成。

### 1.2 四个系统性根因

1. **异步结果没有所有权**：请求只按 `msg_id/topic_id` 标识，没有 request generation；旧任务和新任务会互相清理。
2. **终态不是单调状态**：pending skeleton、terminal message、deleted tombstone 可以通过无条件 upsert 相互覆盖。
3. **会话上下文未冻结**：`await` 前后重新读取全局 selected topic/owner，迟到回调用旧数组下标修改新话题。
4. **失败和背压没有成为协议的一部分**：Channel close、DB batch error、socket timeout、FGS readiness、stop ACK 均被忽略或吞掉。

### 1.3 与“卡消息列表”的直接因果图

```text
切换话题/分页异常
  -> 丢弃 is_last chunk
  -> completePromise 永不 resolve
  -> finally 不执行
  -> history loading 与滚动状态不能收敛

分页返回 0 条/报错
  -> scrollHeight 不变
  -> handleContentChange 提前 return
  -> scene 永久停在 loading-top
  -> onScroll 不再允许触发下一页

thinking 事件
  -> 前端异步写空 skeleton
  -> 后端先完成 finalizer
  -> 迟到 skeleton 无条件 upsert
  -> 正文变空 + finish_reason=NULL + active_generation 复活

resume/online/history-load 同时恢复
  -> 同 msg_id 多个 recovery task
  -> 旧 guard/remove/finally 清理新任务
  -> 重连闪停、内容回退、永久 reconnecting
```

---

## 2. 严重度与修复优先级

| ID | 级别 | 结论 | 主要症状 |
| --- | --- | --- | --- |
| SEC-01 | Critical（理论影响）/ Product Accepted | 已按可信圈富 HTML 策略做主动能力门禁，剩余风险接受 | 直接脚本/危险协议/显式宿主能力入口已受控，不阻断发布 |
| SEC-02 | High / Resolved | 前端生产依赖告警已清零 | Mermaid/DOMPurify 与 PostCSS/Nano ID 已更新至安全版本 |
| CHAT-01 | Critical / P0 / Resolved | 后端 begin/finalize 单向事务 | 迟到 skeleton 不再覆盖 terminal |
| CHAT-02 | Critical / P0 / Resolved | attempt lease + token-matched cleanup | 旧任务不能删除新 owner |
| HIST-01 | High / P0 / Resolved | 小批量 invoke Promise 显式结算 | 丢失 Channel 尾帧不再悬挂 |
| HIST-02 | High / P0 / Resolved | Promise page result + generation `finally` | 零条/异常均退出 `loading-top` |
| CHAT-03 | Critical / P0 / Resolved | `ConversationKey + epoch + loadId` | 迟到结果不得提交到新会话 |
| SYNC-01 | Critical / P0 / Resolved | 唯一 SessionHandle，cancel + join | 旧 session 无法清理新 owner |
| DB-01 | Critical / P0 / Resolved with residual | 损坏分类 + 整体归档 + 可查恢复状态 | 瞬时错误/孤立 WAL 均 fail-closed；断电跨三文件 rename 为已记录残余 |
| AND-01 | Critical / P0 / Resolved | connection identity + conditional detach | 旧 socket `finally` 不能关闭新 socket |
| AND-02 | Critical / P0（root）/ Resolved | OOM/root/file 三执行域 + 锁外 IPC | root guard 不再饥饿文件/原生命令 |
| DIST-01 | Critical / P0 / Resolved | lifecycle mutex + generation commit gate | 迟到 ACK 不能复活 Connected |
| CHAT-04–14 | High/Medium / Resolved | 事务错误传播、附件 ready、tombstone cancel、恢复身份、下载预算、timer singleflight | 终态、恢复与多媒体异步提交均受 owner/epoch 约束 |
| CACHE-01/02 | High/Medium / Resolved | hash/schema 命中 + 普通/重建 CAS + 有界编译 | 损坏/旧缓存降级为 miss，迟到编译不能覆盖新正文 |
| HIST-03/04 | High / Resolved | loadId 门闩 + keyset cursor + 500 窗口/返回最新 | 无 OFFSET 漂移、旧 finally 不释放新 owner |
| DB-02–05 | High/Medium / Resolved | Flush Result、短事务、spawn_blocking、实体锁/缓存代次、上传协议边界 | 写失败不伪装成功，删除/同步不产生 ghost cache |
| SYNC-02/03 | High / Resolved | view/attempt 门禁、cancel/join、严格 Phase3、60s/30s watchdog | 不产生隐形同步，失败/缺失 ACK 不再永久 connected |
| AND-03–11 | High/Medium / Resolved at software scope | helper 预算/ACK、FGS generation/readiness、按需服务、Insets 合帧、移除静音音频 | 旧 owner/无界资源与原生生命周期竞态已关闭 |
| LIFE-01 | High / Resolved | transition epoch + 单一 mutex + linger 同 owner | 迟到后台任务不能断开前台连接 |
| DIST-02–05 | High/Medium / Resolved | 正确网络事件、lifecycle 独占 lease、child tracker、单 writer、listener owner | stop 后无旧 I/O；旧事件/快照不能覆盖新 session |
| TOPIC-01–03 | High / Resolved | `ownerType:ownerId + generation + singleflight` | 同 owner 重入、同步失效、A→B→A 均 latest-wins |
| PERF-01/03 | High/Medium / Resolved | hidden snapshot compaction + visibility recovery | 后台帧不再无界积累，恢复事件可达 |
| PERF-02 | High risk / Mitigated | 500 消息有界窗口 + 显式返回最新 | 非完整虚拟列表；长稳与滚动手感保留真机验收 |
| UI-01 | Medium / Resolved | 入口样式共用、去 blur、语义层级、manual chunks | UI 宪法与构建资源重新对齐 |

---

## 3. 安全审计

### SEC-01｜Critical｜普通消息 raw HTML 注入主 Tauri WebView

**原审计确认度：技术路径确定。**

**处置状态（2026-08-10）：受控缓解（Product-profile hardening）；发布不阻断；剩余风险由产品责任人接受。**

#### 产品边界与桌面端基线复核

- VCP Mobile 是 VCPChat 的封闭圈层移植版本，富 HTML 是核心产品能力；普通消息不得退化为纯文本，也不采用会明显损伤 CSS、SVG/MathML、媒体、自定义标签或局部交互的严格标签 allowlist。
- 只读复核 `/home/dudu/VCPChat` 后确认：桌面端普通 assistant 消息明确使用 `marked({ sanitize: false }) -> innerHTML`，保留全部 `on*`，并由 `modules/renderer/animation.js` 主动重新执行消息中的 inline/external `<script>`。桌面端只对 user 消息整体转义、对 Tool Result 的 raw HTML 做封口；其主 CSP 仍允许 `unsafe-inline` 和 `connect-src *`。
- 因此，“严格照搬 VCPChat 普通 assistant 行为”实际等于接受本风险，甚至会比 Mobile 原有的惰性 `<script>` 行为更开放。本次没有移植桌面脚本执行器，而是在不改变富 HTML 信任模型的前提下增加一层 Mobile 主动能力门禁。

#### 原始证据链（处置前）

- `src-tauri/src/vcp_modules/chat/pre_renderer/markdown_parser.rs:928-931`
  - `Event::Html` 和 `InlineHtml` 被保存为 `raw_html`，未转义。
- `src/core/utils/astRenderer.ts:108,166`
  - raw HTML 节点直接返回 `node.content`。
- `src/core/utils/astExecutor.ts:332`
  - AST 流式路径也会把结果赋给 `innerHTML`。
- `src/features/chat/MessageRenderer.vue:964`
  - 普通消息正文最终通过 `v-html` 注入主 WebView。
- `src/features/chat/blocks/ToolBlock.vue:20,219,286`
  - 工具结果经 `marked.parse` 后同样进入 `v-html`，没有 DOMPurify。
- `src-tauri/tauri.conf.json:21-25`
  - `csp: null`，asset protocol scope 为 `["**"]`。
- `src-tauri/src/vcp_modules/infra/settings_manager.rs:17,135`
  - `read_settings` 返回完整 Settings，其中包含 `vcp_api_key`、`vcp_log_key`、`sync_token`、管理员用户名/密码、`file_key` 等敏感字段。
- `src-tauri/capabilities/default.json:6-13`
  - 主 capability 引用了 `vcp-mobile:allow-all`。
- `src-tauri/plugins/vcp-mobile/permissions/all.toml:4-48`
  - 插件命令授权范围包括系统、文件、Root、保活等高权限操作。

`HtmlPreviewBlock.vue:226-233` 的任意 HTML 预览使用了 sandbox iframe，但它不能保护普通 Markdown 气泡和 ToolBlock。

#### 最短攻击路径

1. 远端模型、插件工具结果或同步数据中出现带事件属性/危险 URL 的 HTML。
2. Markdown parser 将其保留为 raw HTML。
3. 原实现通过 `v-html/innerHTML` 注入主 WebView；其中普通 `<script>` 通常因 innerHTML 语义保持惰性，但事件属性、危险 URL 和未 sandbox 的 `iframe/srcdoc` 仍可执行。
4. 原实现中的可执行载荷能在应用主页面上下文接触全局 Tauri invoke surface。
5. 现实现已在 stable AST、fallback、AST snapshot/diff 和 ToolBlock 的 DOM sink 前统一执行产品级门禁，阻断未混淆的直接宿主能力路径。

#### 本次采用的产品级门禁

- 保留 raw HTML、`style/class/id/data-*/aria-*`、SVG、MathML、canvas、音视频、表单、自定义元素，以及只操作当前元素的局部 DOM 交互。
- 移除 `script/base/object/embed/applet`、`meta refresh`、链接 `ping`。
- 拦截 `javascript:`、`vbscript:` 与可执行 HTML/XML 文档 `data:` URL；保留 http/https/blob/asset/vcp 与被动 `data:image` 等渲染路径。
- 事件处理器仅在直接引用 Tauri IPC、全局页面、网络、存储、导航或动态执行能力时移除；现有表情修复内部处理器与纯局部视觉交互保留。
- iframe 保留，但强制 sandbox，剔除 `allow-same-origin` 与顶层导航能力；`srcdoc` 递归应用同一门禁。
- 普通 Markdown 生成的 link/image 同样经过 URL gate，避免绕开 raw HTML 分支。
- 不恢复桌面 VCPChat 的主 DOM 脚本重执行器；需要完整 JavaScript 的 HTML 继续使用现有 `HtmlPreviewBlock` sandbox。

实现入口：

- `src/core/utils/astRenderer.ts`：`filterTrustedRichHtml` / `filterTrustedRichHtmlUrl`，覆盖稳定 AST 与非 AST fallback。
- `src/core/utils/astExecutor.ts`：在 raw HTML 进入 detached/live DOM 前复用门禁，覆盖 snapshot、add/replace、inline 与 Markdown URL。
- `src/features/chat/blocks/ToolBlock.vue`：`marked.parse` 输出进入 `v-html` 前复用门禁。

#### 明确接受的剩余风险

这不是面向恶意对抗输入的通用 XSS sanitizer。主 DOM 仍有意保留纯局部 `on*` JavaScript；攻击者可通过计算属性、字符串拼接、反射、DOM gadget 或间接点击现有 UI 绕过关键词能力门禁。CSS 污染、远程资源请求、UI spoofing 也没有被严格 allowlist 消除。

原审计建议的“raw HTML 全转义 / 所有 HTML 仅进 iframe / 严格 tag+attribute allowlist / CSP / capability 拆分 / Settings 秘密视图拆分”没有在本票采用；它们与本次产品约束冲突或属于独立的大范围加固战役。若未来威胁模型改为公开分发或处理敌对内容，必须重新打开本项并采用严格隔离方案。

#### 已补的回归测试

- 直接调用 Tauri 的 `<img onerror>`、网络外传 `<svg onload>`、危险链接、script/base/meta/object 均被门禁处理，Tauri `invoke` spy 保持 0 次调用。
- stable raw HTML、普通 Markdown URL、AST snapshot、AST replace/tail、ToolBlock 五类入口均覆盖。
- 复杂保真样本覆盖 style/grid、custom element、SVG gradient、MathML、canvas、form、HTTPS、data:image 与局部 class/hidden 交互。
- HtmlPreview 仍保持无 `allow-same-origin` 的 sandbox，并保留预览内部 script。

### SEC-02｜High｜前端生产依赖存在已披露安全告警

**处置状态（2026-08-10）：已解决（Resolved）；前端生产依赖发布不阻断。**

`pnpm audit --prod` 在审计日返回：3 high、15 moderate、5 low、0 critical。

直接依赖版本：

- `mermaid@11.14.0`
- `dompurify@3.4.2`

主要传递依赖：

- `postcss@8.5.12`
- `nanoid@3.3.11`

Mermaid 直接处理不可信的模型内容，因此其 HTML/CSS 注入和 DoS 类告警具有较高可达性；DOMPurify 当前版本也应升级。参考：

- <https://github.com/mermaid-js/mermaid/security>
- <https://github.com/mermaid-js/mermaid/security/advisories/GHSA-ghcm-xqfw-q4vr>
- <https://github.com/cure53/DOMPurify/security/advisories/GHSA-55q2-fjhq-7xh7>
- <https://github.com/advisories/GHSA-r28c-9q8g-f849>

#### 已实施升级

- `mermaid 11.14.0 -> 11.16.1`：覆盖 11.15.0/11.16.1 发布的 HTML/CSS 注入、prototype pollution 与 Gantt/XY/Radar DoS 修复。
- `dompurify 3.4.2 -> 3.4.13`：覆盖当前公开的 hook、clone guard、DOM clobbering 与 `IN_PLACE` 修复。
- `vue 3.5.33 -> 3.5.41`，使生产侧 `@vue/compiler-sfc` 解析到 `postcss 8.5.26 -> nanoid 3.3.18`；随后执行 lockfile 去重，移除旧的 `postcss 8.5.12` / `nanoid 3.3.11` 副本。
- 同步修复开发供应链：`vite 6.4.2 -> 6.4.3`、`eslint 10.2.1 -> 10.8.1`、`@typescript-eslint/* 8.59.1 -> 8.66.0`、`happy-dom 20.10.6 -> 20.11.2`，传递 `brace-expansion` 收敛到 2.1.4 / 5.0.9。

本票没有跨 major 升级 Vite 8、TypeScript 7、Pinia 4，也没有混入 AGP/Kotlin/AndroidX 迁移。Vite 8 已切换 Rolldown/Oxc 并抬高默认浏览器目标，对 Android 系统 WebView 需要单独兼容与真机战役。

#### 验证结果

- `pnpm audit --prod`：0 advisories。
- `pnpm audit`（含开发依赖）：0 advisories。
- `pnpm test:run`：最终收口为 17 files / 64 tests，全部通过。
- `pnpm build`：通过；缺失图标、静态/动态重复导入与主入口 chunk 告警均已消除。Mermaid/Cynefin 大块保持按需加载。
- `pnpm check`：通过。

DOMPurify 3.4.13 的 `WHOLE_DOCUMENT` 在 happy-dom 中会触发其非浏览器标准的 Document 节点限制；对应单测只对 DOMPurify 使用透传桩，继续验证真实组件的 iframe `sandbox`/脚本隔离边界。生产配置没有为测试降级，Android WebView 仍使用真实 DOMPurify。

注意：升级 DOMPurify 不等同于 SEC-01 的产品级处置；普通 Markdown 主 DOM 仍使用 SEC-01 所述 active-capability filter。

#### Rust/Tauri 供应链跟进（2026-08-10）

Rust/Tauri 独立票已完成兼容补丁整理：

- Tauri 版本族同步为 Rust `tauri 2.11.5`、`tauri-build/tauri-plugin 2.6.3`、`tauri-utils 2.9.3`、runtime `2.11.3`、runtime-wry `2.11.4`；前端 API/CLI 分别为 `2.11.1` / `2.11.4`。
- `plist 1.10.0` 与 `pdf_oxide 0.3.77` 将两支 `quick-xml` 统一到 `0.41.0`；`crossbeam-epoch 0.9.20`、`anyhow 1.0.104`、`event-listener 5.4.2`、`memmap2 0.9.11`、`lru 0.16.4` 越过对应修复下限。
- `rkyv 0.7.46` 陈旧链已从 lockfile 移除；不可达的 `quinn-proto` 更新到 `0.11.16`。

原始 `cargo audit` 从 `8 vulnerabilities / 25 warnings` 收敛为 `1 vulnerability / 21 warnings`。剩余 `RUSTSEC-2023-0071` 来自未启用的 `sqlx-mysql -> rsa 0.9.10` 可选链；项目仅启用 SQLite，Linux/default 与 `aarch64-linux-android` 编译图均不包含 RSA，且 RustSec 尚无已修版本。它被作为具名 accepted exception，`cargo audit --ignore RUSTSEC-2023-0071` 门禁通过；不得将结果表述为原始扫描零发现。

21 条 informational warning 仍作为开放上游维护债：19 unmaintained、Linux GTK3 链 1 条 `glib` unsound、SQLx SQLite 链 1 条 yanked `spin`。

升级后已完成 host 与 Android 双链路验证：Rust 单测 94/94、文件提取集成测试 10/10、clippy/fmt/benchmark 编译均通过；`aarch64-linux-android` 在 NDK 29/API 26 工具链下交叉检查通过；Tauri Android init 无 tracked 生成漂移；Android 插件 JVM 测试 25/25 通过；`aarch64` universal APK 构建完成并通过 v2 签名结构校验。本地无 release keystore，产物使用 Android Debug 证书，只证明构建与 APK 完整性，不替代发布证书验收或真机 E2E。

---

## 4. Chat、历史列表与流恢复

### CHAT-01｜Critical｜thinking skeleton 与 finalizer 无序覆盖

**处置状态（2026-08-10）：Resolved。** 后端在 thinking 前事务创建 pending message + active row，terminal 事务仅更新已活跃的 pending 并删除 active row；前端不再落盘 skeleton。

**确认度：极高。Agent 和 Group 两条路径均成立。**

#### 证据

- `src/core/stores/chatStreamStore.ts:330-352`
  - thinking 事件触发 fire-and-forget `append_single_message`，写入空 skeleton，调用方不等待。
- Agent 路径：
  - `src-tauri/src/vcp_modules/agent/agent_chat_application_service.rs:141-151,163-186`
- Group 路径：
  - `src-tauri/src/vcp_modules/group/group_chat_application_service.rs:242-254,266-288`
- `src-tauri/src/vcp_modules/chat/message_service.rs:648-702`
  - skeleton append/upsert 后插入 active generation。
- `message_service.rs:1025-1069`
  - finalizer patch/append 最终消息并删除 active generation。
- `src-tauri/src/vcp_modules/persistence/message_repository.rs:339-391`
  - `upsert_message` 无条件覆盖 content、timestamp、agent、finish_reason、content_hash、render_cache，并将 `deleted_at` 清空。

#### 触发时序

1. 模型很快发出 thinking 事件。
2. 前端启动 skeleton IPC，但 SQLite/IPC 尚未提交。
3. 模型快速结束，后端 finalizer 先写入完整正文并删除 active row。
4. 迟到 skeleton 随后 upsert 空 content、NULL finish_reason，并重新插入 active row。

#### 影响

- 最终回复变空白。
- 应用重启后正文消失。
- 消息永久显示 recovering/thinking。
- render cache 被空 skeleton 覆盖。

#### 修复不变量

- 只有后端可以创建 skeleton；前端不得异步落盘 skeleton。
- 必须先事务提交 `pending` 消息与 active generation，再向前端发 thinking。
- 消息状态只能 `pending -> terminal` 单向演进。
- terminal 写入和 active row 删除必须在同一事务完成。
- pending upsert 永远不能覆盖 terminal；使用显式状态字段或带 generation 的 CAS。

#### 回归测试

- 人工 barrier：阻塞 skeleton transaction，让 finalizer 先完成，再释放 skeleton；最终正文必须保持 terminal。
- Agent、Group 各一例。
- 终态写入失败时 active row 必须保留。

### CHAT-02｜Critical｜ActiveRequests 同 ID 的 ABA 与级联取消

**处置状态（2026-08-10）：Resolved。** `ActiveRequestEntry` 以 attempt UUID 表示唯一 owner，lease 仅在 token 匹配时释放；cancel 只发信号不提前 remove。恢复已合并为一个持 lease 跨越 query/resume/final commit 的 command。

#### 证据

- `src-tauri/src/vcp_modules/infra/vcp_client.rs:108-137`
  - map 结构仅为 `msg_id -> Sender`；guard drop 时按 key 无条件 remove。
- 普通请求覆盖：`vcp_client.rs:358-361`
- resume 覆盖：`vcp_client.rs:2044-2046`
- 多条结束路径也按 key remove：`vcp_client.rs:997-1003,1179-1185,1259-1267,1329-1337,1366-1375`
- 前端恢复 guard 设置太晚：`src/core/stores/chatStreamStore.ts:585-647`

#### 触发时序

1. A 已注册 `msg_id -> sender_A`。
2. B 对相同 ID `insert(sender_B)`，旧 sender 被覆盖。
3. A 的 guard 或结束路径稍后执行 key-only remove。
4. A 实际删除 B 的 sender；B 失去取消句柄，或 receiver 因 sender drop 被动中止。

#### 修复不变量

- map value 必须包含唯一 attempt/generation token。
- 重复 live ID 应拒绝、幂等附着或显式替换并等待旧任务退出。
- remove/cancel/finalize 只能在 token 匹配时生效，即 `remove_if(current.generation == mine)`。
- recovery API 要区分 `already_attached`、`needs_resume`、`terminal`，不能把所有 streaming 状态都再启动一次 resume。

### HIST-01｜High｜历史分页 Channel Promise 永不 settle

**处置状态（2026-08-10）：Resolved。** 前端分页改用现有 `load_chat_history` 小批量返回，invoke Promise 即唯一完成协议，不再等待可丢失的 `is_last` 尾帧。

#### 证据

- `src-tauri/src/vcp_modules/chat/chat_manager.rs:87-117`
  - 所谓流式历史先 `fetch_all`、全量解压/解析，再逐块发送；每次 Channel send 错误被忽略。
- `src/core/stores/chatHistoryStore.ts:172-214`
  - abort/话题变更时 chunk handler 直接 return，包括 `is_last`；当 invoke 返回 `total > 0` 时仍无条件等待 `completePromise`。

#### 触发

- 分页期间切换话题，最后一帧因 aborted 被丢弃。
- 最后一帧 Channel send 失败。
- 后端 invoke 已返回数量，但前端永远收不到完成帧。

#### 修复不变量

- abort、invoke error、channel close、send error、最后一帧都必须 settle 同一个 completion。
- settle 必须幂等。
- 后端必须传播 Channel send error；若保留 Channel，应真正按数据库游标/批次生产，而不是先全量处理。
- 可考虑取消额外的 completePromise：让 command 返回最终 completion，并把 Channel 只用于数据帧。

### HIST-02｜High｜滚动状态永久卡在 `loading-top`

**处置状态（2026-08-10）：Resolved。** 四个加载入口收编到同一 Promise 完成协议，`finally` 在零条、报错与取消下都退出 `loading-top`；reset 会推进 generation 并取消旧 rAF/throttle。

#### 证据

- `src/core/composables/useChatScroll.ts:111-118`
  - `scrollHeight === lastScrollHeight` 时提前 return。
- `useChatScroll.ts:137-147`
  - `loading-top -> free/following` 的收敛逻辑位于提前 return 之后。
- `useChatScroll.ts:243-253`
  - onScroll 只允许 `free/following` 再触发分页。
- `useChatScroll.ts:317-329`
  - loading watcher 调用的仍是同一个会提前 return 的函数。

#### 最短时序

1. 顶部触发分页，scene 变为 `loading-top`。
2. 请求报错或返回零条，DOM 高度不变。
3. watcher 调 `handleContentChange`，高度守卫提前 return。
4. scene 永久保持 `loading-top`；后续 onScroll 不再触发加载。

#### 修复不变量

- 先处理状态机收敛，再执行高度无变化优化。
- 分页 promise 的 `finally` 应显式发出 `page-finished({addedCount,error})`，而不是隐式依赖 DOM 高度。
- `reset()` 同时取消 `scrollThrottleId` 和双 rAF，避免旧话题回调污染新话题。

### HIST-03｜High｜旧请求会释放新请求门闩

**处置状态（2026-08-11）：Resolved。** 历史加载使用 `ConversationKey + epoch + loadId`；旧请求的 success/error/finally 只有在三者仍匹配时才能提交列表、controller 与 loading。

- `src/core/stores/chatHistoryStore.ts:112-130,228-240`
- A load 被 B 替换后，A 的 finally 虽只在 controller 相同时清 controller，却无条件把 `loading/isLoadingHistory` 设为 false。
- B 尚未结束时分页入口被重新开放，第三个请求可再次取消 B；滚动状态也误判完成。

修复：每个 load 分配单调 epoch；消息、offset、loading、controller、scroll completion 的任何提交都要求 `epoch === currentEpoch`。

### HIST-04｜High｜OFFSET 动态分页必然重复或漏项

**处置状态（2026-08-11）：Resolved（有界窗口策略）。** 分页改为 `(timestamp, msg_id)` keyset cursor，并按消息 ID 合并；活跃窗口上限 500。顶部翻页淘汰新端后，置底、发送或输入聚焦先重新载入最新页。

#### 证据

- 后端：`src-tauri/src/vcp_modules/chat/message_service.rs:175-197`
  - `ORDER BY timestamp DESC,rowid DESC LIMIT/OFFSET`。
- 前端：`src/core/stores/chatHistoryStore.ts:188-195,256-265`
  - 直接 prepend，`offset += buffer.length`，没有按 message ID 去重。
- 新发送、流式新增、删除和截断不会同步维护 offset。

修复：改为稳定 keyset cursor，例如 `(timestamp,rowid)` 或服务端生成的 opaque cursor；前端合并仍按 message ID 防御性去重。若需要一致快照，应由后端给 snapshot token，而不是继续补 offset。

### CHAT-03｜Critical｜发送与异步消息操作没有冻结会话上下文

**处置状态（2026-08-10）：Resolved。** `ChatSessionStore` 成为唯一身份 owner，会话切换生成不可变 `ConversationKey { ownerId, ownerType, topicId, epoch }`；历史加载与发送/删除/编辑/重渲染都冻结 key，await 后按 epoch 与 message ID 重新校验。

#### 发送路径

- `src/core/stores/chatHistoryStore.ts:346-397`
  - 附件 JIT 处理 await 后才构造消息/调用 generation。
- `chatHistoryStore.ts:271-337`
  - 即使早期捕获部分 topic/agent，`append_single_message` await 后又读取当前 selected owner type/id。

时序：在 A 点击发送附件 -> await 处理 -> 用户切到 B -> 后续消息和 generation 读取 B 的全局状态。可能得到 A 的 topic + B 的 owner，或直接发进 B。

#### 删除/编辑/重渲染路径

- 删除：`chatHistoryStore.ts:402-430`
- 编辑/patch：`chatHistoryStore.ts:453-487`
- rerender：`chatHistoryStore.ts:580-597`

这些路径在 await 前保存 A 的数组下标，await 后直接用该下标操作当前全局数组。切到 B 后会删除或覆盖 B 的无关消息。

#### 修复不变量

- 用户动作入口一次性冻结 `ConversationKey { ownerId, ownerType, topicId, sessionEpoch }`。
- 整个命令链只使用冻结上下文，不重新读取全局 selection。
- await 后 UI commit 前复核 epoch，并重新按 message ID 查找；永远不用陈旧数组 index。
- 最终形态建议把消息按 topic 分区存储，而不是让所有话题共享一个裸数组。

### CHAT-04｜High｜finalizer 落盘失败仍删除恢复记录并发送 end

**处置状态（2026-08-11）：Resolved。** finalize 在单事务内更新 terminal/render cache/FTS 并删除 active generation；提交失败回滚并保留恢复记录、发 error，只有 commit 成功才发 end。

- `src-tauri/src/vcp_modules/chat/message_service.rs:1025-1061`
  - patch/append 错误只记录日志并转为 `None`。
- `message_service.rs:1064-1097`
  - 随后无条件删除 active row、发送 end、返回 Ok。
- Agent 错误路径：`agent_chat_application_service.rs:189-195`
- Group 错误路径：`group_chat_application_service.rs:313-322`

影响：前端显示完成，SQLite 仍是空 skeleton 或旧正文，而且恢复证据已经删除。

修复：terminal message 写入与 active 删除同一事务；写入失败时保留 active row，发明确 error，不得发送 end。UI 必须展示 pending/failed/retry，而不是仅 console.error。

### CHAT-05｜High｜发送中的附件可以在 loading 状态被提交

**处置状态（2026-08-11）：Resolved。** 发送入口冻结 conversation epoch，并等待附件全部进入 ready；预处理期间切换会话会使本次提交失效。

- `src/features/chat/InputEnhancer.vue:339,369-374`
  - 只要 staged 中有内容就允许发送，没有 all-ready gate。
- `src/core/stores/attachmentStore.ts:251-274,490-505`
  - 完成回调只会在 staged 数组中按 stableId/index 回写。
- send 会先 clear staged；清空后，异步完成无法再把 hash/internal path 写回已复制进消息的附件。

修复：只有全部附件 `done` 才可发送；或将 staged 原子转移为 keyed in-flight draft，后续完成回调更新该 draft。失败时应恢复草稿和可见错误。

### CHAT-06｜High｜切换话题时旧列表仍可操作

**处置状态（2026-08-11）：Resolved。** 会话切换立即 abort/clear；删改、重渲染、附件动作在 IPC 前冻结 key，await 后复核 epoch 并按消息 ID 重定位，不再使用陈旧数组下标。

- `src/features/chat/ChatView.vue:68-87`
- `src/core/stores/chatHistoryStore.ts:245-254`

切换时只重置分页，旧 messages 直到新 invoke 成功才替换；如果加载失败，会长期显示 A 的消息配 B 的标题，并仍允许编辑/删除/发送。

修复：history state 携带 `loadedTopicId`；选择变化立即进入明确的 loading/empty 状态；任何操作都要求 `loadedTopicId === selectedTopicId`。

### CHAT-07｜High｜恢复 singleflight 和 cold/warm 判断均错误

**处置状态（2026-08-11）：Resolved。** recovery 在首个 await 前安装 singleflight；后端返回 typed `already_running / needs_resume / terminal`，仅 `needs_resume` 创建恢复请求。

- `src/core/stores/chatStreamStore.ts:585-647`
  - `isRecovering=true` 在第一次 await 和 UI 预处理之后才设置，多入口可并发穿透。
- `chatStreamStore.ts:601-652`
  - 所有 active generation 先被加入 `activeStreamMessages`，随后再用 `.has(msgId)` 判断 `isWarm`，因此冷启动也总被判 warm。
- `chatStreamStore.ts:667-725`
  - resume fire-and-forget，外层 gate 在真实流结束前已释放；一次异常还会终止剩余消息恢复。

修复：函数入口同步安装一个共享 singleflight Promise；预处理前记录原始 warm set；按 msgId 维护唯一 recovery task 和 attempt token；每条恢复独立捕获错误，不能一条失败中止全部。

### CHAT-08｜High｜删除、截断和删除话题不会取消活跃 generation

**处置状态（2026-08-11）：Resolved。** 删除/截断先在事务内 tombstone 并清 active generation，再向 ActiveRequests 发送 cancel；迟到 terminal 不能复活已删除行。

- `src-tauri/src/vcp_modules/persistence/message_repository.rs:347-358`
  - upsert 冲突无条件 `deleted_at=NULL`。
- `src-tauri/src/vcp_modules/chat/message_service.rs:811-898`
  - 删除消息没有取消 ActiveRequests。
- `message_service.rs:901-946`
  - truncate 甚至没有清 active row。
- `src-tauri/src/vcp_modules/chat/topic_service.rs:205-251`
  - 删除话题同样没有取消网络任务。

修复：删除/截断先推进 topic/message generation 或建立 tombstone，再 cancel + await 对应任务；finalizer 只有在 nondeleted 且 generation 匹配的 pending row 上才能 CAS 到 terminal。

### CHAT-09｜High｜群聊恢复丢失 speaker Agent 身份

**处置状态（2026-08-11）：Resolved。** ActiveGeneration DTO/query 持久并恢复 group speaker identity，恢复请求沿用原 Agent 上下文。

- 原始 group thinking 含 agentId：`group_chat_application_service.rs:225-244`
- resume context 没有 agentId，finalizer 接收 `None`：`vcp_client.rs:2048-2052,2092-2107`
- repository upsert 会覆盖 agent_id/name：`message_repository.rs:347-355`

任何成功的群聊断点续传都可能在重载后丢失 speaker/avatar/name。恢复 claim 时应从 `(topic_id,msg_id)` 读取并锁定原归属，terminal update 不得清除不可变字段。

### CHAT-10｜High｜恢复错误分支更新不存在的列

**处置状态（2026-08-11）：Resolved。** 错误终态改走统一 finalize/update 语义，不再更新不存在的 schema 列。

- SQL：`src-tauri/src/vcp_modules/infra/vcp_client.rs:1718-1725`
- schema：`src-tauri/migrations/0001_create_initial_tables.sql:79-95`

fallback 尝试更新 `messages.is_thinking`，但 schema 没有此列。当 active row 被另一任务抢先删除时，该分支返回 `no such column: is_thinking`，前端恢复循环可能随即中止。

修复：只使用 schema 真实字段，按 `(topic_id,msg_id,generation)` 原子写 `finish_reason='error'`、正文/hash/cache并清 recovery row。

### CHAT-11｜High｜缺失附件自动下载无 timeout、大小上限和 hash 校验

**处置状态（2026-08-11）：Resolved。** 使用共享有界 HTTP client、连接/总时限与 stall timeout，50 MiB 上限、流式 SHA-256 校验及同目录临时文件原子 rename；失败清理临时文件。

- `src-tauri/src/vcp_modules/chat/message_service.rs:611-644`

当前实现可能无限等待、通过 `resp.bytes()` 读取超大 body 导致 OOM、写入错误内容，甚至写失败后仍回填本地路径。

修复：共享 client + connect/idle timeout；流式写临时文件；硬性字节上限；SHA-256 与期望 hash 一致后原子 rename；失败不得标 ready 或返回 internal path。

### CHAT-12｜Medium｜最终完成时间改变消息排序

**处置状态（2026-08-11）：Resolved。** terminal finalize 保留消息起始 timestamp，完成时间只写更新字段，不改变会话排序键。

- `message_service.rs:976-1008`
- `message_repository.rs:352`
- `src/core/stores/chatStreamStore.ts:460-464`

finalizer 把 timestamp 改为完成时间，当前 UI 只改对象不重排；重启后数据库排序可能把较早发起、较晚完成的回复移动到后续轮次之后。应保留 skeleton/start sequence，以显式 turn/sequence 排序，而非完成时刻。

### CHAT-13｜Medium｜相同 Mermaid 并发渲染时第二个永久占位

**处置状态（2026-08-11）：Resolved。** 相同内容复用 in-flight Promise；成功/失败均结算所有 waiter，缓存只在完整结果后提交。

- `src/features/chat/MessageRenderer.vue:527-563`

全局 `renderingMermaids` 命中后直接跳过，首个渲染结束只删除 Set，不通知后续实例重试。应缓存 in-flight Promise，让相同 key await 同一结果。

### CHAT-14｜Medium｜流完成 timer 集合单调增长

**处置状态（2026-08-11）：Resolved。** timer 到期/取消时从注册集合自注销，view generation 失效会统一清理未完成 timer。

- `src/core/stores/chatStreamStore.ts:65,276-284,569-578`

timer 执行后没有从 `cleanupTimers` 删除。修复为回调执行后删除自身，并在 store dispose 时统一取消。

---

## 5. Render Cache、数据库与异步后端

### CACHE-01｜High｜cache 损坏导致空白，正文变化保留旧 cache

**处置状态（2026-08-11）：Resolved。** `render_cache` 增加 `content_hash` 与 renderer schema；命中前双校验，解码/反序列化失败按 miss 回退正文并懒重建，不再返回空白。

#### 空白链

- `src-tauri/src/vcp_modules/persistence/message_repository.rs:28-34`
- `src-tauri/src/vcp_modules/chat/message_service.rs:949-959`
  - cache 解码错误被吞掉。
- `message_service.rs:329-340`
  - 只要 cache 列非 NULL 就被当成 hit；当 `include_content=false` 时原始 content 被省略。坏 cache 最终得到 `blocks=None + content=""`。

#### 旧内容链

- `src-tauri/src/vcp_modules/infra/settings_manager.rs:125`
  - 同步预渲染默认关闭。
- `src-tauri/src/vcp_modules/sync/sync_executor/pull_executor.rs:137-165`
  - 同步可更新正文但不生成 render bytes。
- `src-tauri/src/vcp_modules/persistence/db_write_queue.rs:482-492,518-557`
  - 只有新 render bytes 非空时才更新 cache；正文变化时旧 cache 不失效。

#### 修复不变量

- cache 必须绑定 `content_hash + renderer_schema_version`。
- 正文变化时同事务更新或删除 cache。
- decode 失败回退原始 content 重新编译；只有确认得到 blocks 后才能省略 content。
- 所有异步 cache 回写必须对观察到的 content hash 做 CAS。

### CACHE-02｜Medium｜detached cache 回写可覆盖新正文的 cache

**处置状态（2026-08-11）：Resolved。** 普通 cache miss 与全量重建 writer 都执行 hash-CAS；编译开始后的编辑/删除会使旧写入更新 0 行。CPU 编译进入共享有界 `spawn_blocking` 门禁。

- `message_service.rs:342-370,729-765`

任务读取旧正文、异步编译；并发编辑先写新正文/新 cache 后，旧任务仍可最后无条件覆盖。修复为 `UPDATE ... WHERE content_hash = observed_hash`，或把 cache 构建结果交给单一版本化队列。

### DB-01｜Critical｜数据库自愈可能丢失 WAL 中的已提交数据

**处置状态（2026-08-10）：Resolved at P0 scope。** 只有 SQLite CORRUPT/NOTADB 才确认损坏；连接关闭后将 DB/WAL/SHM 整体归档，任一 rename 失败尝试回滚。主库缺失但旁车存在时在 SQLite open 前 fail-closed；恢复状态与归档路径保存进 `SystemSnapshot` 供前端补领。三文件 rename 不具备掉电原子性，仍是已知工程残余。

#### 证据

- `src-tauri/src/vcp_modules/persistence/db_manager.rs:67-80`
  - 任意 open/check error 都进入自愈。
- `db_manager.rs:203-221`
  - `quick_check` SQL 错误直接折算为 false。
- `db_manager.rs:224-245`
  - 只 rename 主 `.db`，随后删除 `.db-wal/.db-shm`。

瞬时 I/O、锁或查询错误即可触发空库重建；真实损坏时，WAL 中仍可能有已提交但未 checkpoint 的最近消息。只归档主库再删除 WAL，不但丢数据，也使归档无法完整恢复。

修复：区分“数据库暂不可用”和“已确认损坏”；先显式关闭连接；将 DB/WAL/SHM 作为一个恢复单元原样保全；成功 checkpoint/backup 前绝不删除 WAL；不得静默切换空库，应向 UI 暴露恢复状态和归档路径。

### DB-02｜High｜同步写队列吞错并长事务阻塞交互写入

**处置状态（2026-08-11）：Resolved。** batch 缩至 32 tasks/500 messages/10ms，busy timeout 2 秒；worker 汇总 rusqlite/JoinError，`flush() -> Result` 把此前错误传给阶段状态机，失败不得发送完成。

- `src-tauri/src/vcp_modules/persistence/db_write_queue.rs:93-136`
  - 最高合并 200 tasks / 5000 messages，busy timeout 30 秒。
- `db_write_queue.rs:226-236`
  - rusqlite/JoinError 仅日志，任务已经被消费。
- `db_write_queue.rs:238-240,262-275`
  - flush 始终 ACK，API 不返回 Result。
- `src-tauri/src/vcp_modules/sync/sync_finalize.rs:24-26`
  - finalizer 将 flush 当作成功。

修复：每批提交保存 Result；flush 必须汇总并返回此前所有 batch 的错误；BUSY 做有界重试；同步事务缩小并主动让出，正常聊天终态写入应获得更高优先级。

### DB-03｜High｜Tokio worker 上直接执行 CPU 密集解析和压缩

**处置状态（2026-08-11）：Resolved at audited paths。** 消息解压、Markdown/渲染编译与多媒体处理迁入有界 `spawn_blocking`/共享 client；不再在 async worker 上执行审计列出的长 CPU 工作。

- `message_repository.rs:12-34`
- `message_service.rs:329-351,658-664,748-765,787-794`
- `vcp_client.rs:1119-1130,1200-1211`

长 Markdown、Syntect/Aurora parse、zstd 压缩在 async 路径同步执行，会阻塞 Tokio worker，放大 Channel、SQLite 和 stop 延迟。应进入有界 `spawn_blocking`/专用池，并按消息合并任务，避免 cache miss 无界 spawn。

### DB-04｜Medium｜Agent/Group save 与 delete 未共享实体锁

**处置状态（2026-08-11）：Resolved。** read/save/update/delete 共用 per-ID lock，tombstoned ID 拒绝迟到 upsert；同步删除复用 internal delete。同步队列绕过 Facade 后统一推进 cache generation 并 clear，旧 DB 快照不能迟到回填 ghost cache。

- Agent：`src-tauri/src/vcp_modules/agent/agent_service.rs:159-185,257-290`
- Group：`src-tauri/src/vcp_modules/group/group_service.rs:143-159,272-298`

save/update 使用 per-ID lock，delete 未取同一锁。save 可在 delete 后重新填充内存 cache，形成 ghost entity。所有 mutation 应共享同一锁或 tombstone version，cache commit 前复核实体版本。

### DB-05｜Medium｜高速上传临时端点 token 未使用且无协议边界

**处置状态（2026-08-11）：Resolved with local availability residual。** localhost 临时端点校验随机 UUID token 与唯一 Content-Length，header 16 KiB、body 200 MiB、accept 30 秒、连接 10 分钟、IO idle 10 秒；流式 hash、临时文件清理与原子 rename 已建立。监听仍串行，恶意本机慢连接属于已记录的可用性残余。

- `src-tauri/src/vcp_modules/infra/high_speed_channel.rs:35-50,66-130,173-176`

API 返回 token，但服务端不校验；accept 后 read 没有 timeout，header 在 `\r\n\r\n` 前可无界增长，外层 20 秒期限也不能取消已经 spawn 的连接任务。

修复：首包认证 token；header/body 硬上限；逐次 read idle timeout；任务受 CancellationToken/JoinSet 管理；失败清理临时文件。

---

## 6. Sync V2

### SYNC-01｜Critical｜stop sender 永久失效，stop/start 可叠加双会话

**处置状态（2026-08-11）：Resolved。** `SyncState` 持有唯一 SessionHandle/owner generation；start/stop 由 lifecycle mutex 串行，stop 先使代次失效，再 cancel + await 旧主任务与跟踪子任务，仅当前 owner 可清理状态。reconnect attempt 的任务、命令与 tracker 也已收编。

#### 证据

- `src-tauri/src/vcp_modules/sync/sync_service.rs:23-30`
  - `SyncState` 保存 `ws_sender`。
- `sync_service.rs:190-199`
  - 初始化创建 tx/rx，但 rx 当场丢弃。
- `sync_service.rs:1495-1509`
  - `start_manual_sync` 新建真正会话的局部 tx/rx，却不写回 state。
- `sync_service.rs:1457-1466`
  - `stop_sync` 只向死 sender 发 Cancel，并立即把 `is_syncing=false`。
- `sync_service.rs:750-1255`
  - 已连接循环只读局部 rx，也不检查共享 `is_syncing`。
- `sync_service.rs:1364-1375,1502-1505`
  - 旧会话退出时无条件清 logger、is_syncing 和 Guardian tag。

#### 必然时序

1. A 已连接。
2. stop 将 atomic 设 false，但 Cancel 发向死通道，A 继续运行。
3. start B 因 atomic=false 被放行。
4. A/B 各自持有 WebSocket、tracker、DbWriteQueue，并发写数据库。
5. A 较晚退出，又清除 B 的状态、logger 和保活。

#### 修复不变量

- `SyncState` 持有唯一 `SessionHandle { sessionId, cancelToken, commandTx, joinHandle }`。
- start 原子安装 session owner；如已有 session，拒绝或先 stop + await join。
- stop 必须推进 generation、cancel 并 await 旧 session 退出后才能允许重启。
- status/logger/Guardian 清理只允许当前 sessionId owner 执行。

### SYNC-02｜High｜同步面板可关闭后启动“隐形同步”

**处置状态（2026-08-11）：Resolved。** 监听注册绑定 view generation；`startSync` 等待 listener setup，并在首个 await 前把 `connecting` 设为不可卸载。系统返回、顶栏和程序化关闭共用 `canDismiss`，允许关闭时则 await 后端 cancel/join 后才 pop 页面。

- `src/features/sync/composables/syncSession.ts:28-35,111-144`
  - 四个 `listen()` 注册未等待；close 只能清理已 resolve 的 unlisten，迟到 Promise 会泄漏监听器。
- `syncSession.ts:38-76,84-90`
  - start 在电池查询 await 前仍可 dismiss；用户关闭后 stop 先返回，电池查询完成仍继续 invoke start。

影响：后台同步没有面板和事件监听，UI 长期 connecting；还会放大 SYNC-01。

修复：监听注册必须属于 view generation；注册完成前不启动；每个 await 后复核 attempt token；close await 后端真正 cancel/join。

### SYNC-03｜High｜Phase 任务没有 session epoch，Phase 3 可永久卡住

**处置状态（2026-08-11）：Resolved。** 每个 reconnect attempt 拥有独立 tracker/task set/counters；旧 attempt 先 cancel+join，阶段命令携带 attemptId。Phase 3 严格校验 topic 集合与逐项结果，错误立即 `FailAttempt`；60 秒无进展与 30 秒 final ACK deadline 均失败收敛，WS send/close 为 5 秒有界操作。

- `sync_service.rs:388-398`
  - tracker 在连接外创建并跨重连复用。
- `sync_service.rs:849-856`
  - 新 Phase 3 只 clear completed，不清 modified。
- `src-tauri/src/vcp_modules/sync/sync_executor/batch_diff_handler.rs:91-209`
  - batch 任务 detached，无 cancel/join。
- `batch_diff_handler.rs:33-63,197-201`
  - 只遍历服务端返回 results；请求中的缺失 topic 永远不会完成。
- Phase 1/2 有 watchdog：`sync_executor/diff_handler.rs:107-161`；Phase 3 没有。

修复：tracker、task、command 全部绑定 session epoch；断线 cancel+join；严格验证返回 topic 集合等于请求集合；Phase 3 增加失败型 watchdog。

---

## 7. Android 插件、`:helper` 与生命周期

### AND-01｜Critical｜旧 helper socket finally 关闭新 resume socket

**处置状态（2026-08-10）：Resolved。** session 安装不可变 `ClientConnection` 绑定，旧读循环只能以引用同一性 detach/关闭自己的 socket；重复 start 被拒绝，EventSource 安装与回调均校验当前 session identity。

- `src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/SseProxyService.kt:234-255`
  - A 连接退出时按 requestId 重新读取“当前 session”，无条件关闭其中当前 output。
- `SseProxyService.kt:332-363`
  - resume B 会替换 session 的 output。

时序：B 已安装新 output -> A EOF/finally 稍后执行 -> A 关闭 B。修复为每个连接捕获自己的 socket/output，并以引用同一性或 connection generation CAS 清理。

### AND-02｜Critical（root 设备）｜OOM guard 永久占用唯一 executor

**处置状态（2026-08-10）：Resolved。** OOM guard 使用独立 scheduler（非 root 只探测一次），root command 与文件 I/O 分别使用有界单 worker；destroy 取消 Future 并 `shutdownNow`。Rust 侧所有 Android 路径均在锁内 clone `PluginHandle`、锁外等待 Kotlin 返回。

- `src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt:161`
  - 全插件共用 `newSingleThreadExecutor`。
- `VcpMobilePlugin.kt:187-192,329-352`
  - init 在 root 设备把无限循环 OOM guard 投递到该 executor。
- `VcpMobilePlugin.kt:687-700,779-800`
  - 文件、Root 等后续任务仍投递到同一 executor。
- `VcpMobilePlugin.kt:1077-1095`
  - destroy 仅 shutdown，不中断当前无限任务。

修复：guard 使用独立 ScheduledExecutor + 可取消 Future；I/O 使用独立有界池；destroy 执行 `cancel(true)` 和 `shutdownNow`。

### AND-03｜High｜重复 requestId 覆盖 session，但旧 EventSource 成为孤儿

**处置状态（2026-08-11）：Resolved。** start 使用 `putIfAbsent`；EventSource 安装与所有 callback/cleanup 都校验 map identity，失主 source 立即 cancel，旧 session 无权删除新条目。

- `SseProxyService.kt:258-267`
  - 直接覆盖 `activeSessions[requestId]`。
- `SseProxyService.kt:290-323`
  - 旧 EventSource 仍由 listener 闭包持有。
- `SseProxyService.kt:366-423`
  - stop/query 只能看见新 map entry。

修复：`putIfAbsent` 拒绝重复 start，或替换前原子 cancel/close 旧 session；所有 callback 校验 generation。

### AND-04｜High｜helper 无界缓存并在 session 锁内阻塞写 socket

**处置状态（2026-08-11）：Resolved at software scope。** 最多 8 个活跃 session，单 session 20,000 事件/8 MiB、全局 24 MiB；输出使用单连接 writer、128 帧队列与 5 秒写 deadline，session 锁不跨 socket IO。预算需继续做真机长稳。

- `SseProxyService.kt:66-83`
  - eventBuffer、activeSessions 无事件数、字节数、会话数上限。
- `SseProxyService.kt:425-443`
  - 每个事件永久 append，并在 `synchronized(session)` 内 `write/flush`。
- `SseProxyService.kt:446-473`
  - 完成 session 仍可无限等待客户端 stop。
- `SseProxyService.kt:769-775`
  - socket 写无 deadline。

结果：客户端不读时 OkHttp callback 堵塞；stop/query/resume 等待同一 monitor；buffer 持续增长直至 helper OOM。

修复：per-session/global 字节预算；单独有界 writer queue；锁内只更新状态，锁外带 deadline 写；溢出时生成明确 terminal error；完成后有 idle grace 自动清理。

### AND-05｜High｜helper query 无 timeout，stop 没有 ACK 协议

**处置状态（2026-08-11）：Resolved。** helper connect/write/query/stop 均有阶段 deadline；stop 携带 expected generation，ACK 必须 requestId/generation 精确匹配且 `stopped=true`，正常结束与取消不再吞错。

- Rust query：`src-tauri/src/vcp_modules/infra/vcp_client.rs:1879-1913`
- Kotlin query/stop：`SseProxyService.kt:366-423`
- Rust stop：`vcp_client.rs:746-770`

connect/write/read 均无 deadline；Kotlin stop 处理后直接关 socket，Rust 写完即判成功。修复为分阶段 timeout，stop 返回包含 requestId/generation 的 ACK，上层只在 ACK 后完成终态切换。

### LIFE-01｜High｜生命周期 transition 无串行化和 epoch

**处置状态（2026-08-11）：Resolved。** 原生 transition 去重并携带 epoch；Rust spawn 前预留 epoch，主 transition 与 linger destructive action 共用同一 mutex，await 后复核。恢复标志只在当前 epoch 完成补偿后消费。

- Kotlin 连续发事件：`src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/LifecycleBridge.kt:53-63`
- Rust 每个事件独立 spawn：`src-tauri/src/lib.rs:185-199`
- transition 跨多个 await：`src-tauri/src/vcp_modules/infra/lifecycle_controller.rs:20-203`

旧 pause 任务可能在新 resume 后继续安装后台 timer/锁或断开连接。修复为单 transition mutex/actor，并让所有 await 后的副作用校验 lifecycle epoch。

### AND-06｜High｜旧 StreamKeepaliveService.onDestroy 可清除新消费者

**处置状态（2026-08-11）：Resolved。** Service Intent、ready/failure/onDestroy 全部携带 generation；stale destroy 无权清新消费者，最后释放与失败恢复各自按 expected generation 提交。

- `src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt:118-141`
  - 最后消费者 release 后调用 stopService；真正 onDestroy 异步发生。
- `StreamKeepaliveService.kt:67-79`
- `ForegroundGuardian.kt:147-157`
  - onDestroy 无代际地 `releaseAllLocks()`。

窗口：旧 Service 等待销毁时新 acquire 已添加 consumer；随后旧 onDestroy 清空新 consumer。源码竞态成立，具体 OEM 调度窗口需真机验证。

修复：Service generation/expected-stop token；onDestroy 只释放本 generation 的物理资源，不能清除更新代 consumer。

### AND-07｜High｜FGS 启动失败被吞，调用方仍得到 success

**处置状态（2026-08-11）：Resolved。** acquire 保存 previous consumer/desiredGeneration，命令等待 `onServiceReady` 后才 resolve；`startForeground` 失败回滚 consumer 与代次，并以独立 recovery generation 恢复仍有效的旧消费者。

- `ForegroundGuardian.kt:76-88,219-245`
- `VcpMobilePlugin.kt:847-860`
- `StreamKeepaliveService.kt:45-64`

Guardian 先登记 consumer、获取锁，再启动 FGS；异常只日志，Kotlin command 仍 resolve。修复为 service readiness ACK；失败回滚 consumer/locks并向 Rust/前端返回可诊断错误。

### AND-08｜High｜同步 tag 被当作普通 stream，仅获 10 分钟保活

**处置状态（2026-08-11）：Resolved。** sync/prerender/stream/distributed 使用明确 tag、优先级与超时；同步为 30 分钟，并由唯一 owner 配对释放。

- Rust tag：`src-tauri/plugins/vcp-mobile/src/stream.rs:61-75`
- Guardian 匹配/timeout：`ForegroundGuardian.kt:90-110`
- Window flag 清理：`VcpMobilePlugin.kt:89-97,864-872`

`stream:<label>` 先匹配 stream 分支，sync 的 30 分钟分支不可达。timeout 还绕过插件 Window flag 清理，可能失去 FGS 后继续亮屏。

修复：使用明确 domain/tag 或显式 timeout；Window flag 必须由统一 owner 在所有 release/timeout/destroy 路径清理。

### AND-09｜Medium/High｜helper 应用启动即常驻，与按需设计不符

**处置状态（2026-08-11）：Resolved。** 插件 init 不再拉起 helper；首次连接失败/端口缺失时按需启动，3 秒 readiness deadline，空闲无 session 后 30 秒退出。

- `VcpMobilePlugin.kt:173-192`
- `SseProxyService.kt:90-127,523-545`
- `src-tauri/plugins/vcp-mobile/android/AndroidManifest.xml:31-38`

插件 init 无条件启动独立 `:helper`，服务 `START_STICKY`、`stopWithTask=false`；普通空闲不会退出。建议首次 stream 时懒启动，最后 session 后 idle grace stop。

### AND-10｜Medium｜KeyboardInsets 可能造成布局抖动与主线程压力

**处置状态（2026-08-11）：Resolved。** Insets 快照经 `postOnAnimation` 合帧，只发送最新且变化的值；detach 取消 pending frame，避免 Activity 重建后向旧 WebView 注入。

- `src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/KeyboardInsetsManager.kt:23-64`
- `src/App.vue:25-37`

每个 Insets 帧都 log + `evaluateJavascript`，无去重/节流；safe bottom 变为 0 时前端可能保留旧值。需在 Pixel/三星/小米、手势导航、横竖屏和浮动键盘下验证。修复为按值去重、逐帧节流并正确处理 0。

### AND-11｜Medium｜每个活跃 SSE 循环播放静音音频

**处置状态（2026-08-11）：Resolved。** 静音音频保活路径已移除，helper 仅按活跃 session 管理独立 WakeLock/WifiLock。

- `SseProxyService.kt:682-756`

该策略可能触发媒体指示、干扰其他音频并增加耗电，是否真的提高存活没有证据。应在 Android 13-15、蓝牙/耳机/通话场景量化；没有收益则移除。

---

## 8. Distributed

### DIST-01｜Critical｜start/stop 竞态导致 stop 后复活

**处置状态（2026-08-11）：Resolved。** 单一 lifecycle mutex 串行 start/stop，stop 先推进 generation 再 cancel/join 主连接循环，connect/ACK 仅能在代次仍匹配时提交 Connected；派生任务与 writer 也属于 session owner。

- `src-tauri/src/distributed/client.rs:84-156`
  - start 检查状态后跨 emit/keepalive 等 await，较晚才安装 session。
- `client.rs:160-199`
  - stop 在 session 未安装窗口只能改状态，没有 token 可取消。
- `client.rs:511-520`
  - stop 不推进 session_id，迟到 ACK 仍可通过并写回 Connected。

修复：单一 lifecycle mutex；start 一开始就安装 generation + cancel owner；stop 必须推进 generation；最终提交 Connected 前复核 generation。

### DIST-02｜High｜Android 网络恢复事件监听了错误 channel

**处置状态（2026-08-11）：Resolved。** Rust 监听 Kotlin 实际发出的网络事件契约，并只在 online transition 触发当前 DistributedClient 的 reconnect channel；契约测试覆盖名称与 payload。

- Kotlin：`src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt:1023-1029`
  - `trigger("vcp-network-status-changed", ...)`。
- Rust：`src-tauri/src/lib.rs:162-177`
  - 监听裸 `vcp-network-status-changed`。
- 已存在生命周期契约：`src-tauri/src/lib.rs:181-202`
  - 插件短 channel 会映射为 `vcp-mobile://...`。

因此原生网络恢复不会进入即时 distributed reconnect。统一为短 channel `network-status-changed` + Rust `vcp-mobile://network-status-changed`，并加跨层契约测试。

### DIST-03｜High｜Guardian 所有权与前后台需求相反

**处置状态（2026-08-11）：Resolved。** lifecycle controller 是长期 `distributed` tag 的唯一 owner；`distributed:connect` scoped lease 只覆盖 `connect_async` await，不跨 session 或 backoff。

- client start 始终 acquire：`src-tauri/src/distributed/client.rs:127-133`
- resume release：`src-tauri/src/vcp_modules/infra/lifecycle_controller.rs:159-164`
- 下一次 background 只假定 client 已拥有，不再 acquire：`lifecycle_controller.rs:112-139`

结果是前台启动可能常驻通知，而经历一次 resume 后真正后台 linger 反而没有保活。应由 lifecycle controller 独占 distributed tag：background acquire、resume release；client 只拥有连接。

### DIST-04｜High｜子任务脱离 session，socket 写无 deadline

**处置状态（2026-08-11）：Resolved。** tool/report/placeholder 子任务全部进入 session tracker；退出先 cancel 再 join。单一 writer 串行发送，业务发送等待 5 秒完成结果，stop 返回后旧 session 不再有 socket IO。

- detached tool/placeholder task：`src-tauri/src/distributed/client.rs:539-570,660-684`
- 共享 mutex 跨无界 send await：`client.rs:722-733`

stop 只等待 connection loop，不能取消设备子任务；半开 socket 可让所有结果发送排队。修复为每 session 一个 `TaskTracker/JoinSet` + child cancellation token；单 writer 有界队列和 send deadline；operation tag 唯一或真实引用计数。

### DIST-05｜Medium｜前端监听迟到泄漏，旧 snapshot 覆盖新事件

**处置状态（2026-08-11）：Resolved。** 多消费者共享 pending listener 时只有创建者能安装/释放 handle；引用计数最后退出才卸载。listener generation、event revision 与 Rust `session_id` 阻止迟到注册、旧 snapshot 和旧事件提交。

- `src/features/distributed/composables/useDistributed.ts:30-93`

listen Promise pending 时 deactivate 无法 unlisten；resolve 后仍安装永久 listener。activate 的慢 `refreshStatus` 还可覆盖已经收到的新事件。前端类型没有 session_id，无法拒绝旧状态。

修复：disposed generation；迟到 listener resolve 立即 unlisten；保留/比较 session_id；listen reject 后清 promise 以允许重试。

---

## 9. Topic、前端状态与性能

### TOPIC-01｜High｜同 owner 请求重入混合结果

**处置状态（2026-08-11）：Resolved。** `ownerType:ownerId` 作为复合 key；同 key 复用 singleflight Promise，每次真实加载有 generation 与请求内 Map，旧 chunk 无提交权。

- `src/core/stores/topicListManager.ts:86-132`
- `src/core/stores/appLifecycle.ts:169-176`
- `src/features/topic/TopicList.vue:168-180`

Store 只按 current owner 判断旧请求，无法区分同 owner 的第 1/2 次请求。启动/恢复时 lifecycle 和 immediate watch 可同时发起两个 Channel，双方都 push；A -> B -> A 时旧 A 也会被新 A 接受。旧请求 finally 还会提前清 loading。

修复：`{ownerId,ownerType,requestId}` latest-wins；同 owner 只有一个 loader；结果按 owner 分区。

### TOPIC-02｜High｜同步/重建清空缓存后不会重载

**处置状态（2026-08-11）：Resolved。** invalidation 推进 generation 并清空列表；`performFullReload` 随后显式按当前复合 owner 重载，不依赖 selection watch 的依赖再次变化。

- `src/core/stores/topicListManager.ts:43-46`
- `src/core/composables/useDataReload.ts:17-36`
- `src/features/topic/TopicList.vue:168-180`

invalidate 只清 topics，保留 current owner；watch 只依赖 selected ID，其值没变，所以列表可永久显示空。reload 流程必须显式 await 当前 owner 的 `loadTopicList`，或让 cache generation 成为 watcher 依赖。

### TOPIC-03｜High｜Agent/话题选择没有 latest-wins

**处置状态（2026-08-11）：Resolved。** selection 与 topic load 都采用复合 owner + generation；`TopicList.vue` 同时 watch ID/type，A→B→A 的旧请求仍因 generation 失效。

- `src/core/stores/chatSessionStore.ts:128-168`
- `src/features/topic/TopicCreator.vue:77-84`
- `src/core/stores/topicListManager.ts:139-174`

快速选择 A、B 时迟到的 A 可覆盖 B；创建话题期间切 owner，命令使用旧 owner，await 后选择阶段却重新读取新 owner。需要 selection epoch 和冻结 owner snapshot。

### PERF-01｜高可信风险｜后台 rAF 队列无界累积

**处置状态（2026-08-11）：Resolved at software scope。** hidden 状态不再逐帧排队渲染，而是保留有界最新快照；恢复可见时合并提交一次，相关 timer/rAF 在 generation 失效后清理。

- `src/core/stores/chatStreamStore.ts:44-61,110-148,410-436`

隐藏 WebView 中 rAF 可能暂停或强节流，但 SSE 仍持续把字符串和 mutations 合并到 pending 对象，没有字节或 mutation 上限。回前台可能一次 apply 巨量 mutation，造成首帧冻结。

修复：后台切换为最新完整 snapshot/有限 tail；设置每消息字节和 mutation 上限；超限丢弃中间 diff并请求/使用最新状态。

### PERF-02｜高可信风险｜消息列表没有真正虚拟化

**处置状态（2026-08-11）：Mitigated。** 未引入高复杂度虚拟列表；Store/DOM 采用 500 消息有界窗口。加载更旧历史淘汰最新端后，置底/发送/输入聚焦会显式返回最新页。极长会话手感与内存仍需真机长稳。

- `src/features/chat/ChatView.vue:289-296`
- `src/features/chat/MessageRenderer.vue:580-596,725-735,755-917,1055-1060`
- `src/features/chat/composables/useMessageEvents.ts:106-116`

当前仍全量实例化所有消息，每条包含 watcher、动态渲染状态和监听器。`content-visibility` 只能减少布局/绘制，不能减少 Vue 实例、DOM 和响应依赖。长历史分页后内存与主线程成本单调增长。

修复方向：可变高度窗口化；若短期不做完整虚拟列表，至少只保留有限页并用稳定锚点回载。需要 Android 真机长会话 soak 确定阈值。

### PERF-03｜Medium｜visibility fallback 不会触发流恢复

**处置状态（2026-08-11）：Resolved。** visibility/foreground recovery 走统一 singleflight，旧恢复 Promise 受 view/conversation generation 门禁，不会遗漏或重复恢复。

- `src/core/composables/useAppLifecycle.ts:30-35,38-63`

visibility 变 visible 只更新 `isBackground`；只有 Tauri resume/online 调 recovery。若原生 resume 丢失，兜底无法恢复。应统一进入幂等 foreground recovery scheduler。

### UI-01｜Medium｜UI 宪法与构建资源漂移

**处置状态（2026-08-11）：Resolved。** 主/浮动入口复用同一样式清单，移除内容区 blur 与裸魔法层级，构建按 Vue/render/Tauri 与懒加载领域拆 chunk，700 KiB 仅作为 lazy chunk 审查阈值。

- `src/assets/message-blocks.css:1011`
  - 滚动消息区域使用 `backdrop-filter: blur(6px)`，违反移动端滚动内容禁用 blur 的约束。
- `src/features/settings/components/AboutSection.vue:342-347`
  - 内容卡片使用 24px blur。
- `src/features/assistant/AssistantView.vue:164`
  - 使用裸 `z-50`，未使用语义层级。
- `src/components/layout/PermissionGate.vue:462`
  - `heroicons-rocket` 在构建时找不到对应图标。

`pnpm build` 还报告主 chunk 约 530.51 kB、Mermaid core 约 586.51 kB，均超过 500 kB 警告阈值；另有 `useRenderedImageViewer` 同时静态/动态导入的警告。

---

## 10. 插件契约、仓库治理与测试缺口

### 10.1 插件契约

只读核对显示：当前 Rust handler、`build.rs`、`permissions/default.toml`、`permissions/all.toml` 的 43 条命令没有发现现存数量漂移。

但仍有治理缺口：

- `src-tauri/plugins/vcp-mobile/guest-js/index.ts:1-93` 只封装约 12 条命令，业务代码大量 raw invoke；AGENTS 声明的“四重注册”没有被完整执行。
- `PluginContractTest.kt:28-78` 只检查：
  - Rust 命令出现在 `default.toml`；
  - 一个 guest-js 参数；
  - Rust 方法名存在于 Kotlin。
- 未检查真正运行的 `all.toml`、`build.rs`、参数 schema、完整 guest API、插件事件 channel。
- `ForegroundGuardianTest.kt:26-113` 只覆盖 happy path，未覆盖 FGS 启动失败、timeout、屏幕 flag 和 stop/onDestroy/new acquire 竞态。
- 没有 `SseProxyService`、LifecycleBridge、KeyboardInsets、native network event、sync session cancellation 的有效测试。

### 10.2 文档与实际仓库漂移

当前仓库实际不存在以下 AGENTS.md 所描述内容：

- 根目录 `scripts/`
- 根目录 `plans/`
- `build_android_release.ps1`
- `src/tests/integration/`

因此：

- `pnpm dev:usb`、`pnpm memory:refresh` 等依赖缺失脚本的命令可能不可用。
- `pnpm test:integration` 当前直接以 “No test files found” 退出 1。
- AGENTS.md 中 Rust 41 项、前端 23 项的计数已过期；最终收口后本轮实际为 Rust 94 项、前端 64 项（17 个文件），Android 插件 JVM 为 25 项。

接手 Agent 修业务缺陷时不要顺手重写 AGENTS.md；应把治理修复作为独立、可审阅的提交。

### 10.3 当前关键覆盖与剩余缺口

P0/P1/P2 收口已新增定向覆盖：

- 前端：`ConversationKey` A→B→A epoch、迟到 owner/topic 选择、旧 history load 不释放新门闩、发送/停止冻结原会话、冷恢复 singleflight、零条分页退出 `loading-top`。
- Rust：pending/terminal 事务单向性与回滚、ActiveRequest 重复申领/旧 lease 释放、keyset/窗口返回最新、cache hash/schema/CAS 与 rebuild CAS、DB/WAL/SHM 归档与孤立旁车 preflight、DbWriteQueue 错误传播、Agent/Group cache generation、Sync owner/attempt/tracker/final ACK、Distributed 迟到 ACK 与子任务 quiescence。
- Android JVM：SSE connection identity、bounded writer/session budget、执行域隔离/拒绝/关闭、FGS generation/readiness/失败回滚、screen owner OR、stop ACK 与插件跨层契约。

软件门禁之外仍需补证的重点：

- 真实 Android WebView 中的富 HTML/复杂 Mermaid、500 消息窗口和后台流恢复手感。
- DB 三文件归档在进程崩溃/掉电中的故障注入；三次 rename 不能宣称具备断电原子性。
- 真实远端 Sync/Distributed 服务端的丢包、半开连接与协议故障注入。
- root 真机、Android 14 FGS 拒绝/OEM 后台调度、Activity 旋转下双 screen owner、主进程死亡后 helper 恢复，以及 API 26/36 完整 E2E。

---

## 11. 本次验证记录

| 命令 | 结果 | 备注 |
| --- | --- | --- |
| `pnpm check` | PASS | `vue-tsc --noEmit` + `cargo check` |
| `pnpm test:run` | PASS | 17 files / 64 tests；含会话 epoch、历史窗口、listener owner、Sync close gate 与流渲染背压 |
| `cargo test --locked --lib` | PASS | 94 tests；含 terminal 事务、attempt lease、cache CAS/generation、DB 孤立旁车、Sync/Distributed/Lifecycle owner |
| `cargo test --locked --test file_extractor_integration` | PASS | 10 tests |
| `cargo fmt --all -- --check` | PASS | 无格式漂移 |
| `cargo clippy --locked -- -D warnings` | PASS | 无 clippy warning |
| `pnpm build` | PASS | 无 Vite warning；主入口 461.35 kB，Mermaid/Cynefin 为按需 chunk |
| `cargo bench --locked --profile perf --no-run` | PASS | 仅编译，2m22s；四个 benchmark executable 均生成 |
| `pnpm test:integration` | GOVERNANCE OPEN | package script 存在但 `src/tests/integration` 不存在；未以空测试冒充通过，不作为本轮已覆盖门禁 |
| Android Gradle JVM test | PASS | 25 tests；含 SSE owner、执行域、FGS generation/rollback、screen OR 与跨层契约 |
| `cargo check --locked --target aarch64-linux-android` | PASS | NDK `29.0.13846066` / API 26 clang |
| `pnpm tauri android init --ci` | PASS | tracked Android 生成树零漂移 |
| Android `aarch64` APK build | PASS | 29,432,994 bytes；SHA-256 `d9703365c0de363ceb47c0508f6f047bffe8aed6755720413a7f1ecaa48ebaad`；v2 签名结构通过，本地产物为 Android Debug 证书 |
| Android E2E/perf | NOT RUN | `adb devices -l` 无设备 |
| `pnpm audit --prod` | PASS | 0 advisories |
| `pnpm audit` | PASS | 含开发依赖 0 advisories |
| `cargo audit` | PASS WITH EXCEPTION | 原始扫描 1 vulnerability / 21 warnings；仅忽略不可达且无补丁的 `RUSTSEC-2023-0071` 后门禁通过 |

Android Gradle 早期验证曾遇到依赖镜像/传输故障；本轮在不关闭任何校验的前提下完整重跑，插件 JVM 测试 25/25 与 universal Release APK 打包均通过。本地 Gradle 由 JDK 21 执行时报出 source/target 1.8 废弃提示，CI/发布基线仍是 JDK 17；该提示不属于本轮 P0 回归。当前没有连接 adb 设备，因此不得把本表解读为真机通过。

---

## 12. 修复战役状态

Phase 0–4 的代码不变量与自动化门禁均已完成；Phase 4 的“完成标准”包含真机/OEM 证据，仍属于 release 前验收。Phase 5 中与本次风险直接相关的窗口、背压和 blocking pool 已完成，缺失脚本与空的前端 integration suite 保留为独立仓库治理项，未用占位测试伪造覆盖率。

### Phase 0：安全止血（按产品策略完成）

目标：先消除可被远端内容利用的入口。

1. **SEC-01 已按产品策略处置**：普通 Markdown/ToolBlock 保留富 HTML，经 active-capability filter 后进入主 DOM；严格 raw HTML 禁用方案不采用。
2. **SEC-01 已完成**：危险主动文档 scheme gate，同时保留产品需要的网络/asset/blob/data-media URL。
3. 恢复 CSP（未纳入 SEC-01 本次产品级处置，作为独立纵深防御候选）。
4. 缩减 `vcp-mobile:allow-all` 和 Settings 秘密读取面（未纳入 SEC-01 本次产品级处置）。
5. **SEC-02 已完成**：Mermaid、DOMPurify、Vue/Vite 安全补丁及 PostCSS/Nano ID/brace-expansion 传递依赖已更新，生产与完整 npm audit 均为 0。
6. **SEC-01 已完成**：建立 stable/stream/ToolBlock raw HTML -> IPC spy 与保真回归测试。

SEC-01 产品口径完成标准：直接脚本、危险 scheme、显式 Tauri/网络/存储事件在所有消息渲染路径被拦截；富 HTML 保真；HTML Preview sandbox 功能不回退。刻意混淆绕过属于已记录并接受的剩余风险。

### Phase 1：消息持久化单一所有权（完成）

目标：保证消息只能从 pending 单向进入 terminal/deleted。

1. skeleton 移到后端事务中创建。
2. terminal write + active delete 同事务。
3. DB error 必须保留恢复证据并向 UI 发 error。
4. upsert 增加状态/generation CAS，禁止 terminal/deleted 被 pending 覆盖。
5. cache 与 content_hash/schema version 绑定。
6. 删除/截断先 tombstone + cancel/join。

完成标准：人工构造所有逆序时序，最终数据库和 UI 仍保持同一 terminal 状态。

### Phase 2：统一 generation/epoch（完成）

目标：所有迟到异步任务都只能清理自己。

1. Chat `ConversationKey + requestEpoch`。
2. ActiveRequests value 增加 attemptId，remove-if-match。
3. Recovery singleflight 和 per-msg task owner。
4. Sync `SessionHandle`，stop cancel + await join。
5. Helper connection generation。
6. Lifecycle、Guardian、Distributed generation。

完成标准：A -> B -> A、stop -> start、old finally after new start 等 ABA 测试全部稳定。

### Phase 3：历史分页与滚动收敛（完成）

1. Channel 所有退出路径 settle。
2. latest-wins loading flag。
3. OFFSET 改 keyset cursor，前端按 ID 去重。
4. Scroll state 由明确 page completion 事件驱动。
5. 话题切换立即隔离旧 messages。

完成标准：零条、异常、abort、切换、动态插入/删除均不会重复、漏项或卡 loading。

### Phase 4：Android/helper/distributed 可靠性（软件范围完成，待真机验收）

1. helper bounded buffer + writer queue + timeout + stop ACK。
2. 修正 old socket/new socket ABA。
3. Guardian readiness、generation、tag 与 Window flag owner。
4. Root guard 独立可取消 executor。
5. Distributed lifecycle mutex + child task tracker。
6. 修正 native network event channel。

完成标准：Android 13-15 真机执行断网、后台、恢复、通知拒绝、10 分钟以上同步和 root 场景。

### Phase 5：性能与治理（审计代码范围完成，仓库治理独立跟进）

1. 长消息列表窗口化/有限页回载。
2. 后台 rAF 改有界 snapshot 策略。
3. CPU 密集预渲染进入有界 blocking pool。
4. 补齐 test:integration、插件契约与缺失脚本/文档，作为独立提交。

---

## 13. 验收测试矩阵

下列软件竞态与协议反例已纳入 Rust/Vitest/Kotlin 定向测试；最后的真机性能场景以及依赖真实远端服务的故障注入仍未执行。

### 安全

- raw HTML 中直接触达宿主能力的事件属性、SVG 事件、危险链接不能执行；纯局部 DOM 交互按产品策略保留。
- 普通消息和 ToolBlock 的已覆盖直接攻击样本不能调用 Tauri invoke。
- sandbox HTML Preview 仍按预期工作，且无 `allow-same-origin`。

### 消息事务

- delayed skeleton after fast finalizer。
- terminal DB write failure。
- delete/truncate while streaming。
- cache decode failure and content hash mismatch。
- group resume preserves speaker identity。

### 历史与会话

- A -> B -> A 三次历史请求乱序返回。
- abort 在最后一帧之前/之后。
- Channel send error、0 条分页、加载异常。
- 新消息插入/删除后继续分页，无重复无漏项。
- A 中删除/编辑 await 时切到 B，B 数据保持不变。
- 附件 loading 时发送被阻止；附件处理期间切 topic 不串话题。

### 恢复与 helper

- resume、online、history-load 同时触发同一 msgId。
- helper A EOF 发生在 B resume 安装之后，B 不能被关闭。
- 重复 requestId start。
- 客户端不读 socket、buffer 达上限、query/stop timeout。
- cold recovery 与 warm recovery 分支都实际可达。

### Sync

- connected 状态 `start -> stop -> immediately start`。
- 旧 session 最后退出不能清新 session 的 logger/status/Guardian。
- DbWriteQueue batch error 必须令 flush/finalizer 失败。
- Phase 3 服务端漏一个 topic result，必须失败而不是永久等待。

### 数据库

- transient quick_check error 不得触发空库重建。
- WAL 含未 checkpoint 已提交数据时，自愈必须完整保全 DB/WAL/SHM。
- 同步正文变化必须失效旧 render cache。

### Android/distributed

- pause/stop/resume 人工乱序 barrier。
- old Service onDestroy after new acquire。
- FGS startForeground 失败/通知拒绝返回明确错误并回滚。
- 同步持续超过 10 分钟。
- root 设备上文件、图库、Root command 均有界返回。
- distributed start 中立即 stop，最终必须 Disconnected 且无子任务。
- 原生 network-status event 能触发且只触发当前 session reconnect。

### 性能真机验收

- 500/1000/3000 条含代码块、Mermaid、图片消息的滚动内存与长任务记录。
- 后台持续流 5/15/30 分钟后恢复前台的首帧时间。
- helper buffer、主进程内存、WakeLock/FGS/音频状态的长稳 soak。

---

## 14. 审计边界

- 本次没有 Android 真机，因此 Android 14 FGS 拒绝与恢复、OEM 后台调度、KeyboardInsets、Activity 旋转下 manual/Guardian 双 owner、WebView 后台流恢复和 500 消息窗口阈值仍需设备证据。
- Rust/Tauri 供应链已完成兼容补丁整理；原始 `cargo audit` 仍保留 1 个 feature-inactive RSA accepted exception 与 21 个 informational warning，详见 SEC-02 跟进记录。
- 没有真实远端 Sync/Distributed 服务端进行协议故障注入；相关确认项基于客户端源码状态与必然时序。
- DB/WAL/SHM 恢复已在任何 SQLite open 前 fail-closed，并覆盖 rename 失败回滚；但三文件 rename 在进程断电下不是事务原子操作，仍需故障注入/人工恢复预案。
- 高速上传入口已经有 token、总大小、header、accept/idle/connection deadline 与原子落盘；单 listener 被本地慢客户端占用属于受 deadline 限制的本机可用性残余。
- render cache 的正文/hash/schema/CAS 保证展示正确性；全量重建枚举发生底层读取错误时可退化为后续 lazy rebuild，而不是发布错误 cache。
- Agent/Group 本地写与同步写仍采用 SQLite 最终提交顺序（last commit wins）；tombstone 不会被清除，cache generation 防止旧快照回填，但这不是跨设备业务冲突合并算法。
- 500 条窗口、ASCII UUID 的 SQLite/JavaScript keyset 顺序一致性和 700 KiB lazy chunk 审查阈值是明确工程取舍，不等价于完整虚拟列表或性能真机结论。
- 本文不是“发现数量 KPI”。修复优先级应围绕不变量和用户可见数据正确性，不要逐条打补丁后保留原有所有权冲突。

## 15. 最终结论

消息列表偶发卡住并非渲染层偶然抖动，而是分页 completion、滚动状态、终态持久化、恢复代次和同步取消之间的跨层一致性缺失。本轮没有通过拆分 God File 或引入全局 mega-state-machine 解决它，而是在现有 owner 上建立了同一组轻量不变量：

> **每个异步会话只有一个 owner；每个副作用必须携带 generation；每个终态只能单向提交；每个取消必须等待任务真实退出；每个错误必须成为协议结果。**

截至 2026-08-11，审计清单的软件代码范围已闭环，三路对抗复核未发现剩余 blocker，完整静态检查、单测、供应链、benchmark 编译、Android target 与 APK 构建均通过。该提交可作为候选发布基线；正式 release 仍须完成第 14 节列出的真机/OEM、真实远端故障注入与发布证书验收，不得将本地 Debug 签名 APK 当作正式产物。
