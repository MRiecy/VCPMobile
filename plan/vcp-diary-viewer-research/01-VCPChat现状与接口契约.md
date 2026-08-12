# 01｜VCPChat 现状与接口契约

> 目的：把桌面 VCPMemo 的真实领域模型、调用链和服务接口固化为移动端施工基线。  
> 证据日期：2026-08-12。  
> 结论标签：**已证实**表示当前源码直接证明；**提案**表示移动端策略；**未知**表示必须用真实部署补证。

## 1. 核心结论

桌面 VCPMemo 是一个“文件夹 → memo 文件”的远程文件管理器。每个列表对象代表一个完整的 `.txt` 或 `.md` 文件，点击后整文件读取和编辑。结构化日期、时间、标题来自文件名的可选解析，不是文件内条目模型。

因此：

- 可以有“中心列表 → 阅读 → 编辑”三个 UI 状态；
- 不能宣称服务端存在“文件夹 → 日期文件 → 时间戳条目”的三级数据结构；
- 不能基于未经验证的 `[日期] - 署名` 语法切分、删除或覆盖所谓条目；
- 任意合法文件名、空预览和无法解析的时间戳都必须正常工作。

## 2. 桌面端调用链

### 2.1 入口和窗口

桌面入口不是普通页面路由：

1. 主聊天界面的论坛按钮右键触发 `openMemoWindow`：`VCPChat/modules/event-listeners.js:1185-1193`。
2. preload 把它转换为 `open-memo-window` IPC：`VCPChat/preloads/chat.js:150-159`。
3. Electron 主进程创建独立 BrowserWindow：默认 1200×800，最小 800×600，加载 `Memomodules/memo.html`：`VCPChat/modules/ipc/windowHandlers.js:311-365`。

这解释了桌面布局为何可以长期依赖 260px 侧栏、280px 最小卡片和双栏编辑器；它从未真正解决 360dp 手机宽度。

### 2.2 初始化

`VCPChat/Memomodules/memo.js:112-147` 的启动顺序是：

1. 读取 `vcpServerUrl`；
2. 去掉末尾 `/v1/chat/completions`，生成服务 base URL；
3. 从论坛配置读取 username/password，构造 Basic Authorization；
4. 读取本机 `memo.config.json` 中的隐藏文件夹、折叠分类和顺序；
5. 请求文件夹列表。

`modules/ipc/memoHandlers.js:11-46` 只读写 UI 偏好，不读取或保存日记正文。正文真相始终在外部 VCP 服务。

### 2.3 桌面状态所有权

| 状态 | 当前所有者 | 证据 |
|---|---|---|
| 文件与搜索结果 | 外部 VCP 服务 | 所有正文操作均为 HTTP |
| 当前文件夹、列表、当前 memo、搜索范围 | `memo.js` 文件级全局变量 | `memo.js:6-23` |
| 编辑草稿 | DOM textarea | `memo.html:106-133` |
| 管理 API 凭据 | 论坛配置 | `memo.js:123-129` |
| DailyNote/LightMemo 密钥 | 全局 settings | `memo.js:1037-1065,1142-1167` |
| 隐藏、折叠、排序偏好 | `memo.config.json` | `memoHandlers.js:11-46` |
| 工作台引用 | `DiaryWorkbench` 全局对象 | `memo-workbench.js` |
| 联想图 | `graphState` 全局对象 | `memo-graph.js:6-24` |

## 3. 两套鉴权不能混用

### 3.1 管理 API

桌面 `apiFetch()` 固定调用：

~~~text
{VCP origin}/admin_api/dailynotes{endpoint}
Authorization: Basic base64(username:password)
Content-Type: application/json
~~~

证据：`VCPChat/Memomodules/memo.js:501-520`。

VCPMobile 已经有对应设置：

- `vcpServerUrl`
- `adminUsername`
- `adminPassword`

Rust 侧字段见 `src-tauri/src/vcp_modules/infra/settings_manager.rs:32-54`，现有 Basic Auth 请求范式见 `src-tauri/src/vcp_modules/chat/emoticon_manager.rs:187-238`。

### 3.2 Human Tool API

下列能力走 Bearer `vcpApiKey`：

- DailyNote 创建：`POST /v1/human/tool`
- LightMemo 语义检索：`POST /v1/human/tool`

这不是管理 API 的替代鉴权。特别是“已配置 API Key”不能推出“日记管理接口可用”。

### 3.3 传输边界

Basic Auth 只做编码，不做加密。HTTPS 是默认推荐；HTTP 只能在产品已有的显式 trusted-LAN 模式下使用。移动端不得：

- 在 Vue 参数、日志、Toast、错误详情或遥测中暴露密码与 Authorization；
- 跟随到不同 origin 的重定向后继续携带 Basic Auth；
- 为日记功能另开绕过既有 Release 明文策略的网络通道。

## 4. 管理 API 契约

桌面消费者与当前官方 VCPToolBox 路由共同确认下表。服务端事实来自审计快照中的 [dailyNotesRoutes.js](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/routes/dailyNotesRoutes.js)；生产部署仍需 P0 fixture 验证。

### 4.1 已消费端点

| 方法与路径 | 请求 | 已确认响应/语义 | 首版 |
|---|---|---|---|
| `GET /folders` | 无 | `{ folders: string[] }`；仅顶层、排除符号链接目录 | 必须 |
| `GET /folder/:folder` | 单独编码 folder 段 | `{ notes: NoteSummary[] }`；只列当前目录下 `.txt/.md`，按修改时间倒序 | 必须 |
| `GET /note/:folder/:file` | 两个路径段分别编码 | `{ content: string }`；当前服务端 GET 本身没有正文大小上限 | 必须 |
| `POST /note/:folder/:file` | `{ content: string }` | `{ message }`；整文件覆盖，没有 ETag/revision/If-Match | 必须但有写入门 |
| `GET /search?term=&folder?=` | term；folder 可省略 | `{ notes, total, limited }`，返回仍是文件摘要，不含命中 offset | 必须 |
| `POST /move` | `{ sourceNotes, targetFolder }` | `{ message, moved, errors }`；逐文件可能部分成功，目标重名不覆盖 | 后置 |
| `POST /delete-batch` | `{ notesToDelete }` | `{ deleted, errors }`；逐文件可能部分成功 | 后置 |
| `POST /folder/delete` | `{ folderName }` | 仅允许删除空目录 | 后置 |
| `POST /associative-discovery` | `{ sourceFilePath,k,range,tagBoost }` | 消费端使用 `results`，可有 `warning` | 后置 |

`NoteSummary` 当前可依赖字段只有：

~~~ts
type NoteSummary = {
  name: string
  lastModified: string
  preview: string
  folderName?: string // 搜索结果
}
~~~

未确认字段包括稳定 ID、Agent ID、Tags、正文大小、条目数、搜索 offset、命中行和 revision。

### 4.2 搜索事实

当前官方路由配置：

- term 最长 100 字符；
- 最多取 5 个空白分隔关键词；
- 服务端最大结果数 200；
- 搜索超时 30 秒，排队超时 10 秒，并发槽 2；
- 目录缓存 TTL 15 秒；
- JS fallback 最深遍历 3 层，并跳过超过 1 MiB 的文件；
- 文件夹预览同样在文件超过 1 MiB 时返回“文件过大，无法预览”。

这里的 1 MiB **不是通用读取上限**：单文件 GET 当前直接 `readFile`，没有上限；Rust 搜索器的内部默认也不能由该调用代码完全推出。旧研究稿“读取上限约 1 MB”的表述因此不成立。

搜索响应定位到“文件”，不是文件内条目。移动端可以高亮文件名、摘要，以及打开正文后本地找到的文字，但不能把条目锚点写成服务端现有能力。

### 4.3 保存事实

当前管理路由直接 `writeFile(filePath, content, 'utf-8')` 并失效目录缓存。代码中没有：

- 条件写；
- revision/ETag；
- 临时文件 + rename 的原子替换；
- 显式 fsync；
- 和 DailyNote resident FIFO 的直接协调；
- 和 KnowledgeBaseManager 索引完成的确认响应。

审计快照中的 [DailyNote 插件清单](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/Plugin/DailyNote/plugin-manifest.json) 说明插件 create/update 走单一 FIFO，并与索引队列协调；这不能自动证明 admin raw-save 具有同样语义。P0 必须验证“保存后 RAG 索引是否正确刷新”，否则编辑功能不能验收。

## 5. DailyNote 创建/更新工具

桌面新建表单经 `/v1/human/tool` 调用 DailyNote create，字段为：

- 必填：`maid`、`Date`、`Content`
- 可选：`folder`、`fileName`、`Tag`

证据：`VCPChat/Memomodules/memo.js:1019-1088`；审计快照实现见 [dailynote.js](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/Plugin/DailyNote/dailynote.js)。

VCPMobile 聊天层已经兼容 DailyNote create/update 的消息展示：

- Rust：`content_parser.rs:980-1105`
- Vue：`features/chat/blocks/DiaryBlock.vue`
- 测试：`tests/unit/chat/DailyNoteCompatibility.test.ts`

这项资产只解决“聊天消息如何解析和展示”，不等于 Memo 管理服务。后续可为 DiaryBlock 添加深链，但不能拿它直接读取远端整文件。

## 6. 桌面功能全貌

### 6.1 已有能力

- 文件夹分类：“日记 / 知识库”与名称以“簇”结尾的“思维簇”；
- 过滤 `MusicDiary`；
- 文件夹本地过滤、折叠、隐藏、拖拽排序、删除空目录；
- 文件卡片列表与结构化文件名展示；
- 当前文件夹、全局、LightMemo 语义三种搜索；
- 整文件 Markdown 编辑和预览；
- 创建、删除、批量移动、批量删除；
- 多文档工作台；
- 联想发现和 Canvas 力导向图。

### 6.2 文件名只是展示增强

`memo.js:1555-1599` 可解析：

~~~text
2026-06-01-06_21_33-六一晨间的温柔拥抱.txt
~~~

解析成功时展示格式、标题和时间；失败时原样显示文件名。移动端必须保持同一回退原则，不能把正则匹配成功作为打开文件的前置条件。

## 7. 桌面实现中不可复制的缺陷

### 7.1 竞态

- 文件夹 A→B 快速切换没有 generation，A 的迟到响应可覆盖 B；
- 快速打开两篇 memo 没有 owner 检查，迟到正文可覆盖当前编辑器；
- list/read/refresh/save/delete 无取消或提交门；
- 只有搜索使用 AbortController，语义搜索额外做 controller identity 检查；
- 删除后没有 tombstone，迟到 list/get 可以把已删对象重新显示。

### 7.2 编辑一致性

- 保存是 blind last-write-wins；
- 没有 dirty guard；
- 没有保存冲突；
- 保存超时无法判断“未执行”还是“已写入但响应丢失”；
- 标题输入框看似可编辑，但保存仍使用原 `folder/file`，实际不支持 rename；
- 创建成功后固定等待 1 秒刷新，不是提交完成协议。

### 7.3 错误处理

`memo.js:64-71` 全局重写 `window.alert` 为启动阻断函数。保存、读取或表单校验的普通错误可能把整个 Memo 标成“初始化未完成”。移动端必须使用局部、可恢复、按 operation 分类的错误状态。

### 7.4 渲染安全

`memo.js:907-938` 把 `marked.parse(content)` 直接写入 `innerHTML`；`memo.html:6` 又允许 `unsafe-inline` 和 `connect-src *`。若照搬到 Tauri WebView，恶意日记或错误响应可形成主 DOM 注入面。

VCPMobile 的 `astRenderer.ts:27-31` 也明确说明其 active-content guard 面向 trusted-circle 聊天内容，并非通用 sanitizer。远程日记必须使用独立严格净化策略，详见 03。

### 7.5 移动性能与视觉

- 窗口最小 800×600，没有响应式 `@media`；
- 网格卡片最小宽 280px、最小高 180px；
- 侧栏 blur 12px、编辑遮罩 blur 20px、工作台 blur 25px；
- 大量大圆角、厚阴影、hover/scale、逐卡淡入；
- 联想图持续 requestAnimationFrame，并人为增加 800ms“算力感”延迟；
- 工作台串行读取全部全文并一次性拼接。

这些属于桌面氛围或技术债，不是移动端功能契约。

## 8. 旧版排版报告勘误

| 旧稿主张 | 当前裁决 |
|---|---|
| 文件夹 → 日期文件 → 时间戳条目 | 改为文件夹 → memo 文件 → 不透明正文 |
| 文件内固定 `[日期] - 署名` | 无事实依据；只能作为未来 fixture 驱动的可选解析 |
| 单文件读取约 1 MiB 上限 | 错；预览/JS 搜索 fallback 有边界，GET 没有 |
| 搜索可直接定位条目 | 错；响应没有 offset 或条目 DTO |
| 已有 URL/API key 即可 | 错；管理 API 还需 Basic admin credentials |
| Tags、文件数、条目数、最近摘要可直接展示 | `/folders` 只有字符串，不能凭空补数据或制造 N+1 |
| Agent 卡片可直达对应日记本 | 没有稳定 Agent ID → folder 映射 |
| 编辑不在范围 | 与本次“查看、编辑 Agent 记忆”目标冲突 |
| 必须引入 Vue Virtual Scroller | 无测量依据；项目已依赖 `useVirtualList`，但首版甚至未必需要窗口化 |
| 阅读偏好进入 Delta Sync | 当前 settings 不属于 Sync V2 实体 |
| viewer=70、editor=80 | 当前源码实际是 editor=70、viewer=80；施工使用语义层名和当前源码 |

## 9. 从桌面提炼出的移动端硬需求

### 必须保留

- Basic Auth 下的文件夹/文件浏览；
- 任意文件名回退；
- 普通文件级搜索；
- 完整正文阅读；
- 整文件编辑，但增加冲突与 dirty 保护；
- 明确刷新、空态、认证态和可恢复错误；
- 结构化文件名仅作展示增强。

### 改造后保留

- 文件夹侧栏 → 当前文件夹选择层；
- 卡片网格 → 高密度线性列表；
- 双栏编辑/预览 → 单栏双态；
- 右键 → 显式更多菜单或长按；
- 联想图 → 后续相关 memo 线性列表；
- 本地隐藏/排序 → 有真实需求后再恢复。

### 后置

- DailyNote 创建；
- 删除、移动、批量模式；
- 语义搜索；
- 工作台；
- 联想发现；
- 条目切分；
- 离线缓存和跨设备偏好。

## 10. 参考源

- [VCPToolBox 官方仓库](https://github.com/lioensky/VCPToolBox)
- [审计快照 dailyNotesRoutes.js](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/routes/dailyNotesRoutes.js)
- [审计快照 DailyNote 实现](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/Plugin/DailyNote/dailynote.js)
- [审计快照 DailyNote manifest](https://github.com/lioensky/VCPToolBox/blob/1ae9b63c5afcea7677db5d71e5cf561a0f5debd9/Plugin/DailyNote/plugin-manifest.json)

外部官方源码会继续演进；P0 仍以实际部署响应为最高事实源。
