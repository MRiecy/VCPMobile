# 02 DOM 语义投影与渲染态编辑核心

## 1. 先拆开“HTML DOM 转自然语言”

Scriptorium 没有一个统一的 `DOM → natural language` 引擎。源码中至少有五条方向不同的转换链：

| 编号 | 输入 → 输出 | 目的 | 关键实现 |
|---|---|---|---|
| A | HTML/HTM 或 Office 中间 HTML → Markdown-first 混合源码 | 把 HTML/DOCX/RTF 语义纳入 VDOCX 真源 | HTML 原文直存；Office 中间 HTML 用 Turndown + 保留规则 |
| B | 混合源码 → 编译 HTML/编辑投影 | 显示、编辑、索引与诊断 | Hybrid Compiler |
| C | 编译 HTML/页源码 → 纯文本 | 给 Agent 一份便宜、确定的可读文本 | 删除脚本/样式 + `textContent` |
| D | 结构化结果 → Markdown | 把工具返回包装成 Agent 易读协议 | 字段映射 + 标题/列表/代码围栏 |
| E | 编辑 DOM/运行态文字 → 源码 patch | 让人在视觉表面编辑，同时保持源码真相 | 区间事务、稳定 ID 或文本指纹 |

A 和 C 最接近用户观察到的“HTML 转自然语言”，但都不是 LLM/NLP：A 是源码收纳或结构保真的格式规范化，C 是有损纯文本抽取。E 是编辑映射，更不等于语言生成。

## 2. A：HTML 与 Office 内容导入为混合源码

这里必须区分两条真实路径：

- `.html/.htm`：UTF-8 解码、去 BOM 并记录原换行类型后，**HTML 原文直接作为 `markdown-hybrid` 一等源码入库**，不经过 Turndown；
- `.docx/.rtf`：DOCX 先由 Mammoth 生成并补充段落语义，RTF 先转为中间 HTML；随后才由 Turndown 把可表达结构降为 Markdown，并保留 Markdown 无法无损表达的 HTML。

Turndown 保留规则覆盖：

- `table/thead/tbody/tfoot/tr/th/td`；
- `style/script/svg/canvas/video/audio`；
- 带 `style`、`class` 或 `data-vdoc-island` 的节点整体 `outerHTML`；
- DOCX 中仅含一个正 `text-indent` 声明、且单位为 `em` 或 `pt` 的 `<p>`，会把该缩进归一为两个全角空格，避免四个 ASCII 空格被 Markdown 当代码块。

证据：

- DOCX 段落缩进规范化：[`scriptoriumImportService.js:199-228`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/services/scriptoriumImportService.js#L199-L228)
- Turndown 与 HTML 保留规则：[`230-256`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/services/scriptoriumImportService.js#L230-L256)
- DOCX → 中间 HTML：[`686-708`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/services/scriptoriumImportService.js#L686-L708)
- 各格式分流与 HTML 原文直存：[`721-758`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/services/scriptoriumImportService.js#L721-L758)

对 DOCX/RTF 中间 HTML 而言，这是一个很务实的“可表达则降为 Markdown，不可表达则保留 HTML island”的混合语言策略。它避免了两种极端：

- 全部留成 HTML，导致普通文字难以被人和 Agent 直接修改；
- 强行全部转 Markdown，丢失表格、布局、媒体、SVG 和交互脚本。

需要注意：这项 round-trip 限制针对 DOCX/RTF 的规范化路径；中间 HTML 的样式、解析归一化和 Turndown 规则都可能改变内容。HTML/HTM 路径则选择原文直存，但之后的编译、编辑和保存仍不能据此宣称任意浏览器 DOM 可逆。

## 3. B：混合编译器

### 3.1 保护结构再编译

VDOCX 源码可以混合 Markdown、HTML island、`style`、代码围栏、Mermaid 和数学表达式。编译器不能把所有内容直接交给 Marked，因此采用“扫描—占位—编译—恢复”：

1. 扫描代码/Mermaid 围栏；
2. 扫描带 `data-vdoc-island` 的完整 `<div>`，拒绝空 ID、重复 ID 和未闭合根；
3. 扫描 style 和数学区间；
4. 用不可碰撞 token 暂时替换稳定结构；
5. 仅把剩余 Markdown 交给 Marked；
6. 恢复被保护的源码域；
7. 执行 HTML 清理，生成依赖、诊断、block index 和 edit regions。

关键证据：

- 围栏扫描：[`vdoc-hybrid-compiler.js:67-95`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-hybrid-compiler.js#L67-L95)
- island 扫描与 refuse 诊断：[`149-211`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-hybrid-compiler.js#L149-L211)
- 结构保护：[`422-453`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-hybrid-compiler.js#L422-L453)
- 全编译返回：[`780-887`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-hybrid-compiler.js#L780-L887)

### 3.2 编辑区间不是持久节点 ID

编译器把普通 Markdown/静态 HTML 分成可编辑 region，把 island、style、代码、Mermaid 和块级数学标为 `stable-atomic`。每个 region 包含：

```ts
{
  key: `edit-${ordinal + 1}-${hash(sourceSlice)}`,
  type,
  flowKind,
  sourceRange: { start, end },
  sourceHash,
  islandId
}
```

依据：[`vdoc-hybrid-compiler.js:637-685`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-hybrid-compiler.js#L637-L685)。

这里的 `edit-*` 是“当前编译结果里的瞬态锚点”，源码变化后会重新计算；不能把它当跨版本永久 ID。永久稳定身份只用于显式 island、VPPTX 文本节点和视觉对象。

### 3.3 两种输出

- `html`：完整语义渲染结果，供阅读、Agent 语义提取等使用；
- `previewHtml`：把每个 region 包进带 `data-vdoc-edit-key/type/flow-kind` 的 shell，供编辑表面建立源码映射。

`previewHtml` 的职责不是存储，而是把源码区间和渲染节点关联起来。源码单换行用 Marked `breaks: true` 编译为 `<br>`，使静态显示和展开编辑的换行语义一致。

## 4. C：给 Agent 的纯文本投影

活跃 `textFromHtml()` 的算法非常小：

1. 将 HTML 放入 `<template>`；
2. 删除 `style/script/noscript`；
3. 读取 `template.content.textContent`；
4. 归一化 NBSP、行尾空格和三连以上换行。

证据：[`scriptorium-agent-port.js:22-33`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L22-L33)。

`GetRenderedText` 对 VDOCX 先编译源码，再对 `compiled.html` 做上述抽取；对 VPPTX 则直接对每页 `slide.source` 做抽取，并附带 notes。证据：[`175-202`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L175-L202)。

能力边界：

- 不理解 computed style、视觉层次、ARIA、Canvas、SVG 图形含义或图片内容；
- 不读取脚本执行后生成的 live DOM；
- 不保留 Markdown 标题、列表、链接等完整结构；
- 其结构感主要依赖编译 HTML 自带的文本换行；
- 优点是确定、便宜、不会把脚本源码直接暴露为正文。

因此接口字段名 `semanticFormat: compiled-html` 更准确的解释是“从语义编译产物抽取文本”，不是“自然语言语义理解”。

## 5. D：结构化结果包装为 Markdown

VCP `ScriptoriumCollaboratorService` 将响应对象递归序列化为中文 Markdown：

- 常见字段映射为“文档 ID、修订号、渲染文本、源码”等标签；
- 数组转列表或分节；
- `source/html/documentCss/deckCss/target/replace` 使用动态长度代码围栏；
- 完整结构继续保留在 `details`，Markdown 只是模型友好的表层。

证据：[`ScriptoriumCollaboratorService.js:178-377`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/VCPDistributedServer/Plugin/ScriptoriumCollaborator/ScriptoriumCollaboratorService.js#L178-L377)。

这一层会产出面向 Agent 阅读的 Markdown 工具回执，但内容没有经过生成模型改写；它是稳定、可测试的协议渲染器。

## 6. E1：VDOCX 渲染态文字编辑

### 6.1 不是直接编辑并序列化 live DOM

VDOCX 在未编辑时显示编译后的被动 DOM。用户单击某个普通 region 后，编辑器才临时用一棵“源码投影编辑树”替换这个 region：

- Markdown 分隔符作为隐藏 marker 留在 DOM 中；
- 可见内容放进 `strong/em/del/code` 等语义元素；
- 静态 HTML 的开闭标签也作为隐藏 marker 保留；
- marker 虽然 `display:none`，仍参与 `textContent`（该样式只改变视觉呈现），所以按 DOM 顺序能还原原始源码。

关键实现：

- Markdown marker 与语义装饰：[`scriptorium-flow-editor.js:465-574`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L465-L574)
- HTML 标签 marker 与可见语义节点：[`576-672`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L576-L672)
- 从编辑 DOM 重建源码：[`237-281`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L237-L281)

### 6.2 最重要的不变量：先证明可逆，再开放编辑

编辑树创建后必须满足：

```text
editableSourceText(editor) === originalRegionSource
```

不相等就提示“无法建立无损渲染态编辑映射”，拒绝进入 contenteditable。证据：[`scriptorium-flow-editor.js:869-903`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L869-L903)。

这是整套设计最值得提取的思想：WYSIWYG 不是先接受 DOM 变异再努力清洗，而是先建立一条可证明的局部可逆投影，只对成功建立投影的区域开放编辑。

### 6.3 精确源码事务

提交时使用 `{from, to, expected, insert}`：

- 当前源码切片必须仍等于 `expected`；
- 默认不能跨过 `stable-atomic` 边界；
- 成功后通过 adapter 替换 Source Buffer；
- revision 增加，历史调度；
- 重新编译后只有找到精确的新 region 才恢复 caret，否则全量刷新。

依据：[`scriptorium-flow-editor.js:55-117`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L55-L117)、[`1499-1615`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-flow-editor.js#L1499-L1615)。

编辑器显式处理 `beforeinput`、粘贴、硬换行和 IME `compositionstart/compositionend`。这说明它已经意识到 contenteditable 不是简单 `input` 事件问题，但桌面 Electron 行为仍不能直接代表 Android WebView IME 已通过。

## 7. E2：VPPTX 稳定 ID 回写

VPPTX 的每页真源是完整 HTML。归一化阶段为可编辑节点分配唯一 `data-vdoc-text`，为容器和对象分配其他稳定身份。编辑器不会保存整棵渲染 DOM，而是：

1. 克隆被编辑节点；
2. 恢复数学语义，移除 editor-only 属性；
3. 清理 clone 的 `innerHTML`；
4. 在源码 template 中按 `data-vdoc-text` 找目标；
5. 只更新目标内部语义 HTML和显式允许的属性；
6. 约 2 秒批量 flush 后替换当前页源码。

证据：[`vdoc-core.js:409-446`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/vdoc-core.js#L409-L446)、[`scriptorium-deck-editor.js:215-306`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-deck-editor.js#L215-L306)。

这条链路适合结构本来就是完整 HTML scene 的模型，不适合拿来覆盖 Markdown-first VDOCX。

## 8. E3：可编程 island 的运行态文字回写

脚本可能动态生成表格、图例或数据卡片，这些节点在源码 HTML 中没有稳定 ID。`scriptorium-rendered-text.js` 为每个可见 Text 节点建立指纹：

```text
text
ordinal / sameTextOrdinal
previousText / nextText
elementPath / textNodeIndex / parentTag
```

写回时先在当前 island 源码中查找全部相同文本，再按前后邻居、同文出现次序和相对文档位置打分。只有最高分和次高分的差值满足阈值才返回范围；否则 fail closed。

证据：

- 可见文本与指纹：[`scriptorium-rendered-text.js:35-133`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-rendered-text.js#L35-L133)
- 候选评分与歧义拒绝：[`194-258`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-rendered-text.js#L194-L258)
- 仅在当前 island 源码内 patch：[`396-469`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-rendered-text.js#L396-L469)

它还执行三项保守限制：

- 排除脚本、样式、Canvas、SVG、媒体、常见表单控件、交互宿主和 atomic 区；
- 只让“宿主全部文本恰好由当前 Text 节点承担”的最小叶子节点进入输入态；
- focusout 时才提交；定位失败时恢复进入编辑前的 DOM 文本，避免“看似改好但无法保存”。

这是一种有工程价值的弱锚点算法，但不能保证任意生成 DOM 可编辑。重复短文本、脚本拼接文本、转义差异和源码中同文常量都可能导致拒绝；拒绝本身是正确行为。

## 9. 视觉上下文补足纯文本缺陷

`GetVisualContext` 等待可选稳定时间和两个动画帧，返回纯文本摘要与待截取矩形；Electron 主进程再对真实页面区域截图，组合成：

```text
content[0] = Markdown 元数据 + renderedText
content[1] = image_url(base64 JPEG/PNG)
details    = 结构化语义与截图元数据
```

证据：[`scriptorium-agent-port.js:423-472`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/scriptorium-agent-port.js#L423-L472)、[`scriptoriumAgentControlService.js:182-240`](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/modules/services/scriptoriumAgentControlService.js#L182-L240)。

因此 Scriptorium 的“AI 理解文档”不是靠更聪明的 DOM 文本抽取，而是把四种互补证据交给 Agent：完整源码、结构化目录/区块、廉价纯文本、真实截图。

## 10. 可提取的不变量

1. **真源不变量**：任何持久修改最终都落到 Source Buffer/scene source，不保存 live DOM。
2. **投影不变量**：普通渲染树可随时丢弃并从真源重建。
3. **可逆准入**：局部编辑树只有通过 `project(editTree) == originalSourceSlice` 才能开放输入。
4. **原子边界**：脚本 island、代码、Mermaid、style、块级数学等不能被普通文本事务跨越。
5. **过期拒绝**：source hash 或 expected slice 失效就不写入。
6. **身份分层**：瞬态 edit key 用于当前编译；稳定 ID 用于长期定向结构；弱文本指纹只用于无法分配 ID 的运行态节点。
7. **歧义失败**：不能唯一定位时恢复视图，而不是猜一个源码位置。
8. **语义与视觉互补**：纯文本、结构和截图各有边界，不宣称单一路径理解全部文档。
