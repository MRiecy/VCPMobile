import { convertFileSrc } from "@tauri-apps/api/core";
import type { MarkdownNode, InlineNode, AstMutation } from "../types/chat";

const registryShards = new Map<string, Map<string, Node>>();

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
  registryShards.delete(messageId);
}

/**
 * 递归删除前缀符合的缓存映射（在执行 Replace 和 Remove 时调用）
 */
function cleanupSubtreeRefs(prefix: string, registry: Map<string, Node>): void {
  for (const key of registry.keys()) {
    if (key === prefix || key.startsWith(prefix + ".")) {
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

    case "raw_html":
      el = document.createElement("div");
      el.className = "vcp-raw-html-container";
      el.innerHTML = node.content || "";
      break;

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
      span.innerHTML = node.content || "";
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
 * 执行单条 AST Mutation 指令，以增量方式更新 DOM
 */
function executeMutation(
  mutation: AstMutation,
  messageId: string,
  sandbox: HTMLElement
): void {
  const registry = getRegistry(messageId);

  switch (mutation.op) {
    case "append": {
      const node = registry.get(mutation.id);
      if (node && node.nodeType === Node.TEXT_NODE) {
        (node as CharacterData).appendData(mutation.chunk);
      }
      break;
    }

    case "text": {
      const node = registry.get(mutation.id);
      if (node) {
        node.textContent = mutation.value;
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
      }
      break;
    }

    case "replace": {
      const oldNode = registry.get(mutation.id);
      if (oldNode && oldNode.parentNode) {
        const newDom = createDomFromNode(mutation.node, mutation.id, registry);
        if (newDom instanceof HTMLElement) {
          newDom.classList.add("vcp-stream-element-fade-in");
        }
        oldNode.parentNode.replaceChild(newDom, oldNode);
        cleanupSubtreeRefs(mutation.id, registry);
      }
      break;
    }

    case "replace_inline": {
      const oldNode = registry.get(mutation.id);
      if (oldNode && oldNode.parentNode) {
        const newDom = createInlineDom(mutation.node, mutation.id, registry);
        if (newDom instanceof HTMLElement) {
          newDom.classList.add("vcp-stream-element-fade-in");
        }
        oldNode.parentNode.replaceChild(newDom, oldNode);
        cleanupSubtreeRefs(mutation.id, registry);
      }
      break;
    }

    case "remove": {
      const node = registry.get(mutation.id);
      if (node && node.parentNode) {
        node.parentNode.removeChild(node);
        cleanupSubtreeRefs(mutation.id, registry);
      }
      break;
    }
  }
}

/**
 * 批量执行当前帧的 mutations 并直推更新至沙箱 DOM 元素
 */
export function applyFrame(
  mutations: AstMutation[],
  messageId: string,
  sandbox: HTMLElement
): void {
  for (const m of mutations) {
    executeMutation(m, messageId, sandbox);
  }
}
