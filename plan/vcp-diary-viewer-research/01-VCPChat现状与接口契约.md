# 01｜VCPChat 现状与接口契约

> 目的：把桌面 VCPMemo 的真实领域模型、调用链和服务接口固化为移动端施工基线。  
> 证据日期：2026-08-12。  
> 结论标签：**已证实**表示当前源码直接证明；**提案**表示移动端策略；**未知**表示必须用真实部署补证。

本轮末次复核以官方最新线性版本为准：VCPToolBox `351dadc7`、VCPChat `856c1db0`。VCPChat 最新 Memo 相关文件与本地 `29ab88c` 快照 SHA-256 相同；VCPToolBox 的本轮相关文件与此前审计快照无差异，服务端契约仍以最新源码为最高静态证据。

## 1. 核心结论

桌面 VCPMemo 是一个“文件夹 → memo 文件”的远程文件管理器。每个列表对象代表一个完整的 `.txt` 或 `.md` 文件，点击后整文件读取和编辑。结构化日期、时间、标题来自文件名的可选解析，不是文件内条目模型。

因此：

- 可以有“中心列表 → 阅读 → 编辑”三个 UI 状态；
- 不能宣称服务端存在“文件夹 → 日期文件 → 时间戳条目”的三级数据结构；
- DailyNote create 会生成 `[日期] - 署名` 前缀，但管理 API 与人工编辑允许任意整文件正文；不能把生成器惯例升级为所有文件的条目 schema；
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

桌面消费者与当前官方 VCPToolBox 路由共同确认下表。服务端事实来自审计快照中的 [dailyNotesRoutes.js](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/routes/dailyNotesRoutes.js)；生产部署仍需 P0 fixture 验证。

### 4.1 已消费端点

| 方法与路径 | 请求 | 已确认响应/语义 | 首版 |
|---|---|---|---|
| `GET /folders` | 无 | `{ folders: string[] }`；仅顶层、排除符号链接目录 | 必须 |
| `GET /folder/:folder` | 单独编码 folder 段 | `{ notes: NoteSummary[] }`；只列当前目录下 `.txt/.md`，按修改时间倒序 | 必须 |
| `GET /note/:folder/:file` | 两个路径段分别编码 | `{ content: string }`；当前服务端 GET 本身没有正文大小上限 | 必须 |
| `POST /note/:folder/:file` | `{ content: string }` | `{ message }`；整文件覆盖，没有 ETag/revision/If-Match | 必须但有写入门 |
| `GET /search?term=&folder?=` | term；folder 可省略 | `{ notes, total, limited }`，返回仍是文件摘要，不含命中 offset | 必须 |
| `POST /move` | `{ sourceNotes, targetFolder }` | `{ message, moved, errors }`；逐文件可能部分成功，目标重名不覆盖 | 必须 |
| `POST /delete-batch` | `{ notesToDelete }` | `{ deleted, errors }`；逐文件可能部分成功 | 必须 |
| `POST /folder/delete` | `{ folderName }` | 仅允许删除空目录 | 必须 |
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

### 4.3 管理 mutation 与索引事实

最新管理路由的 save/move/delete 不再是孤立文件操作。`routes/admin/dailyNotes.js` 从 `vectorDBManager` 绑定 `runExternalFileMutation` 并注入通用路由；该方法由 KnowledgeBaseManager 转交 DatabaseCoordinator。文件变化经单一外部 mutation FIFO 落盘，再把 `mutationPaths.upserts/deletes` 交给 SQLite/Rust 索引批处理。

管理路由显式传入 `waitForIndex: false`：HTTP 在文件 mutation 完成后即可返回，内部尾队列继续调度索引。因此契约是：

- HTTP 2xx：文件 mutation 已完成；
- 索引：已进入最终一致调度，但响应不携带完成 revision 或 index receipt；
- 紧接着发起 LightMemo 语义搜索时，可能短暂读到旧索引；
- Mobile 可以显示“文件已保存”，并在语义视图标记“索引同步中/可重试”，不再以“索引是否会更新未知”阻断编辑。

当前保存仍是 `writeFile(filePath, content, 'utf-8')`。代码中没有：

- 条件写；
- 临时文件 + rename 的原子替换；
- 显式 fsync；
- ETag、revision 或 `If-Match`；
- 和 KnowledgeBaseManager 索引完成同步返回的确认 receipt。

服务端协调证据见 [dailyNotesRoutes.js](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/routes/dailyNotesRoutes.js)、[admin dailyNotes 注入](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/routes/admin/dailyNotes.js) 与 [DatabaseCoordinator](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/modules/knowledgeBase/databaseCoordinator.js)。部署验收仍需测量索引追平延迟，但这不再是未知架构能力。

### 4.4 已确认错误响应

| 状态 | 最新上游语义 | Mobile 归一化 |
|---:|---|---|
| 400 | 缺搜索词、请求体非法、非空文件夹删除 | `DIARY_INVALID_REQUEST` |
| 401 | Basic 凭据缺失或错误，JSON `{ error: 'Unauthorized' }` | `DIARY_AUTH_REQUIRED` |
| 403 | IP 黑名单、路径/符号链接拒绝 | `DIARY_FORBIDDEN` |
| 404 | 文件夹或文件不存在 | `DIARY_NOT_FOUND` |
| 429 | 登录失败导致临时封禁，带 `Retry-After` | `DIARY_RATE_LIMITED` |
| 499 | 搜索因客户端断开/取消 | 本地主动取消不报错 |
| 500 | 文件操作或搜索内部错误，可带 `details` | `DIARY_SERVER_ERROR` |
| 503 | admin 凭据未在服务端配置，或搜索并发队列已满 | `DIARY_SERVICE_UNAVAILABLE` |
| 504 | 搜索 30 秒超时 | `DIARY_TIMEOUT` |

服务端 `details/message` 可以进入受控错误摘要，但文件名、错误文本不通过 `innerHTML` 渲染。

## 5. DailyNote 创建/更新工具

桌面新建表单经 `/v1/human/tool` 调用 DailyNote create，字段为：

- 必填：`maid`、`Date`、`Content`
- 可选：`folder`、`fileName`、`Tag`

证据：`VCPChat/Memomodules/memo.js:1019-1088`；审计快照实现见 [dailynote.js](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/Plugin/DailyNote/dailynote.js)。

最新 [ToolCallParser](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/modules/vcpLoop/toolCallParser.js) 已提供正式 ESCAPE 字段协议：字段可用 `「始ESCAPE」...「末ESCAPE」` 包裹，工具块字面量使用 `<<<[TOOL_REQUEST_ESCAPE]>>>` / `<<<[END_TOOL_REQUEST_ESCAPE]>>>`。Mobile 创建与 LightMemo 调用必须复用这套协议和 fixture，不再把正文换行、引号或普通 `「始」「末」` 视为未定义行为。包含 ESCAPE 自身结束标记的极端正文仍应进入边界测试。

VCPMobile 聊天层已经兼容 DailyNote create/update 的消息展示：

- Rust：`content_parser.rs:980-1105`
- Vue：`features/chat/blocks/DiaryBlock.vue`
- 测试：`src/tests/unit/chat/DailyNoteCompatibility.test.ts`

这项资产只解决“聊天消息如何解析和展示”，不等于 Memo 管理服务。现有 create 块中的 `file_name` 是请求后缀而非服务端最终文件名，不能据此打开远端整文件；当前版本不提供聊天块深链。未来只有在工具结果可被可靠关联且显式给出最终 `folder + fileName` 时才接入精确 key，仍不得按 Agent 名称猜测。

## 6. 桌面功能全貌

### 6.1 已有能力

- 长期稳定的文件夹分类：“日记 / 知识库”与名称以“簇”结尾的“思维簇”；
- 桌面当前硬过滤 `MusicDiary`，另有本机 `hiddenFolders` 偏好；两者都是桌面消费端行为，不作为 Mobile 的目录真相；
- 文件夹本地过滤、折叠、隐藏、拖拽排序、删除空目录；
- 文件卡片列表与结构化文件名展示；
- 当前文件夹、全局、LightMemo 语义三种搜索；
- 整文件 Markdown 编辑和预览；
- 创建、删除、批量移动、批量删除；
- 多文档工作台；
- 联想发现和 Canvas 力导向图。

LightMemo 当前命令与参数以最新 [plugin manifest](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/Plugin/LightMemo/plugin-manifest.json) 为准：`SearchRAG` 默认 `k=5`，并支持 `maid`、`folder`、`search_all_knowledge_bases`、rerank 与 `tag_boost`。manifest 的默认 `EXCLUDED_FOLDERS` 是 `已整理,夜伽,MusicDiary`，定义为服务端语义检索排除配置；Mobile 不复制、不改写，也不能用本机“取消隐藏”绕过它。当前服务端另有显式 `[音乐检索]` 特例，会由 LightMemo 自己把 `MusicDiary` 从本次排除集合移出；这仍是服务端裁决，不是 Mobile 隐藏偏好的副作用。

这里必须区分三条规则：当前管理路由 `GET /admin_api/dailynotes/folders` 枚举日记根目录下全部非符号链接文件夹，并不会应用 LightMemo 的 `EXCLUDED_FOLDERS`；普通 `/search` 只使用管理路由注入的 `VectorStore,DebugLog` 忽略项；LightMemo 才在候选 SQL 中应用自身排除规则及服务端特例，并额外排除 `已整理*` 与 `*簇`。因此 Mobile 始终先服从对应服务端响应，再用用户主动设置的 `hiddenFolders` 对普通浏览、普通搜索和语义结果做附加展示过滤。`hiddenFolders` 默认空，不由 `EXCLUDED_FOLDERS` 填充，客户端也不硬编码 `MusicDiary`；“取消隐藏”只能撤销本机过滤，不能取回服务端从未返回的语义结果。

当前 VCPToolBox 没有“只读 LightMemo 生效排除目录”的窄 DTO；`GET /admin_api/plugins` 返回的是全插件清单及插件配置文本，可能夹带与 Diary 无关的敏感配置，也不能作为 LightMemo 运行时有效值的专用契约。Mobile 不为预判目录而消费它：服务端实际返回的 LightMemo 结果就是允许范围的权威上限。若未来 UI 需要提前标出或禁用被排除目录，应先在 VCPToolBox 增加目的明确、去敏后的只读契约。

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
- 用户已在桌面运行态确认文件名可以重命名；此前仅凭 `handleSaveMemo()` 未读取标题输入框就断言“没有重命名”是错误的证据外推；
- 创建成功后固定等待 1 秒刷新，不是提交完成协议。

需要保留一个实现层事实：最新 tracked VCPChat `handleSaveMemo()` 仍只 POST 原 `{folder,file}`，最新 VCPToolBox 也没有独立 rename endpoint。也就是说，“重命名是必须复原的产品能力”已经确认，但不能据此虚构服务端原子 rename 契约。Mobile 施工时先用运行态请求 fixture 找到真实调用；若部署确实没有专用端点，则用“写入新文件 → 校验 → 删除旧文件”的可恢复 transaction 实现，并显式处理重名及“新旧文件并存”的部分成功。

### 7.3 错误处理

`memo.js:64-71` 全局重写 `window.alert` 为启动阻断函数。保存、读取或表单校验的普通错误可能把整个 Memo 标成“初始化未完成”。移动端必须使用局部、可恢复、按 operation 分类的错误状态。

### 7.4 可信 HTML 边界

本产品明确把用户自有 VCP 服务中的日记正文视为可信内容。Mobile 复刻桌面语义：允许 Markdown 中的 raw HTML，经 `marked.parse(content)` 后直接写入 `v-html`/`innerHTML`；最多复用现有轻量过滤，不新增 Diary 专属严格白名单，不剥离用户主动保存的 HTML。

这条渲染链是 Diary feature 自己的正文契约，不借用聊天消息的解析或渲染链。

信任范围只覆盖正文。folder/file、列表 preview、搜索元信息和服务端错误仍通过 Vue 文本绑定或 `textContent` 展示，不能拼入 HTML。该边界是显式产品决定，而不是把所有网络响应都视为可信。

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
| 文件内固定 `[日期] - 署名` | DailyNote create 会生成该前缀，但 admin/人工写入可生成任意正文；不能作为通用条目 schema |
| 单文件读取约 1 MiB 上限 | 错；预览/JS 搜索 fallback 有边界，GET 没有 |
| 搜索可直接定位条目 | 错；响应没有 offset 或条目 DTO |
| 已有 URL/API key 即可 | 错；管理 API 还需 Basic admin credentials |
| Tags、文件数、条目数、最近摘要可直接展示 | `/folders` 只有字符串，不能凭空补数据或制造 N+1 |
| Agent 卡片可直达对应日记本 | Agent 与日记本相互独立；公共日记本可由多个 Agent 和用户共同写入，不能按 Agent 名猜 folder |
| 编辑不在范围 | 与本次“查看、编辑 Agent 记忆”目标冲突 |
| 必须引入 Vue Virtual Scroller | 不新增依赖；已知单目录可达数千文件，首版直接复用项目已有 `useVirtualList` |
| 阅读偏好进入 Delta Sync | 当前 settings 不属于 Sync V2 实体 |
| viewer=70、editor=80 | 当前源码实际是 editor=70、viewer=80；施工使用语义层名和当前源码 |

## 9. 从桌面提炼出的移动端硬需求

### 必须保留

- Basic Auth 下的文件夹/文件浏览；
- 任意文件名回退；
- 普通文件级搜索；
- LightMemo 语义搜索；
- 完整正文阅读；
- 整文件编辑与文件名重命名，并增加冲突、dirty 与部分成功保护；
- DailyNote 创建、移动、删除、批量管理和空文件夹删除；
- 本机隐藏/恢复、折叠和排序管理；`hiddenFolders` 默认空，不硬编码 `MusicDiary`，也不改变服务端语义排除范围；
- 明确刷新、空态、认证态和可恢复错误；
- 结构化文件名仅作展示增强。

### 改造后保留

- 文件夹侧栏 → 当前文件夹选择层；
- 卡片网格 → 高密度线性列表；
- 双栏编辑/预览 → 单栏双态；
- 右键 → 显式更多菜单或长按；
- LightMemo 桌面隐式范围 → 移动端明确的语义搜索入口与 scope；
- 批量悬浮条 → 移动端选择态与底部动作栏；
- 联想图 → 后续相关 memo 线性列表。

### 后置

- 工作台；
- 联想发现与图谱；
- 条目切分；
- 离线缓存和跨设备偏好。

## 10. 参考源

- [VCPChat Memo 审计快照](https://github.com/lioensky/VCPChat/blob/856c1db0404ebff0365aea8b16fdc0a4a68f9d5e/Memomodules/memo.js)
- [VCPToolBox 官方仓库](https://github.com/lioensky/VCPToolBox)
- [审计快照 dailyNotesRoutes.js](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/routes/dailyNotesRoutes.js)
- [审计快照 DailyNote 实现](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/Plugin/DailyNote/dailynote.js)
- [审计快照 DailyNote manifest](https://github.com/lioensky/VCPToolBox/blob/351dadc74836ebf78d25fa942619cd34d9c82987/Plugin/DailyNote/plugin-manifest.json)

外部官方源码会继续演进；P0 仍以实际部署响应为最高事实源。
