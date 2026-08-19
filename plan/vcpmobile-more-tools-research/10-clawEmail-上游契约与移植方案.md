# 10 · clawEmail（VCPClawMail）—— 上游契约与移植方案

> 目标：在 VCPMobile 中实现邮件的查看与管理。用户已配置 clawEmail 让 Agent 处理邮箱，
> 但查看/管理邮件要借助第三方邮箱 App——这正是移动端的主场。
> ⚠️ 功能近乎从零创建（桌面端无任何邮箱 UI），是三候选中复杂度最高的一个。
> 本文档基于对 VCPClawMail 插件（2153 行）、admin 路由与 AdminPanel-Vue 的全量精读。

参考实现：
- `/home/dudu/VCPToolBox-main/Plugin/VCPClawMail/VCPClawMail.js`（插件真相，已全文精读）；
- `/home/dudu/VCPToolBox-main/routes/admin/clawMail.js`（91 行，4 条 HTTP 路由）；
- `/home/dudu/VCPToolBox-main/AdminPanel-Vue/src/{api/clawMail.ts, views/ClawMailManager.vue}`；
- `/home/dudu/VCPChat`——确认**完全没有邮箱 UI**（仅工具参数表单识别 mail1-4 槽位枚举）。

---

## 1. 结论速览

- clawEmail **不走 IMAP/SMTP**：经 `@clawemail/node-sdk@0.2.4` 访问网易 ClawEmail
  （claw.163.com）**私有云 HTTP API** + WuKongIM WebSocket 推送（wss://claw.126.net:5210）。
- **无数据库**：邮件本体全在云端；本地仅 `Plugin/VCPClawMail/data/` 下的缓存快照/
  幂等状态/附件文件；列表/详情请求实时穿透到云端。
- 现有 4 条 `/admin_api/claw-mail/*` 路由已能支撑 **MVP：账户切换 + 列表分页 + 详情 + 移入垃圾箱**。
- **最大缺口：发信/回复无 HTTP 出口**；但插件内 sendMail/replyMail/listFolders/
  downloadAttachment 均已实现，上游补丁 <150 行。
- **移动端直连 IMAP/SMTP 基本不可行**（API Key 认证体系，非邮箱密码）→ 推荐经 VCP 后端代理。

---

## 2. 后端架构

```
VCPToolBox 主进程 (PORT)
└─ VCPClawMail 插件 (hybridservice 常驻)
   ├─ MailClient(@clawemail/node-sdk) × N 个邮箱用户
   │   ├─ client.mail:      read / getAttachment / send / reply（仅这 4 个高层方法）
   │   ├─ client.transport: listMessages / listFolders / moveMessages
   │   └─ client.ws:        onMessage({mailId})，断线指数退避 1s→60s 带抖动
   ├─ 内存缓存 cache{users:{user:[摘要×≤20]}, updatedAt, lastError}（只是热缓存，非存储）
   ├─ 低频兜底轮询 pollOnce()（默认 10min，代码强制 ≥5min）
   ├─ 静态占位符 {{VCPClawMailInbox}} / {{VCPClawMailInboxMail1..4}}
   └─ 子邮箱(mail1..4)新邮件 → 自动读信/解析附件 → AgentAssistant 投递绑定 Agent

HTTP: /admin_api/claw-mail/* → pluginManager.getServiceModule('VCPClawMail') 进程内直调
       ↓ HTTPS（API Key 鉴权，非邮箱密码）
ClawEmail 云服务（claw.163.com）→ 真实邮箱 bot@claw.163.com 等
```

本地文件（`Plugin/VCPClawMail/data/`）：`mailbox-cache.json`（摘要快照）、
`submail-processed.json`（子邮箱幂等去重，每槽 500 条）、`attachments/`（工具下载的附件）。

---

## 3. HTTP API 契约（已核实）

Base `/admin_api/claw-mail`，Basic Auth。成功 `{status:'success', ...}`，
错误 `{status:'error', error}`（⚠️ 与 forum 的 `{success}` 包裹不同）；插件未加载 503。

| 方法/路径 | 参数 | 响应 | 插件侧实现 |
| --- | --- | --- | --- |
| GET `/state?refresh=` | refresh=true 先触发 pollOnce | `{status, sdkLoaded, updatedAt, lastError, mailboxes[], users, wsStates[]}` | `getAdminMailboxState` :2088 |
| GET `/messages` | `mailbox`/`user`/`limit`/`unreadOnly`/`fid`/`start`/`order`/`desc` | `{status, meta, emails[], markdown}` | `adminListEmails` :2104 → `listEmails` :714 |
| GET `/messages/:mailId` | `mailbox`/`user`/`markRead`/`includeAttachmentContent`/`maxAttachments` | `{status, meta, markdown, content[]}` | `adminReadMail` :2113 → `readMail` :1029 |
| POST `/messages/:mailId/trash` | body `{mailbox, user, sourceFolderId?, targetFolderId?}`（confirm 强制 true） | `{status, meta, markdown}` | `adminMoveToTrash` :2125 → `moveToTrash` :855 |

行为要点：
- **mailboxes[]** 每项 `{user, mailbox:'public'|'mail1..4', label, agentName|null, enabled, cachedCount}`
  ——移动端账户切换器数据源；
- **分页**：`fid`（默认 1=收件箱）/`start`（offset）/`order`（默认 date）/`desc`（默认 true）
  直接透传 transport.listMessages → **支持 offset 分页、排序、文件夹过滤、仅未读**；
- **详情**：admin 默认 `includeAttachmentContent=false`；传 true 时图片附件以 base64
  `image_url` 块内联 content[]，文档附件（pdf/docx/xlsx/txt 等）解析为纯文本（截断 16000 字符，
  附件上限 25MB）；
- **标读唯一途径**：读详情带 `markRead=true`（未传回落 `ClawMailAutoMarkRead`，默认 false）；
  **无独立标读/标未读路由**；
- **trash 是软删除**：按名称识别垃圾箱文件夹（Trash/Deleted/垃圾箱/已删除），识别不到拒绝执行；
  执行后自动 pollOnce 刷新缓存；
- **不存在的路由**：发送、回复、文件夹列表、附件二进制下载、搜索、移动任意文件夹、恢复、彻底删除；
- 鉴权失败 401；**主动错误凭据按 IP 计数 → 429 + Retry-After**（claw-mail 不在只读白名单内）；
  未配置管理员 503。建议直连主端口（PORT+1 管理进程有 30s 代理超时，多一跳）。

---

## 4. 数据模型字段字典

### 4.1 邮件摘要（`normalizeMailSummary` :462）

| 字段 | 说明 |
| --- | --- |
| `user` | 所属邮箱地址（插件注入） |
| `id` / `mailId` | **详情/操作的唯一键** |
| `subject` | 缺省 `"(无主题)"` |
| `from` / `to` | **形态不稳定**（string/array/object），需兜底渲染 |
| `date` | **未做时区/格式归一**，移动端宽容解析 |
| `read` / `unread` | 可能双双 undefined（SDK 不返回时状态未知） |
| `hasAttachments` / `attachSize` | |
| `preview` | 空白压缩，截断 260 字符 |

### 4.2 邮件详情（`normalizeReadMail` :535）

摘要字段 + `cc`/`bcc`/`text`/`html`/`markdown`（html 经 turndown 有损转换）/
`preview`（600 字符）/`imageUrls[]`/`attachments[]`/`rawKeys`。

- **SDK 已完成 MIME 解码**，移动端消费 `markdown` 字段直接渲染即可；
- ⚠️ **原始 HTML 不通过 HTTP 返回**——复杂排版邮件展示有损（已知限制，V2 上游需求）。

### 4.3 附件元数据（`normalizeAttachmentMeta` :498）

`{id/attachmentId, partId（下载优先）, filename, contentType, size, cid（内联图）, url}`。
⚠️ HTTP 层无二进制下载出口：admin 链路只能拿 base64 内联图或解析后文本；
`downloadAttachment` 只写服务器磁盘返回 `file://` 路径，对移动端不可达 → **上游缺口**。

### 4.4 文件夹与 wsStates

- 文件夹 `{id, fid, name}`；`listFolders` 已实现但无 HTTP 路由；fid=1=收件箱为约定；
- `wsStates[]` 每项 `{user, connected, retries, lastMailAt, lastMailId, lastError...}`
  → 移动端可显示"推送通道在线/掉线"。

---

## 5. 多账户模型

- **公共邮箱池**：`ClawMailUsers`（逗号列表）+ `ClawMailDefaultUser`，**共享一个 ClawMailKey**；
- **子邮箱**：固定 4 槽 mail1-4，各绑一个 Agent（User+Agent 同时配置才启用），
  新邮件自动读信并经 AgentAssistant 投递绑定 Agent；
- **寻址**：所有操作接受 `mailbox=mailN`（优先）或 `user=完整地址`；都不传用默认公共邮箱。

---

## 6. AdminPanel / 桌面端现状

- 仓库面板其实有「Agent 信箱」页（manifest.ts:205，agentContent 组）：state + 邮箱切换 +
  列表（limit/仅未读）+ 详情（固定 markRead=false）+ 移入垃圾箱。
  **用户部署版只有"基础管理"= 部署版本旧于检出版本；后端 API 独立存在，移动端不受此限**。
- 面板不做：发信/回复、已读标记、文件夹切换、附件下载/预览、搜索、分页。
- VCPChat 桌面端：确认完全无邮箱 UI。移动端确实从零起步。

---

## 7. 实时性

- 服务端：WS 推送 `{mailId}` → 立即 pollOnce 刷新缓存；断线退避重连；兜底轮询 ≥5min；
- **对移动端无任何推送出口**（无 SSE/WebSocket/webhook 转发）：
  - MVP：前台 30-60s 轮询 `state?refresh=false`（轻量，读服务端热缓存），比对
    `updatedAt`/未读数；下拉刷新用 `refresh=true` 触发穿透；复用 isBackground 停轮询；
  - V2：上游新增回调/SSE 出口 → Android 本地通知。

---

## 8. 移植路线评估

### 路线 A：经 VCP 后端代理（✅ 推荐）

- 利：与 Agent 看到**同一邮箱视图/已读状态/缓存**，无状态漂移；认证复用 Admin Basic；
  服务端 WS 保证移动端轮询拿到热数据；附件解析/HTML→Markdown 全部白嫖服务端；
  不触碰私有 SDK 协议；
- 弊：发信/回复/附件下载需上游小补丁；详情只有 markdown；服务端宕机邮件功能全灭
  （对"Agent 的邮箱"语义可接受）。

### 路线 B：移动端直连 IMAP/SMTP（❌ 基本不可行）

- claw.163.com 认证是 **API Key 而非邮箱密码**，是否开放标准 IMAP/SMTP 未证实；
- 即便可行：需引入 Android 邮件库自建 MIME/长连保活，已读状态与 Agent 侧漂移，密钥面扩大；
- 变体 B'（直连 ClawEmail HTTP API）：协议私有文档不足、API Key 下发设备不安全——明确不建议。

### 能力缺口与上游补丁（<150 行，风险低）

| 能力 | 现状 | 补救 |
| --- | --- | --- |
| 列表/详情/分页/垃圾箱 | ✅ 现有路由 | — |
| 标读 | ⚠️ 仅详情顺带 | 上游加独立 mark-read/unread |
| **发送/回复** | ❌ 无路由 | 插件 `sendMail`:1254 / `replyMail`:1345 成熟，加路由+admin 包装 |
| 文件夹列表 | ❌ | `listFolders`:803 已实现 |
| 附件下载 | ❌ | 需新增字节流响应的 admin 方法 |
| 推送出口/搜索/星标 | ❌ | V2 上游需求 |

补丁模式：参照现有 `adminListEmails`/`adminReadMail` 包装（每个约 10 行）+
clawMail.js 加 4-5 条路由。VCPToolBox 为用户自部署，可先本地补丁验证再提上游 PR。

---

## 9. 移动端功能范围建议

### MVP（纯现有 API，零上游改动）

1. 邮箱切换：state → mailboxes 线性列表 + wsStates 在线指示 + lastError 横幅；
2. 邮件列表：messages（limit=20 + start 增量加载 + unreadOnly）→ 高密度线性列表
   （发件人/主题/相对时间/未读点/附件标记/preview）；下拉刷新 = `state?refresh=true` + 重拉；
3. 详情页：markdown 渲染（复用消息块渲染管线），默认 `markRead=false`，用户显式操作才标读；
   附件区只读展示元数据；
4. 操作：移入垃圾箱（二次确认）；
5. 新邮件感知：前台 30-60s 轮询 state 比对 updatedAt/未读数；
6. 错误处理：401（凭据错误，停轮询）/429（读 Retry-After）/503（插件未加载专态）。

### V1.1（依赖上游 <150 行补丁）

发送/回复 + 编辑器 UI（reply 传 mailId，服务端自动带原邮件上下文与标读）；
文件夹列表 + fid 切换；附件字节流下载 + 预览/保存；独立标读/标未读。

### V2（上游需求清单）

新邮件 SSE/回调 → 推送通知；全文搜索（SDK mail.search）；星标；移动任意文件夹；
详情返回原始 HTML 保真渲染。

---

## 10. 架构落位（沿用共享架构）

- Rust：`vcp_modules/mail/mail_service.rs`——`mail_state(refresh) / mail_list(分页参数) /
  mail_read(mailId, markRead) / mail_trash(mailId)`；复用 `infra/admin_api` +
  `HttpProfile::AdminApi`；注意响应包裹是 `{status:'success'}` 而非 `{success:true}`，
  错误映射 401/429(Retry-After)/503(PLUGIN_UNAVAILABLE)；
- 前端：`features/mail/{mailTypes.ts, mailStore.ts, MailBoxListView.vue（或内嵌切换器）,
  MailListView.vue, MailDetailView.vue}`；虚拟滚动列表 + 生命周期感知轮询；
- 集成：overlay page type + 右边栏「更多」入口 + 懒加载 latch + 治理测试；
- from/to 宽容渲染、date 宽容解析的工具函数放 mailTypes.ts 并配单测。

---

## 11. 关键文件索引

| 内容 | 路径:行号 |
| --- | --- |
| 插件主体 | `VCPToolBox-main/Plugin/VCPClawMail/VCPClawMail.js`（摘要模型:462, 详情:535, 附件:498, 列表:714, 垃圾箱:855, 详情读:1029, 发送:1254, 回复:1345, 附件下载:1441, 轮询:1523, WS:1580/1826, admin 包装:2073-2153） |
| HTTP 路由 4 条 | `VCPToolBox-main/routes/admin/clawMail.js:32-88`；挂载 `adminPanelRoutes.js:114` |
| 鉴权中间件 | `VCPToolBox-main/server.js:659-835` |
| 配置样例 | `VCPToolBox-main/Plugin/VCPClawMail/config.env.example`（59 行） |
| 面板 API 封装（TS 类型最佳参考） | `AdminPanel-Vue/src/api/clawMail.ts:3-134` |
| 面板「Agent 信箱」页 | `AdminPanel-Vue/src/views/ClawMailManager.vue:169-269`；注册 `app/routes/manifest.ts:205-213` |
