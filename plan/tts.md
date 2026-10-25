# 语音指导块（`<voice_guidance>`）重构执行方案

> 2026-10-25 讨论修订。本文负责指导协议、标准块解析与消息朗读输入边界；[tts-fix.md](tts-fix.md) 负责气泡入口、试听生命周期、正文清洗和分段。两份文档共同构成实施方案；用户在文档确认后已单独授权代码重构，验证记录见增补方案末尾。

## 零、 已确定的行为与职责

- **指导只认整条原始消息第一行开头**的 `<voice_guidance>`。允许标签前存在同一行的空格或 Tab，不跳过空行、正文、思考块或代码块。后续行的同名标签不作为指导，不保留旧协议和旧范围扫描器的识别规则。
- **流式朗读只消费 stable 块**。语音正文清洗与分段不处理 chunk，不读取或推测 tail；本次不改变共享解析器的普通正文沉淀粒度，也不另建按句提前沉淀的机制。
- **终态补读**：生成结束后，剩余普通正文进入朗读；仍未闭合的特种块解除外层封装后，其内容也进入朗读，不因模型漏写结束标记而丢弃。主动停止朗读、挂断等取消意图仍立即停止，不触发补读。
- **气泡就是完整消息对象**。历史气泡朗读使用完整原文，不按视觉拆分后的子气泡切割输入。
- **试听独立**：直接朗读用户输入，不提取指导；设置页拥有试听生命周期，退出语音设置前完成停止。通话与气泡朗读不维护试听状态。
- 默认 `voice.guidance.rules` 保持 **“占位中”**，仍原样注入；本轮不撰写默认提示词。
- 公式区域跳过，周围解释文字保留。其他清洗规则及已确认的气泡生命周期见增补方案。

| 职责所有者 | 输入与责任 | 不承担的工作 |
| :--- | :--- | :--- |
| 共享块解析器 | 识别标准块、按原始消息起点识别指导、输出 stable 与终态剩余内容 | TTS 请求、播放和历史改写 |
| 消息朗读 owner（复用 `MessageText` 的消息级生命周期） | 接纳 stable、持有指导与累计可读正文、执行一次终态收束 | 增加第二套指导扫描器、推测普通 tail |
| `ReplyBuffer` | 对已经接纳的可读正文执行风格继承与阶梯分段 | 块识别、网络传输 |
| `TtsJob` / TTS 注册表 | 合成、播放、停止及任务槽位生命周期 | 决定消息是否属于指导 |
| 气泡朗读前端 owner | 完整消息身份、用户操作和任务状态呈现 | 复制后端解析或合成调度 |
| 语音设置页试听 owner | 直接文本试听与离页停止 | 气泡与通话的业务状态 |

## 一、 重构目标与核心原则

1. **协议标记统一切换为 `<voice_guidance>`**：
   - 起始标记：原文起点锚定 `(?i)\A[ \t]*<voice_guidance>`，仅允许整条消息第一行开头。
   - 锚定必须基于完整原文的绝对位置；不得因扫描 `remaining` 切片、去缩进或 `trim` 而把后续行重新当成消息开头。
   - 结束标记：`(?i)</voice_guidance>`
   - 废弃原 `<<<[VOICE_GUIDANCE]>>>` / `<<<[END_VOICE_GUIDANCE]>>>` 标记。
2. **禁止后处理篡改历史上下文**：
   - 模型输出的原文在持久化与多轮历史上下文组装（`assemble_history_for_vcp`）中**100% 原样保留**，不再剥离 `<voice_guidance>` 块（保留在 `assistant` 历史中还能充当天然的 In-Context Few-shot 示范）。
3. **并入标准特种块解析流水线（消灭全量重扫外挂）**：
   - 将 `BlockType::VoiceGuidance` 收编进 [`content_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/content_parser.rs) 与 [`stream_block_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/stream_block_parser.rs)（标准块识别 + `PendingBlock` 游标续扫 + `aurora_pipeline.rs` 尾部投影）。复用 Think 的块生命周期，但指导起始位置独立遵守“整条消息首行开头”的限制。
4. **彻底移除过度设计的 `voice_guidance.rs` 状态机，复用块解析器统一流式/非流式朗读过滤**：
   - 删除 [`src-tauri/src/vcp_modules/chat/voice_guidance.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_guidance.rs) 中 560+ 行的 `MAX_SCOPE_RESCANS = 16` 多轮 `pulldown_cmark` 字节遮罩重扫状态机及 `exhausted` 报错机制。
   - 流式与完整消息朗读复用 `StreamBlockParser` 的块边界。进行中仅将 stable Markdown 正文送入 `readable.rs`；已闭合的非正文块跳过，首行有效指导转为 instruction。所有 tail 都留在输入解析边界，不进入进行中的正文投影。
   - 结束时由同一消息 owner 一次性接纳剩余内容；未闭合特种块按已确定的终态例外补读。显示端保留原文的终态处理与语音端解除封装的补读投影各自负责，不改写持久化内容。

---

## 二、 分模块修改清单

### 1. 移除历史上下文后处理（[`context_assembler.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/context_assembler.rs)）

- **修改位置**：[`assemble_history_for_vcp`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/context_assembler.rs#L165-L169)
- **改动内容**：
  1. 移除文件顶部的 `voice_guidance` 导入。
  2. 将第 165–169 行的分支：
     ```rust
     if msg.role == "assistant" {
         combined_text.push_str(&voice_guidance::strip_guidance_for_history(&msg.content));
     } else {
         combined_text.push_str(&msg.content);
     }
     ```
     直接简化为：
     ```rust
     combined_text.push_str(&msg.content);
     ```
  3. 删除旧“剥离指导”的断言，改为验证 assistant/user 历史均原样保留指导标记与正文；群聊既有发言人前缀不受影响。

---

### 2. 收编为标准静态特种块（[`content_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/content_parser.rs)）

- **修改位置**：[`content_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/content_parser.rs#L309-L890)
- **改动内容**：
  1. **`BlockType` 枚举**（第 310 行）：新增 `VoiceGuidance` 变体。
  2. **正则定义**（`lazy_static!` 块，第 371 行附近）：
     ```rust
     pub(crate) static ref VOICE_GUIDANCE_START: Regex = Regex::new(r"(?i)\A[ \t]*<voice_guidance>").unwrap();
     pub(crate) static ref VOICE_GUIDANCE_END: Regex = Regex::new(r"(?i)</voice_guidance>").unwrap();
     ```
  3. **原始位置约束**：在任何去缩进或文本预处理前，确认完整原文是否以合法指导开头。后续识别保留该资格与消息绝对起点，不借 `is_vcp_marker` 的全局去缩进把后续示例提升为指导。
  4. **消除外挂二次解析**（[`parse_content`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/content_parser.rs#L555-L577)）：
     - 删除 `parse_content` 中调用 `voice_guidance::blocks` 的前置切分外壳，让 `parse_content(raw_text: &str)` 直接执行原 `parse_ordinary_content` 的单趟逻辑。
  5. **并入 `MASTER_START` 与匹配分支**（第 590–890 行）：
     - 将指导候选接入标准块分派，但仅在原文首行资格成立、扫描位置仍为消息起点时参与匹配。可复用 `MASTER_START` 的分派结构，不把指导正则无条件加入所有剩余切片的多行扫描。
     - 合法候选返回 `BlockType::VoiceGuidance`；不符合首行位置的标签不产生指导块，也不参与 instruction 提取。
     - 在结束标记查找中：`BlockType::VoiceGuidance => VOICE_GUIDANCE_END.find(search_area).map_or((None, None, false), |m| (Some(m.start()), Some(m.end()), true))`。
     - 未闭合时：遵循与 `BlockType::Think` 相同的规则（第 739 行），流式中断未闭合的起始标记在静态终态降级为普通 Markdown。
     - 闭合时：构造 `ContentBlock::VoiceGuidance { content: inner_content.trim().to_string(), is_complete: true, hash: None }`。

---

### 3. 收编为标准流式特种块（[`stream_block_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/stream_block_parser.rs) & [`aurora_pipeline.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/aurora_pipeline.rs)）

#### 3.1 [`stream_block_parser.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/stream_block_parser.rs)
- **改动内容**：
  1. **删除外挂扫描逻辑**：
     - 删除 `process` 中的 `guidance_blocks` 局部变量及第 373–411 行的 `has_guidance_candidate` 分支。
     - 删除 `finalize` 中第 548–569 行针对 `voice_guidance::blocks` 的特殊收尾逻辑。
     - 删除 `has_guidance_candidate` 与 `final_markdown_block` 辅助函数。
  2. **接入 `O(Δ)` 游标扫描**：
     - 新增常量：`const VOICE_GUIDANCE_END_LOOKBEHIND: usize = 16;`（对应 `</voice_guidance>` 长度 17 减 1）。
     - 在 `find_earliest_start_marker` 接入指导候选，并传递/验证消息绝对起点资格；不能仅向多行 `bounded` 数组追加一个不受位置约束的正则。
     - 在 `process` 的 `end_scan_from` 计算（第 457 行）中，将 `BlockType::VoiceGuidance` 加入定长回看分支：
       ```rust
       BlockType::VoiceGuidance => floor_char_boundary(
           remaining,
           rel.saturating_sub(VOICE_GUIDANCE_END_LOOKBEHIND),
       ),
       ```
     - 在 `find_end_marker`（第 701 行）中加入：
       ```rust
       BlockType::VoiceGuidance => VOICE_GUIDANCE_END.find_at(search_area, scan_from),
       ```
     - 在 `build_stream_block`（第 784 行）中加入：
       ```rust
       BlockType::VoiceGuidance => {
           let trimmed = inner_content.trim();
           let hash = HashAggregator::compute_content_hash(trimmed);
           StreamBlock::VoiceGuidance {
               content: trimmed.to_string(),
               is_complete: true,
               hash,
           }
       }
       ```
  3. **未闭合尾部投影（与 `speculative_thought_tail` 同构）**：
     - 新增 `speculative_voice_guidance_tail(tail: &str, tail_type: Option<BlockType>) -> Option<(&str, usize)>`（或扩展现有结构），当 `tail_type == Some(BlockType::VoiceGuidance)` 时，剥离起始 `<voice_guidance>` 标签（及紧随的首个换行），返回内部正在流式生成的纯文本切片与字节偏移。
     - 此投影服务于显示；进行中的 TTS 不消费它。终态补读复用解析器已有的起始范围，避免重新扫描整条消息。
  4. **统一围栏配对**：按已确认范围补齐 `~~~`，静态、流式、去缩进预扫描与 Aurora 围栏尾展示统一使用相同的围栏字符、长度和缩进规则；反引号与波浪号不能互相闭合。

#### 3.2 [`aurora_pipeline.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/aurora_pipeline.rs) 与前端流式尾对接
- **改动内容**：
  1. `TailBlockType`（第 66 行）新增 `VoiceGuidance`（序列化为 `"voice-guidance"`）。
  2. `current_tail_wire_block`（第 435 行）：当 `projection.block_type == TailBlockType::VoiceGuidance` 时，构造 `StreamBlock::VoiceGuidance { content, is_complete: false, hash }`。
  3. `process_queue`（第 706 行）：当 `raw_tail_type == Some(BlockType::VoiceGuidance)` 时，与 `Thought` 完全一致走 `TailRenderMode::Plain` 纯文本增量投影（不做 Markdown AST 解析，每帧 `O(Δ)`），块类型标记为 `TailBlockType::VoiceGuidance`。
  4. **前端对接**（[`chatStreamStore.ts`](file:///e:/VCPMobile/src/core/stores/chatStreamStore.ts#L39-L76)、[`useMessageTailRenderer.ts`](file:///e:/VCPMobile/src/features/chat/composables/useMessageTailRenderer.ts)、[`MessageRenderer.vue`](file:///e:/VCPMobile/src/features/chat/MessageRenderer.vue)、[`VoiceGuidanceBlock.vue`](file:///e:/VCPMobile/src/features/chat/blocks/VoiceGuidanceBlock.vue)）：
     - `TailBlockType`（`src/core/types/chat.ts`）加入 `"voice-guidance"`。
     - `buildTailBlockFromOp` 与 `buildTailSnapshotOp`（`chatStreamStore.ts`）支持 `blockType === "voice-guidance"`，生成 `{ ...shared, type: "voice-guidance", is_complete: false }`。
     - 在 `MessageRenderer.vue` 中，若 `message.tailBlock?.type === 'voice-guidance'`，直接渲染 `<VoiceGuidanceBlock :block="message.tailBlock" />`（内部直接展示 `block.content`），闭合后无缝切换为 stable 区的 `<VoiceGuidanceBlock>`。
     - 指导尾部不挂载 Markdown AST 沙箱，排除 `tailContent` 的重复文本兜底；切回普通 Markdown 时沿原 owner 恢复沙箱。
     - 默认保持折叠。当前消息组件持有指导块的展开状态，尾部转 stable 和终态 hash 变化时延续用户操作，不新增全局展开状态。

---

### 4. 重构 TTS 朗读过滤与指导提取（复用块解析不变量，删除 `voice_guidance.rs`）

#### 4.1 明确 TTS 朗读跳过与保留规则（对齐桌面端 VCPChat 与移动端 12 种特种块）
下表用于**已经闭合并沉淀的块**。流式消费只处理 stable；终态仍未闭合的特种块适用 §4.3 的补读规则，不套用本表的整块跳过规则。普通正文中的 `(风格)`、`[音频标签]` 原样保留：

| 块/元素类别 | 对应 `BlockType` / 语法 | 朗读策略 | 处理方式 |
| :--- | :--- | :---: | :--- |
| **思考链 / 推理块** | `Thought` (`[--- VCP元思考链 ---]`)、`Think` (`<think>`) | **跳过** | 块级直接忽略 |
| **语音指导块** | `VoiceGuidance` (`<voice_guidance>`) | **跳过（转 `instruction`）** | 仅原始消息首行开头的闭合块有效；内容非空且不超过 2000 字符时接纳为该消息 instruction，随后各分段共享 |
| **工具调用与结果** | `Tool` (`<<<[TOOL_REQUEST]>>>`)、`ToolResult`、`ToolCallSummary` | **跳过** | 块级直接忽略 |
| **日记本 / 角色分割线** | `Diary` (`<<<DailyNoteStart>>>`)、`RoleDivider` (`<<<[ROLE_DIVIDE_*]>>>`) | **跳过** | 块级直接忽略 |
| **围栏代码块** | `CodeFence` (`` ``` `` / `~~~`) 及 Markdown 内置 `Tag::CodeBlock` | **跳过** | 块级直接忽略（行内短代码 `` `code` `` 保留朗读） |
| **HTML 文档 / 样式 / 容器** | `HtmlDoc`、`Style` (`<style>`)、`HtmlContainer` (`<div>` 等块级 HTML) | **跳过** | 块级直接忽略 + `without_html` 剥离内联 HTML |
| **行内交互与媒体元素** | `[button:xx]`、`@tag(...)`、Markdown 图片 `![alt](url)` | **跳过** | `pulldown_cmark` 忽略 `Tag::Image`，行内正则剥离 `[button:...]` 与 `@tag(...)` |
| **Markdown 普通正文** | 已沉淀的 `StreamBlock::Markdown` | **保留朗读** | 经 `pulldown_cmark` 清洗后送入现有 `ReplyBuffer` 切句；普通 tail 在终态转为最终输入后再处理 |
| **公式区域** | 行内公式与独立公式 | **跳过** | 保留公式周围的解释文字，详见增补方案 |

实际 `StreamBlock` 类型与内部 `BlockType` 不完全一一对应：Think 产出 Thought，普通 CodeFence、HtmlContainer 可产出 Markdown。后两类仍由 Markdown 投影中的代码/HTML 过滤处理，不能仅凭 wire 类型为 Markdown 就跳过清洗；无需为此扩展前端 wire 类型。

#### 4.2 删除 [`src-tauri/src/vcp_modules/chat/voice_guidance.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_guidance.rs)
- 从 [`src-tauri/src/vcp_modules/chat/mod.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/mod.rs) 移除 `pub(crate) mod voice_guidance;` 并删除 `voice_guidance.rs`（562 行）。

#### 4.3 流式朗读重构（[`chat_feed.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs) 复用 `StreamBlockParser` 不变量）
复用当前 `ChatFeed` / `MessageText` 的逐消息身份与群聊顺序。输入适配边界驱动 `StreamBlockParser`，语音消费端只接纳它产出的 stable 块和一次终态输入；不另建 chunk 清洗、tail 推测或正文提前稳定化逻辑。

1. **消息级 owner**：从现有 `MessageText` 提取/整理共用的消息朗读状态，持有解析进度、`stable_spoken`、`instruction`、指导是否已决定、终态标志及 `ReplyBuffer`。ChatFeed 和完整气泡复用这份实现；指导与对应正文在同一状态锁内接纳，不能另加一把独立 instruction 锁。
2. **进行中接纳 stable**：
   - 仅处理 `parser.process` 返回的新增 stable 块；普通与特种 tail 均保留在输入边界，完全不进入清洗或分段。
   - 首行指导闭合时按非空、2000 字符上限决定是否接纳，结果锁定到当前消息；无效指导不作为 instruction，后续位置的标签没有再次争取指导资格的机会。
   - Markdown 块清洗后按 `\n` 连接到 `stable_spoken`。代码、HTML 等按 §4.1 过滤，其他非正文块直接忽略。累计可读正文交给 `ReplyBuffer`。
   - 指导在消息输入接纳时决定，不能等播放 worker 空闲后再选择；群聊每条消息拥有自己的指导和分段进度。
3. **终态一次性收束**：
   - 接纳本次新闭合的 stable 块后，处理尚未消费的剩余内容。
   - 普通 Markdown 剩余内容作为最终正文清洗并追加。
   - 未闭合特种块使用解析器已经确认的起始范围，解除该块的外层起始封装，将其内部内容作为最终可读文本追加。未闭合的指导不生成 instruction，其内部内容也按此补读。
   - 补读投影不得再次把该外层代码、HTML、Think、Tool 等内容整体排除；允许清除协议/排版标记，保留内容，并继续应用 URL、公式等独立清洗规则。不改动显示和存储原文。
   - 然后将 `ReplyBuffer` 标为 finished，释放不足最小字数的末段。重复终态不能重复追加或播放；朗读任务已被明确取消时不进行补读，不能把模型生成终止与朗读任务取消混为一谈。
4. **输入替换与恢复**：累积文本只追加时保留解析游标。若现有真实输入给出非追加 replacement，重建未播放部分的解析缓存，保留已决定指导和已发送前缀校验；已播放正文变化时沿现有错误路径停止，不重置播放位置。恢复事件不重播。

#### 4.4 非流式 / 历史气泡点击朗读与 [`readable.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/readable.rs) 简化
1. **完整回复/历史气泡共用入口**：
   - 完整消息原文一次交给 §4.3 的消息朗读 owner，驱动同一个 `StreamBlockParser`，接纳 stable 后执行同一终态收束。完整模型回复、历史气泡与流式终态不得各写一套指导提取算法。
   - 不从显示层 `ContentBlock::Markdown.content` 反取正文：该字段为可选值，静态解析可能只保留 AST。语音输入始终来自完整消息原文和标准解析器的文本块。
   - 不把“无通话 origin 的独立任务”与“TTS 音频非流式”混为一类；正文来源与 `config.tts_streaming()` 分别决定输入处理和阶梯分段策略。
2. **精简 `readable.rs`**：
   - 移除 `voice_guidance::speech_opaque_projection` 与重复的特种块整文扫描；不再为进行中的 chunk/tail 保留另一套半标记识别状态机。
   - `project_markdown_block` 只负责 stable/最终 Markdown 正文的格式清洗。终态未闭合块的解除封装投影有明确入口，不复用会整块吞掉内容的过滤方式。具体规则见增补方案。

---

## 三、 验证标准（交付门禁）

专项 Agent 完成上述代码修改后，依次执行以下验证：
1. **静态与类型检查**：在仓库根运行 `pnpm check`（确保 ESLint、`vue-tsc`、`cargo check --locked` 零警告通过）。
2. **Rust 单元测试**：在 `src-tauri/` 运行 `cargo test --locked --lib`，更新相关旧协议测试。重点验证：
   - 首行指导有效，前置空行/正文/Think/围栏及后续段落中的标签均不获得指导身份；扫描切片不能重新开启首行资格。
   - `~~~` 与反引号围栏的字符、长度、缩进和分片边界一致。
   - 进行中只有 stable 正文进入朗读，所有 tail 均不产生语音正文；不以测试要求增加提前沉淀机制。
   - 已接纳生成终态的普通剩余内容与未闭合 Think、指导、Tool、代码、HTML 等内容补读一次；闭合特种块仍不读，朗读任务被显式取消后不补读。
   - 完整消息与同内容流式终态得到一致的指导和可读正文；历史原文保留；群聊指导隔离、快照替换与已发送前缀边界成立。
3. **前端单元测试**：运行 `pnpm test:run`，覆盖指导尾部的增量/快照、闭合交接、展开状态延续与 AST 沙箱切换；气泡与试听的验证项见增补方案。
4. **文档同步**：同步更新 [`src-tauri/src/AGENTS.md`](file:///e:/VCPMobile/src-tauri/src/AGENTS.md#L68) 中关于 `voice_guidance` 的局部说明。

---

实施以两份修订方案和用户已确认的行为为准。默认提示词保持“占位中”，最终预设另行打磨。
