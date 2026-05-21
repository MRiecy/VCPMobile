---
id: MOD-AURORA-010
version: "1.0"
date: 2026-05-21
module: aurora_pipeline.rs
scope: src-tauri/src/vcp_modules/
related: [vcp_client.rs, stream_block_parser.rs, pre_renderer, sync_hash.rs]
---

# 10_Aurora 语义沉淀管道（Aurora Pipeline）

## 1. 概述

### 1.1 模块定位

`aurora_pipeline.rs` 是 VCP Mobile 对话渲染 pipeline 中的**语义沉淀层（Semantic Precipitation Layer）**，位于 `src-tauri/src/vcp_modules/aurora_pipeline.rs`（112 行）。该模块运行在 Rust 后端，在 SSE（Server-Sent Events）流式传输过程中，对持续累积的响应文本进行**增量块解析**，产出「已确认闭合的语义块（Stable Blocks）」和「当前正在增长的尾部（Tail）」，通过 `StreamEvent::aurora` 推送到前端，实现增量式 UI 更新。

名称"Aurora"寓意：流式文本如极光般持续涌现，语义块如光带般逐渐凝固沉淀。

### 1.2 职责边界

| 职责领域 | 具体行为 | 对应源码位置 |
|---------|---------|------------|
| 全文累积 | 将每个 SSE text chunk 追加到内部 `full_text` | `append_chunk:41` |
| 增量块解析 | 调用 `StreamBlockParser::process` 识别新增的已闭合块 | `process_queue:56` |
| 推测渲染 | 将未闭合 tail 视为临时 Markdown 块，预渲染 AST 并计算 Hash | `process_queue:66` |
| HTML 标签平衡 | 对 tail 内容补全未闭合的 HTML 标签，防止 DOM 异常 | `balance_html_tags:99` |
| 流结束强制闭合 | 调用 `StreamBlockParser::finalize` 将剩余 tail 强制解析为块 | `finalize:91` |

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

### 2.1 AuroraUpdate

```rust
#[derive(Debug, Serialize, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuroraUpdate {
    pub stable_blocks: Vec<StreamBlock>,   // 已确认闭合的语义块（只增不减）
    pub tail_block: Option<StreamBlock>,   // 推测块：当前未闭合尾部
    pub tail: String,                      // 平衡后的尾部 HTML 字符串
    pub content: String,                   // 全文原始文本（用于调试或全量回退）
}
```

- `stable_blocks` 是**单调递增**的：已确认的块不会回退或修改，前端可以安全地追加渲染
- `tail_block` 是**易失的**：每次 `process_queue` 都可能被替换或清空，前端应将其视为临时状态
- `tail` 是经 `balance_html_tags` 处理后的字符串，可直接插入 DOM 作为"正在输入"区域的 HTML
- `content` 保留原始全文，供前端在极端场景下做全量回退

### 2.2 AuroraBuffer

```rust
pub struct AuroraBuffer {
    pub full_text: String,              // 累积的完整响应文本
    pub stable_blocks: Vec<StreamBlock>, // 已确认的语义块（对外只读镜像）
    pub tail_content: String,           // 当前未闭合的尾部纯文本
    pub tail_block: Option<StreamBlock>, // 推测渲染后的尾部块（对外只读镜像）
    parser: StreamBlockParser,          // 增量块解析器（内部状态机）
    is_finishing: bool,                 // 是否已进入结束阶段（防重入锁）
}
```

字段访问语义：

| 字段 | 可读性 | 可变性 | 说明 |
|------|--------|--------|------|
| `full_text` | public | 内部追加 | 累积全文，驱动解析器 |
| `stable_blocks` | public | 内部扩展 | `process_queue` 产出新块时 extend |
| `tail_content` | public | 内部替换 | 每次 `process_queue` 后更新 |
| `tail_block` | public | 内部替换 | 基于 `tail_content` 推测渲染 |
| `parser` | private | — | 维护 `processed_len` 游标的增量解析器 |
| `is_finishing` | private | — | 防止 `finalize` 被重复调用 |

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

#### process_queue

```rust
pub fn process_queue(&mut self) -> (bool, bool) {
    if self.is_finishing { return (false, false); }

    let prev_stable_count = self.stable_blocks.len();
    let prev_tail = self.tail_content.clone();

    // 1. 增量解析
    let (new_blocks, new_tail) = self.parser.process(&self.full_text);
    if !new_blocks.is_empty() {
        self.stable_blocks.extend(new_blocks);
    }
    self.tail_content = new_tail;

    // 2. 推测渲染
    if !self.tail_content.is_empty() {
        let nodes = parse_markdown_to_ast(&self.tail_content);
        let hash = compute_content_hash(&self.tail_content);
        self.tail_block = Some(StreamBlock::markdown(
            self.tail_content.clone(),
            Some(nodes),
            hash,
        ));
    } else {
        self.tail_block = None;
    }

    let stable_changed = self.stable_blocks.len() != prev_stable_count;
    let tail_changed = self.tail_content != prev_tail;
    (stable_changed, tail_changed)
}
```

**关键设计决策**：

1. **为什么每次解析全文而非仅解析新增部分？**
   - `StreamBlockParser::process` 内部维护 `processed_len` 游标，实际只扫描未处理区域
   - 但传入 `&self.full_text` 是为了让解析器能在必要时回溯（如块结束标记跨越 chunk 边界）
   - 这种设计牺牲少量冗余扫描，换取边界情况下的正确性

2. **推测渲染（Speculative Rendering）的目的**
   - 流式输出时，尾部文本是"不完整"的（可能截断在 Markdown 语法中间）
   - 直接将其视为临时 Markdown 块，调用 `parse_markdown_to_ast`，让前端能看到实时的粗体/代码/列表预览
   - 该块不进入 `stable_blocks`，流结束时会丢弃，由 `finalize` 产出的正式块替代

3. **Hash 计算**
   - 使用 `sync_hash::HashAggregator::compute_content_hash`（基于 `std::collections::hash_map::DefaultHasher`）
   - Hash 作为前端 `v-for` 的 `key`，确保块级 diff 效率

### 3.2 finalize — 流结束强制闭合

当 SSE 流正常结束（`[DONE]`）、被中止、或意外断开时，`vcp_client.rs` 调用 `finalize()`：

```rust
pub fn finalize(&mut self) {
    if self.is_finishing { return; }
    self.is_finishing = true;
    let final_new_blocks = self.parser.finalize(&self.full_text);

    self.stable_blocks.extend(final_new_blocks);
    self.tail_content.clear();
    self.tail_block = None;
}
```

- `is_finishing` 防重入：确保多次调用（如中止后立即收到 `[DONE]`）不会重复解析
- `parser.finalize` 与 `process` 的区别：
  - `process` 只识别**已确认闭合**的块（遇到未闭合标记会保留在 tail）
  - `finalize` 将剩余内容**强制封装**为最后一个 Markdown 块，无论是否闭合
- 调用后 `tail_content` 和 `tail_block` 被清空，意味着前端"正在输入"区域应消失

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

### 7.4 同层：sync_hash.rs

`tail_block` 的 Hash 计算委托给 `sync_hash::HashAggregator::compute_content_hash`，确保块级 Hash 与同步子系统使用的 Hash 算法一致。

---

*最后更新：2026-05-21 | VCP Mobile v0.9.14*
