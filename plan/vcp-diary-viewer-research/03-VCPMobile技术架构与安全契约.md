# 03｜VCPMobile 技术架构与安全契约

> 架构目标：一个 Rust typed service、一个 Composition Pinia Store、一个懒加载 Diary Feature。  
> 约束：不新增 Router、SQLite 日记库、Sync V2 实体、Android 插件命令或运行时依赖。

## 1. 总体数据流

~~~mermaid
flowchart LR
    UI[DiaryCenterView<br/>列表/阅读/编辑] --> STORE[useDiaryStore<br/>唯一前端状态所有者]
    STORE -->|Tauri invoke: typed args| CMD[Diary commands]
    CMD --> SERVICE[DiaryServiceState<br/>reqwest + search owners + mutation gate]
    SERVICE --> SETTINGS[SettingsState<br/>URL/admin credentials]
    SERVICE -->|Basic Auth| ADMIN[VCP admin_api/dailynotes]
    SERVICE -->|Bearer + Tool ESCAPE| TOOL[VCP v1/human/tool<br/>DailyNote / LightMemo]
    ADMIN -->|bounded JSON| SERVICE
    SERVICE -->|typed DTO / stable error code| STORE
    STORE --> UI
~~~

边界：

- **VCP 服务**：远端文件最终事实源；
- **Rust**：凭据、URL、HTTP、大小/超时、schema、hash、保存冲突与响应归一化；
- **Pinia Store**：当前视图、资源数据、请求 generation、搜索意图和编辑草稿；
- **Vue 组件**：展示、可访问交互、可信 Markdown/raw HTML 直接渲染；
- **overlayStore / ModalHistory**：唯一全局页面和返回键所有者。

## 2. 模块落点

### 2.1 Rust

Diary 是独立远端文件管理领域，不挂在 chat 的消息解析/渲染模块之下：

~~~text
src-tauri/src/vcp_modules/diary/
├─ mod.rs                     # 领域声明与最小 re-export
├─ diary_service.rs           # HTTP、校验、hash、owners、commands、纯逻辑测试
└─ diary_types.rs             # DTO 与稳定错误类型

src-tauri/src/vcp_modules/mod.rs  # 声明 diary 领域并做最小 facade re-export
src-tauri/src/lib.rs              # manage state + command 注册
~~~

`lib.rs` 只做状态挂载和 command 路由，不承载业务逻辑。

这些是普通 Tauri commands，不属于 `tauri-plugin-vcp-mobile`。因此不需要 Android Kotlin、插件 `build.rs`、permissions TOML 或 guest-js 四重注册。

### 2.2 Vue

建议的首版文件边界：

~~~text
src/features/diary/
├─ DiaryCenterView.vue        # 唯一全局 SlidePage，组织内部视图
├─ diaryStore.ts              # 一个 Composition Pinia Store
├─ types.ts                   # command DTO 与 UI view model
├─ diaryMarkdown.ts           # marked 配置与可信 HTML 渲染 helper
└─ components/
   ├─ DiaryFolderSheet.vue
   ├─ DiaryNoteList.vue
   ├─ DiaryReader.vue
   ├─ DiaryEditor.vue
   ├─ DiaryManager.vue
   └─ DiaryComposer.vue
~~~

接线修改：

- `src/core/stores/overlay.ts`
- `src/components/FeatureOverlays.vue`
- `src/components/layout/RightSidebar.vue`

`FeatureOverlays.vue` 使用 `defineAsyncComponent`，确保 Diary chunk 只在第一次打开时加载。

## 3. Rust 服务状态

建议显式注册：

~~~rust
DiaryServiceState {
    http_client,
    mutation_gate,
    active_text_search_owner,
    active_semantic_search_owner,
}
~~~

职责：

- `http_client`：连接池、redirect policy、connect/total timeout；
- `mutation_gate`：串行化本 Mobile 进程的 save/rename/create/move/delete mutation；
- 两类 search owner：普通搜索与 LightMemo 语义检索分别取消旧请求，并确保旧 owner 不能清理新状态。

它**不持有**当前 folder、当前 document、列表缓存或编辑草稿。VCPToolBox 本身把外部 mutation 串行入队；Mobile 使用一个全局 mutation gate 与服务端所有权保持一致，避免无界 `NoteKey → Mutex` 锁表。

所有网络和文件 I/O 必须 async；Tauri command 中不使用 `unwrap()` 或 `expect()`。

## 4. Command 与 DTO

### 4.1 当前里程碑 commands

| Command | 参数 | 返回 | 备注 |
|---|---|---|---|
| `diary_list_folders` | 无 | `DiaryFolderList` | 不伪造统计 |
| `diary_list_notes` | `folder` | `DiaryNoteSummary[]` | 保留服务端顺序 |
| `diary_get_note` | `DiaryNoteKey` | `DiaryDocument` | 返回 raw content + SHA-256 |
| `diary_search` | `requestId, term, folder?` | `DiarySearchResponse` | 新 owner 取消旧 owner |
| `diary_cancel_search` | `requestId?` | `()` | 页面关闭或清空搜索 |
| `diary_semantic_search` | `requestId, query, folder?, searchAll, k` | `DiarySemanticResponse` | Bearer LightMemo，显式提交 |
| `diary_cancel_semantic_search` | `requestId?` | `()` | 与普通搜索独立 |
| `diary_save_note` | `DiarySaveRequest` | `DiarySaveOutcome` | 复读、冲突、保存、验证 |
| `diary_rename_note` | `source, targetFile, baselineHash` | `DiaryRenameOutcome` | 专用端点优先，否则可恢复 transaction |
| `diary_create_note` | `DiaryCreateRequest` | `DiaryCreateOutcome` | DailyNote + Tool ESCAPE |
| `diary_move_notes` | `sources, targetFolder` | `DiaryBatchOutcome` | 保留 partial success |
| `diary_delete_notes` | `sources` | `DiaryBatchOutcome` | 保留 partial success |
| `diary_delete_empty_folder` | `folder` | `()` | 只删空目录 |

### 4.2 后续 command

- `diary_associative_discovery`

LightMemo 是当前范围，必须有独立 DTO、Bearer 配置态与取消生命周期；输出中的 chunk 先归一化为 `folder + file` 文件结果，再按本机 `hiddenFolders` 做附加过滤。服务端结果是语义允许范围的权威上限；禁止为了镜像 `EXCLUDED_FOLDERS` 去消费会返回全插件配置文本的 `/admin_api/plugins`。联想发现和工作台仍后置。

### 4.3 DTO 草案

~~~ts
type DiaryNoteKey = {
  folder: string
  file: string
}

type DiaryNoteSummary = DiaryNoteKey & {
  lastModified: string
  preview: string
}

type DiaryDocument = {
  key: DiaryNoteKey
  content: string
  contentHash: string
}

type DiarySearchResponse = {
  notes: DiaryNoteSummary[]
  total: number
  limited: boolean
}

type DiarySaveRequest = {
  key: DiaryNoteKey
  content: string
  baselineHash: string
  force: boolean
}

type DiarySaveOutcome = {
  contentHash: string
  verified: boolean
}

type DiarySemanticHit = {
  key: DiaryNoteKey
  preview: string
  score?: number
}

type DiarySemanticResponse = {
  hits: DiarySemanticHit[]
  indexMayBeCatchingUp: boolean
}

type DiaryRenameOutcome = {
  key: DiaryNoteKey
  contentHash: string
  status: 'renamed' | 'copied_source_retained'
}

type DiaryCreateRequest = {
  maid: string
  date: string
  folder?: string
  fileNameSuffix?: string
  tag?: string
  content: string
}

type DiaryCreateOutcome = {
  key: DiaryNoteKey
  indexStatus: 'queued'
}

type DiaryBatchOutcome = {
  succeeded: DiaryNoteKey[]
  errors: Array<{ key: DiaryNoteKey; message: string }>
}
~~~

不把未经确认的 `agentId`、`entryCount`、`matchOffset` 或 `revision` 放进文件 DTO。`Tag` 只属于 DailyNote 创建参数；LightMemo chunk 必须先折叠成文件命中，不能成为可编辑实体。

## 5. 错误契约

内部使用 typed `DiaryError`；Tauri adapter 依照项目约定映射为 `Result<T, String>`。字符串使用稳定前缀，Vue 不解析自然语言：

| code | 含义 | UI |
|---|---|---|
| `DIARY_CONFIG_MISSING` | URL、对应管理员凭据或语义检索 API Key 缺失 | 前往设置 |
| `DIARY_INVALID_REQUEST` | 400 或本地字段校验失败 | 对应字段就地提示 |
| `DIARY_AUTH_REQUIRED` | 401 | 检查对应 Basic/Bearer 凭据 |
| `DIARY_FORBIDDEN` | 403 | 路径、符号链接或 IP 被拒绝 |
| `DIARY_NOT_FOUND` | folder/file 已不存在 | 返回列表并刷新 |
| `DIARY_RATE_LIMITED` | 429，可能带 Retry-After | 倒计时后再试 |
| `DIARY_CONFLICT` | 远端 hash 已不同 | 保留草稿，进入冲突动作层 |
| `DIARY_TIMEOUT` | connect/total/idle timeout | 重试；mutation 进入结果核验 |
| `DIARY_TRANSPORT` | DNS、TLS、断网 | 保留已有数据 |
| `DIARY_RESPONSE_TOO_LARGE` | 超过客户端预算 | 不截断、不渲染 |
| `DIARY_INVALID_RESPONSE` | JSON/schema/UTF-8 不合法 | 可重试并记录安全摘要 |
| `DIARY_SERVICE_UNAVAILABLE` | 503，服务未配置或搜索队列满 | 按原因设置/重试 |
| `DIARY_SERVER_ERROR` | 其他 5xx | 局部错误 |
| `DIARY_SAVE_UNCERTAIN` | 无法确认保存结果 | 保留草稿，禁止宣称成功 |
| `DIARY_PARTIAL_SUCCESS` | move/delete 部分成功 | 成功项退出，失败项保留 |
| `DIARY_RENAME_SOURCE_RETAINED` | 新文件已验证、旧文件删除失败 | 明确显示两份文件均存在 |
| `DIARY_TOOL_ERROR` | DailyNote/LightMemo 返回插件错误 | 保留输入并允许重试 |

日志只记录 code、HTTP status、operation ID 和脱敏 target hash；不记录 Authorization、密码、完整正文、完整错误 body 或带敏感 query 的 URL。

## 6. URL、认证与重定向

Rust 每次 command 从 `SettingsState` 读取配置快照：

1. 解析 `vcpServerUrl`，只接受 `http`/`https`；
2. 拒绝 embedded username/password、NUL、控制字符；
3. 去掉 query 与 fragment；
4. 移除已知末尾 `/v1/chat/completions`；
5. 保留 `/v1/chat/completions` 之前的反向代理 path prefix，不能只取 origin；
6. 使用 URL path-segment API 拼接固定 `admin_api/dailynotes`；
7. folder/file 各自作为单一 segment 编码，并在客户端防御性拒绝空值、`.`、`..`、`/`、`\`、绝对路径形式。

客户端校验不替代服务端路径防护。

`reqwest` redirect policy 设为禁止或只允许 same-origin；任何跨 origin 重定向都不能携带 Basic Auth。

## 7. HTTP 预算

已知正文为数百字节至数十 KiB、平均数 KiB，单目录为数百至数千文件。下表按这一产品事实留出数量级余量，同时防止异常响应耗尽 WebView 内存：

| 项目 | 初始值 |
|---|---:|
| connect timeout | 10s |
| list/read/save total timeout | 30s |
| search total timeout | 40s（服务端自身 30s） |
| streaming idle timeout | 15s |
| 解码后单篇正文 | 2 MiB |
| list/search JSON body | 8 MiB |
| error body | 64 KiB |
| 单次 folder summary 数量 | 10000 |
| 普通搜索结果 | 200（服务端硬上限） |
| LightMemo 结果 k | 跟随上游默认 5，客户端硬上限 50 |
| 编辑提交正文 | 2 MiB UTF-8 bytes |

每个响应同时检查：

- `Content-Length`（若有）；
- `bytes_stream` 实际累计长度；
- checked-add 溢出；
- JSON 解码后的数组数量和正文长度。

超过边界必须 fail closed，不能静默截断后允许编辑保存。现有可复用范式见 `message_service.rs:20-115`。

单目录已知可达数千文件，`DiaryNoteList` 从首版起复用 `@vueuse/core/useVirtualList`：固定 84px 行高、稳定 key=`folder + file`、适量 overscan；不新增虚拟滚动依赖，也不对正文做窗口化。

## 8. 前端 Store 所有权

`useDiaryStore` 只保留一套状态：

~~~text
navigation:
  screen = list | reader | editor | preview | manager | composer

resources:
  folders
  selectedFolder
  notes
  textSearch { query, scope, results, limited }
  semanticSearch { query, scope, results, state }
  document { key, content, contentHash }

editor:
  draft
  dirty
  saveState

management:
  selectedKeys
  tombstones
  activeMutation

local preferences:
  hiddenFolders      # default empty; local discovery filter only
  collapsedCategories
  folderOrder

request ownership:
  foldersGeneration
  notesGeneration + targetFolder
  textSearchGeneration + requestId
  semanticSearchGeneration + requestId
  documentGeneration + targetKey
~~~

不要为 shelf、reader、editor 分三个 Store；也不要把全局页面栈复制进 Diary Store。

正文、搜索结果和草稿不写入 localStorage/Pinia persist，只在内存保留当前 document、baseline 和 draft。`hiddenFolders`、`collapsedCategories` 与 `folderOrder` 是本机显示偏好，可用现有 Pinia persist 单独持久化，不进入 Delta Sync；其中 `hiddenFolders` 默认空，对文件夹列表、普通搜索和 LightMemo 已返回结果做附加展示过滤，但不充当访问控制，显式 `{folder,file}` 深链仍按服务端鉴权裁决。语义搜索先使用 LightMemo 已执行 `EXCLUDED_FOLDERS` 后的结果，Mobile 不复制或改写服务端排除配置，本机恢复也不能扩大服务端语义范围。若未来需要崩溃恢复，单独设计有上限的 draft 表，绝不演化成离线 mutation queue。

## 9. 异步提交门

每个读请求都遵循：

1. 开始时递增对应 generation；
2. 捕获 generation 和完整 target identity；
3. await 后同时检查 generation 与当前 target；
4. 只有当前 owner 可以提交 success；
5. 迟到 error 静默丢弃；
6. `finally` 也必须检查 owner，不能清掉新请求的 loading；
7. 页面关闭、切 folder、切 file 时使旧 generation 失效。

伪流程：

~~~text
owner = ++notesGeneration
target = selectedFolder
loading = true

result = await invoke(...)

if owner != notesGeneration or target != selectedFolder:
    discard
else:
    atomically replace notes

if owner == notesGeneration:
    loading = false
~~~

刷新时保留旧内容，成功后原子替换，避免闪白。项目已有 `LatestIntentOwner` 和 Topic/Sync generation 模式可参考；当前 utility 的 ID 前缀是 `local-share`，应复用机制而不是生搬名称。

普通搜索和 LightMemo 分别需要 Rust active owner/cancellation。普通搜索可能占用最长 30 秒的服务端扫描；语义检索还有 embedding/RAG 延迟。新请求取消同类旧 token；完成清理必须再次核对 owner。

## 10. 保存与冲突

### 10.1 Best-effort 保存协议

~~~mermaid
sequenceDiagram
    participant UI as DiaryEditor
    participant Store as diaryStore
    participant Rust as diary_service
    participant VCP as VCP server

    UI->>Store: save(draft, baselineHash)
    Store->>Rust: diary_save_note(snapshot)
    Rust->>Rust: acquire mutation gate
    Rust->>VCP: GET current content
    VCP-->>Rust: current
    Rust->>Rust: hash(current)
    alt hash differs and force=false
        Rust-->>Store: DIARY_CONFLICT
        Store-->>UI: keep draft, show conflict actions
    else baseline matches or force=true
        Rust->>VCP: POST full content
        VCP-->>Rust: response
        Rust->>VCP: GET read-back
        VCP-->>Rust: persisted content
        Rust->>Rust: verify candidate hash
        Rust-->>Store: new contentHash + verified
        Store-->>UI: update baseline, dirty=false
    end
~~~

请求开始前固定不可变快照：

~~~text
{ NoteKey, draft, baselineHash, force, operationId }
~~~

完成回调不能重新读取“当前文件”全局状态来决定写到哪里。

### 10.2 超时与不确定结果

POST 超时可能代表：

- 服务端未执行；
- 服务端已写入但响应丢失。

Rust 应在可行时重新 GET：

- hash 等于候选内容：确认成功；
- hash 等于 baseline：确认未生效；
- 其他 hash：冲突；
- 无法读取：`DIARY_SAVE_UNCERTAIN`。

任何不确定状态都保留草稿，并禁止自动重试写入。

### 10.3 能力边界

GET→POST 之间仍有 TOCTOU。只有服务端 revision/ETag/If-Match 或事务化 compare-and-swap 才能真正消除多客户端覆盖。产品文案与验收报告必须把当前方案称为“冲突预检”，不能称为原子 CAS。

最新 VCPToolBox 已确认 admin save/move/delete 进入 `runExternalFileMutation`，文件提交后把 upsert/delete 放入 SQLite/Rust 索引批处理。因为管理路由使用 `waitForIndex: false`，HTTP 成功只证明文件 mutation 完成；Store 应把相关语义结果标记 stale，并允许稍后重试，不阻塞保存成功提示。

### 10.4 重命名 transaction

重命名属于当前范围。执行顺序：

1. 固定 `{sourceKey,targetFile,baselineHash,operationId}`；
2. 校验并规范化目标扩展名；
3. GET 源正文并核对 baseline；
4. 探测目标；目标已存在则 `DIARY_CONFLICT`，禁止覆盖；
5. 若部署提供经 fixture 证实的专用 rename，调用它并读回验证；
6. 否则 POST 相同正文到目标 key，GET 目标并核对 hash；
7. 目标已确认后调用 `delete-batch` 删除源；
8. 删除成功后原子迁移 Store key；删除失败则返回 `DIARY_RENAME_SOURCE_RETAINED`，保留两份文件并刷新列表。

兼容 transaction 不是原子 rename。目标探测与 POST 之间也存在跨客户端 TOCTOU；Mobile 会拒绝已知重名，但在上游没有 create-if-absent/rename 条件端点时不能声称消除了并发覆盖窗口。不能在删除源失败时自动删除已验证的新文件“回滚”，因为这可能把用户唯一确认成功的新副本再次置于风险中。

### 10.5 创建、移动与删除

- DailyNote 创建将 `fileNameSuffix` 序列化为 Tool 的 `fileName`；它只是日期/时刻文件名后的可选后缀。成功响应必须解析服务端最终分配的 `folder + fileName`；
- move/delete 原样保留服务端 `moved/deleted/errors`，不能把 HTTP 200 等同于全量成功；
- 成功删除/移出的 key 立即进入 tombstone，迟到 list/get 不得复活；
- 文件夹删除只在当前列表确认空后开放，服务端 400 仍是最终裁决；
- mutation 完成后相关普通列表刷新，LightMemo 结果标记为可能等待索引追平。

## 11. Markdown 与可信 HTML

### 11.1 产品信任决策

日记正文来自用户自己管理的 VCP 服务，按可信内容处理。当前版本保留 raw HTML：

~~~text
raw diary content
  → marked.parse
  → 可选复用现有轻量 trusted-content filter
  → v-html / innerHTML
~~~

不新增 Diary 专属 DOMPurify 严格 profile，不建立标签/属性白名单，也不把合法 HTML 转义成纯文本。可直接采用桌面 `marked → innerHTML` 语义；如果复用现有过滤，必须先用包含图片、音频、表格、样式和自定义 HTML 的真实日记 fixture 验证不会破坏内容。

日记正文使用 feature-local 的可信内容渲染 helper，不借用聊天消息的解析或渲染链。

信任边界仅覆盖 `DiaryDocument.content`：

- folder/file、preview、搜索 scope、批量错误和服务端 `error/message/details` 走 Vue 插值或 `textContent`；
- LightMemo 返回的路径和元信息先解析成 DTO，再以文本渲染；
- HTML 只在 reader/preview 的专用正文容器进入 DOM；
- 编辑器始终保留并保存原始正文，不把浏览器规范化后的 `innerHTML` 回写到文件。

这是明确的产品取舍；无需把它包装成通用 Web 安全保证。

### 11.2 编辑预览

- 默认手动切换预览；
- 若实测需要实时预览，使用 300–500ms debounce；
- 预览只处理当前正文；
- KaTeX、Mermaid、代码高亮按需加载，并单独做性能与内容兼容验收；
- 不在每次键击上执行桌面式整篇 Markdown + KaTeX。

## 12. 页面、层级与返回

- 全局只新增 `diaryCenter` 一个 page type；
- `DiaryCenterView` 使用 `SlidePage` 和动态 page z-index；
- reader/editor 是 feature 内部视图，进入时向 ModalHistory 注册唯一子状态；
- Sheet 使用 `z-sheet`，冲突/离开确认使用 `z-dialog`；
- 不新增 z-index 数字；
- 当前源码事实是 `editor=70, viewer=80`，与用户提供的部分文字表格漂移；实现只引用语义层名。

Diary 根页关闭由 overlayStore 处理；内部状态先由 ModalHistory 消费。保存中的 close handler 返回 `false`，dirty 状态先打开确认。

## 13. 生命周期与离线

- 退后台：不自动清空 draft；保存中维持现有 operation owner；
- 回前台：只把干净的列表/文档标为 stale；dirty editor 不自动 GET 覆盖；
- 无网：已加载正文可继续读，并明确“可能不是最新”；
- 不离线排队 save/delete/move；
- 当前版本不保证 Android 杀进程后的草稿恢复；
- 不把日记正文加入 Delta Sync 或 SQLite 缓存。

## 14. 安全与并发验收

至少覆盖：

- URL scheme、userinfo、path prefix、路径段和 redirect；
- Basic Auth 只在 Rust 出现；
- 400/401/403/404/429/499/500/503/504、超时、断网、畸形 JSON；
- 有/无 Content-Length 的超大响应；
- A→B→A folder/document/search 的迟到 success/error/finally；
- 普通/语义新搜索分别取消同类旧 Rust owner；
- dirty back、saving back、保存多击；
- baseline conflict、force overwrite 二次确认、POST 超时后读回；
- rename 重名、成功、目标创建后源删除失败以及 Store key 迁移；
- create 的 Tool ESCAPE、marker 字面量与插件错误；
- move/delete partial success 与删除后迟到 GET 不复活；
- 数千 summary 的虚拟列表、滚动恢复与刷新原子替换；
- raw HTML 在 reader/preview 中按可信内容保留；
- 文件名、preview、LightMemo 元信息和服务端错误不进入 `v-html`。

## 15. 明确不改

当前里程碑不修改：

- `src/core/router/index.ts`
- Sync V2 类型与协议
- SQLite schema
- Android 原生插件及 permissions
- 全局 z-index 数字
- package runtime dependencies
- VCPChat 或 VCPToolBox 参考仓库
