# 03｜VCPMobile 技术架构与安全契约

> 架构目标：一个 Rust typed service、一个 Composition Pinia Store、一个懒加载 Diary Feature。  
> 约束：不新增 Router、SQLite 日记库、Sync V2 实体、Android 插件命令或运行时依赖。

## 1. 总体数据流

~~~mermaid
flowchart LR
    UI[DiaryCenterView<br/>列表/阅读/编辑] --> STORE[useDiaryStore<br/>唯一前端状态所有者]
    STORE -->|Tauri invoke: typed args| CMD[Diary commands]
    CMD --> SERVICE[DiaryServiceState<br/>reqwest + search owner + mutation gate]
    SERVICE --> SETTINGS[SettingsState<br/>URL/admin credentials]
    SERVICE -->|Basic Auth| ADMIN[VCP admin_api/dailynotes]
    SERVICE -. Phase 3 Bearer .-> TOOL[VCP v1/human/tool]
    ADMIN -->|bounded JSON| SERVICE
    SERVICE -->|typed DTO / stable error code| STORE
    STORE --> UI
~~~

边界：

- **VCP 服务**：远端文件最终事实源；
- **Rust**：凭据、URL、HTTP、大小/超时、schema、hash、保存冲突与响应归一化；
- **Pinia Store**：当前视图、资源数据、请求 generation、搜索意图和编辑草稿；
- **Vue 组件**：展示、可访问交互、严格净化后的 Markdown；
- **overlayStore / ModalHistory**：唯一全局页面和返回键所有者。

## 2. 模块落点

### 2.1 Rust

建议在现有 chat 领域共置，因为 DailyNote 解析和 Markdown 资产已位于该领域：

~~~text
src-tauri/src/vcp_modules/chat/
├─ diary_service.rs           # DTO、校验、HTTP、hash、commands、纯逻辑测试
└─ mod.rs                     # 声明 diary_service

src-tauri/src/vcp_modules/mod.rs  # facade re-export
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
├─ diaryMarkdown.ts           # marked + strict DOMPurify profile
└─ components/
   ├─ DiaryFolderSheet.vue
   ├─ DiaryNoteList.vue
   ├─ DiaryReader.vue
   └─ DiaryEditor.vue
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
    active_search_owner,
}
~~~

职责：

- `http_client`：连接池、redirect policy、connect/total timeout；
- `mutation_gate`：串行化本 Mobile 进程的 diary mutation；
- `active_search_owner`：新搜索取消旧搜索，并确保旧 owner 不能清理新状态。

它**不持有**当前 folder、当前 document、列表缓存或编辑草稿。写操作低频，一个全局 mutation gate 比无界 `NoteKey → Mutex` 映射更简单，也避免锁表泄漏。若未来批量操作证明吞吐不足，再用测量结果调整。

所有网络和文件 I/O 必须 async；Tauri command 中不使用 `unwrap()` 或 `expect()`。

## 4. Command 与 DTO

### 4.1 Release 1 commands

| Command | 参数 | 返回 | 备注 |
|---|---|---|---|
| `diary_list_folders` | 无 | `DiaryFolderList` | 不伪造统计 |
| `diary_list_notes` | `folder` | `DiaryNoteSummary[]` | 保留服务端顺序 |
| `diary_get_note` | `DiaryNoteKey` | `DiaryDocument` | 返回 raw content + SHA-256 |
| `diary_search` | `requestId, term, folder?` | `DiarySearchResponse` | 新 owner 取消旧 owner |
| `diary_cancel_search` | `requestId?` | `()` | 页面关闭或清空搜索 |
| `diary_save_note` | `DiarySaveRequest` | `DiarySaveOutcome` | 复读、冲突、保存、验证 |

### 4.2 后续 commands

- `diary_create_note`
- `diary_move_notes`
- `diary_delete_notes`
- `diary_delete_empty_folder`
- `diary_associative_discovery`

语义检索可以复用 Human Tool 能力，但必须有单独 DTO 和取消生命周期，不能解析成不受约束的 HTML。

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
~~~

不把未经确认的 `tags`、`agentId`、`entryCount`、`matchOffset` 或 `revision` 放进 DTO。

## 5. 错误契约

内部使用 typed `DiaryError`；Tauri adapter 依照项目约定映射为 `Result<T, String>`。字符串使用稳定前缀，Vue 不解析自然语言：

| code | 含义 | UI |
|---|---|---|
| `DIARY_CONFIG_MISSING` | URL 或管理员凭据缺失 | 前往设置 |
| `DIARY_AUTH_REQUIRED` | 401/403 | 检查管理员凭据 |
| `DIARY_NOT_FOUND` | folder/file 已不存在 | 返回列表并刷新 |
| `DIARY_CONFLICT` | 远端 hash 已不同 | 保留草稿，进入冲突动作层 |
| `DIARY_TIMEOUT` | connect/total/idle timeout | 重试；mutation 进入结果核验 |
| `DIARY_TRANSPORT` | DNS、TLS、断网 | 保留已有数据 |
| `DIARY_RESPONSE_TOO_LARGE` | 超过客户端预算 | 不截断、不渲染 |
| `DIARY_INVALID_RESPONSE` | JSON/schema/UTF-8 不合法 | 可重试并记录安全摘要 |
| `DIARY_SERVER_ERROR` | 其他 4xx/5xx | 局部错误 |
| `DIARY_SAVE_UNCERTAIN` | 无法确认保存结果 | 保留草稿，禁止宣称成功 |

日志只记录 code、HTTP status、operation ID 和脱敏 target hash；不记录 Authorization、密码、完整正文、完整错误 body 或带敏感 query 的 URL。

## 6. URL、认证与重定向

Rust 每次 command 从 `SettingsState` 读取配置快照：

1. 解析 `vcpServerUrl`，只接受 `http`/`https`；
2. 拒绝 embedded username/password、NUL、控制字符；
3. 去掉 query 与 fragment；
4. 移除已知末尾 `/v1/chat/completions`；
5. P0 确认后决定是否保留反向代理 path prefix，不能盲目只取 host；
6. 使用 URL path-segment API 拼接固定 `admin_api/dailynotes`；
7. folder/file 各自作为单一 segment 编码，并在客户端防御性拒绝空值、`.`、`..`、`/`、`\`、绝对路径形式。

客户端校验不替代服务端路径防护。

`reqwest` redirect policy 设为禁止或只允许 same-origin；任何跨 origin 重定向都不能携带 Basic Auth。

## 7. HTTP 预算

下表是 **Mobile 初始提案**，不是现有服务端事实；P0 用真实数据 P50/P95/max 校准：

| 项目 | 初始值 |
|---|---:|
| connect timeout | 10s |
| list/read/save total timeout | 30s |
| search total timeout | 40s（服务端自身 30s） |
| streaming idle timeout | 15s |
| 解码后单篇正文 | 2 MiB |
| list/search JSON body | 4 MiB |
| error body | 64 KiB |
| 单次 summary 数量 | 5000 |
| 编辑提交正文 | 2 MiB UTF-8 bytes |

每个响应同时检查：

- `Content-Length`（若有）；
- `bytes_stream` 实际累计长度；
- checked-add 溢出；
- JSON 解码后的数组数量和正文长度。

超过边界必须 fail closed，不能静默截断后允许编辑保存。现有可复用范式见 `message_service.rs:20-115`。

## 8. 前端 Store 所有权

`useDiaryStore` 只保留一套状态：

~~~text
navigation:
  screen = list | reader | editor | preview

resources:
  folders
  selectedFolder
  notes
  search { query, scope, results, limited }
  document { key, content, contentHash }

editor:
  draft
  dirty
  saveState

request ownership:
  foldersGeneration
  notesGeneration + targetFolder
  searchGeneration + requestId
  documentGeneration + targetKey
~~~

不要为 shelf、reader、editor 分三个 Store；也不要把全局页面栈复制进 Diary Store。

内容不写入 localStorage/Pinia persist。MVP 只在内存保留当前 document、baseline 和 draft；退出干净页面可释放。若未来需要崩溃恢复，单独设计有上限的 draft 表，绝不演化成离线 mutation queue。

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

搜索还需要 Rust 侧 active owner/cancellation，避免快速输入留下多个最长 30 秒的服务端扫描。新请求取消旧 token；完成清理必须再次核对 owner。

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

此外，P0 必须验证 admin raw-save 后 KnowledgeBase/RAG 索引更新；只验证文件内容变化不等于记忆系统一致。

## 11. Markdown 与内容安全

### 11.1 独立严格 profile

远程日记按不可信内容处理。首版采用现有依赖：

~~~text
raw content
  → marked.parse
  → feature-local DOMPurify strict profile
  → vcp-markdown-block typography
  → main DOM
~~~

要求：

- 禁止 raw `script/style/iframe/form/object/embed/meta/base/link`；
- 禁止所有事件属性；
- 禁止 `javascript:`、`vbscript:` 和活动型 data document；
- 只允许受控的 Markdown 标签/属性；
- 外链添加 `noopener noreferrer`，由受控 opener 打开；
- 外部图片默认不自动跨 origin 加载；同源图片 lazy load；
- 错误、文件名、摘要、搜索高亮使用 Vue 文本节点，不拼 HTML；
- HTML 被移除时正文仍可读，不显示空白。

不能直接复用：

- 桌面 `marked → innerHTML`；
- `HtmlPreviewBlock` 为完整沙箱文档放宽的 DOMPurify 配置；
- `astRenderer` 的 trusted-circle active-content guard。

若未来改为 Rust Markdown AST，也必须在进入主 DOM 前删除 `raw_html/raw_html_inline` 或再经过同等严格净化；“来自 Rust”不等于安全。

### 11.2 编辑预览

- 默认手动切换预览；
- 若实测需要实时预览，使用 300–500ms debounce；
- 预览只处理当前正文；
- KaTeX、Mermaid、代码高亮按需加载，并单独做安全/性能验收；
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
- MVP 不保证 Android 杀进程后的草稿恢复；
- 不把日记正文加入 Delta Sync 或 SQLite 缓存。

## 14. 安全与并发验收

至少覆盖：

- URL scheme、userinfo、path prefix、路径段和 redirect；
- Basic Auth 只在 Rust 出现；
- 401/403/404/429/5xx、超时、断网、畸形 JSON；
- 有/无 Content-Length 的超大响应；
- A→B→A folder/document/search 的迟到 success/error/finally；
- 新搜索取消旧 Rust owner；
- dirty back、saving back、保存多击；
- baseline conflict、force overwrite 二次确认、POST 超时后读回；
- 删除后迟到 GET 不复活（后续 mutation 阶段）；
- script、事件属性、危险 URL、恶意 SVG/data document；
- 文件名和服务端错误不进入 `v-html`。

## 15. 明确不改

首版不修改：

- `src/core/router/index.ts`
- Sync V2 类型与协议
- SQLite schema
- Android 原生插件及 permissions
- 全局 z-index 数字
- package runtime dependencies
- VCPChat 或 VCPToolBox 参考仓库
