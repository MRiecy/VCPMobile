# 02 — AST 差分中提前构造节点 ID 的审查

审查日期：2026-10-31。代码基线：`a771315ec4b5e71652621cf7343bce2344031479`。范围：原报告维度二第 2 项；本次只审查并新增报告，没有修改生产代码或已有测试。

## 结论

**问题存在，建议定为 P3（微优化），不支持原报告的“中高紧迫性”。** 两个公共节点循环确实先执行 `format!`，随后才判断已有哈希是否一致；遇到哈希命中时，这个新建的节点 ID 没有被使用就被丢弃。把构造语句移至哈希检查之后是直接、低风险的局部优化。

但原报告把它放大为“绝大多数公共历史节点、每帧数十次无效分配”，遗漏了生产链的范围与门禁：差分输入仅为**当前未沉淀 tail 的两份 AST**，不包含此前的 `stable_blocks`；一个持续增长的纯文本段落通常没有这里可省下的哈希命中 ID；较长列表或富格式段落才可能出现几十个命中。当前源码和附件都没有提供分配计数、耗时占比、Android 掉帧或分配器竞争的测量，不能据此将它认定为严重性能瓶颈。

## 1. 源码证据与调用范围

以下链接相对本专项目录，行号针对本次基线。

| 证据 | 当前行为 | 审查含义 |
| --- | --- | --- |
| [`ast_diff.rs:154–175`](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L154) | 块级公共循环在 166 行构造 `node_id`，170 行才进行 `Some(hash)` 相等判断 | 原报告指出的语句顺序属实 |
| [`ast_diff.rs:524–544`](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L524) | 行内公共循环在 535 行构造 ID，539 行才进行相同判断 | 行内也有相同冗余 |
| [`aurora_pipeline.rs:950–977`](../../src-tauri/src/vcp_modules/chat/aurora_pipeline.rs#L950) | 仅在非 reset 帧调用 `diff_ast_streaming(&self.prev_tail_ast, &new_nodes, "t", ...)` | 比较的是上一版和当前版 tail，不是消息全部历史 AST |
| [`aurora_pipeline.rs:778–786`](../../src-tauri/src/vcp_modules/chat/aurora_pipeline.rs#L778) | 新块沉淀后追加至 `stable_blocks`，清空 `prev_tail_ast`，设 reset | 已沉淀的历史不会继续进入本次公共节点循环；新 tail 首帧走快照路径 |
| [`stream_block_parser.rs:945–979`](../../src-tauri/src/vcp_modules/chat/stream_block_parser.rs#L945) | 普通 Markdown 在 `\n\n` 处沉淀，剩余部分成为 tail | 消息总长度不能直接用作这项成本的规模变量 |
| [`aurora_pipeline.rs:895–948`](../../src-tauri/src/vcp_modules/chat/aurora_pipeline.rs#L895) | 普通 Markdown tail 大于 65536 字节转 Plain；围栏等单独处理 | 本问题的富 Markdown 遍历存在 64 KiB 字节范围限制，仍可有很多短节点 |
| [`vcp_client.rs:2087–2092`](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L2087)、[`2200–2259`](../../src-tauri/src/vcp_modules/infra/vcp_client.rs#L2200) | 非空 `pending_chunk` 才解析；常规时钟 33 ms，错过 tick 选择 Skip | SSE token、网络 chunk、AST 差分次数不能视为同一个频率 |

生产链可以归纳为：

```text
SSE 输入累计到 pending_chunk
  → flush_aurora_window（常规 33 ms 窗口，另有重接/终态等触发）
  → AuroraBuffer::process_queue
  → 将闭合块移入 stable_blocks；为剩余 tail 生成 new_nodes
  → 非 reset 且 AST 模式时 diff_ast_streaming(prev_tail_ast, new_nodes)
  → 公共块级循环；仅在父节点未命中时递归到子列表/行内循环
```

33 ms 是常规合并周期，不是对所有恢复、终态触发的绝对频率上限，也不意味着每个周期一定解析。将原报告的“每帧”理解为每个 60 Hz 屏幕帧同样不准确。

## 2. 哈希命中的真实条件

源码条件是“两边哈希相等，并且旧节点有哈希”，不是 `None == None` 就跳过：

```rust
if old_node.get_hash() == new_node.get_hash() && old_node.get_hash().is_some() {
    continue;
}
```

- [`MarkdownNode::get_hash`](../../src-tauri/src/vcp_modules/chat/pre_renderer/markdown_ast.rs#L150)：Paragraph、Heading、CodeBlock、Blockquote、List、Table、RawHtml 返回其已有哈希；ThematicBreak 返回 `None`。
- [`InlineNode::get_hash`](../../src-tauri/src/vcp_modules/chat/pre_renderer/markdown_ast.rs#L288)：Text、Code、Break 返回 `None`；Strong、Emphasis、Link、Image、InlineMath、VcpCustom、Strikethrough、RawHtmlInline 返回已有哈希。
- [`markdown_parser.rs:1149–1151`](../../src-tauri/src/vcp_modules/chat/pre_renderer/markdown_parser.rs#L1149)：通用 Markdown 解析完成后递归计算哈希。这些判断在生产中确实可命中，不是仅存在于手工构造的 AST。
- [`ast_diff.rs:218–280`](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L218)：仅在当前块未被哈希跳过时才进入其行内或子块比较。父节点命中后不会继续为该子树每一个后代生成 ID。

因此，准确的计数变量应是：

> `H = 本次实际被访问、且 Some(hash) 相等的公共块级/行内节点数`。

当前实现为这两处循环中的这些 `H` 个节点各执行一次无用的 `format!`，并产生随后释放的字符串。只移动两处语句能够消去这 `H` 次 ID 格式化，而不是消除全部差分分配。具体分配器调用次数、字符串容量与优化构建后的成本需计数器或 profiler 验证，不能把“执行一次 format!”严格等同于“任意构建下恰好一次 allocator 调用”。

## 3. 典型输入对成本判断的影响

以下是由控制流推导的场景分析，**不是本次运行测量**。

| 场景 | 可避免的 ID 工作 | 说明 |
| --- | --- | --- |
| 一个纯文本 Paragraph，末尾持续追加文字 | 通常 `H = 0` | Paragraph 内容变化使哈希变化；其 Text 无哈希。两处公共循环仍各生成 ID，但这些 ID 用于下游比较和 AppendText，移动语句不能省掉 |
| 一棵根节点哈希完全未变的子树 | 在已访问根处计 1，后代不计 | 根处 continue 已截断递归，不能按整棵子树节点数估算这项浪费 |
| 同一未沉淀段落包含许多已经闭合的 Strong/Link 等，末尾继续追加 | 可有数十次 | 父段落哈希改变，进入行内列表；旧的富格式节点可各自命中。需要此类结构才能支持“数十次”的量级 |
| 单换行连接的长列表，已有很多项目，继续追加末项或新项 | 可有数十次或更多 | 列表根改变，逐个比较公共项；旧项目内部的 Paragraph 可以命中。空行沉淀与 64 KiB 上限限制它持续增长的范围 |
| 消息很长，但先前内容已经沉淀为 stable_blocks，当前只有短 tail | 不随历史块数线性增长 | stable_blocks 不传入 `diff_ast_streaming` |
| reset、普通 tail 已转 Plain、思维链 Plain 投影 | 不经过这两个公共差分循环，或不在当次差分 | 不能按整条会话 token 数机械相乘 |

长未沉淀列表确实是值得关注的放大输入。对应的 [`ast_diff.rs:306–317`](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L306) 还为公共项目生成 `item_prefix` 和块级子前缀；即使下移本报告关注的 `node_id`，这些其他分配仍保留。

此外，`diff_ast_streaming` 随后的 [`mark_stream_code_nodes`](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L483) 另有路径 ID 格式化，用于增量高亮器帧生命周期标记。不能将两处语句调整描述为“差分变为零分配”，也不能为清理本项而直接删除这次遍历。

## 4. 原报告逐句裁决

| 原断言 | 判定 | 修正 |
| --- | --- | --- |
| `format!` 在哈希检查前无条件执行 | **成立** | 限定在两个公共节点循环的每次实际迭代 |
| 哈希命中后 ID 立即被丢弃 | **成立** | ID 不进入 mutation，也不被下游消费 |
| “绝大多数公共历史节点哈希均匹配” | **条件成立，不能泛化** | 只可能指当前 tail 内已有节点；普通持续增长的纯文本段落是反例 |
| “每帧徒劳产生数十次” | **可达但未证实为典型负载** | 长列表或大量富格式前缀可以达到；需要测得真实输入结构和 `H` |
| 这是一项中高紧迫性的性能黑洞 | **证据不足** | 现有证据能证明小型临时分配，不能证明它是 frame time、OOM、GC 或线程竞争的主因 |

Rust 中这些短命 String 的释放不能直接称为 JVM Young Gen GC 压力；这也不是内存泄漏。分配器锁竞争、缓存未命中或 Android 掉帧均未在本次审查中测量。

## 5. 紧迫性与建议

**当前 P3，适合在相关性能维护中顺手处理，不建议独立阻断开发或发布。**

理由：

1. 冗余行为确定，但目前没有错误输出、资源未释放或数据丢失证据。
2. 影响范围被 tail 边界、父哈希短路、reset 和普通 Markdown 字节上限限制。
3. 许多常见纯文本追加帧不会受益于这两处调整；长列表和富格式段落的收益才更明显。
4. 修复规模小、语义风险低，所以可以提前合入一次经过验证的性能清理；“容易修”不等于“故障紧迫”。

建议仅把两处 `let node_id = format!(...)` 移到原哈希检查之后，保持哈希规则、递归顺序、节点 ID 和 mutation 顺序完全不变。无需为此引入 ID 池、跨帧缓存、额外哈希体系或新的产品门禁。

不要改为无条件跳过所有相等 `Option`：`None == None` 的 Text 必须继续比较。也不要顺带给 Text 添加哈希或移除高亮器的节点标记；那已扩大到不同算法与生命周期问题。

只有在具名 Android 设备的分配/CPU 采样显示这些 ID 构造占据可观的流式帧预算，或高并发长列表输入下明确造成持续延迟时，才有理由升为 P2，并再评估更大的路径字符串优化。

## 6. 验证范围与后续验收

本次完成：按生产调用链核查两处循环、哈希定义、解析后的哈希计算、stable/tail 边界、33 ms 调度及 Plain 门禁；阅读了已有 AST 差分测试。没有运行 cargo 重编译、基准测试或 Android 真机采样，没有声称获得任何耗时收益。

若实施这两处调整，建议：

- 保持修改前后同一批 AST 输入生成的 mutation **内容与顺序完全一致**；重点覆盖纯文本追加、富格式前缀命中、列表追加、无哈希 Code/Break 和类型替换。
- 现有 [`ast_diff.rs` 测试](../../src-tauri/src/vcp_modules/chat/ast_diff.rs#L736) 可提供文字前缀、列表增删、Code/Break 行为证据，但它们没有证明分配数量或移动端性能，且本次未执行。
- 若需要量化收益，对实际 diff 调用统计分配次数与总字节，区分“相同根节点”“纯文本追加”“多 Strong 前缀”“长列表末项变化”，报告 tail 大小和节点数。用完整 `AuroraBuffer` 路径补充验证，防止测量把已沉淀历史错误放回 hot path。
- 当前 [`ast_tail_bench.rs:123–185`](../../src-tauri/benches/ast_tail_bench.rs#L123) 的稳态基准覆盖解析、diff、帧取出与序列化，适合观察总成本，但不是 ID 分配专用计数器；不能将总耗时变化全部归因于这两行。
- 按仓库规则，生产核心逻辑实际修改后运行 `pnpm check`；本次仅新增审查文档，不触发生产代码变更检查。
