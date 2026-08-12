# VCPChat Scriptorium 核心技术研究

> 状态：`SOURCE-AUDIT-COMPLETE / DESIGN-EXTRACTION-COMPLETE / RUNTIME-VALIDATION-NOT-PERFORMED`
> 审计日期：2026-08-12
> 上游快照：`lioensky/VCPChat@17822ca450b95b657d775073066cc277dd56aeea`
> 官方 `main` 复核截止：2026-08-12T22:39:13+08:00
> 范围：只读审计 VCPChat；本目录只提出 VCPMobile 技术吸收方案，不包含功能实现

## 结论先行

VCPChat 最新落地的系统叫 **VCP Scriptorium（共笔文坊）**。它不是传统 DOCX/PPTX 编辑器，也不是一个“把任意 HTML DOM 交给大模型改写成自然语言”的组件。其真正有价值的核心是：

1. 用 VDOCX/VPPTX 自有工程模型保存完整源码、资源和文脉，**源码是唯一真相**；
2. 把 Markdown、静态 HTML、数学、Mermaid、代码和可编程 island 编译成带源码区间与哈希的渲染投影；
3. 用户点击渲染结果时，临时建立一棵能够无损还原源码的编辑树，再以精确区间事务写回源码，不以整棵 live DOM 序列化作为持久化路径；
4. Agent 读取源码、目录、编译后纯文本和真实截图，并通过带 `requestId + documentId + expectedRevision` 的可审阅 PR 修改同一份源码；
5. 保存格式是 ZIP 容器，二进制资源按 SHA-256 内容寻址、去重和校验。

所谓“HTML DOM 转自然语言”，在活跃代码里其实是四条方向不同的确定性转换：

| 链路 | 真实实现 | 是否是生成式自然语言 |
|---|---|---|
| HTML/HTM 或 Office 中间 HTML → 混合源码 | HTML/HTM 原文直接入库；DOCX/RTF 中间 HTML 经 Turndown，复杂节点保留为一等 HTML | 否，源码收纳或确定性格式规范化 |
| 编译 HTML → 可读文本 | 删除 `style/script/noscript` 后读取 `textContent` | 否，确定性纯文本投影 |
| 结构化工具结果 → Markdown | 把字段、数组、源码和诊断包装为 Agent 易读 Markdown | 否，确定性序列化 |
| 编辑态/运行态 DOM → source patch | 使用源码区间、稳定 ID 或文本指纹做局部回写 | 否，保守的编辑映射 |

视觉上下文是另一层组合：把上述纯文本摘要和 Electron 对真实渲染区域的截图一并交给 Agent。运行时脚本生成的 DOM 文字回写则根据文本、前后邻居、重复次序和相对位置排序候选；无法唯一定位时拒绝写回并恢复原 DOM 文本。

## 对 VCPMobile 的直接判断

不建议把桌面 Scriptorium 整体搬进移动端。应按能力分层吸收：

- **优先吸收**：VDOC v2 容器契约、Source SSOT、编译区间/哈希、Agent 只读语义接口、PR 乐观并发与文脉记录；
- **改造后吸收**：区域渲染态编辑、可见文本定位、资源生命周期、视觉上下文；必须补 Android IME、WebView、内存与进程生命周期设计；
- **暂缓吸收**：同一渲染进程内执行文档 JavaScript、Three.js、桌面自由画布和 100 MB 级前端 ZIP 打包。

VCPMobile 已经有 `scraper` 驱动的 HTML → VCP Markdown 聊天上下文格式器、`marked`、DOMPurify、KaTeX、Mermaid、morphdom，以及 Rust 侧 `zip + sha2 + pulldown-cmark`。这些是可复用的解析、渲染和存储原语，但**还不是安全文档管线**：现有 HTML 格式器不是 sanitizer，当前 trusted-rich guard 也不是通用 HTML/CSS/URL 安全边界。移动端应抽取协议和不变量，另建有界文档安全边界，不复制桌面经典脚本模块。

## 文档导航

| 文档 | 内容 |
|---|---|
| [01-上游提交与系统架构.md](./01-上游提交与系统架构.md) | 最新提交快照、活跃模块、VDOC 模型、核心数据流与上游漂移 |
| [02-DOM语义投影与渲染态编辑核心.md](./02-DOM语义投影与渲染态编辑核心.md) | “DOM → 自然语言”勘误、混合编译器、VDOCX/VPPTX/运行态 DOM 三套回写算法 |
| [03-Agent协作容器版本与安全边界.md](./03-Agent协作容器版本与安全边界.md) | Agent 查询与 PR 协议、revision/generation、文脉、ZIP/CAS 资源、脚本安全边界 |
| [04-VCPMobile技术吸收与分期建议.md](./04-VCPMobile技术吸收与分期建议.md) | 现有能力对照、保留/重写/后置矩阵、目标分层、阶段与验收门槛 |
| [05-Magi三方审查与综合裁决.md](./05-Magi三方审查与综合裁决.md) | Melchior、Balthasar、Casper 三方审查及最终裁决 |

## 证据与限制

- 官方远端在本轮审计过程中从 `ebe1f87` 连续推进到 `17822ca`；本报告固定后者，不使用漂移的 `main` 作为行号依据。
- 从重构起点 `e032ecb` 到固定快照共 29 个提交，相对父提交 `deaa02f` 涉及 61 个文件，约 `+27,528 / -7,576`；这是快速变化的 Alpha，而非稳定 SDK。
- 上游 `ScriptoriumModules/AGENT.md` 明确禁止读取/运行旧测试与语法检查。本次只做活跃加载链和源码审计，没有把“能读懂源码”包装成运行时通过。
- 重构序列中 `ebe1f87` 加入的 `scriptorium-agent.js` 和若干 `*.origin.js` 没有被 `scriptorium.html` 加载；报告只把 `scriptorium-agent-port.js` 视为当前 Agent 活跃实现。
- 审计早期 README 仍称容器根含 `document.json`；最终提交 `17822ca` 已重写文档并与 `manifest/source/lineage/resources` v2 布局对齐。这个当日修正说明文档仍在快速收敛，协议判断继续以固定代码快照为准。

## 一句话吸收原则

> 吸收“源码真相 + 可逆投影 + 冲突保护 + 人类审批”这套内核，不把桌面 DOM、Electron 桥和可执行文档脚本原样塞进 Android WebView。

## 上游入口

- [VCPChat 官方仓库](https://github.com/lioensky/VCPChat)
- [本研究固定提交](https://github.com/lioensky/VCPChat/commit/17822ca450b95b657d775073066cc277dd56aeea)
- [Scriptorium README（固定快照）](https://github.com/lioensky/VCPChat/blob/17822ca450b95b657d775073066cc277dd56aeea/ScriptoriumModules/README.md)
