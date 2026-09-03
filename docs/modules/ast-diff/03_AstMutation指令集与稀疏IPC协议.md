---
id: AST-DIFF-003
title: AstMutation 指令集与稀疏 IPC 协议 (AstMutation Instruction Set & Sparse IPC Protocol)
module: ast_diff.rs (AstMutation enum) + aurora_pipeline.rs (AuroraUpdate, TailFrame)
related: [chat.ts (types), astExecutor.ts, chatStreamStore.ts]
version: "1.1.0"
last_updated: 2026-09-03
---

# 03_AstMutation 指令集与稀疏 IPC 协议

## 1. 概述

### 1.1 模块定位

AstMutation 指令集是 Rust 后端与 Vue 3 前端之间的**通用协议（Contract / Wire Protocol）**。它定义了 10 种最小粒度的 DOM 操作指令，Rust 侧通过通用 `diff_ast()` 或 Aurora `diff_ast_streaming()` 产生，前端通过 `astExecutor.ts` 执行。

这套协议的三大设计目标：

1. **最小化 IPC 载荷**：只传输变化的部分，而非完整的 HTML 字符串
2. **确定性寻址**：每条指令通过路径 ID 精确指向目标 DOM 节点
3. **前端无状态解析**：前端不需要理解 Markdown，只需"机械地"执行指令

### 1.2 从 SSE 到 DOM 的完整链路

```mermaid
sequenceDiagram
    participant SSE as SSE Stream
    participant VC as vcp_client.rs
    participant AB as AuroraBuffer
    participant DF as ast_diff.rs
    participant Event as Tauri Event
    participant CSS as chatStreamStore
    participant MR as MessageRenderer
    participant AE as astExecutor.ts
    participant DOM as Browser DOM

    SSE->>VC: SSE text chunk
    VC->>VC: pending 合并，固定33ms最多 flush 一次
    VC->>AB: append_chunk + process_queue
    AB->>DF: diff_ast_streaming(prev_tail, new_tail)
    DF-->>AB: Vec<AstMutation>
    AB->>VC: prepare_delta_update() → TailTextOp + TailFrame
    VC->>Event: emit("vcp-stream-event", { aurora: AuroraUpdate })
    Event->>CSS: processStreamEvent (type="aurora")
    CSS->>CSS: mergeTailFrame + 下一次 rAF 前原子合并
    CSS->>MR: ChatMessage.tailFrame = TailFrame
    MR->>AE: applyFrame(mutations, messageId, sandbox)
    AE->>DOM: appendData / replaceChild / setAttribute

    Note over DF,Event: 🔑 v1.1.0 新增路径
```

---

## 2. AstMutation —— 10 种突变指令

### 2.1 指令总览

定义于 `ast_diff.rs:4-35`，Rust 侧：

```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "op")]
pub enum AstMutation {
    Add { id, parent, node: MarkdownNode },                    // op="add"
    AddInline { id, parent, node: InlineNode },                // op="add_inline"
    AddListItem { id, parent, children: Vec<MarkdownNode> },   // op="add_list_item"
    UpdateText { id, value: String },                          // op="text"
    AppendText { id, chunk: String },                          // op="append"
    UpdateProp { id, key, value: String },                     // op="prop"
    Replace { id, node: MarkdownNode },                        // op="replace"
    PatchCode { id, completed_html, active_html },             // op="patch_code"
    ReplaceInline { id, node: InlineNode },                    // op="replace_inline"
    Remove { id },                                             // op="remove"
}
```

TypeScript 镜像（`chat.ts:126-134`）：

```typescript
export type AstMutation =
  | { op: "add"; id: string; parent: string; node: MarkdownNode }
  | { op: "add_inline"; id: string; parent: string; node: InlineNode }
  | { op: "add_list_item"; id: string; parent: string; children: MarkdownNode[] }
  | { op: "text"; id: string; value: string }
  | { op: "append"; id: string; chunk: string }
  | { op: "prop"; id: string; key: string; value: string }
  | { op: "replace"; id: string; node: MarkdownNode }
  | { op: "patch_code"; id: string; completed_html: string; active_html: string }
  | { op: "replace_inline"; id: string; node: InlineNode }
  | { op: "remove"; id: string };
```

> **Serde 标签映射**：Rust 的 `#[serde(rename = "...")]` 确保 `UpdateText` → `"text"`、`AppendText` → `"append"`、`UpdateProp` → `"prop"`，与 TS 侧的 `op` 字段完全一致。

### 2.2 AppendText —— 热路径之星

```rust
AppendText { id: String, chunk: String }
```

| 属性 | 值 |
|------|-----|
| 在流式场景中的占比 | **~60-70%** 的突变是 AppendText |
| 触发条件 | 新 Text 值以旧 Text 值为前缀（`strip_prefix`） |
| 前端执行 | `textNode.appendData(chunk)` —— 零 DOM 重排 |
| 典型载荷 | `{"op":"append","id":"t0.i0","chunk":" 新增的文本"}` |
| 设计考量 | 不能简单地在父节点下加 TextNode（会累积大量碎片节点） |

**为什么 AppendText 比 Add(Text) + Remove 更优**：

| 方案 | 操作 | DOM 影响 |
|------|------|---------|
| ❌ Add + Remove | 删除旧 TextNode → 创建新 TextNode → appendChild | 触发 reflow |
| ✅ AppendText | `textNode.appendData(chunk)` | 原地修改，0 reflow |

### 2.3 UpdateText —— 全量文本替换

```rust
UpdateText { id: String, value: String }
```

| 属性 | 值 |
|------|-----|
| 在流式场景中的占比 | ~5-10% |
| 触发条件 | 新 Text 值不以旧值为前缀（非单调增长） |
| 前端执行 | `node.textContent = value` |
| 典型场景 | Agent 在流式中修改了前面已输出的文本，或 Markdown 解析导致文本结构重组 |

> `UpdateText` 和 `AppendText` 是互斥的——`diff_text_node` 中通过 `strip_prefix` 自动选择最优路径。

### 2.4 Add / AddInline —— 新增节点

```rust
Add { id: String, parent: String, node: MarkdownNode }
AddInline { id: String, parent: String, node: InlineNode }
```

| 属性 | 值 |
|------|-----|
| 触发条件 | 新 AST 在尾部有旧 AST 中不存在的节点 |
| `parent` 取值 | `"root"`（挂载到 sandbox）或父节点的路径 ID（如 `"t0"`, `"t0.i0"`) |
| 前端执行 | `createDomFromNode(node, id, registry)` → `parentNode.appendChild(dom)` |
| CSS 动画 | 仅开启“平滑流式输出”时，paragraph / heading / blockquote / list / table 的块级 `Add` 播放一次固定 250ms 的斜向柔边揭露；code / thematic break / raw HTML 与 `AddInline` 静态挂载 |

### 2.4b AddListItem —— 新增列表项

```rust
AddListItem { id: String, parent: String, children: Vec<MarkdownNode> }
```

| 属性 | 值 |
|------|-----|
| 语义 | 新增一个列表项 (`<li>`)。列表项是「多个块级节点」的集合（`Vec<MarkdownNode>`），无法用 Add 的单一 `MarkdownNode` 表达，故单列一个变体 |
| `id` 取值 | `<li>` 路径 ID（如 `"t3.li5"`） |
| `parent` 取值 | 列表 `<ul>`/`<ol>` 的 ID（如 `"t3"`） |
| 触发条件 | 流式列表新增尾部项时（见 02 文档的 List diff）。删除列表项则复用通用 `Remove { id: "t3.li5" }` |
| 前端执行 | `astExecutor.ts` 的 `"add_list_item"` case：在存活的 `<ul>`/`<ol>` 下创建 `<li>`，按 `{id}.b{n}` 注册其块级子节点，追加到列表末尾 |
| CSS 动画 | 仅开启“平滑流式输出”且祖先没有正在揭露的块时，新增 `<li>` 播放一次固定 250ms 的斜向柔边揭露 |

> **取代旧行为**：这取代了过去「列表项数量变化就整体 Replace 整个 list」的低效行为（O(n²) 重建）。现在新增尾部项只产生一条 AddListItem，已有列表项的 DOM 与 registry 完全保留。

### 2.5 PatchCode —— 流式代码尾部补丁

```rust
PatchCode { id: String, completed_html: String, active_html: String }
```

| 字段 | 语义 |
|------|------|
| `completed_html` | 本帧新形成的一个或多个完整高亮行，只追加到稳定区 |
| `active_html` | 当前尚未换行的末行高亮，替换活跃行容器内容 |

首次 Add/恢复 Snapshot 会建立 `data-vcp-code-stable` 与 `data-vcp-code-active` 两个锚点。普通追加帧不再携带完整 `CodeBlock.code` 或完整 `highlighted_html`；若语言、前缀或 DOM 锚点不一致，则回退到一次 Replace/恢复 Snapshot。

### 2.6 Replace / ReplaceInline —— 节点替换

```rust
Replace { id: String, node: MarkdownNode }
ReplaceInline { id: String, node: InlineNode }
```

| 属性 | 值 |
|------|-----|
| 触发条件 | 类型相同但内容差异过大无法增量表达（如代码块内容全部重写、表格数据变化） |
| 前端策略 | **四级替换策略**（见 04 文档 §5），根据节点类型自动选择最优方式 |
| 典型载荷 | `{"op":"replace","id":"t0","node":{"type":"paragraph","children":[...]}}` |

**前端的四级替换决策树**：

```
Replace 节点类型?
  ├─ code_block → 策略 A: 原地 innerHTML 覆盖（保留 pre 外壳）
  ├─ mermaid   → 策略 B: 原地 textContent 覆盖（保留 placeholder）
  ├─ raw_html / table → 策略 C: morphdom 局部 DOM diff（保留媒体状态）
  └─ default   → 策略 D: createDomFromNode + replaceChild
```

> **策略 C 的 registry 一致性**：块级 `raw_html`/`table` 始终是「整节点全替换」，从不做子节点级 diff。因此 morphdom 执行后，注册表只保留**根 ID**（映射回页面上存活的 `oldNode`，而非被 morphdom 丢弃的临时 `newDom`），其余后代条目一律清除。后代条目不可从临时 registry 的后代节点取用——那些节点可能已被 morphdom 抛弃。行内容器（link/strong/emphasis 等）的 registry 重建规则更复杂（需在 morphdom 前后对路径求值），详见 04 文档 §5。

### 2.7 Remove —— 节点删除

```rust
Remove { id: String }
```

| 属性 | 值 |
|------|-----|
| 触发条件 | 旧 AST 尾部有新 AST 中不存在的节点 |
| 前端执行 | `parentNode.removeChild(node)` + `cleanupSubtreeRefs(id)` |
| 典型场景 | Tail 内容收缩（如 Agent 删除了未完成的段落） |

### 2.8 UpdateProp —— 属性变更

```rust
UpdateProp { id: String, key: String, value: String }
```

| 属性 | 值 |
|------|-----|
| 触发条件 | 节点类型相同但属性值变化（目前仅 Heading level 变化触发） |
| 前端执行 | 对 `level` 属性：创建新 `<hN>` 元素 + replaceChild；对其它属性：`setAttribute(key, value)` |

---

## 3. TailFrame —— 帧信封

### 3.1 结构定义

定义于 `aurora_pipeline.rs:10-22`：

```rust
pub struct TailFrame {
    pub epoch: u64,          // 纪元编号
    pub revision: u64,       // 纪元内修订号
    pub frame_seq: u64,      // 全局单调帧序号（前端去重用）
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset: bool,         // 是否为 epoch 重置帧
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Vec<MarkdownNode>>,  // reset 时的全量 AST 快照
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mutations: Vec<AstMutation>,           // 增量突变指令
}
```

### 3.2 帧的三种形态

| 帧类型 | reset | snapshot | mutations | 前端行为 |
|--------|:-----:|:--------:|:---------:|---------|
| **增量帧** | false | None | 非空 | `applyFrame(mutations)` → 逐条执行 |
| **重置帧** | true | Some(nodes) | 空 | 清空 sandbox → `rebuildSnapshot(snapshot)` |
| **清空帧** | true | Some([]) | 空 | 清空 sandbox（finalize 时发送） |

### 3.3 frame_seq 去重

```rust
// aurora_pipeline.rs:110
self.tail_frame_seq = self.tail_frame_seq.saturating_add(1);
```

前端使用 `lastAppliedFrameSeq` 跟踪已应用的最后一帧序号：

```typescript
// MessageRenderer.vue:820-822
if (frame.frameSeq <= lastAppliedFrameSeq) {
    return;  // 已应用或乱序帧，直接跳过
}
```

这防止了以下情况：
- **重复帧**：如果 rAF 合并导致同一帧被多次触发
- **乱序帧**：如果网络延迟导致旧帧在新帧之后到达

### 3.4 reset 与 mutations 的互斥关系

当 `reset=true` 时，`mutations` **强制为空**：
```rust
// aurora_pipeline.rs:117
mutations: if reset { Vec::new() } else { mutations },
```

设计原因：epoch 重置意味着 DOM 需要全量重建，在已清空的 sandbox 上执行增量 mutations 无意义。

---

## 4. AuroraUpdate —— 顶层 IPC 事件

### 4.1 结构定义

定义于 `aurora_pipeline.rs:26-50`：

```rust
pub struct AuroraUpdate {
    pub kind: AuroraUpdateKind,
    pub stream_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_blocks: Option<Vec<StreamBlock>>,   // Snapshot 全量基线
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_append: Option<StableAppend>,         // Delta 仅追加新稳定块
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_block: Option<StreamBlock>,             // Snapshot tail 基线
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_op: Option<TailTextOp>,                 // Delta 原文 append/replace/clear
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_frame: Option<TailFrame>,               // AST 增量帧或 reset snapshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,                     // Snapshot/恢复全量正文
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk: Option<String>,                       // 新增原始正文
}
```

`TailTextOp::Append/Replace` 同时携带 `blockType: "markdown" | "html-preview"`。它是当前 tail 的渲染投影类型，不是第二份解析状态：后端仍以 canonical Markdown AST 为唯一识别结果。HTML 围栏 tail 只在组件外壳上与普通代码区分，正文继续消费同一个 `TailFrame`。`Append.previousHash` 是应用追加前前端应持有的 tail hash；`hash` 是追加后的结果 hash。

### 4.2 稀疏序列化策略

AuroraUpdate 使用**大量 `skip_serializing_if`** 来实现稀疏序列化——只传输变化的字段，减少 IPC 载荷：

```mermaid
graph TB
    subgraph Full["完整 AuroraUpdate（理论最大）"]
        SI["streamId"]
        SB["stableBlocks: [StreamBlock; N]"]
        TB["tailBlock: StreamBlock"]
        TM["tailMode"]
        TF["tailFrame: TailFrame"]
        CT["content: String"]
    end

    subgraph Typical["典型 AuroraUpdate（流中增量）"]
        SB2["❌ 省略（stable 未变）"]
        SA2["stableAppend: 仅新增稳定块时出现"]
        TO2["tailOp: append / replace / clear"]
        TF2["tailFrame: { streamId, epoch, revision, frameSeq, mutations }"]
        CT2["❌ 省略（非终结）"]
    end

    style SB2 fill:#ff4444,color:#fff
    style CT2 fill:#ff4444,color:#fff
```

**典型载荷大小对比**：

| 场景 | v1.0.3 AuroraUpdate | v1.1.0 AuroraUpdate | 缩减比 |
|------|---------------------|---------------------|:------:|
| 流中增量（tail 200 chars） | ~1.5KB（含完整 HTML tail） | ~300 bytes（稀疏 TailFrame） | **~80%** |
| 稳定块到达 | ~4KB（含完整 HTML blocks） | ~800 bytes（AST blocks + TailFrame reset） | **~80%** |
| 流结束 | ~8KB（含完整 content） | ~8KB（content 相同） | 持平 |
| 空帧（tail 未变） | ~100 bytes | 0 bytes（不发送 tail_frame） | **100%** |

### 4.3 前端接收与合并

在 `chatStreamStore.ts` 中，`type === "aurora"` 事件的处理流程：

```typescript
// chatStreamStore.ts（简化）
if (type === "aurora") {
    const aurora = event.aurora;
    let update = rAFPendingUpdates.get(actualMessageId);

    // Snapshot 覆盖基线；Delta 只追加新稳定块。
    if (aurora.content !== undefined) update.content = aurora.content;
    if (aurora.kind === "snapshot") update.blocks = aurora.stableBlocks;
    if (aurora.stableAppend) {
        update.blocks = [...currentStable, ...aurora.stableAppend.blocks];
    }
    if (aurora.tailOp?.op === "replace") {
        update.tailBlock = {
            type: aurora.tailOp.blockType,
            content: aurora.tailOp.content,
            hash: aurora.tailOp.hash,
            render_mode: aurora.tailOp.mode,
        };
    }
    if (aurora.tailFrame) {
        update.tailFrame = mergeTailFrame(update.tailFrame, aurora.tailFrame);
    }

    // rAF 仅将到达事件与下一次绘制对齐；33ms 频率门禁只在 Rust 侧。
    if (update.animationFrameId === null) {
        update.animationFrameId = requestAnimationFrame(runRenderLoop);
    }
}
```

**`mergeTailFrame` 合并策略**（`chatStreamStore.ts:43-62`）：

```typescript
function mergeTailFrame(existing, incoming) {
    if (!existing || incoming.reset || incoming.epoch !== existing.epoch) {
        // Epoch 不同或首次或 reset → 替换整个帧
        return { ...incoming, mutations: incoming.reset ? [] : [...incomingMutations] };
    }
    // 同 Epoch → 合并 mutations
    return {
        ...incoming,
        mutations: [...existing.mutations, ...incomingMutations],
    };
}
```

> **合并的意义**：在高频 SSE 流中，两个 AuroraUpdate 可能在同一个 rAF 间隔内到达。mergeTailFrame 将它们合并为一个帧，减少 Vue 响应式触发次数和 DOM 操作批次。

---

## 5. 完整 IPC 生命周期

```mermaid
sequenceDiagram
    participant SSE as VCP Server
    participant VC as vcp_client.rs
    participant AB as AuroraBuffer
    participant IPC as Tauri Event
    participant CSS as chatStreamStore
    participant Vue as MessageRenderer
    participant AE as astExecutor
    participant DOM as Sandbox DOM

    Note over SSE,DOM: Phase 1: 流式输出进行中

    loop 每33ms最多一次，终结时强制 flush
        SSE->>VC: SSE data chunk
        VC->>VC: pending chunk 合并
        VC->>AB: append_chunk + process_queue
        AB->>AB: diff_ast_streaming → mutations
        AB->>VC: prepare_delta_update()
        VC->>IPC: emit("vcp-stream-event", { aurora: { tailOp, tailFrame } })
        IPC->>CSS: processStreamEvent
        CSS->>CSS: mergeTailFrame + 暂存字段
        Note over CSS: 下一次 VSync 原子提交，无第二个33ms门禁
        CSS->>Vue: ChatMessage.tailFrame = TailFrame
        Vue->>AE: applyFrame(mutations, id, sandbox)
        AE->>DOM: appendData / PatchCode / replaceChild
    end

    Note over SSE,DOM: Phase 2: 新的稳定块到达

    SSE->>VC: SSE data chunk (含完整语义块)
    VC->>AB: append_chunk + process_queue
    AB->>AB: new stable_block → epoch=4, rev=0, reset=true
    AB->>VC: prepare_delta_update() → stableAppend + reset snapshot
    VC->>IPC: emit(..., { stableAppend, tailFrame })
    IPC->>CSS: processStreamEvent
    CSS->>Vue: ChatMessage.tailFrame = TailFrame + append stableBlocks
    Vue->>DOM: 清空 sandbox
    Vue->>AE: rebuildSnapshot(snapshot, id, sandbox)
    AE->>DOM: 重建 tail DOM 子树 + 填充 registry

    Note over SSE,DOM: Phase 3: 流结束

    SSE->>VC: SSE stream end
    VC->>AB: finalize()
    AB->>VC: prepare_snapshot_update() + durable final blocks
    VC->>IPC: emit(..., { tailFrame, content, blocks })
    IPC->>CSS: clearRAFUpdate(id, forceFlush=true)
    CSS->>Vue: ChatMessage.blocks = finalBlocks, tailFrame = empty frame
    Vue->>DOM: 清空 tail sandbox，渲染稳定 blocks
```

---

## 6. 与相关协议的关系

| 协议/模块 | 层级 | 说明 |
|----------|------|------|
| `StreamBlock` SSR | Rust → Frontend | 稳定块的结构化传输（含 AST 预渲染），流式期间使用 |
| `ContentBlock` SSR | Rust → Frontend | 消息完全渲染后的最终块（非流式），历史消息加载使用 |
| **`AstMutation`** | Rust → Frontend | **v1.1.0 新增**——流式 tail 的增量 DOM 指令 |
| **`TailFrame`** | Rust → Frontend | **v1.1.0 新增**——单次 Diff 结果的帧封装 |
| **`AuroraUpdate`** | Rust → Frontend | **v1.1.0 重构**——Aurora 事件的稀疏容器，同时容纳 stable/tail/frame |
| `vcp-stream-event` | Tauri Event | Tauri 的 SSE 事件通道，`type` 字段区分 `"aurora"`/`"chunk"`/`"end"` 等 |

---

*文档基于 `src-tauri/src/vcp_modules/chat/ast_diff.rs`（AstMutation enum）、`src-tauri/src/vcp_modules/chat/aurora_pipeline.rs`（TailFrame + AuroraUpdate）及 `src/core/types/chat.ts`（TypeScript 镜像类型）的源码分析生成。*
