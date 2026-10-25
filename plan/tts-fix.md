# TTS 气泡朗读入口、非流式传输与 Markdown 无障碍清洗/阶梯分段方案（二轮增补）

> 2026-10-25 讨论修订，与 [tts.md](tts.md) 配套使用。用户已确认：气泡指完整消息；语音消费端只处理 stable，不改变上游沉淀粒度；结束时补读未闭合内容；试听直接输入且离开语音设置即完成停止；默认指导保持“占位中”；公式跳过。文档确认后已另行授权代码实施。

> 本文档为 [`plan/tts.md`](file:///e:/VCPMobile/plan/tts.md) 的二轮深度增补方案，重点解决施工过程中暴露的三大核心缺失：
> 1. **独立气泡朗读缺失交互入口**（对齐桌面端长按气泡菜单）；
> 2. **非流式 / 历史气泡朗读路径未走块级清洗与 `<voice_guidance>` 提取，且缺乏针对非流式 API（尤其是音色复刻 Clone 模式）的分段传输策略**；
> 3. **Markdown 转可读文本清洗器存在吞字/抹平换行/念出 URL 等硬伤，以及分段器缺乏最小字数下限与流式/非流式自适应阶梯窗口**。

---

## 一、 独立气泡朗读入口设计（对齐桌面端长按菜单）

### 1. 现状问题
后端 [`tts.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/tts.rs#L359-L535) 已在 [`commands.rs`](file:///e:/VCPMobile/src-tauri/src/commands.rs#L204-L207) 注册了 `start_tts_session`、`submit_tts_text`、`stop_tts`，但前端仅有语音设置页的“试听声音”（[`VoiceTtsPreview.vue`](file:///e:/VCPMobile/src/features/chat/voice/VoiceTtsPreview.vue)）和全双工语音通话调用，聊天消息气泡没有任何独立朗读入口。

### 2. 修改方案

以下生命周期已经确认，不再保留分支选项：

| 用户操作 / 场景 | 确定行为 | 责任位置 |
| :--- | :--- | :--- |
| 通话中请求朗读气泡 | 不启动气泡朗读，提示先结束通话 | 气泡入口 + Rust 通话/TTS 接纳边界 |
| 气泡朗读中启动通话 | 停止气泡朗读，完成停止后进入通话 | 通话启动与 TTS owner |
| 从气泡 A 切换到气泡 B | 停止 A 后启动 B | 气泡朗读 owner |
| 切换话题或智能体 | 停止当前气泡朗读 | 会话切换 owner |
| 应用进入后台或锁屏 | 气泡朗读继续，沿现有 Rust 工作租约与原生播放链运行 | Rust TTS owner |
| 编辑、删除源消息，或重新生成使其失效 | 停止该消息的朗读；确认框取消不停止 | 实际消息变更 owner |
| 普通气泡组件卸载、列表复用 | 不直接决定朗读终止 | 组件只消费状态 |
| 退出语音设置 | 设置页完成试听停止后退场 | 设置页试听 owner |

#### 2.1 新增轻量级气泡朗读控制器（`src/features/chat/voice/useBubbleNarration.ts`）
负责管理全局唯一的独立气泡朗读会话生命周期。使用应用作用域的 Composition API Store/共享 owner，与后端 `TtsServiceState` 单槽位模型对齐；不能在每个气泡实例内各建一套状态或监听：
- **响应式状态**：
  - 持有现有 `MessageActionKey`（会话身份 + 完整消息 ID）；`activeMessageId` 从该身份派生，不单独复制一份来源状态。
  - `activeTtsId: Ref<string | null>`：当前朗读任务的 `ttsId`（格式：`bubble-tts-${crypto.randomUUID()}`）。
  - 当前启动意图代次、停止完成 Promise 和监听初始化 Promise 属于该 owner；不存入业务消息或配置。
- **核心动作**：
  1. `startBubbleNarration(messageKey: MessageActionKey, rawText: string)`：
     - **互斥检查**：若当前正处于实时语音通话（`voiceCallStore.active === true`），弹出 Toast 提示 `"语音通话进行中，请先结束通话再单独朗读气泡。"` 并返回。
     - **抢占切换**：停止旧任务并等待其实际停止确认，再接纳新任务。每次 await 后核对当前意图和消息身份；快速 A→B→C 时，过期的 B 不得继续启动。
     - **事件监听**：启动前完成唯一的 `listen('vcp-tts', ...)` 注册。仅接纳匹配当前 `ttsId` 的事件；错误由该任务通知一次。终态事件更新界面，但不能用“已收到 stopped/error”替代槽位实际回收；切换任务继续等待停止完成 Promise。迟到监听在 owner 已销毁时立即释放。
     - **启动与提交**：
       - 调用 `start_tts_session`，携 `ttsId`、`originUserMessageId: null` 和 `bubbleSource: { topic: { ownerType, ownerId, topicId }, messageId }`。后端保留完整来源身份；启动确认消息仍有效，提交时确认原文快照一致，软删除记录不再作为有效来源。
       - 若 `!res.configured`，清空状态并弹出 Toast 提示 `"请先在语音设置中配置语音 API Key。"`。
       - 调用 `await invoke('submit_tts_text', { ttsId, text: rawText, finished: true })`，提交完整消息原文。
       - 启动/提交失败时回收本次任务；迟到的启动结果只能停止其自身 `ttsId`，不能覆盖或停止新任务。
  2. `stopBubbleNarration()`：
     - 撤销当前启动意图并停止本任务；可立即更新界面，但在停止确认前仍保留内部任务身份和停止 Promise，防止新任务越过回收边界。
  3. **接入已有生命周期 owner**：会话切换、消息变更和通话启动按上表触发停止；Rust 在真实接纳边界维护通话与独立气泡互斥，不仅依赖前端快照。

#### 2.2 接入气泡长按菜单（[`src/features/chat/MessageRenderer.vue`](file:///e:/VCPMobile/src/features/chat/MessageRenderer.vue#L734-L926)）
在 `showMessageContextMenu` 中（位于“复制内容”之后）：
- 当 `!isStreaming.value` 时，加入朗读操作项：
  - 若 owner 按完整 `MessageActionKey` 判断当前气泡正在朗读：
    - 显示 **`label: "停止朗读"`**，图标使用 `Square`。点击时复核菜单捕获的消息身份和 `ttsId` 仍对应当前任务，再执行停止；气泡 A 的旧菜单不能停止后来启动的气泡 B。
  - 否则：
    - 显示 **`label: "朗读气泡"`**，图标使用 `Volume2`。这里的“气泡”是完整消息对象，保留项目既有称呼；不因 `brk` 等视觉拆分而只朗读一个子气泡。
    - 点击后以菜单打开时捕获的 `MessageActionKey` 获取完整原文；`await getFullText()` 后再次确认会话、消息和生成状态仍有效，非空时调用 `bubbleNarration.startBubbleNarration(messageKey, fullText)`。不在 await 后用变化了的 `props.message.id` 重新绑定目标。
- **生命周期清理**：在现有 `chatHistoryStore` 等实际变更 owner 接入停止，覆盖编辑、删除与重新生成影响的全部消息。重新生成可能截断后续多条消息，不能只比较菜单目标 ID。确认通过并实际开始变更时才停止，取消确认不停止。
  - 前端撤销待启动意图；Rust 消息变更提交后，依据实际受影响的消息 ID 停止已注册任务，覆盖分页窗口外的消息。清理失败不能把已经提交成功的消息变更报告为失败。

#### 2.3 试听作为设置页内的独立功能

- 保留 `preview_tts` 的草稿配置快照与直接文本输入。将它从气泡使用的 `submit_tts_text` 处理链拆开，提供内部直接文本提交入口；不提取指导、不做消息块清洗，不调用聊天模型。
- 设置页拥有试听任务和监听。返回、切换设置页面及其他实际退场统一进入设置页的离开流程，等待 `VoiceTtsPreview.stop()` 完成原生停止与任务释放后再退场；通用卸载只作为清理入口，不代替正常离开流程。
- 试听任务开始即绑定设置页本次访问身份，退场撤销该身份；尚未完成的启动也在试听 owner 内收束，退出后不能继续合成、播放或回写界面。
- 设置页离开停止闭环落地后，移除既有 `begin_voice_conversation → stop_preview` 的试听停止分支及其失去消费方的辅助函数，不将这份职责迁移给气泡。气泡与通话不维护试听状态或 watcher；页面拥有试听生命周期，底层只复用 TTS 网络与播放执行器。

---

## 二、 打通非流式 / 历史气泡朗读的后端管线（[`tts.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/tts.rs)）

### 1. 新增气泡入口的接入缺口
当前 `origin_user_message_id == None` 表示独立输入来源，不代表音频合成采用非流式 API。此前该提交入口由试听使用；新增气泡入口需要接入消息处理，而试听继续直接文本。当前实现情况如下：
1. `submit_tts_text` 直接将原始 `text` 原封不动塞进了 `narration.reply.lock()?.update(text, finished)`：
   - 该直接文本行为用于试听；若新增气泡原样复用，会将 Think、工具、代码、HTML 等消息结构送入朗读。
   - 当前独立输入的 `speak_inner` 返回 `(next(), None)`，没有气泡所需的首行指导提取。
   - 气泡需要接入 `readable` 投影，试听仍保留直接文本。
2. `TtsJob.reply` 使用了 `ReplyBuffer::default()`：
   - `inherit_global_style` 为 `false`；新增气泡需要让开头的 `(慵懒)` 等全局风格在后续分句中继承。
   - 未感知当前 TTS 渠道是否为非流式（`!config.tts_streaming()`，特别是音色复刻 `VoiceTtsMode::Clone`）。

### 2. 重构方案

1. **`TtsJob` 的输入状态按来源分开，执行器共用**：
   - 通话使用 `ChatFeed` 的逐消息队列，完整气泡使用同一种消息朗读状态，试听使用直接文本分段状态；来源在任务创建时确定，不在提交函数中猜测。
   - 消息朗读 owner 在同一锁内持有指导接纳状态、可读正文与 `ReplyBuffer`，产出一一对应的 `(piece, instruction)`。不增加独立于正文状态的 `instruction: Mutex<_>`。
   - 气泡和通话的 `ReplyBuffer::new_with_mode(true, config.tts_streaming())` 开启圆括号风格继承并选择阶梯窗口。试听保持直接输入语义，使用自己的分段状态，不继承消息指导规则。
2. **完整气泡复用主方案 §4.3 的输入与终态处理**：
   - 原文一次交给消息朗读 owner，使用 `StreamBlockParser` 产出 stable，并执行一次终态收束。进行中同样只消费 stable，不对 chunk/tail 做第二套清洗。
   - 只有原始消息首行开头、有效闭合且内容非空并不超过 2000 字符的指导可以转为 instruction；之后各片段共享该消息指导。
   - 已闭合的非正文块跳过；Markdown 清洗后以 `\n` 连接。普通剩余正文和未闭合特种块按终态补读规则进入最终文本，未闭合指导不作为 instruction。
   - 不调用显示端静态 `parse_content` 后读取可空的 Markdown `content`，也不另建一个与 ChatFeed 规则重复的 `extract_and_project_complete` 扫描器。
3. **提交与合成**：
   - 气泡 `submit_tts_text` 接纳完整原文及 finished 状态，经上述 owner 处理；若保留累积提交能力，同样只接纳 stable，不把每次输入都视为已经完整结束。
   - 真正完成终态补读后仍没有可读正文，才回收该气泡任务并返回 `此消息没有可朗读的正文内容。`；不能因存在未闭合块就提前判断为空。
   - `speak_inner` 从对应输入 owner 一次取得片段及指导；消息任务携指导，试听始终不携指导。网络、样本、原生播放及任务取消继续复用现有执行器。
   - 不以 UI 收到终态事件代替后端资源释放；停止确认与注册表回收由 TTS owner 完成。

---

## 三、 无障碍级 Markdown 可读文本清洗与渐进式阶梯分段策略（核心重构）

### 1. 路线定位：已稳定正文的朗读投影
- **默认提示词保持原决定**：`DEFAULT_VOICE_GUIDANCE_RULES` 继续为“占位中”；用户规则仍原样注入，不增加口语化要求、指导格式或标签生成要求。
- **清洗输入边界**：`readable.rs` 只处理已经接纳的 stable 正文和终态文本，将 Markdown 的视觉结构转成听觉停顿。上游输出如何分 chunk、何时形成 stable 不由清洗器决定，不扩展解析器的正文沉淀策略。
- **终态例外有独立入口**：未闭合特种块解除外层封装后保留内容进入朗读，不能再用整块过滤将其吞掉。公式、URL 等具体规则仍适用。

---

### 2. 升级 [`readable.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/readable.rs)：修复现有 5 大硬伤

#### 2.1 现有 `readable.rs` 的 5 个严重缺陷
1. **抹平所有块级换行导致无标点粘连**：第 191–202 行将 `Heading`、`Paragraph`、`Item`、`SoftBreak`、`HardBreak` 全部替换为空格 `' '`，并在末尾执行 `split_whitespace().collect::<Vec<_>>().join(" ")` **彻底消灭了 `\n`**。由于 Markdown 标题（`### 核心结论`）和列表项（`- 第一点\n- 第二点`）末尾通常不带句号，压成空格后会导致标题与列表项一口气连读，且让 [`segmentation.rs` 第 217 行](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs#L217) 的 `\n` 断句逻辑彻底失效！
2. **吞掉所有行内代码（`` `code` ``）**：`pulldown_cmark` 将行内代码解析为 `Event::Code(CowStr)` 而非 `Event::Text`，现有代码漏掉了 `Event::Code` 分支，导致 `请将参数设为 \`true\` 即可` 变成 `请将参数设为 即可`！
3. **裸 URL 未过滤**：正文中的 `https://...` 裸链接被原样送入 TTS 逐字母朗读。
4. **删除线（`~~废弃内容~~`）未跳过**：`Tag::Strikethrough` 未计入 `opaque`，被划掉的文字仍会被念出。
5. **表格与公式无停顿或念出宏指令**：表格所有单元格被空格拼成无标点长串。

#### 2.2 无障碍 Markdown 清洗规则（`project_markdown_block`）

在 `readable.rs` 中启用 `Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH`，对已接纳文本按下列规则处理。公式沿用项目已有数学定界符规范化方式识别，不新增公式转口语系统：

| AST 事件 / 文本元素 | 无障碍清洗策略 | 说明与听觉效果 |
| :--- | :--- | :--- |
| **`Tag::CodeBlock(_)` / `Tag::Image { .. }` / `Tag::Strikethrough`** | `opaque += 1`（块内文本全部跳过） | 围栏/缩进代码块、图片描述、**删除线文字**一律不读 |
| **`Event::Code(code)`（行内短代码）** | 当 `opaque == 0` 时：若 `is_speakable_inline_code(&code)` 为真则追加 `code`；否则跳过 | 仅朗读 `<= 40` 字符、非 URL、非长文件路径（不含连续 `/` `\`）、非纯符号串的短标识符（如 `` `config.json` ``、`` `true` ``） |
| **`Tag::Link { dest_url, .. }`** | 保留链接显示文本；显示文本本身就是 URL 时跳过，不另读域名 | `[官方文档](https://...)` 只读“官方文档”；裸 URL 与自动链接不读 |
| **裸 URL** | 在 Markdown 文本事件中按地址边界删除 URL，保留后面的中文标点与正文；不在原始 Markdown 上用贪婪正则跨越语法清洗 | `详情 https://example.com。下一句。` 必须保留“详情”与“下一句。” |
| **行内控制标记与提及** | 删除完整 `[button:...]`、`[[点击按钮:...]]`、`@tag(...)` 控制表达式；`@Agent` / `@!Agent` 只按独立提及边界识别 | 不残留 `@tag` 参数；不得匹配邮箱或标识符内部的 `@`，不能把 `user@example.com` 改成 `user.com` |
| **`Event::InlineMath` / `Event::DisplayMath`** | 跳过公式内容 | 覆盖项目使用的行内/独立公式定界形式，保留周围解释文字 |
| **`TagEnd::Heading(_)` / `TagEnd::Paragraph` / `TagEnd::Item` / `TagEnd::BlockQuote(_)`** | 调用 `push_block_break(&mut output)`：若 `output` 末尾非空且不是 `\n`，追加 **`\n`** | **保留块级换行符 `\n`！** 让分段器在标题、段落、列表项结尾处天然切分并产生语音停顿 |
| **`Event::SoftBreak` / `Event::HardBreak` / `Event::Rule`** | 调用 `push_block_break(&mut output)` 追加 `\n` | 保留行级断句边界 |
| **表格（`Tag::Table`）** | 同行单元格之间追加 `"，"`；行末追加 `"\n"`，不留下多余尾逗号；表头同样形成独立行 | 单元格之间与行之间均保留停顿 |
| **空白折叠（`normalize_speech_whitespace`）** | **严禁使用 `split_whitespace().join(" ")`！** 改为：同一行内的连续水平空白（`' '`、`'\t'`）折叠为单个 `' '`；连续多个 `'\n'` 折叠为单个 `'\n'` | 既清理多余空格，又完整保留 `\n` 结构停顿边界供 `ReplyBuffer` 断句 |

上述 Markdown 事件处理不另行过滤所有 `*`、`_`、`~` 字符，避免删除正文里的字面字符。进入这里的文本已经稳定，不为模型 chunk 维护 URL、删除线、表头等猜测状态；完整末段由终态入口交付。`(风格)` 与 `[音频标签]` 保留给分段器使用。

---

### 3. 升级 [`segmentation.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs)：渐进式阶梯分段策略（Progressive Staircase Chunking）

#### 3.1 现有 `ReplyBuffer` 的两大痛点
1. **缺少最小切分字数（`min_chars`），短句切得粉碎**：
   - 现有代码（第 211 行）只要遇到 `。！？!?`，哪怕只有 2 个字（如 `"嗯。"`、`"好的！"`、`"是的。"`），就立刻切成一个独立 `ReplyPiece` 发起一次 HTTP 请求。
   - 在音色复刻（`VoiceTtsMode::Clone`）模式下，每次 HTTP 请求都要在 JSON 请求体中携带数百 KB 的参考音频 Base64，碎片化短句会导致极大的带宽浪费与段间衔接卡顿。
2. **流式与非流式共用死板参数（固定 24 / 160 字）**：
   - 未区分当前是**第几个分片（首段 vs 后续段）**，也未区分当前 TTS 通道是**流式（Builtin Stream）**还是**非流式（Clone 音色复刻 / Builtin 非流式）**。

#### 3.2 渐进式阶梯分片算法设计

在 [`ReplyBuffer`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs#L70) 中新增兩個状态字段：
- `streaming_tts: bool`（标识底层合成通道是否为流式：`config.tts_streaming()`）
- `emitted_segments: usize`（已发出的语音分片计数，`0` 代表决定首响延迟的**首段**）

根据 `(emitted_segments, streaming_tts)` 动态计算当前分片的阶梯切分窗口 `ChunkPolicy`：

| 分片阶段与模式 | `min_sentence_chars`<br>（句末标点 `.。！？!?\n` 最小切分字数） | `clause_cut_chars`<br>（子句标点 `，,；;：:、` 触发切分字数） | `max_chars`<br>（硬上限切分字数） | 设计目的 |
| :--- | :---: | :---: | :---: | :--- |
| **阶段 0：首段 (`emitted_segments == 0`)**<br>*(流式与非流式通用)* | **10 字** | **24 字** | **60 字** | 尽早合成首段；将“嗯。好的。”等短句合并，在各自阈值满足后的合适句末/子句边界发送 |
| **阶段 1：次段 (`emitted_segments == 1`)**<br>*(Builtin 流式 `streaming_tts = true`)* | **28 字** | **64 字** | **140 字** | 首段已在播放，次段合并 1~3 个自然句，减少请求次数并提升句子间语调连贯度 |
| **阶段 2+：后续段 (`emitted_segments >= 2`)**<br>*(Builtin 流式 `streaming_tts = true`)* | **40 字** | **90 字** | **160 字** | 稳定流式输出，保持中等段落粒度 |
| **阶段 1：次段 (`emitted_segments == 1`)**<br>*(非流式 / 音色复刻 `streaming_tts = false`)* | **60 字** | **100 字** | **150 字** | 合并中段正文，减少复刻音频的重复上传 |
| **阶段 2+：后续段 (`emitted_segments >= 2`)**<br>*(非流式 / 音色复刻 `streaming_tts = false`)* | **120 字** | **160 字** | **220 字** | 将多个完整句子合并为较长分片；每片仍遵守 220 字上限 |

字数按 Unicode 字符统计，圆括号全局风格前缀与方括号音频标签不占正文预算。只有实际产出非空正文片段才递增 `emitted_segments`；群聊每条完整消息各自从首段开始。未结束且不足切分条件时等待后续 stable，真正结束时释放不足最小字数的末段。指导词不是朗读正文，不计入分片字数。

#### 3.3 标点与边界判定细节优化
1. **句末标点（`is_sentence_end`）**：
   - 包含 `'。' | '！' | '？' | '!' | '?' | '…' | '\n'`，以及满足单词/数字边界、后跟空白或文本结束的英文句点 `'.'`；不能把小数点、标识符内部句点直接作为句末，也不引入缩写字典或新的语法兼容层。
   - 句末达到 `min_sentence_chars` 才切分，子句标点达到 `clause_cut_chars` 才切分；不足阈值的短句在当前正文中继续合并。
   - **连续标点吸附**：在完整的 stable/最终正文上整理切点，保留 `？！`、`……`、`。”`、`！）` 等序列和紧随的闭合符；开引号留给后文。整理切点同时遵守当前字数上限，必要时选择更早的切点。不为此读取模型 tail。
2. **换行符 `\n` 的清理**：
   - `\n` 在 `ReplyBuffer` 中作为**句末级断句点（`is_sentence_end`）**完成切分使命；
   - 分片内部的换行：前一段已有停顿标点时转单个空格，否则补 `，` 形成停顿；分片末尾的换行直接去掉。转换发生在片段产出时，不改写 `ReplyBuffer` 内用于已发送前缀校验的累计正文。
3. **沿用原生队列与串行合成**：
   - `speak_inner` 按顺序合成、向原生队列追加 PCM，并服从现有队列接纳与停止流程；当前片段完成提交后继续下一片段。
   - 本轮通过阶梯窗口减少碎片请求，复用原生播放队列，不引入并发 HTTP 预取池或另一套播放器。

---

## 四、 修改文件清单与验证标准

### 1. 修改文件清单
| 层级 | 文件路径 | 核心修改内容 |
| :--- | :--- | :--- |
| **Rust 后端** | [`src-tauri/src/vcp_modules/chat/voice_service/readable.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/readable.rs) | 清洗已稳定 Markdown：短行内代码、删除线、URL/控制标记、公式及换行；提供终态未闭合内容的补读投影，不重复识别整条消息指导。 |
| **Rust 后端** | [`src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/segmentation.rs) | 1. `ReplyBuffer` 增加 `streaming_tts` 与 `emitted_segments` 状态；<br>2. 实现渐进式阶梯切分窗口（首段 10~60 字极速首响，后续段按流式/非流式动态合并多句）；<br>3. 增加连续标点/闭合引号吸附与分片内 `\n` 转停顿标点。 |
| **Rust 后端** | [`src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/chat_feed.rs) | 整理可供完整气泡复用的消息朗读 owner，接纳 stable 与一次终态输入，在同一状态内管理指导/正文/分段，透传 `streaming_tts`；是否抽到共置文件按实际实现体量决定。 |
| **Rust 后端** | [`src-tauri/src/vcp_modules/chat/voice_service/tts.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/tts.rs) | 按通话、气泡、试听区分输入状态；气泡接入消息 owner，试听拆出直接文本内部入口；合成读取对应片段及指导，注册表负责停止确认和回收。 |
| **Rust 后端** | [`src-tauri/src/vcp_modules/chat/voice_service/call.rs`](file:///e:/VCPMobile/src-tauri/src/vcp_modules/chat/voice_service/call.rs) | 通话启动接纳时停止独立气泡朗读，与 TTS owner 完成双向互斥；设置页试听收尾落地后，移除旧通话→试听停止分支。 |
| **Vue 前端** | `src/features/chat/voice/useBubbleNarration.ts` *(新增)* | 应用作用域的唯一气泡朗读 owner：完整消息身份、启动/停止意图、单一事件监听及任务切换；不随单个消息组件卸载而销毁。 |
| **Vue 前端** | [`src/features/chat/MessageRenderer.vue`](file:///e:/VCPMobile/src/features/chat/MessageRenderer.vue) | `showMessageContextMenu` 新增“朗读气泡 / 停止朗读”长按菜单项。 |
| **Vue 前端** | `src/core/stores/chatHistoryStore.ts` 与现有会话切换入口 | 在实际消息编辑/删除/重新生成、话题/智能体切换位置停止受影响的气泡朗读；复用现有身份与 owner，不只挂菜单回调。 |
| **Vue 前端** | `src/features/chat/voice/VoiceTtsPreview.vue`、`VoiceSettingsPage.vue` 及现有设置页离开流程 | 设置页内独立试听，直接输入；统一离页时等待停止完成，避免任务越过设置页访问生命周期。 |

本表是增补范围。主方案中的解析器、Aurora、指导块前端类型/展示、历史上下文、旧模块删除以及文档同步仍需一并完成；不修改默认指导配置或生成契约内容。

### 2. 后续实施顺序

1. 按主方案完成首行指导、围栏配对和显示块接入，删除旧指导扫描器的实际调用与模块声明。
2. 统一消息朗读 owner 的 stable 接纳和终态补读，再完成 Markdown 清洗与阶梯分段；完整气泡直接复用该 owner。
3. 拆开试听输入与消息输入，落实设置页离开时停止试听，再移除通话侧旧试听停止分支；接通 TTS 注册表、停止确认和通话/气泡生命周期。
4. 接入气泡前端 owner、长按菜单及消息/会话变更入口。
5. 完成下列验证并同步生产文档。各阶段不以尚未更新的旧测试反向保留旧协议或旧行为。

### 3. 验证门禁（代码实施阶段执行）
1. **静态检查**：运行 `pnpm check` 零错误零警告通过。
2. **Rust 单元测试**（`cargo test --locked --lib`）：
   - `readable::tests`：行内短代码保留、删除线/代码块/图片/公式跳过；URL 后中文句子不被删除、邮箱不被提及规则破坏、控制标记参数不泄漏；标题、列表和表格保留停顿。
   - `segmentation::tests`：首段短句合并（`"嗯。好的。我们开始吧。"` 不切成 2 字碎片）；两种音频合成模式的阶梯窗口、终态不足最小字数的末段、标签不独占分片、全局风格继承、闭合引号与换行停顿。
   - 消息朗读 owner：同一正文以多个 stable 输入或完整消息一次输入时，指导和最终可读正文一致；终态未闭合块补读一次，闭合特种块仍排除，朗读任务取消后不补读。
   - `tts::tests`：气泡首行指导随各分段发送；试听直接文本且无指导；空正文任务回收、停止确认、快速任务切换与通话接纳成立。指导与正文不会跨消息或跨任务混用。
3. **前端单元测试**（`pnpm test:run`）：
   - 非生成中气泡菜单出现“朗读气泡”，提交完整消息原文；视觉拆分不改变输入范围，朗读中显示“停止朗读”。
   - 异步取原文后会话变化不会启动旧消息；快速 A→B→C、等待启动时停止、迟到事件均只影响所属任务。
   - 切换话题/智能体、编辑/删除/重新生成使源消息失效时停止；取消确认不停止；普通组件卸载不停止。
   - 试听以草稿直接文本提交；返回或切换设置页面时，停止确认完成后才退场，迟到启动不能在退出后继续播放。
4. **文档同步**：实现完成后同步主方案指定的局部 AGENTS 及设置/消息渲染文档，记录实际验证结果。

### 4. 实施与验证记录（2026-10-25）

- 已接通首行指导、标准静态/流式块与 Plain 显示，移除旧 `voice_guidance.rs`；历史原文保留。
- `MessageSpeech` 统一 stable 接纳、指导与终态补读；`ReplyBuffer` 完成两种合成模式的阶梯分段，并以有界正文前瞻切句。
- 气泡朗读新增完整来源身份，实际消息变更按权威受影响 ID 收尾；原文读取排除软删除，防止迟到启动朗读已经失效的消息。
- 试听走独立直接文本入口，设置页退场等待启动收束与停止确认；移除通话侧旧试听停止分支。
- `pnpm check` 通过；`cargo test --locked --lib -- --format terse`：517 项通过；`pnpm test:run`：93 个测试文件、766 项通过；`git diff --check` 通过。
- host Rust 单测编译输出 23 条未使用代码/导入警告；普通 `pnpm check` 的 Rust 检查无警告。本轮未执行 Android/iOS 构建或真机试听，自动化结果不代表设备音频效果验收。
