# AST与fallback：函数修改索引

实现代码见[00第1节](00-综合结论与改造路线.md)。core只维护宿主和最新源码，消息controller在实际DOM批次结束后同步绘制。

## 1. 新增与替换

| 文件/函数 | 改动 |
|---|---|
| core/utils/diagramDom.ts | createDiagramHost、renderDiagramHostHtml、isDiagramHost、writeDiagramSource、preserveDiagramMorph、normalizeDiagramHosts |
| astRenderer.ts:243 renderNode(code_block) | Mermaid返回renderDiagramHostHtml(node.code) |
| astExecutor.ts:248 createDomFromNode | Mermaid调用createDiagramHost；函数末尾原registry.set保留 |
| astExecutor.ts:909 executeMutation/replace B | 双marker判身份；只writeDiagramSource；删除class判断与host.textContent写入 |
| useMessageTailRenderer.ts:8 MessageTailRendererOptions | 加onDomCommitted(root: HTMLElement|null):void |
| useMessageTailRenderer.ts:159 AST watcher | 整个同步批次try/finally，finally调用onDomCommitted |
| useMessageTailRenderer.ts:112 fallback watcher | fresh树normalize，morph后finally调用onDomCommitted |

## 2. 五处morphdom，四个options落点

~~~text
astExecutor.morphdomLiveOptions:570
  ├─ applyRawHtmlFreezeFrame:620
  └─ executeMutation.patch_raw_html:770

astExecutor.executeMutation.replace(raw_html/table):943
astExecutor.executeMutation.replace_inline:1062
useMessageTailRenderer fallback:123
~~~

四个options第一条判断统一为preserveDiagramMorph(from,to)，成功则return false；fresh树先规范化。返回false前写入source是必要步骤，因为morphdom也会跳过宿主属性同步。

## 3. 清理的执行位置

| 路径 | 同步批次出口 |
|---|---|
| applyFrame成功 | 认领frameSeq后，finally syncRoot+sweep |
| applyFrame部分成功后失败 | 保留既有恢复流程，finally协调实际已变更DOM |
| rebuildSnapshot | 重建完成后同一finally处理 |
| 无mutations但发生reset | 提前return仍经过finally |
| sandbox变更/清空 | finally基于当前根，sweep移除旧引用 |
| fallback morphdom删除/类型改变 | finally处理；同tag类型改变必须同时检查双marker |
| Vue移除整块stable | stable post的sweep处理 |
| MessageRenderer卸载 | dispose清空entries和promotionTemplates |

sweep使用messageRoot.contains(host)且isDiagramHost(host)；syncRoot只发现给定子树，二者不可共用tailRoot作为删除范围。

## 4. 两条可直接执行的反例

- 先绘图，再morph到同宿主的新source：检查host和SVG引用均不变、dataset已更新；这会抓住“先return false而忘记写source”。
- applyFrame先删除图A，再在后续mutation失败：批后entries必须不含A；这会抓住“仅result.ok时清理”。

已执行的[contract-probe.mjs](evidence/contract-probe.mjs)覆盖第一条以及同tag宿主变普通div的清理。frame失败场景加入现有AstExecutorContinuity集成用例。
