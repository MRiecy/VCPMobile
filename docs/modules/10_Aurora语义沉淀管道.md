---
id: MOD-AURORA-010
version: "1.1.0"
date: 2026-06-14
module: aurora_pipeline.rs
scope: src-tauri/src/vcp_modules/
related: [vcp_client.rs, stream_block_parser.rs, pre_renderer, sync_hash.rs, ast_diff.rs, ast-diff/专栏]
---

# 10_Aurora 语义沉淀管道（Aurora Pipeline）

## 1. 概述

### 1.1 模块定位

`aurora_pipeline.rs` 是 VCP Mobile 对话渲染 pipeline 中的**语义沉淀层（Semantic Precipitation Layer）**，位于 `src-tauri/src/vcp_modules/chat/aurora_pipeline.rs`（~237 行）。该模块运行在 Rust 后端，在 SSE（Server-Sent Events）流式传输过程中，对持续累积的响应文本进行**增量块解析**，产出「已确认闭合的语义块（Stable Blocks）」和「当前正在增长的尾部（Tail）」，通过 `StreamEvent::aurora` 推送到前端，实现增量式 UI 更新。

> **🆕 v1.1.0 重大更新**：AuroraBuffer 新增了增量 AST Diff 能力——`prev_tail_ast`、`pending_mutations`、`tail_epoch`/`tail_revision` 等 7 个字段，配合 `ast_diff.rs` 在每次 `process_queue` 时对 tail AST 做增量差异计算，产出 `AstMutation` 指令集。详见 **[增量AST Diff渲染引擎专栏](ast-diff/00_专栏总览与导读.md)**。本文件仅覆盖块级沉淀层的核心逻辑；Diff 算法、AstMutation 指令集和前端执行引擎见专栏各文档。

名称"Aurora"寓意：流式文本如极光般持续涌现，语义块如光带般逐渐凝固沉淀。

### 1.2 职责边界

| 职责领域 | 具体行为 | 对应源码位置 |
|---------|---------|------------|
| 全文累积 | 将每个 SSE text chunk 追加到内部 `full_text` | `append_chunk:41` |
| 增量块解析 | 调用 `StreamBlockParser::process` 识别新增的已闭合块 | `process_queue:56` |
| 推测渲染 | 将未闭合 tail 视为临时 Markdown 块，预渲染 AST 并计算 Hash | `process_queue:149` |
| 🆕 AST Diff | 对 tail 新旧 AST 做增量 diff，产出 AstMutation 指令集 | `process_queue:172-184` |
| 🆕 Epoch/Revision 管理 | 追踪 tail 世代更迭和增量修订，管理 reset/snapshot 生命周期 | `process_queue:134-142, 180-204` |
| HTML 标签平衡 | 对 tail 内容补全未闭合的 HTML 标签，防止 DOM 异常 | `balance_html_tags:99` |
| 流结束强制闭合 | 调用 `StreamBlockParser::finalize` 将剩余 tail 强制解析为块 | `finalize:220` |

### 1.3 在流式生命周期中的位置

```text
VCP 服务器 ──→ SSE data: {...delta.content...}
                    │
                    ▼
            ┌───────────────┐
            │ vcp_client.rs │
            │ 流式读取循环   │
            └───────┬───────┘
                    │ text_chunk
                    ▼
            ┌───────────────┐
            │ AuroraBuffer  │
            │  · append_chunk
            │  · process_queue
            └───────┬───────┘
                    │ AuroraUpdate
                    ▼
            ┌───────────────┐
            │ StreamEvent   │
            │  type="aurora"│
            └───────┬───────┘
                    │ Tauri Channel
                    ▼
              Vue 3 前端
            ┌───────────────┐
            │ 增量渲染层     │
            │ · stable_blocks → v-for 渲染（带 key）
            │ · tail_block → "正在输入..." 区域
            └───────────────┘
```

---

## 2. 核心类型与数据结构

### 2.1 AuroraUpdate（v1.1.0 重构）

```rust
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    /// 流式增量块：已确认闭合的语义块（仅 stable_changed 时发送）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_blocks: Option<Vec<StreamBlock>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stable_changed: bool,
    /// 推测块：当前正在增长的尾部（仅 tail_changed 时发送）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_block: Option<StreamBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tail_changed: bool,
    /// 🆕 流式 AST 单帧补丁（v1.1.0 新增核心字段）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_frame: Option<TailFrame>,
    /// 🆕 reset/recovery 使用的完整 tail AST 快照
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_snapshot: Option<Vec<MarkdownNode>>,
    /// 全量内容（仅终结事件时发送，正常流式中省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}
```

**🆕 v1.1.0 新增字段说明**：

| 字段 | 类型 | 发送条件 | 说明 |
|------|------|---------|------|
| `tail_frame` | `Option<TailFrame>` | 每次 `process_queue` 产生 mutations 或 reset 时 | 增量 AST 差异帧，含 epoch/revision/mutations |
| `tail_snapshot` | `Option<Vec<MarkdownNode>>` | reset 时或错误恢复时 | 完整 tail AST，供前端 snapshot 重建 |
| `stable_changed` | `bool` | stable_blocks 变化时为 true | 替代 v1.0.x 的空数组判断，`false` 时省略序列化 |
| `tail_changed` | `bool` | tail 内容变化时为 true | 同上，`false` 时省略序列化 |

**🆕 v1.1.0 稀疏序列化设计**：

所有字段均被 `#[serde(skip_serializing_if)]` 包裹——仅在字段有值时才包含在 JSON 中。这使得典型的流中增量 AuroraUpdate 载荷从 v1.0.x 的 ~2-8KB 缩减至 ~200-800 bytes。

> `stable_blocks` 和 `tail`、`content` 均改为 `Option` 类型——v1.0.x 中它们是必填字段，即使无变化也必须序列化为空数组/空字符串。v1.1.0 的稀疏序列化通过 `skip_serializing_if = "Option::is_none"` 完全消除这种浪费。

### 2.2 AuroraBuffer（v1.1.0 扩展）

```rust
pub struct AuroraBuffer {
    pub full_text: String,              // 累积的完整响应文本
    pub stable_blocks: Vec<StreamBlock>, // 已确认的语义块（对外只读镜像）
    pub tail_content: String,           // 当前未闭合的尾部纯文本
    tail_projection: Option<TailProjection>, // 仅保存 hash 与 AST/plain mode
    parser: StreamBlockParser,          // 增量块解析器（内部状态机）
    is_finishing: bool,                 // 是否已进入结束阶段（防重入锁）
    // ═══════ 🆕 v1.1.0 新增字段 ═══════
    pub prev_tail_ast: Vec<MarkdownNode>,            // 唯一 canonical tail AST，下一帧 Diff 的旧基准
    pub pending_mutations: Vec<AstMutation>,          // 待发送的增量 AST 突变指令暂存池
    pub tail_epoch: u64,                              // 纪元计数器（stable block 到达时递增）
    pub tail_revision: u64,                           // 纪元内修订计数器（每次 process_queue 产突变时递增）
    pub tail_reset_pending: bool,                     // 是否需前端全量重建 DOM
    pub tail_frame_seq: u64,                          // 单调全局帧序号（前端去重用）
}
```

**🆕 v1.1.0 新增字段详解**（详见 [AST Diff 专栏 - 02](ast-diff/02_Aurora管道Epoch体系与增量Diff算法.md) §2）：

| 字段 | 类型 | 作用 |
|------|------|------|
| `prev_tail_ast` | `Vec<MarkdownNode>` | 唯一 canonical tail AST，也是 `diff_ast()` 的"旧树"输入 |
| `tail_projection` | `Option<TailProjection>` | 常数级 hash/mode 元数据；Snapshot 时才构造完整 wire block |
| `pending_mutations` | `Vec<AstMutation>` | 待发送的突变指令暂存池，防抖丢帧时防止差异丢失 |
| `tail_epoch` | `u64` | 标识 tail 的不同"世代"——stable blocks 到达、tail 清空等时递增 |
| `tail_revision` | `u64` | 同一 epoch 内的增量计数——每次 `process_queue` 产突变时递增 |
| `tail_reset_pending` | `bool` | 为 true 时前端需清空 sandbox 并全量重建 |
| `tail_frame_seq` | `u64` | 单调递增的全局帧序号，前端用于去重乱序帧 |

---

## 3. 核心算法详解

### 3.1 标准处理循环

在 `vcp_client.rs` 的流式循环中，每收到一个非空 text chunk，执行以下三步：

```rust
// Step 1: 追加文本
aurora_buffer.append_chunk(&text_chunk);

// Step 2: 增量解析
let (stable_changed, tail_changed) = aurora_buffer.process_queue();

// Step 3: 条件触发事件推送
if stable_changed || tail_changed || last_aurora_send.elapsed().as_millis() > 50 {
    send_aurora_update(&aurora_buffer, None, None);
    last_aurora_send = std::time::Instant::now();
}
```

#### append_chunk

纯粹追加，无计算开销：

```rust
pub fn append_chunk(&mut self, chunk: &str) {
    self.full_text.push_str(chunk);
}
```

#### process_queue（v1.1.0 扩展——含 AST Diff）

```rust
pub fn process_queue(&mut self) -> (bool, bool) {
    if self.is_finishing { return (false, false); }

    let prev_stable_count = self.stable_blocks.len();
    let prev_tail = self.tail_content.clone();

    // 1. 增量解析全文，产出本次新增的已闭合块 + 尾部纯文本
    let (new_blocks, new_tail) = self.parser.process(&self.full_text);

    // 🆕 1a. Epoch 重置：新稳定块到达时
    if !new_blocks.is_empty() {
        self.stable_blocks.extend(new_blocks);
        self.tail_epoch = self.tail_epoch.saturating_add(1);  // 纪元 +1
        self.tail_revision = 0;
        self.tail_reset_pending = true;                       // 通知前端全量重建
        self.pending_mutations.clear();
        self.prev_tail_ast.clear();
    }

    self.tail_content = new_tail;

    // 2. 推测渲染（Speculative Rendering）
    if !self.tail_content.is_empty() {
        let nodes = if self.tail_content.len() > MAX_SPECULATIVE_TAIL_AST_BYTES {
            None  // 超过 64KB → 跳过 AST，降级到纯文本
        } else if is_html_tag_block(&self.tail_content) {
            Some(vec![MarkdownNode::raw_html(self.tail_content.clone())])
        } else {
            Some(parse_markdown_to_ast_streaming(&self.tail_content))
        };
        let hash = HashAggregator::compute_content_hash(&self.tail_content);

        // 🆕 2a. AST Diff：对 tail AST 做增量差异计算
        let mode = if let Some(mut new_nodes) = nodes {
            for node in &mut new_nodes {
                node.compute_hashes_recursively();
            }
            let mutations = diff_ast(&self.prev_tail_ast, &new_nodes, "t");
            if !mutations.is_empty() {
                self.pending_mutations.extend(mutations);
            }
            self.tail_revision = self.tail_revision.saturating_add(1);
            self.prev_tail_ast = new_nodes;
            TailRenderMode::Ast
        } else {
            // 超长 tail → epoch reset
            self.prev_tail_ast.clear();
            if !self.tail_reset_pending {
                self.tail_epoch = self.tail_epoch.saturating_add(1);
                self.tail_revision = 0;
                self.tail_reset_pending = true;
            }
            self.pending_mutations.clear();
            TailRenderMode::Plain
        };

        self.tail_projection = Some(TailProjection { hash, mode });
    } else {
        self.tail_projection = None;
        if !self.prev_tail_ast.is_empty() || !self.tail_content.is_empty() {
            self.tail_epoch = self.tail_epoch.saturating_add(1);
            self.tail_revision = 0;
            self.tail_reset_pending = true;
            self.pending_mutations.clear();
        }
        self.prev_tail_ast.clear();
    }

    let stable_changed = self.stable_blocks.len() != prev_stable_count;
    let tail_changed = self.tail_content != prev_tail;
    (stable_changed, tail_changed)
}
```

> **🆕 AST Diff 算法的详细分析**（`diff_ast()`、Epoch/Revision 状态机、AppendText 优化等）见 **[AST Diff 专栏 - 02_Aurora管道Epoch体系与增量Diff算法](ast-diff/02_Aurora管道Epoch体系与增量Diff算法.md)**。

### 3.2 take_tail_frame（🆕 v1.1.0 新增）

`take_tail_frame()` 是兼容测试入口；生产发送采用 prepare → send → commit。reset Snapshot 在 prepare 时直接从最新 `prev_tail_ast` 按需构造，不再常驻第二棵 AST。

```rust
pub fn take_tail_frame(&mut self) -> Option<TailFrame> {
    let frame = self.peek_tail_frame(false)?;
    self.tail_reset_pending = false;
    self.pending_mutations.clear();
    self.tail_frame_seq = frame.frame_seq;
    Some(frame)
}
```

> 当 `reset=true` 时，`mutations` 强制清空——前端将全量重建 DOM，增量突变无意义。

### 3.3 finalize — 流结束强制闭合（v1.1.0 更新）

当 SSE 流正常结束（`[DONE]`）、被中止、或意外断开时，`vcp_client.rs` 调用 `finalize()`：

```rust
pub fn finalize(&mut self) {
    if self.is_finishing { return; }
    self.is_finishing = true;
    let final_new_blocks = self.parser.finalize(&self.full_text);

    self.stable_blocks.extend(final_new_blocks);
    self.tail_content.clear();
    self.tail_projection = None;
    // 🆕 v1.1.0: 清空 AST Diff 状态，发送空 snapshot 以清空前端 tail DOM
    self.prev_tail_ast.clear();
    self.pending_mutations.clear();
    self.tail_epoch = self.tail_epoch.saturating_add(1);
    self.tail_revision = 0;
    self.tail_reset_pending = true;
}
```

- `is_finishing` 防重入：确保多次调用（如中止后立即收到 `[DONE]`）不会重复解析
- `parser.finalize` 与 `process` 的区别：
  - `process` 只识别**已确认闭合**的块（遇到未闭合标记会保留在 tail）
  - `finalize` 将剩余内容**强制封装**为最后一个 Markdown 块，无论是否闭合
- 调用后 `tail_content` 和 `tail_projection` 被清空，意味着前端"正在输入"区域应消失

### 3.3 balance_html_tags — HTML 标签补全

```rust
pub fn balance_html_tags(html: &str) -> String {
    let tags = ["div", "pre", "code", "p", "span", "blockquote"];
    let mut balanced = html.to_string();
    for tag in tags {
        let open_count = html.matches(&format!("<{tag}>")).count()
            + html.matches(&format!("<{tag} ")).count();
        let close_count = html.matches(&format!("</{tag}>")).count();
        if open_count > close_count {
            balanced.push_str(&format!("</{tag}>").repeat(open_count - close_count));
        }
    }
    balanced
}
```

**问题背景**：
- 流式输出可能在 HTML 标签中间截断（如 `<div>内容`）
- 前端将 `tail` 直接作为 innerHTML 插入时，未闭合标签会导致后续 DOM 结构错乱

**算法策略**：
- 仅关注最常见的块级/行内标签：div、pre、code、p、span、blockquote
- 统计开标签数（`<tag>` 和 `<tag `）与闭标签数（`</tag>`）
- 在尾部追加缺失的闭标签

**限制**：
- 不处理自闭合标签（`<img>`、`<br>`）
- 不处理嵌套深度的正确性（仅保证数量平衡）
- 不处理属性值中包含 `>` 的边界情况（极罕见）

---

## 4. 事件推送策略

### 4.1 触发条件

在 `vcp_client.rs` 流式循环中，`aurora` 事件在以下任一条件下推送：

| 条件 | 语义 | 设计意图 |
|------|------|---------|
| `stable_changed = true` | 有新的块被确认闭合 | 确保前端立即渲染新确认的块，减少延迟 |
| `tail_changed = true` | 尾部文本发生变化 | 确保"正在输入"区域实时更新 |
| `elapsed > 50ms` | 距离上次推送超过 50ms | 时间节流：即使 stable/tail 未变（如长空白），也定期同步状态 |

### 4.2 中止路径上的 Aurora 行为

当用户在流式输出中点击"停止"：

1. `interruptRequest` → `oneshot::Sender::send(())`
2. 流式循环的 `tokio::select!` 捕获中止信号
3. 调用 `aurora_buffer.finalize()` —— 强制闭合剩余内容
4. 发送最终 `aurora` 事件：`finish_reason = "cancelled_by_user"`，`error = "请求已中止"`
5. `vcp_client.rs` 外层 `sendToVCP` 再发送 `end` 事件

这意味着前端会先收到一个带 `error` 的 `aurora` 事件（更新最终块状态），再收到 `end` 事件（终止输入动画）。

### 4.3 错误路径上的 Aurora 行为

当 SSE 读取发生错误（网络断开、流解析异常）：

1. 调用 `aurora_buffer.finalize()`
2. 发送 `aurora` 事件：`finish_reason = "error"`，`error = "流读取错误/网络连接意外断开"`
3. 同时发送 `error` 类型的 `StreamEvent`

---

## 5. 错误处理与边界情况

### 5.1 process_queue 的防重入

- `is_finishing` 为 `true` 时，`process_queue` 直接返回 `(false, false)`
- 这发生在 `finalize()` 之后：即使流结束后意外收到额外 chunk，也不会破坏已稳定的块列表

### 5.2 空 tail 处理

- 当 `new_tail` 为空字符串时，`tail_block` 被设为 `None`
- 前端应处理 `tail_block = None` 的情况：隐藏"正在输入"区域或显示空白

### 5.3 全文本为空时的 finalize

- 若整个流没有任何文本 chunk（如纯工具调用响应），`finalize()` 不会产出任何块
- `stable_blocks` 保持为空，`tail_content` 被清空

---

## 6. 性能特征

| 指标 | 数值/策略 | 说明 |
|------|----------|------|
| 解析器复杂度 | `StreamBlockParser::process` 为 O(n)，n = 新增文本长度 | 基于 `processed_len` 游标的增量扫描 |
| 推测渲染开销 | 每次 `process_queue` 调用 `parse_markdown_to_ast` | 尾部通常较短（数十到数百字符），开销可控 |
| Hash 计算 | `compute_content_hash` 基于 Rust 默认 Hasher | 单次计算 O(n)，n = tail 长度 |
| 事件节流 | 50 ms | 避免前端在高频 chunk 场景下过度重渲染 |
| 内存占用 | `full_text` 累积全文 + `stable_blocks` 累积块 | 与响应长度线性相关；长响应（>100KB）应考虑是否需要在 `finalize` 后释放 `full_text` |

---

## 7. 与相关模块的关系

### 7.1 上游：vcp_client.rs

`aurora_pipeline.rs` 本身无 Tauri Command，完全由 `vcp_client.rs` 的流式循环驱动：

- `vcp_client.rs:508` —— `AuroraBuffer::new()`
- `vcp_client.rs:592` —— `aurora_buffer.append_chunk()`
- `vcp_client.rs:593` —— `aurora_buffer.process_queue()`
- `vcp_client.rs:572, 540, 558, 624` —— `aurora_buffer.finalize()`
- `vcp_client.rs:519` —— `AuroraBuffer::balance_html_tags()`

详见 [09_VCP请求客户端](09_VCP请求客户端.md) §3.4。

### 7.2 下游：pre_renderer

`process_queue` 中的推测渲染调用 `pre_renderer::parse_markdown_to_ast`，产出 `Vec<MarkdownNode>` AST。该预渲染器是前端渲染逻辑的后端镜像，确保前后端对 Markdown 的解析结果一致。

### 7.3 同层：stream_block_parser.rs

`AuroraBuffer` 内部持有 `StreamBlockParser`，使用其 `process()` 和 `finalize()` 方法。两者的关系详见 [02_流式响应解析器](02_流式响应解析器.md)。

简要对比：

| 维度 | `StreamBlockParser`（在 AuroraBuffer 内） | `content_parser.rs`（非流式） |
|------|------------------------------------------|------------------------------|
| 调用者 | `aurora_pipeline.rs` | `message_repository.rs` 等 |
| 生命周期 | 随 SSE 流持续存在，增量更新 | 消息完全接收后一次性调用 |
| 输出 | `Vec<StreamBlock>` + `tail: String` | `Vec<ContentBlock>` |
| 状态 | 有状态（`processed_len`） | 无状态（纯函数） |

### 7.5 🆕 同层：ast_diff.rs（v1.1.0 新增）

`ast_diff.rs` 是 v1.1.0 新增的 AST Diff 核心算法模块，由 `AuroraBuffer::process_queue` 调用：

- `diff_ast(&self.prev_tail_ast, &new_nodes, "t")` —— 计算旧/新 tail AST 的差异，产出 `Vec<AstMutation>`
- 所有 Diff 细节（Epoch/Revision 状态机、8 种 AstMutation 指令、前端执行引擎）见 **[增量AST Diff渲染引擎专栏](ast-diff/00_专栏总览与导读.md)**

| 维度 | `aurora_pipeline.rs` | `ast_diff.rs` |
|------|---------------------|---------------|
| 职责 | 状态管理、流程编排、事件发送 | 纯算法：AST 差异计算 |
| 状态 | 有状态（AuroraBuffer 12 字段） | 无状态（纯函数） |
| 输入 | SSE text chunks | `(&[MarkdownNode], &[MarkdownNode], prefix)` |
| 输出 | `AuroraUpdate` (via `take_tail_frame`) | `Vec<AstMutation>` |
| 行数 | ~237 | ~516（含测试） |

---


*最后更新：2026-06-14 | VCP Mobile v1.1.0*
*文档基于 `src-tauri/src/vcp_modules/chat/aurora_pipeline.rs`（~237行）及 `src-tauri/src/vcp_modules/chat/ast_diff.rs`（~516行）的源码分析生成。*
