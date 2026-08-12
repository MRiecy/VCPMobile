# 03 Agent 协作、容器、版本与安全边界

## 1. Agent 读取协议

活跃 Agent Port v3 按 common/docx/pptx 暴露能力。主要只读接口包括：

| 接口 | 返回 |
|---|---|
| `GetDocumentInfo` | documentId、kind、revision、dirty、scene、页状态 |
| `GetRenderedText` | 编译后纯文本、blocks/diagnostics 或逐页文本/notes |
| `GetOutline` | VDOCX 源码标题索引或 VPPTX 页目录 |
| `GetSection` | 章节源码、区间、编译纯文本 |
| `GetSource` | 指定源码类型和行范围 |
| `SearchSource` | 字符串/正则源码检索，最多 200 项 |
| `GetViewportSource` | 可见 edit key 对应源码范围及邻近行 |
| `GetVisualContext` | 语义摘要、截屏矩形，主进程补真实截图 |
| `GetPrHistory` | 人类 checkpoint 与 Agent PR 记录 |

这一组接口体现了正确的上下文分层：Agent 可以先读轻量目录/纯文本，再按章节、视口或搜索结果拉取精确源码，必要时请求截图，不必每次把整个 ZIP 或 live DOM 塞进上下文。

## 2. 写操作是 PR，不是直接改 DOM

### 2.1 入口校验

Electron 主进程维护 endpoint/method allowlist。写操作必须提供：

- `author`/`maid` 署名；
- `summary`；
- `requestId` 幂等键；
- 推荐提供 `expectedRevision`。

主进程相同 `requestId` 的进行中请求共用一个 Promise；读请求超时 30 秒，写请求等待审批最长 5 分钟。审批超时返回 `PR_RECEIPT_TIMEOUT + submitted:true + pending:true`，提案仍留在文脉，Agent 需要稍后查询历史获取最终结果。

证据：[`docxHandlers.js:15-48,73-163`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/ipc/docxHandlers.js#L15-L163)。

### 2.2 提交与应用

`SubmitSourcePr` 使用 `target/replace` 列表。流程是：

1. revision 预检；
2. 在当前源码上初步应用 replacements；
3. VDOCX 混合源码执行 compiler validate；
4. 写入 pending lineage，保存基础 revision 和 documentId；
5. 人类查看源码/渲染差异，批准或拒绝；
6. 批准队列串行执行；
7. 再检查 documentId 和 baseRevision；
8. 在**当前源码**上重新定位 target；
9. 成功后记录 before/after、operation、receipt、snapshot，重渲染并持久化。

证据：[`scriptorium-agent-port.js:498-692`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L498-L692)、[`948-1084`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L948-L1084)。

这是静态源码中产品闭环最完整的概念：Agent 提议完整源码变更，人类同时看局部源码差异和隔离渲染差异，最终合并动作有署名、回执、变更集和版本快照。

## 3. revision、generation 与保存上下文

`DocumentStore` 持有两个不同概念：

| 字段 | 变化时机 | 语义 |
|---|---|---|
| `revision` | 当前文档每次真实 mutation | 同一文档内的乐观并发版本 |
| `generation` | 整份 document 被 replace/open/restore | 当前内存文档实例的生命周期代次 |

`replaceDocument()` 会 `generation += 1`、`revision = 0`；普通 `mutate()` 只增加 revision。异步保存先捕获 `{generation, documentId, revision}`，ZIP 打包和 IPC 写入完成后只有上下文仍是当前代次/文档时才 `markSaved`。依据：[`scriptorium-document-store.js:126-179,248-268`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-document-store.js#L126-L268)、[`scriptorium-session.js:216-274`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-session.js#L216-L274)。

这个 `generation + documentId + revision` 模式非常适合 VCPMobile：它比单纯 dirty/revision 更能阻止“旧异步结果写进新文档”。

## 4. 文脉与回溯

Lineage 将人类 checkpoint 和 Agent PR 放在同一时间线。每条记录可以包含：

```text
id, source, author, name, summary, note
createdAt, baseRevision, revision
proposal, operation, changeSet
status, receipt, snapshot
```

快照会移除其他 checkpoint 内嵌 snapshot，避免递归膨胀。回溯前先创建当前版本备份，再以目标快照替换 document；后续文脉不被删除。证据：[`scriptorium-lineage-store.js:57-187`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-lineage-store.js#L57-L187)。

需要区分：80 个左右的窗口内 undo/redo 是短期编辑历史；lineage 是工程内持久的协作与审计历史。二者不应合并成一个栈。

## 5. VDOC v2 容器与内容寻址资源

### 5.1 实际容器布局

活跃代码的 v2 格式为：

```text
manifest.json
source/document.md          # flow 文档
source/document.css         # flow 文档
lineage/checkpoints.json
resources/media/<sha256>.<ext>
resources/fonts/<sha256>.<ext>
mimetype
```

VPPTX 的页源码和 deck CSS 仍在 manifest 模型中；VDOCX 的正文/CSS/lineage 被外置到明确条目。审计早期 README 的 `document.json` 描述曾落后于实现，最终提交 `17822ca` 已把 README 改为上述 v2 布局；本节继续以容器代码为协议依据。

### 5.2 CAS 资源协议

本地媒体/字体注册时：

1. 对字节计算 SHA-256；
2. SHA 同时作为 `id` 与 `sha256`；
3. 相同内容自动去重；
4. 源码只保存 `vdoc-resource://media/<sha>` 或 `.../fonts/<sha>`；
5. 解包时重算哈希，文件缺失或摘要不一致就拒绝；
6. 编辑时解析为受生命周期管理的 Blob URL；
7. 单文件导出只在副本中转 data URL，不污染源模型。

证据：[`vdoc-container.js:4-81,84-194,207-267`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-container.js#L4-L267)。

### 5.3 文件提交

Electron 保存和导出都先写同目录临时文件，再移动覆盖目标；工程/导出限制 100 MB，单外部资源限制 80 MB。证据：[`docxHandlers.js:15-25`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/ipc/docxHandlers.js#L15-L25)、[`722-797`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/ipc/docxHandlers.js#L722-L797)。

这提供了“目标文件不出现半写”的基本保障，但不等于目录 fsync、跨平台掉电一致性或多进程 CAS；移动端实现仍需单独定义这些承诺。

## 6. 可编程内容运行时

Scriptorium 允许 HTML island/slide 携带内联 JavaScript。当前纵深防御包括：

- HTML/CSS 清理：删除独立执行宿主、事件属性和危险 URL；
- 外部脚本依赖本地化：只允许 Anime.js/Three.js 已知来源；
- JavaScript regex 规则：Node/process/fs/进程/Electron/动态 eval/constructor escape 等为 refuse，网络/存储/全局事件/持续任务/WebGL 为 warn；
- 运行时跟踪 `requestAnimationFrame`、timeout、interval 和 cleanup；
- surface 切换/重渲染时释放生命周期；
- 不可用的审查器 fail closed。

证据：[`scriptorium-programmable-content.js:37-168,207-395`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-programmable-content.js#L37-L395)、[`scriptorium-runtime.js:35-123,236-335`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-runtime.js#L35-L335)。

### 6.1 这不是安全沙箱

代码和上游 README 都明确支持以下判断：

- 文档脚本通过 `new Function` 在 Scriptorium renderer 中运行；
- CSP 需要 `script-src 'unsafe-eval'`；
- BrowserWindow 是 `contextIsolation:true`、`nodeIntegration:false`，但 `sandbox:false`；
- `scopedDocument.querySelector/getElementById` 在局部找不到时会回退宿主 `document`；
- preload 在同一页面全局暴露文件、窗口和 Agent IPC 桥；
- 安全审查是规则扫描，而且人类可以在本机关闭。

证据：[`scriptorium.html:6`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium.html#L6)、[`scriptorium-runtime.js:126-180,236-320`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-runtime.js#L126-L320)、[`docxHandlers.js:1007-1025`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/ipc/docxHandlers.js#L1007-L1025)、[`preloads/docx.js:12-52`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/preloads/docx.js#L12-L52)。

结论：它是面向可信本地文档的 Alpha 级纵深防御，不是恶意 JavaScript 隔离。VCPMobile 绝不能把这条 same-realm + regex 路线原样迁移。

## 7. 源码审计发现的静态风险

以下是活跃调用链中可见的缺口。本轮没有按上游禁令运行测试或构造动态复现，因此统一标记为“静态可达风险”，后续必须用最小复现和修复后回归确认。

### R-01 普通 Source PR 的脚本审查结果没有进入 proposal

`submitSourcePr()` 只做 target/replace 和 hybrid compiler validate，没有调用 `programmableReview()`，也没有给 `proposal.programmableContent` 赋值；完整脚本审查只在 `buildProjectArtifact()` 路径调用。审批和自动允许逻辑却只在 `proposal.programmableContent.status === 'refuse'` 时强制阻止自动批准。

结果是：普通 source/slide PR 不能被描述为“refuse 脚本一定进入强制人工审阅”。运行时仍会再次做执行审查，但提案期/自动允许的承诺存在断层。

证据：[`scriptorium-agent-port.js:610-692,784-965`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L610-L965)、[`scriptorium-lineage-ui.js:295-329`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-lineage-ui.js#L295-L329)。

### R-02 pending PR 没有绑定 generation

pending entry 保存 documentId，record 保存 baseRevision；批准只比较这两项。`replaceDocument()` 会增加 generation 并把 revision 重置为 0，而 lineage restore 可以保留同一 documentId。理论上，相同 documentId 经替换/回溯又回到同 revision 时，旧 PR 可能越过文档实例代次保护。

修复契约应是 pending 保存 `{generation, documentId, baseRevision}`，审批三项完全相等才允许执行。

### R-03 `target/replace` 对重复 target 默认取第一个

`locateTarget()` 收集全部匹配；没有 `startLine` 时即使匹配多次也返回 `offsets[0]`，有提示时也只是选最近项，不要求唯一胜者。证据：[`scriptorium-pr-diff.js:4-60`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-pr-diff.js#L4-L60)。

这与运行态文字的“歧义即失败”原则不一致。更安全的 PR contract 应要求：唯一 target，或提供 `expectedRange + expectedHash`；行号只能辅助诊断，不能作为无校验身份。

### R-04 VPPTX 延迟 flush 的换文档时序

deck 输入先把 DOM node 放入 `pendingNodes`，约 2 秒后才写 Source Store；这段时间 store 仍可能是 `dirty:false`。打开另一文档时，session 会先 `replaceDocument(new)`，之后 `activateAdapter()` 才调用旧 editor 的 `flush()`。旧 DOM 节点因此可能针对新文档的 adapter/source 执行定向更新；若稳定 ID 巧合匹配，存在旧输入污染新文档的路径。

相关调用链：deck editor 的 [`274-306`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-deck-editor.js#L274-L306)、[`725-730`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-deck-editor.js#L725-L730)、[`754-765`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-deck-editor.js#L754-L765)，session 的 [`31-49`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-session.js#L31-L49)、[`152-157`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-session.js#L152-L157)，以及 composition root 的 [`scriptorium.js:337-356`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium.js#L337-L356)。

修复应在 document replace **之前** flush 当前 editor，并让 pending buffer 捕获 generation/documentId；进入输入即同步标 dirty 或由独立 pending 状态阻止无提示切换。

### R-05 媒体异步链缺少代次提交检查

本地媒体依次等待 `arrayBuffer → metadata → registerResource`，网络媒体等待 metadata，最后才调用当前 adapter 插入。链路没有捕获和检查 generation/documentId。用户在等待期间切换文档，资源注册与插入可能落到后来激活的工程。

证据：[`scriptorium-media.js:244-360`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-media.js#L244-L360)。应使用 operation context、取消令牌，并只在 commit 点写资源 Map 和源码。

### R-06 Visual Context 语义与截图不是同一一致性快照

flow 的 `scope:'viewport'` 只是一项标签：截图取当前 surface 矩形，`renderedText` 仍可为全文。等待机制只有固定延迟 + 双 RAF，不等待字体、图片、Mermaid、动画真正稳定，也未在文本读取与截图之间检查 revision/generation。

Agent 因此可能收到“新文本 + 旧画面”或“全文文本 + 局部画面”。移动端应返回 capture context，并在前后 revision 不一致时重试或显式标 `stale:true`。

### R-07 PR 渲染差异对动态内容不保真

差异预览使用 `sandbox=""` 的 iframe `srcdoc`，主动暂停动画，不执行文档脚本，也不启动主渲染面的 KaTeX/Mermaid 管线。它安全地给出静态前后对照，但不能证明交互 island、动画、数学或图表的最终动态效果。证据：[`scriptorium-pr-diff.js:102-142`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-pr-diff.js#L102-L142)。

### R-08 pending PR 跨重启没有可恢复执行计划

Lineage 会从工程恢复状态为 pending 的记录，但真正的 `operation` closure 只存在 renderer 内存 `pending` Map。重启后 UI 可以看到 pending 记录，`approvePr()` 却只能返回 `PR_NOT_PENDING`。此外，审批开始时先从 Map 删除 entry，再执行异步 mutation/persist；异常没有统一 catch 把记录收口为 failed/aborted。证据：[`scriptorium-lineage-store.js:38-43`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-lineage-store.js#L38-L43)、[`scriptorium-agent-port.js:948-1083`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L948-L1083)。

Mobile 必须二选一：持久化可重新验证的结构化 proposal 并在恢复后重建执行计划，或在进程重启时把所有未决提案原子终结为 `aborted`。不能留下“显示待审但永远无法审批”的状态。

### R-09 视觉对象 ID 去重契约弱于文本节点

文本节点归一化会同时补齐缺失 ID 和重签重复 ID；对象路径则只补缺失的 `data-vdoc-object-id`，后续查找使用首个 `querySelector`。手工复制带原 ID 的对象源码时，定向属性/拖拽操作可能落到同 ID 的另一个对象。证据：`scriptorium-objects.js` 的 [`324-356`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-objects.js#L324-L356)、[`515-519`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-objects.js#L515-L519)、[`618-651`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-objects.js#L618-L651)。

Mobile 的所有持久身份都应在 normalize/commit 两处执行“非空且全局唯一”，不能只在 UI 插入时生成一次。

### R-10 `runtimeTextOverrides` 是未闭环的模型字段

schema/adapter 会归一化并保留 `runtimeTextOverrides`，但排除未加载的 origin 文件后，活跃模块没有看到写入或应用消费者。当前真实策略是可编程 island 文本无法唯一映射时不开放编辑。相关定义见 [`vdoc-core.js:170-204`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-core.js#L170-L204)，真实拒绝路径见 [`scriptorium-rendered-text.js:541-555`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-rendered-text.js#L541-L555)。

因此不能把 overrides 当成已完成的动态 DOM 持久化兜底；Mobile schema 在有真实 owner、读写者和迁移契约前不应复制该字段。

### R-11 历史去重可能被 `modifiedAt` 干扰

历史快照用完整序列化字符串判断是否与上一项相同，而 `core.serialize()` 每次都会刷新 `manifest.modifiedAt`。静态上存在“没有语义变化的 capture 仍因时间戳不同占用历史槽”的可能。证据：[`scriptorium-edit-history.js:25-41`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-edit-history.js#L25-L41)、[`vdoc-core.js:629-633`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-core.js#L629-L633)。

更稳妥的方式是对不包含易变元数据的 canonical source state 计算内容摘要，时间戳只作为记录元数据，不进入语义去重键。

### R-12 文档脚本与预加载桥位于同一页面主世界

运行时用 `new Function` 执行通过规则扫描的文档脚本，没有遮蔽 `window/globalThis`；同一页面又通过 `contextBridge` 暴露 `window.scriptoriumAPI`，其中包含打开/读取/保存、外部资源、窗口控制和 Agent 事件桥。refuse 规则会匹配 `electron/ipcRenderer/ipcMain` 等字面量，却没有匹配 `scriptoriumAPI` 或禁止一般 `window` 成员访问。

据此存在一条静态可达路径：文档脚本不需要直接写出 `ipcRenderer`，就可能调用已经暴露的预加载桥。其实际影响仍需专项最小复现确认，但在确认前不能把 regex review + context isolation 描述成脚本与宿主能力的隔离边界。

证据：[`scriptorium-runtime.js:236-320`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-runtime.js#L236-L320)、[`scriptorium-programmable-content.js:37-120`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-programmable-content.js#L37-L120)、[`preloads/docx.js:12-52`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/preloads/docx.js#L12-L52)。

## 8. 对移动端应冻结的安全契约

1. Agent 写操作永远不直接获得 WebView、文件系统或 Tauri invoke 权限。
2. 所有 proposal 在入队前跑与最终执行相同的解析、脚本和资源策略。
3. 审批与异步 commit 必须绑定 `generation + documentId + revision`。
4. 任何弱锚点多义都 fail closed，不能默认第一个匹配。
5. 文档 JavaScript 默认不执行；若未来需要，必须放到没有 Tauri bridge 的独立沙箱进程/隔离 WebView，并使用消息 allowlist。
6. ZIP 解包必须限制总展开字节、单项字节、entry 数、压缩比、路径规范化和资源哈希；不能只沿用桌面 100 MB 总文件上限。
7. 视觉截图和语义快照必须带同一个一致性上下文；不能把“等待两帧”称为完成稳定。
8. 自动批准策略由人类持有，但 refuse/高风险类别必须由后端硬门禁，不能只由 UI 字段决定。
