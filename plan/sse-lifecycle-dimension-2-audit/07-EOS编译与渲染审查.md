# 07 — EOS 全文编译与前端终态交接审查

审查日期：2026-10-31。生产代码基线：`a771315ec4b5e71652621cf7343bce2344031479`。本文件核实原报告维度二第 7 项；未修改生产代码或已有测试。

## 裁决

**后端重复解析属实，但“已有可直接落库的同构成果被无意义丢弃”不成立；前端终态存在更具体的节点身份交接问题，不能将其归因于 `v-memo` 状态翻转。**

| 子项 | 核实结果 | 紧迫性 |
| --- | --- | --- |
| 正常流终态重新编译全文 | 确实发生；闭合代码块会再次走终态高亮。编译在有并发上限的 `spawn_blocking` 池，不占用 WebView 主线程 | **P2**：常规性能优化；长代码/多块消息优先测量 |
| 直接将 Aurora `stable_blocks` 交给 `serialize_render_async` | 当前类型与语义不满足条件，不能作为局部替换方案 | 不建议直接实施 |
| 活跃标志改变使当前消息各块 memo 失效 | 属实，但失效表示重新执行渲染比较，不等于 DOM 全部重建；其他历史消息的标志并未因此统一翻转 | **P3**：单独看只是可细化的缓存依赖 |
| 成功终态换新 blocks、普通块 hash/key 改变 | 属实；普通 Markdown 不享有工具块/语音指导块的身份对齐。实际生产 Renderer + Store 探针复现了表格旧节点移除、节点内滚动位置丢失 | **P1**：近期优先处理终态交接；不是发布阻断结论 |
| “全局雪崩”“明显掉帧/冻结” | 本次没有具名 Android 设备帧时证据，不成立为已证实结论 | 留待设备测量 |

P1 的依据是可复现的 DOM 身份与交互状态损失，不是未测量的耗时猜测。该影响局限于发生终态投影替换的消息及相应块，不代表整个会话历史重建。

## 1. 后端终态实际链路

正常 SSE 收尾依次执行 `flush_aurora_window`、`aurora_buffer.finalize()`、`send_update`；随后返回的 `VcpResponse` 只有 `full_content`、`finish_reason`、`is_aborted`，并不携带已解析块。见 [vcp_client.rs:2423–2434](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L2423) 与 [VcpResponse:230–236](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L230)。

`AuroraBuffer::finalize` 调用流式 parser 的 finalize，将最后的块加入 `stable_blocks`，清除 Tail、旧 AST 和高亮状态。见 [aurora_pipeline.rs:1026–1042](../../src-tauri/src/vcp_modules/chat/aurora_pipeline.rs#L1026)。因此原报告所说“流式阶段已有成果”有事实基础，但这些成果当前只负责流式展示。

单聊和群聊都把 `response.full_content` 原样传给统一 finalizer，正常成功分支没有额外的正文后处理：[agent_chat_application_service.rs:266–278](../../src-tauri/src/vcp_modules/agent/agent_chat_application_service.rs#L266)、[group_chat_application_service.rs:254–267](../../src-tauri/src/vcp_modules/group/group_chat_application_service.rs#L254)。

`commit_stream_message` 在打开写事务之前调用 `compile_and_serialize_render_async(final_content.to_string())`。该函数调用 `MessageRenderCompiler::compile`，后者进入完整的 `parse_content`，再将结果 JSON + zstd 序列化。见 [message_service.rs:1783–1802](../../src-tauri/src/vcp_modules/chat/message_service.rs#L1783)、[message_repository.rs:157–169](../../src-tauri/src/vcp_modules/persistence/message_repository.rs#L157)、[编译与序列化实现:228–243](../../src-tauri/src/vcp_modules/persistence/message_repository.rs#L228)。

`parse_content` 的代码围栏分支会再次调用 `finalized_code_block_node`，该终态节点构造继续使用完整代码高亮。因此同一段已在流式闭合时高亮过的代码，在正常持久化前再高亮一次，是确定的重复工作。见 [content_parser.rs:643–665](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L643)。

**线程与时序边界：** `run_render_work` 使用 `spawn_blocking`，许可数为可用 CPU 数量限制到 1–4；这不是把同步全文解析放在 WebView 主线程。终态事件却必须等编译和持久化成功才发出，所以重编译可能增加“内容已吐完到正式完成”的延迟，并产生 CPU/内存峰值。这里只确认工作量和等待关系，没有测得延迟毫秒数。见 [message_repository.rs:122–150](../../src-tauri/src/vcp_modules/persistence/message_repository.rs#L122)、[message_service.rs:1678–1768](../../src-tauri/src/vcp_modules/chat/message_service.rs#L1678)。

附带可核实的小范围浪费：编译在判断消息已删除、已终结或不再 pending 之前执行；幂等重试/迟到终态也可能先完整编译再返回已有状态。见 [message_service.rs:1797–1843](../../src-tauri/src/vcp_modules/chat/message_service.rs#L1797)。若优化此处，不能以提前读取替代事务内最终裁决，也不应将 CPU 编译移进写事务延长占锁时间。

## 2. 为什么不能直接序列化 Aurora 块

### 2.1 类型、哈希与分块不同

流式保存的是 `Vec<StreamBlock>`，持久化接口要求 `Vec<ContentBlock>`。前者 Markdown 带原始 `content: String`、文本 hash 字符串；后者常用 `content: None` 加 AST，hash 是结构计算出的 `Option<u64>`。流式 Tool/JevTool 还没有终态对应的 `is_complete` 字段。见 [StreamBlock 定义:14–45](../../src-tauri/src/vcp_modules/chat/stream_block_parser.rs#L14)、[ContentBlock 定义:8–35](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L8)。

流式 hash 是 `DefaultHasher(原始文本)` 的**十六进制字符串**，不是 SHA-256；终态 hash 对 `ContentBlock` 结构做 Hash，序列化为数值。两者的输入与表示都不同。见 [sync_hash.rs:236–241](../../src-tauri/src/vcp_modules/sync/sync_hash.rs#L236)、[content_parser.rs:279–305](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L279)。

流式按空行把已经完成的 Markdown 拆成多个 `StreamBlock`；终态未遇到特种块的剩余正文会一次进入 Markdown parser，形成一个包含多个 MarkdownNode 的 `ContentBlock`。例如 `first\n\nsecond` 在流式完成后可对应两个块，而终态对应一个带两个段落节点的块。见 [stream_block_parser.rs:945–975](../../src-tauri/src/vcp_modules/chat/stream_block_parser.rs#L945)、[content_parser.rs:895–914](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L895)。该例是源码分支推导，本次未执行 Rust 样例。

### 2.2 两条解析链存在真实语义差异

终态 `parse_content` 先按全文执行 `de_indent_misinterpreted_code_blocks`；流式 parser 直接按原始文本识别/沉淀内容。终态对未闭合协议块保留起始标记为 Markdown 后继续处理；流式 finalize 将剩余 Tail 单独转为 Markdown。见 [终态入口:559–563](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L559)、[终态未闭合分支:731–748](../../src-tauri/src/vcp_modules/chat/content_parser.rs#L731)、[流式 finalize:550–568](../../src-tauri/src/vcp_modules/chat/stream_block_parser.rs#L550)。

这些现状不代表永远不能复用，而是复用前要定义唯一的终态语义、块身份和必要转换，不能仅以两边界面“看起来一样”证明二进制缓存可以互换。

### 2.3 并非所有 finalizer 调用都有完整 Aurora 成果

| 路径 | 当前权威输入/差异 |
| --- | --- |
| 正常成功、在线取消收尾 | `VcpResponse` 返回全文及 finish metadata；Aurora 数据不随结果返回 |
| 流错误 | `stream_error_content` 会把错误说明追加进终态正文；不能使用错误前旧 AST 作为最终完整缓存 |
| Helper 离线完成恢复 | 从文件读回全文；可能没有仍存活的 AuroraBuffer；有错误时还要追加说明 |
| 停止恢复 | 可能从 `stop_partial_content` 数据库快照恢复取消结果 |
| 终态提交失败后的重试 | `PendingStreamTerminal` 保留正文、原因和 agent identity，不保留解析块 |
| 群聊 | 与单聊共用终态编译，并以真正提交成功的正文进入下一位 speaker 上下文 |

依据：[错误正文构造:341–349](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L341)、[错误 finalizer:2872–2893](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L2872)、[离线恢复:3138–3166](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L3138)、[停止恢复:3454–3465](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L3454)、[PendingStreamTerminal 与重试:1542–1613](../../src-tauri/src/vcp_modules/chat/message_service.rs#L1542)。

## 3. 前端真正发生了什么

### 3.1 memo 的两个状态确实参与终态更新，但不是 DOM 销毁命令

`isStreaming` 和 `isMessageInActiveStream` 都按当前会话及**具体 message id**查询活动状态。[状态计算:140–164](../../src/features/chat/MessageRenderer.vue#L140) 与 [memo 列表:1047–1049](../../src/features/chat/MessageRenderer.vue#L1047) 足以证明：该消息退出活动流时，每项 memo 的状态依赖会改变。

本地安装的 Vue `3.5.44` runtime 中，`isMemoSame` 只比较依赖以决定是否复用上次 VNode；VNode 是否为相同实体由 `type` 与 `key` 决定。核对路径为 `node_modules/.pnpm/@vue+runtime-core@3.5.44/node_modules/@vue/runtime-core/dist/runtime-core.cjs.js:7797–7806,8792–8815`。这说明 memo 失效本身不能推出 DOM 被移除。

项目还另有按块引用保存 HTML 的 `WeakMap`，因此只改变状态、保留块引用时，不必重新把 AST 转成 HTML。见 [MessageRenderer.vue:426–434](../../src/features/chat/MessageRenderer.vue#L426)。本次运行探针进一步观察到：**只撤销活动标志，稳定表格节点与其 `scrollLeft` 保留。**

### 3.2 正常 end 会换掉整个块投影；普通块 key 没有跨阶段身份

`chatStreamStore` 收到 `end` 后，将 `event.blocks` 交给 `finalizeStream`；`replaceRenderProjection` 直接赋值 `message.blocks = blocks` 并清除 Tail，然后撤销活动状态。见 [终态接管:1427–1474](../../src/core/stores/chatStreamStore.ts#L1427)、[投影替换:374–385](../../src/core/stores/chatStreamStore.ts#L374)。

新对象引用已经能使 memo 失效和 WeakMap 缓存失配；更关键的是，普通块的 key 是：

```text
${bubbleId}-${block.type}-${String(block.hash)}-${index}
```

流式文本 hash 与终态结构 hash 不同，分块数量也可能变化，因而相同可见正文不保证相同 key。见 [getBlockKey:494–505](../../src/features/chat/MessageRenderer.vue#L494)。普通 Markdown 表格由 AST renderer 生成；生产 parser 为外层设置 `vcp-scrollable no-swipe`，表格元素本身有 `display: block; overflow-x: auto`。旧 key 对应节点被移除时，其滚动位置、选区所指向的节点等浏览器状态没有交接代码。见 [markdown_parser.rs:1411–1415](../../src-tauri/src/vcp_modules/chat/pre_renderer/markdown_parser.rs#L1411)、[astRenderer.ts:274–280](../../src/core/utils/astRenderer.ts#L274)、[message-blocks.css:235–242](../../src/assets/message-blocks.css#L235)。

这里不能宣称所有块都丢状态：工具块已有按语义内容与次序保留 key 的机制，语音指导块按 ordinal 保留身份/展开状态，正是现有代码对这类问题的局部处理。见 [工具身份对齐:437–483](../../src/features/chat/MessageRenderer.vue#L437)、[指导块身份:486–500](../../src/features/chat/MessageRenderer.vue#L486)。

### 3.3 删除流状态 memo 依赖会遗漏实际产品更新

稳定块并非全部与流状态无关：

- `ToolBlock.defaultExpanded` 随 `expandToolsByDefault` 更新，同时内部保留用户手动折叠意图。
- `ThoughtBlock.defaultExpanded` 使用 `isMessageInActiveStream`，watcher 会更新展开状态。
- `HtmlPreviewBlock.isStreaming` 控制预览/复制/全屏等按钮及流式状态切换。
- 父组件退出活动流后要结束文本动效、重新调度重内容渲染。

依据：[模板实际 props:1070–1097](../../src/features/chat/MessageRenderer.vue#L1070)、[ToolBlock watcher:58–73](../../src/features/chat/blocks/ToolBlock.vue#L58)、[ThoughtBlock watcher:18–25](../../src/features/chat/blocks/ThoughtBlock.vue#L18)、[HtmlPreviewBlock 流状态:25–34](../../src/features/chat/blocks/HtmlPreviewBlock.vue#L25)、[父级终态 watcher:719–735](../../src/features/chat/MessageRenderer.vue#L719)。

因此原报告“只从稳定块 memo 中剥离流状态即可确保零重排零闪烁”的建议既不充分，也可能冻结必要的 UI 更新。

## 4. 本次验证与证据边界

已运行既有生产组件相关测试：

```powershell
pnpm exec vitest run src/tests/unit/chat/MessagePresentationRenderer.test.ts src/tests/unit/chat/StreamRenderCommit.test.ts src/tests/unit/chat/ToolBlockLazyMount.test.ts
```

结果：**3 个文件、37 项通过**。这些测试覆盖稳定块在普通 Tail 推进时保留、终态投影接管、工具/语音指导身份处理等；并未测量 GPU、帧时或证明普通 Markdown 终态不会重建。

另留档审查探针 [probes/eos_identity_probe.test.ts](./probes/eos_identity_probe.test.ts)，直接挂载生产 `MessageRenderer` 并驱动生产 `chatStreamStore`。它构造同一表格 AST 的三个对照分支：

| 对照条件 | 表格节点身份 | 预设节点 `scrollLeft = 73` 的结果 |
| --- | --- | --- |
| 仅移除 active 状态，不换块 | 同一节点 | 保留 73 |
| 收到 end，换新块对象但保留相同 key | 同一节点 | 保留 73 |
| 收到 end，按流式字符串 hash → 终态数值 hash 的协议差异换 key | 新节点，旧节点从 document 断开 | 新节点为 0 |

**3 项断言均通过。** 这将状态翻转、对象引用变化和 key 变化分离，证据指向终态身份交接。探针是复现当前行为的审查证据，不加入项目默认测试门禁；运行方式写在文件头，使用 `startVitest` 的临时 include，不改仓库配置。

边界：这是 happy-dom 中的 DOM 身份与节点状态断言；fixture 按已核实的 Rust DTO 形状构造，hash 值是示例，并未执行真实 Rust parser 生成这份样例。实际 hash 输入与输出差异由生产源码确认。本次**未**测量物理屏幕横向滚动、真实选区表现、Reflow、Paint、主线程长任务或 Android 掉帧，不能将它写成“真机已复现卡顿”。

仅新增审查文档和独立探针，没有修改核心/跨层生产逻辑，因此未把 `pnpm check` 或 Rust 全量编译作为本次文档审查的替代证据。

## 5. 建议的修复边界与验收问题

1. **优先处理终态身份交接。** 为语义相同、已经稳定的内容找到跨流式/终态一致的身份与分块边界；允许必要的内容修正替换，但避免把无内容变化的表格/代码/富文本宿主重新挂载。不能只去掉 memo 依赖，也不能把 hash 当业务身份强行同化。先用上面的对照探针确定目标，再补真实后端产物与前端联动验证。
2. **全文编译复用需要先统一契约。** 正常成功路径可以探索携带具有明确原文、schema、闭合状态的终态编译结果；错误追加、无内存 parser 的离线恢复和取消快照仍要有明确的编译来源。不要同时保留两套互不证明一致的永久权威缓存。
3. **保留事务正确性。** 内容 hash、renderer schema、active generation 与最终写入必须仍在仓储所有权下闭环；终态失败不能提前发正常 end，恢复不能使用与最终正文不符的块。
4. **设备验证针对因果。** 在具名 Android 设备上分别记录“最后一个 SSE 数据→最后 Aurora→编译结束→提交完成→end DOM patch”；使用长代码、多段 Markdown、含已闭合宽表格/公式/HTML preview 的消息。观察表格横向位置与文本选区是否保留，并测量编译耗时、事件体积、主线程任务与帧时。避免将整个 EOS 时段所有耗时归到 `v-memo`。

三方工程判断：Melchior 关注同构终态与持久化所有权；Balthasar 关注既有表格位置、选区与展开意图连续性；Casper 建议先修可复现的身份交接，再用数据决定是否扩大为双解析器统一，避免从一个 memo 数组修改演变为未经验证的终态协议重构。
