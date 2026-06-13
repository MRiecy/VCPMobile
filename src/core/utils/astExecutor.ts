import { convertFileSrc } from "@tauri-apps/api/core";
import type { MarkdownNode, InlineNode, AstMutation } from "../types/chat";

function recordAstTrace(data: any): void {
  if (import.meta.env.DEV) {
    if (!(window as any).__VCP_AST_TRACES__) {
      (window as any).__VCP_AST_TRACES__ = [];
    }
    (window as any).__VCP_AST_TRACES__.push({
      timestamp: performance.now(),
      ...data
    });

    if (typeof window !== "undefined" && !(window as any).__VCP_ANALYZE_AST_TRACES__) {
      (window as any).__VCP_ANALYZE_AST_TRACES__ = () => {
        const traces = (window as any).__VCP_AST_TRACES__ || [];
        if (traces.length === 0) {
          console.log("%c[AST Trace Analyzer] 暂无任何 AST 录制数据。请先开始对话！", "color: #ff9800; font-weight: bold;");
          return;
        }

        const mutations = traces.filter((t: any) => t.type === "mutation");
        const frames = traces.filter((t: any) => t.type === "frame_done");
        const cleanups = traces.filter((t: any) => t.type === "cleanup_registry");

        const failedMutations = mutations.filter((m: any) => m.status === "failed");

        console.log(`%c[AST Trace Analyzer] 📊 录制统计面板`, "color: #2196f3; font-weight: bold; font-size: 1.2em;");
        console.log(`- 录制时间段: 从首条 ${traces[0].timestamp.toFixed(2)}ms 到末条 ${traces[traces.length - 1].timestamp.toFixed(2)}ms`);
        console.log(`- 帧渲染次数 (applyFrame): ${frames.length} 次`);
        console.log(`- 突变总指令数 (executeMutation): ${mutations.length} 条`);
        console.log(`- 缓存销毁次数 (cleanupRegistry): ${cleanups.length} 次`);

        if (failedMutations.length === 0) {
          console.log("%c- 运行健康度: 100% (所有突变成功执行！)", "color: #4caf50; font-weight: bold;");
        } else {
          console.log(`%c- 运行健康度: 异常 (存在 ${failedMutations.length} 条执行失败的突变！)`, "color: #f44336; font-weight: bold;");
          console.group("❌ 失败突变详细列表 (按时间排序):");
          failedMutations.forEach((m: any, idx: number) => {
            console.log(
              `[%c${idx + 1}%c] MsgId: %c${m.messageId}%c | Op: %c${m.op}%c | TargetNodeId: %c${m.mutationId}%c\n  └─ 失败原因: %c${m.detail}\n  └─ 负载参数:`,
              "color: #ff5722;", "",
              "color: #9c27b0; font-family: monospace;", "",
              "color: #009688; font-weight: bold;", "",
              "color: #e91e63; font-family: monospace;", "",
              "color: #f44336;",
              m.mutationPayload
            );
          });
          console.groupEnd();
        }

        console.groupCollapsed("🔍 每一帧渲染后 Registry 缓存节点数走势:");
        frames.forEach((f: any, idx: number) => {
          console.log(
            `Frame #${idx + 1} | MsgId: %c${f.messageId}%c | 突变数: ${f.mutationsCount} | 缓存节点数: ${f.registryKeys.length}\n  └─ HTML长度: ${f.afterHtml.length}`,
            "color: #9c27b0; font-family: monospace;", ""
          );
        });
        console.groupEnd();

        console.log("%c提示: 可以直接在控制台输入 `window.__VCP_AST_TRACES__` 查看完整底端数据结构。", "color: #9e9e9e; font-style: italic;");
      };
    }
  }
}

const registryShards = new Map<string, Map<string, Node>>();

export type ApplyFrameResult = {
  ok: boolean;
  applied: number;
  failed?: {
    index: number;
    mutation: AstMutation;
    reason: string;
  };
};

type ExecuteMutationResult = {
  ok: boolean;
  reason?: string;
};

/**
 * 获取或者为指定 Message ID 初始化一个 DOM 节点缓存表分片
 */
function getRegistry(messageId: string): Map<string, Node> {
  let shard = registryShards.get(messageId);
  if (!shard) {
    shard = new Map();
    registryShards.set(messageId, shard);
  }
  return shard;
}

/**
 * 释放特定 Message ID 占用的全部 DOM 节点引用，防止内存泄漏。
 * 在 MessageRenderer.vue 卸载（onUnmounted）或清除聊天时调用。
 */
export function cleanupRegistry(messageId: string): void {
  const registry = registryShards.get(messageId);
  const size = registry ? registry.size : 0;
  registryShards.delete(messageId);

  recordAstTrace({
    type: "cleanup_registry",
    messageId,
    registrySizeReleased: size
  });
}

/**
 * 递归删除前缀符合的缓存映射（在执行 Replace 和 Remove 时调用）
 */
function cleanupSubtreeRefs(prefix: string, registry: Map<string, Node>, includeSelf = false): void {
  for (const key of registry.keys()) {
    if ((includeSelf && key === prefix) || key.startsWith(prefix + ".")) {
      registry.delete(key);
    }
  }
}

/**
 * 将 MarkdownNode 递归地渲染为真实 DOM 并存入 Registry 缓存中
 */
function createDomFromNode(
  node: MarkdownNode,
  id: string,
  registry: Map<string, Node>
): Node {
  console.warn(`[AST createDomFromNode] id=${id}, node=${JSON.stringify(node)}`);
  let el: HTMLElement;
  switch (node.type) {
    case "paragraph":
      el = document.createElement("p");
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        const childDom = createInlineDom(child, childId, registry);
        el.appendChild(childDom);
      });
      break;

    case "heading":
      el = document.createElement(`h${node.level || 1}`);
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        const childDom = createInlineDom(child, childId, registry);
        el.appendChild(childDom);
      });
      break;

    case "code_block": {
      el = document.createElement("pre");
      el.className = "vcp-code-block vcp-scrollable";
      if (node.highlighted_html) {
        let html = node.highlighted_html;
        // 剥离多余的 <pre><code> 嵌套包裹以满足前端样式
        const nestedPreMatch = html.match(/<pre[^>]*>\s*<code>([\s\S]*?)<\/code>\s*<\/pre>/i);
        if (nestedPreMatch && nestedPreMatch[1].trim().startsWith("<pre")) {
          const innerMatch = nestedPreMatch[1].match(/<pre[^>]*>([\s\S]*?)<\/pre>/i);
          if (innerMatch) {
            html = innerMatch[1];
          }
        }
        el.innerHTML = html;
      } else {
        const code = document.createElement("code");
        code.textContent = node.code || "";
        el.appendChild(code);
      }
      break;
    }

    case "blockquote":
      el = document.createElement("blockquote");
      node.children?.forEach((child, i) => {
        const childId = `${id}.b${i}`;
        const childDom = createDomFromNode(child as any, childId, registry);
        el.appendChild(childDom);
      });
      break;

    case "list": {
      const tag = node.ordered ? "ol" : "ul";
      el = document.createElement(tag);
      node.items?.forEach((itemNodes, itemIdx) => {
        const li = document.createElement("li");
        const liId = `${id}.li${itemIdx}`;
        registry.set(liId, li);
        itemNodes.forEach((itemNode, bIdx) => {
          const childId = `${liId}.b${bIdx}`;
          const childDom = createDomFromNode(itemNode, childId, registry);
          li.appendChild(childDom);
        });
        el.appendChild(li);
      });
      break;
    }

    case "table": {
      const wrapper = document.createElement("div");
      wrapper.className = node.wrapper_class || "vcp-table-wrapper";
      const table = document.createElement("table");

      const thead = document.createElement("thead");
      const headerTr = document.createElement("tr");
      node.header?.forEach((cell, colIdx) => {
        const th = document.createElement("th");
        const thId = `${id}.th${colIdx}`;
        registry.set(thId, th);
        cell.forEach((inlineNode, i) => {
          const childId = `${thId}.i${i}`;
          const childDom = createInlineDom(inlineNode, childId, registry);
          th.appendChild(childDom);
        });
        headerTr.appendChild(th);
      });
      thead.appendChild(headerTr);
      table.appendChild(thead);

      const tbody = document.createElement("tbody");
      node.rows?.forEach((row, rowIdx) => {
        const tr = document.createElement("tr");
        row.forEach((cell, colIdx) => {
          const td = document.createElement("td");
          const tdId = `${id}.tr${rowIdx}.td${colIdx}`;
          registry.set(tdId, td);
          cell.forEach((inlineNode, i) => {
            const childId = `${tdId}.i${i}`;
            const childDom = createInlineDom(inlineNode, childId, registry);
            td.appendChild(childDom);
          });
          tr.appendChild(td);
        });
        tbody.appendChild(tr);
      });
      table.appendChild(tbody);
      wrapper.appendChild(table);
      el = wrapper;
      break;
    }

    case "thematic_break":
      el = document.createElement("hr");
      break;

    case "mermaid":
      el = document.createElement("div");
      el.className = "mermaid-placeholder";
      el.textContent = node.code || "";
      break;

    case "raw_html": {
      el = document.createElement("div");
      el.className = "vcp-raw-html-container";
      // 物理防御：由于 node.content 在流式打字期间可能是极度残缺、未闭合的裸 HTML（如 <img src="...），
      // 直接赋给 innerHTML 会导致部分 WebView 解析器因无法定位标签边界而直接丢弃并生成空 DOM。
      // 我们通过外层临时 <div> 进行强行诱导闭合补全，确保浏览器能够正确还原并渲染中间状态节点。
      const temp = document.createElement("div");
      temp.innerHTML = `<div>${node.content || ""}</div>`;
      const parsed = temp.firstElementChild;
      if (parsed) {
        el.innerHTML = parsed.innerHTML;
      } else {
        el.innerHTML = node.content || "";
      }
      break;
    }

    default:
      el = document.createElement("div");
  }
  registry.set(id, el);
  return el;
}

/**
 * 将 InlineNode 递归地渲染为真实 DOM 并存入 Registry 缓存中
 */
function createInlineDom(
  node: InlineNode,
  id: string,
  registry: Map<string, Node>
): Node {
  let el: Node;
  switch (node.type) {
    case "text":
      el = document.createTextNode(node.value || "");
      break;

    case "strong":
      el = document.createElement("strong");
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        el.appendChild(createInlineDom(child, childId, registry));
      });
      break;

    case "emphasis":
      el = document.createElement("em");
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        el.appendChild(createInlineDom(child, childId, registry));
      });
      break;

    case "strikethrough":
      el = document.createElement("del");
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        el.appendChild(createInlineDom(child, childId, registry));
      });
      break;

    case "code":
      el = document.createElement("code");
      el.textContent = node.value || "";
      break;

    case "link": {
      const a = document.createElement("a");
      a.href = node.needs_asset_conversion && node.href ? convertFileSrc(node.href) : (node.href || "");
      a.title = node.title || "";
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        a.appendChild(createInlineDom(child, childId, registry));
      });
      el = a;
      break;
    }

    case "image": {
      const img = document.createElement("img");
      img.src = node.needs_asset_conversion && node.src ? convertFileSrc(node.src) : (node.src || "");
      img.alt = node.alt || "";
      img.title = node.title || "";
      img.loading = "lazy";
      img.className = "vcp-markdown-image";
      el = img;
      break;
    }

    case "line_break":
    case "soft_break":
      el = document.createElement("br");
      break;

    case "inline_math": {
      const isDisplay = node.display_mode || false;
      const span = document.createElement("span");
      span.className = isDisplay ? "vcp-math-block no-swipe" : "vcp-math-inline no-swipe";
      span.setAttribute("data-latex", node.content || "");
      span.textContent = node.content || "";
      el = span;
      break;
    }

    case "quoted_text": {
      const span = document.createElement("span");
      span.className = "highlighted-quote";
      node.children?.forEach((child, i) => {
        const childId = `${id}.i${i}`;
        span.appendChild(createInlineDom(child, childId, registry));
      });
      el = span;
      break;
    }

    case "highlight_tag": {
      const span = document.createElement("span");
      span.className = "highlighted-tag";
      span.textContent = node.value || "";
      el = span;
      break;
    }

    case "alert_tag": {
      const span = document.createElement("span");
      span.className = "highlighted-alert-tag";
      span.textContent = node.value || "";
      el = span;
      break;
    }

    case "raw_html_inline": {
      const span = document.createElement("span");
      // 物理防御：使用临时 <div> 强行闭合可能未闭合的 inline 标签，防止 WebView 抛弃节点
      const temp = document.createElement("div");
      temp.innerHTML = `<div>${node.content || ""}</div>`;
      const parsed = temp.firstElementChild;
      if (parsed) {
        span.innerHTML = parsed.innerHTML;
      } else {
        span.innerHTML = node.content || "";
      }
      el = span;
      break;
    }

    default:
      el = document.createTextNode("");
  }
  registry.set(id, el);
  return el;
}

/**
 * 从完整 AST 快照重建沙箱 DOM 与 registry。用于 tail epoch reset 或增量执行失败后的恢复。
 */
export function rebuildSnapshot(
  nodes: MarkdownNode[] | undefined,
  messageId: string,
  sandbox: HTMLElement
): void {
  sandbox.innerHTML = "";
  cleanupRegistry(messageId);
  const registry = getRegistry(messageId);
  for (const [index, node] of (nodes || []).entries()) {
    const dom = createDomFromNode(node, `t${index}`, registry);
    sandbox.appendChild(dom);
  }

  recordAstTrace({
    type: "snapshot_rebuild",
    messageId,
    nodesCount: nodes?.length || 0,
    registryKeys: Array.from(registry.keys()),
    html: sandbox.innerHTML
  });
}

/**
 * 执行单条 AST Mutation 指令，以增量方式更新 DOM
 */
function executeMutation(
  mutation: AstMutation,
  messageId: string,
  sandbox: HTMLElement
): ExecuteMutationResult {
  const registry = getRegistry(messageId);
  console.warn(`[AST Mutation Exec] op=${mutation.op}, id=${mutation.id}, parent=${(mutation as any).parent || ''}, chunk=${(mutation as any).chunk || ''}, val=${(mutation as any).value || ''}`);

  let status = "success";
  let detail = "";

  switch (mutation.op) {
    case "append": {
      const node = registry.get(mutation.id);
      if (node && node.nodeType === Node.TEXT_NODE) {
        (node as CharacterData).appendData(mutation.chunk);
      } else {
        status = "failed";
        detail = node ? `Node type is not text (${node.nodeType})` : "Node not found in registry";
      }
      break;
    }

    case "text": {
      const node = registry.get(mutation.id);
      if (node) {
        node.textContent = mutation.value;
      } else {
        status = "failed";
        detail = "Node not found in registry";
      }
      break;
    }

    case "add": {
      const parentNode = mutation.parent === "root"
        ? sandbox
        : registry.get(mutation.parent);
      if (parentNode) {
        const newDom = createDomFromNode(mutation.node, mutation.id, registry);
        if (newDom instanceof HTMLElement) {
          newDom.classList.add("vcp-stream-element-fade-in");
        }
        parentNode.appendChild(newDom);
      } else {
        status = "failed";
        detail = `Parent node '${mutation.parent}' not found`;
      }
      break;
    }

    case "add_inline": {
      const parentNode = registry.get(mutation.parent);
      if (parentNode) {
        const newDom = createInlineDom(mutation.node, mutation.id, registry);
        if (newDom instanceof HTMLElement) {
          newDom.classList.add("vcp-stream-element-fade-in");
        }
        parentNode.appendChild(newDom);
      } else {
        status = "failed";
        detail = `Parent node '${mutation.parent}' not found`;
      }
      break;
    }

    case "prop": {
      const node = registry.get(mutation.id);
      if (node instanceof HTMLElement) {
        if (mutation.key === "level" && /^H[1-6]$/i.test(node.tagName)) {
          const level = Math.max(1, Math.min(6, Number(mutation.value) || 1));
          const replacement = document.createElement(`h${level}`);
          replacement.innerHTML = node.innerHTML;
          replacement.className = node.className;
          for (const attr of Array.from(node.attributes)) {
            if (attr.name !== "class") replacement.setAttribute(attr.name, attr.value);
          }
          if (node.parentNode) {
            registry.set(mutation.id, replacement);
            node.parentNode.replaceChild(replacement, node);
          } else {
            status = "failed";
            detail = "Node has no parentNode";
          }
        } else {
          node.setAttribute(mutation.key, mutation.value);
        }
      } else {
        status = "failed";
        detail = node ? "Node is not an HTMLElement" : "Node not found in registry";
      }
      break;
    }

    case "replace": {
      const oldNode = registry.get(mutation.id);
      if (oldNode) {
        if (oldNode.parentNode) {
          const parent = oldNode.parentNode;
          cleanupSubtreeRefs(mutation.id, registry, true);
          const newDom = createDomFromNode(mutation.node, mutation.id, registry);
          if (newDom instanceof HTMLElement) {
            newDom.classList.add("vcp-stream-element-fade-in");
          }
          parent.replaceChild(newDom, oldNode);
          console.warn(`[AST replace success] id=${mutation.id}`);
        } else {
          status = "failed";
          detail = "Old node has no parentNode";
          console.warn(`[AST replace fail - oldNode has no parent] id=${mutation.id}`);
        }
      } else {
        status = "failed";
        detail = "Old node not found in registry";
        console.warn(`[AST replace fail - oldNode not found in registry] id=${mutation.id}`);
      }
      break;
    }

    case "replace_inline": {
      const oldNode = registry.get(mutation.id);
      if (oldNode) {
        if (oldNode.parentNode) {
          const parent = oldNode.parentNode;
          cleanupSubtreeRefs(mutation.id, registry, true);
          const newDom = createInlineDom(mutation.node, mutation.id, registry);
          if (newDom instanceof HTMLElement) {
            newDom.classList.add("vcp-stream-element-fade-in");
          }
          parent.replaceChild(newDom, oldNode);
        } else {
          status = "failed";
          detail = "Old node has no parentNode";
        }
      } else {
        status = "failed";
        detail = "Old node not found in registry";
      }
      break;
    }

    case "remove": {
      const node = registry.get(mutation.id);
      if (node) {
        if (node.parentNode) {
          node.parentNode.removeChild(node);
          console.warn(`[AST remove success] id=${mutation.id}`);
          cleanupSubtreeRefs(mutation.id, registry, true);
        } else {
          status = "failed";
          detail = "Node has no parentNode";
          console.warn(`[AST remove fail - node has no parent] id=${mutation.id}`);
        }
      } else {
        status = "failed";
        detail = "Node not found in registry";
        console.warn(`[AST remove fail - node not found in registry] id=${mutation.id}`);
      }
      break;
    }
  }

  recordAstTrace({
    type: "mutation",
    messageId,
    op: mutation.op,
    mutationId: mutation.id,
    mutationPayload: {
      parent: (mutation as any).parent,
      chunk: (mutation as any).chunk,
      value: (mutation as any).value,
      nodeType: (mutation as any).node?.type || null
    },
    status,
    detail,
    registrySize: registry.size
  });

  return status === "success" ? { ok: true } : { ok: false, reason: detail };
}

/**
 * 批量执行当前帧的 mutations 并直推更新至沙箱 DOM 元素
 */
export function applyFrame(
  mutations: AstMutation[],
  messageId: string,
  sandbox: HTMLElement
): ApplyFrameResult {
  const registry = getRegistry(messageId);
  const beforeHtml = sandbox.innerHTML;
  let result: ApplyFrameResult = { ok: true, applied: 0 };

  for (const [index, mutation] of mutations.entries()) {
    const mutationResult = executeMutation(mutation, messageId, sandbox);
    if (!mutationResult.ok) {
      result = {
        ok: false,
        applied: index,
        failed: {
          index,
          mutation,
          reason: mutationResult.reason || "Mutation failed"
        }
      };
      break;
    }
    result.applied += 1;
  }

  const afterHtml = sandbox.innerHTML;
  console.warn(`[AST Executor Frame Done] messageId=${messageId}, ok=${result.ok}, html=${afterHtml}`);

  recordAstTrace({
    type: "frame_done",
    messageId,
    mutationsCount: mutations.length,
    appliedCount: result.applied,
    ok: result.ok,
    failed: result.failed,
    beforeHtml,
    afterHtml,
    registryKeys: Array.from(registry.keys())
  });

  return result;
}
