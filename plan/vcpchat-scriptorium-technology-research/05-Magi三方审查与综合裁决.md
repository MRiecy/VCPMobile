# 05 Magi 三方审查与综合裁决

## 1. 审查方法

三位审查者在同一只读上游 checkout 上独立工作：

- **Melchior（逻辑与系统）**：真源、编译、回写、revision/generation、PR 与异步一致性；
- **Balthasar（直觉与体验）**：所谓“DOM → 自然语言”的实际语义、视觉上下文、渲染态编辑和人机审阅体验；
- **Casper（务实与交付）**：提交范围、模块边界、依赖/Electron 耦合、Mobile 复用成本和分期。

三方核心源码基线是 `ebe1f87`。主审计随后把官方远端更新到 `17822ca` 并逐段复核：`3ff0e0c` 以前的增量修改 renderer/lineage/navigation/session/shell/CSS 等文件，最后两个提交只修改 Collaborator 提示词和 README；compiler、store、flow/deck editor、agent-port、media、runtime 和 container 未变。本文件中的核心结论与风险仍适用，但风险均为静态源码审计推断，未按上游禁令执行测试或动态复现。

## 2. Melchior：逻辑与系统

### 2.1 肯定项

1. Source of Truth 清晰：VDOC model/source 是真相，DOM 是可丢弃派生物。
2. 混合编译器先分区、保护 atomic 内容，再编译并产生 sourceRange/hash，是合理的结构化增量基础。
3. Flow、Deck、动态 island 使用三套不同身份策略，没有假装一个算法能覆盖全部 DOM。
4. 保存已经使用 generation/documentId/revision context，说明作者正确理解了异步跨文档污染问题。
5. 容器 v2 的 ZIP 条目与 SHA-256 CAS 资源协议可独立提取。

### 2.2 关键质疑

1. pending PR 只绑定 documentId/revision，没有 generation，存在同文档 restore 后 ABA 风险。
2. 普通 source PR 未把 programmable-content review 写入 proposal，自动批准的 refuse 防线可能失效；运行时 review 仍在，但提案期承诺断层。
3. 重复 target 默认取第一次匹配，与“歧义即失败”原则相冲突。
4. Deck 的 2 秒 DOM 队列在 Store 仍未 dirty 时可进入换文档路径，而且 `replaceDocument` 发生在旧 editor flush 之前。
5. Media 跨多个 await 后使用“当前” document/adapter，没有 generation context；资源注册又直接改 manifest/map，绕过 Store mutation。
6. 重启后 lineage 可以恢复 pending 记录，但执行 closure 只存在内存 Map，记录会变成不可继续审批的 pending。
7. `approvePr` 先从 pending 删除再跑异步 mutation/persist，异常路径缺少完整终态收口。
8. 对象 ID 补缺但未全局查重、history serialize 每次更新 modifiedAt、未接线的 `runtimeTextOverrides` 等都说明 Alpha 的数据不变量尚未彻底收束。

### 2.3 Melchior 裁决

核心范式可取，但不能直接承诺事务安全。Mobile 若吸收，必须把所有异步和审批统一提升为：

```text
operation context = generation + documentId + revision
commit condition   = context current + target unique/hash matched
terminal state     = applied | rejected | conflict | failed | aborted
```

pending/terminal 不能跨重启回退，媒体/编辑/保存/截图不能各自发明一套弱化保护。

## 3. Balthasar：直觉与体验

### 3.1 术语纠偏

Balthasar 的首要结论是：不要把当前能力称为“HTML DOM 转自然语言引擎”。活跃实现是：

> 源码优先的渲染语义投影（source-derived rendered-text projection），配合可视上下文、保守源码回映射与可追责 PR 协作闭环。

纯文本、Markdown 工具回执、details 结构体和截图是四个不同表层。混为一谈会让设计者误以为系统理解了表格、媒体、可见性和运行时组件。

### 3.2 体验价值

1. Agent 的 `GetRenderedText` 不读取 live DOM，因此能自然排除工具栏、resize handle、contenteditable 标记和动画临时节点。
2. 纯文本不足时提供真实截图，Agent 可以同时看源码、结构、文字和视觉。
3. Outline 从源码扫描标题，GetSection 返回源码与编译文字，适合渐进读取上下文。
4. 人类在同屏查看“局部渲染差异 + 源码差异”，输入审阅回执，再合并或拒绝，是强可追责体验。
5. Flow 点击后才把局部被动渲染树切换成可逆编辑树，兼顾拖选阅读与精准编辑，是值得保留的交互思想。

### 3.3 体验盲区

1. `textContent` 会丢链接 URL、媒体属性、表格行列关系、alt/ARIA，也可能读入 CSS 隐藏但仍在源码中的文字。
2. 动态脚本生成的文字不进入 Agent renderedText；Canvas/SVG/KaTeX/Mermaid 最终视觉只在截图中。
3. flow 的 `scope:'viewport'` 截的是 surface，但语义文字仍可为全文，范围标签容易误导。
4. 固定延迟 + 双 RAF 不代表图片、字体、图表、动画已稳定，也没有 capture revision guard。
5. sandbox PR 预览不执行脚本/KaTeX/Mermaid，动态内容的“渲染差异”并不忠实。
6. 活跃 `scriptorium-agent-port.js` 没有 dormant `scriptorium-agent.js` 的结构化媒体提取，不能把未加载能力写进产品说明。

### 3.4 Balthasar 裁决

Mobile 应把语义做成结构化 IR，同时保留 `plainText` 和 VCP Markdown 两个明确投影。UI 只对可逆 region 开放输入；“无法可靠回写”的文字仍可选择和复制，但不应伪装成可编辑。

## 4. Casper：务实与交付

### 4.1 范围判断

Casper 指出，约 2.75 万新增行被 origin/旧 agent 资产和 README 重写放大，不能用行数宣称引擎成熟。官方远端在审计期间约半小时内四次推进；README 与 v2 容器实现的短暂漂移虽已在最终提交修正，但当前更准确的状态仍是“设计丰富、验证证据不足、快速收敛的 Alpha”。

### 4.2 可复用边界

可复用：

- Store owner、adapter 多态和 composition root 思路；
- VDOC container/URI/hash 协议；
- source-first compiler/edit region；
- PR、lineage 和 context-bound save。

必须重写：

- Electron preload/IPC/BrowserWindow、文件对话框、PDF、页面截图；
- `window.ScriptoriumXxx` 经典模块；
- JSZip renderer 打包；
- VCP Collaborator → control service → 必须打开 Electron Window 的调用链。

暂不引入：

- Mammoth/Cheerio/Turndown 的完整导入链；
- CodeMirror 5、Pretext、Anime、Three；
- VPPTX 自由画布和桌面对象 GUI；
- 可编程文档。

### 4.3 依赖判断

桌面是 Marked 12，Mobile 是 Marked 18；token、换行和 HTML 行为不能假定一致。Mobile 已有 DOMPurify、KaTeX、Mermaid、morphdom、Rust zip/sha2/pulldown-cmark，应先用 corpus 验证现有依赖，而不是为复制桌面引入第二套栈。

### 4.4 Casper 裁决

优先做只读 VDOCX：Rust 有界 ZIP/hash，Vue 安全渲染与语义读取，禁脚本、禁外链收纳、禁 VPPTX。之后再做 source editor/PR，最后才准入区域渲染态编辑。没有 Android IME、OOM、长文和生命周期证据前，不进入完整桌面能力迁移。

## 5. 三方共识与分歧

| 议题 | 共识/分歧 |
|---|---|
| 是否是生成式 DOM → 自然语言 | 三方一致否定；是确定性投影和格式封装 |
| 最核心资产 | 三方一致：Source SSOT、可逆/定向投影、context-bound PR、lineage、CAS |
| PR 产品价值 | Balthasar 认为产品概念最成熟；Melchior 指出实现仍有 generation、安全审查和终态缺口；两者不矛盾 |
| 是否整体迁移 | 三方一致反对 |
| 首期 | 三方一致建议只读 VDOCX/协议冻结 |
| 可编程 JS | 三方一致建议 Mobile 主 WebView 禁止 |
| 区域视觉编辑 | 原理可取，但必须后置到 Android IME/Selection/性能证据之后 |

## 6. 综合裁决

### 6.1 研究准入候选（不构成实现授权）

- 可将 Scriptorium 作为 VCPMobile 文档协作研究的技术来源；
- 若用户另行授权进入实现，先冻结 VDOC schema、Source Buffer、edit region、semantic snapshot、operation context、PR receipt 和 ResourceRef；
- 首个候选验证范围仅为现有 Rust zip/sha2 与前端渲染依赖上的只读 spike；
- 把本次发现的 Alpha 风险转为 Mobile 设计的负面需求和挑战集。

本专项只完成技术提取与研究准入判断，不批准创建新领域模块、修改产品代码或启动迁移。

### 6.2 NO-GO

- 当前直接复制 ScriptoriumModules 到 VCPMobile；
- 当前承诺兼容 VCPChat `main` 或称 VDOC v2 为稳定标准；
- 当前开放 Agent 自动写、可编程脚本、VPPTX、Office 保真导入或视觉对象编辑；
- 用 `textContent` 替换 VCPMobile 现有 HTML → VCP Markdown；
- 仅凭桌面源码和旧测试目录宣称 Android 可用、安全或性能达标。

### 6.3 下一决策点

若用户决定进入实现，推荐只做一个选择：

> P1 是否以“只读 VDOCX + 结构化 Agent 语义快照”为产品边界？

默认答案是“是”。这个边界能验证容器、编译、资源、语义和 Android 性能，又不会提前引入编辑、PR、IME 和脚本的复合风险。
