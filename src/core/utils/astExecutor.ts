import { convertFileSrc } from "@tauri-apps/api/core";
import morphdom from "morphdom";
import type { MarkdownNode, InlineNode, AstMutation } from "../types/chat";
import { filterTrustedRichHtml, filterTrustedRichHtmlUrl } from "./astRenderer";
import {
  appendStreamTextFragment,
  clearStreamElementReveals,
  discardStreamTextFragments,
  flushStreamTextFragments,
  STREAM_ELEMENT_REVEAL_CLASS,
  supportsStreamRevealMotion,
} from "./streamTextFade";

function isAstDebugEnabled(): boolean {
  return Boolean(import.meta.env.DEV && (window as any).__VCP_AST_DEBUG__);
}

function astDebugLog(...args: unknown[]): void {
  if (import.meta.env.DEV && isAstDebugEnabled()) {
    console.warn(...args);
  }
}

function recordAstTrace(data: any): void {
  if (import.meta.env.DEV && isAstDebugEnabled()) {
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

export interface ApplyFrameOptions {
  smoothStreaming?: boolean;
}

interface FrameMotion {
  inlineRevealToken?: object;
  animateBlocks: boolean;
}

function animateStreamBlock(element: HTMLElement, parent: Node): void {
  if (
    !supportsStreamRevealMotion()
    || (parent instanceof Element && parent.closest(`.${STREAM_ELEMENT_REVEAL_CLASS}`))
  ) {
    return;
  }
  const clear = (event: Event) => {
    if (event.target !== element) return;
    element.classList.remove(STREAM_ELEMENT_REVEAL_CLASS);
    element.removeEventListener("animationend", clear);
    element.removeEventListener("animationcancel", clear);
  };
  element.classList.add(STREAM_ELEMENT_REVEAL_CLASS);
  element.addEventListener("animationend", clear);
  element.addEventListener("animationcancel", clear);
}

export function shouldRevealAddedMarkdownNode(node: MarkdownNode): boolean {
  switch (node.type) {
    case "paragraph":
    case "heading":
    case "blockquote":
    case "list":
    case "table":
      return true;
    case "code_block":
    case "thematic_break":
    case "raw_html":
      return false;
    default: {
      const exhaustive: never = node;
      return exhaustive;
    }
  }
}

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
  discardStreamTextFragments(messageId);
  const registry = registryShards.get(messageId);
  const size = registry ? registry.size : 0;
  registryShards.delete(messageId);

  if (import.meta.env.DEV && isAstDebugEnabled()) {
    recordAstTrace({
      type: "cleanup_registry",
      messageId,
      registrySizeReleased: size
    });
  }
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
 * 计算 node 相对 root 的 childNodes 索引路径（从 root 到 node 的下标序列）。
 * 不在 root 子树内则返回 null。用 childNodes（含文本节点）而非 children，保证路径可寻回文本节点。
 */
function computeChildPath(node: Node, root: Node): number[] | null {
  const path: number[] = [];
  let cur: Node | null = node;
  while (cur && cur !== root) {
    const parent: Node | null = cur.parentNode;
    if (!parent) return null;
    path.unshift(Array.prototype.indexOf.call(parent.childNodes, cur));
    cur = parent;
  }
  return cur === root ? path : null;
}

/** 沿 childNodes 索引路径从 root 下行取节点，任一层缺失返回 null。 */
function resolveChildPath(root: Node, path: number[]): Node | null {
  let cur: Node | null = root;
  for (const idx of path) {
    if (!cur) return null;
    cur = cur.childNodes[idx] || null;
  }
  return cur;
}

/**
 * 修复流式打字期间未闭合的 HTML 标签和属性引号断口，防止 WebView 发生排版吞噬或解析回退
 */
function repairHtmlFragment(html: string): string {
  if (!html) return "";

  // 语义：只处理「最后一个未闭合标签片段」。最后一个 '<' 之后若再无 '>'，则它到末尾是一段正在
  // 流式输出、尚未闭合的标签（如 '<img src="http://...'）；只有这种片段才会把后续内容（含外层
  // wrapper 的 </div>）吞进未闭合的标签/属性里。若标签已闭合或无标签，则无断口，原样返回——
  // 关键修复：不再对整串做引号配平，避免正文文本中合法的奇数引号被误加一个尾引号。
  const lastOpenAngle = html.lastIndexOf("<");
  const lastCloseAngle = html.lastIndexOf(">");
  if (lastOpenAngle <= lastCloseAngle) {
    return html;
  }

  const head = html.slice(0, lastOpenAngle);
  const fragment = html.slice(lastOpenAngle);

  // 非真实标签起始（如正文里的孤立 '<' 或 '< b'）：直接丢弃该断口片段，交由下一帧补全。
  if (!/^<\/?[a-zA-Z]/.test(fragment)) {
    return head;
  }

  // 仅在该未闭合标签片段内部判断属性引号是否成对（忽略转义引号）。
  let doubleQuotes = 0;
  let singleQuotes = 0;
  for (let i = 0; i < fragment.length; i++) {
    const char = fragment[i];
    if (char === '"' && (i === 0 || fragment[i - 1] !== "\\")) doubleQuotes++;
    if (char === "'" && (i === 0 || fragment[i - 1] !== "\\")) singleQuotes++;
  }
  const quotesBalanced = doubleQuotes % 2 === 0 && singleQuotes % 2 === 0;

  // 属性引号成对（如 '<div class="card" '）：标签结构已完整，仅缺收尾 '>'，补 '>' 让其当帧即可渲染，
  // 体验上长 HTML 容器能尽早显示。引号失衡（如 '<img src="http://foo'）说明正卡在某个属性值中途，
  // 无法安全补全，丢弃整个未闭合标签，下一帧 chunk 到达后完整渲染。
  return quotesBalanced ? `${html}>` : head;
}

/**
 * 将 MarkdownNode 递归地渲染为真实 DOM 并存入 Registry 缓存中
 */
function createDomFromNode(
  node: MarkdownNode,
  id: string,
  registry: Map<string, Node>
): Node {
  if (import.meta.env.DEV && isAstDebugEnabled()) {
    astDebugLog(`[AST createDomFromNode] id=${id}, node=${JSON.stringify(node)}`);
  }
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
      if (node.lang === "mermaid") {
        el = document.createElement("div");
        el.className = "mermaid-placeholder";
        el.textContent = node.code || "";
      } else {
        if (node.highlighted_html) {
          // 🆕 利用浏览器原生 DOM 解析器，直接实例化后端提供的带外壳完整 HTML
          const temp = document.createElement("div");
          temp.innerHTML = node.highlighted_html.trim();
          el = temp.firstElementChild as HTMLElement;
          if (!el || el.tagName !== "PRE") {
            // 兜底安全保障
            el = document.createElement("pre");
            el.className = "vcp-code-block vcp-scrollable";
            el.innerHTML = node.highlighted_html;
          }
        } else {
          el = document.createElement("pre");
          el.className = "vcp-code-block vcp-scrollable";
          const code = document.createElement("code");
          code.textContent = node.code || "";
          el.appendChild(code);
        }
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


    case "raw_html": {
      el = document.createElement("div");
      el.className = "vcp-raw-html-container";
      // 物理防御：由于 node.content 在流式打字期间可能是极度残缺、未闭合的裸 HTML（如 <img src="...），
      // 直接赋给 innerHTML 会导致部分 WebView 解析器因无法定位标签边界而直接丢弃并生成空 DOM。
      // 我们通过外层临时 <div> 进行强行诱导闭合补全，确保浏览器能够正确还原并渲染中间状态节点。
      const temp = document.createElement("div");
      const guardedContent = filterTrustedRichHtml(node.content || "");
      temp.innerHTML = `<div>${repairHtmlFragment(guardedContent)}</div>`;
      const parsed = temp.firstElementChild;
      if (parsed) {
        el.innerHTML = parsed.innerHTML;
      } else {
        el.innerHTML = guardedContent;
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
      const rawHref = node.needs_asset_conversion && node.href ? convertFileSrc(node.href) : (node.href || "");
      const href = filterTrustedRichHtmlUrl(rawHref, "a", "href");
      if (href !== null) a.setAttribute("href", href);
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
      const rawSrc = node.needs_asset_conversion && node.src ? convertFileSrc(node.src) : (node.src || "");
      const src = filterTrustedRichHtmlUrl(rawSrc, "img", "src");
      if (src !== null) img.setAttribute("src", src);
      img.alt = node.alt || "";
      img.title = node.title || "";
      img.loading = "lazy";
      img.className = "vcp-markdown-image";
      el = img;
      break;
    }

    case "break":
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

    case "vcp_custom": {
      const span = document.createElement("span");
      span.className = `vcp-custom-${node.kind}`;
      if (node.children && node.children.length > 0) {
        node.children.forEach((child, i) => {
          const childId = `${id}.i${i}`;
          span.appendChild(createInlineDom(child, childId, registry));
        });
      } else {
        span.textContent = node.value || "";
      }
      el = span;
      break;
    }

    case "raw_html_inline": {
      const span = document.createElement("span");
      // 物理防御：使用临时 <div> 强行闭合可能未闭合的 inline 标签，防止 WebView 抛弃节点
      const temp = document.createElement("div");
      const guardedContent = filterTrustedRichHtml(node.content || "");
      temp.innerHTML = `<div>${repairHtmlFragment(guardedContent)}</div>`;
      const parsed = temp.firstElementChild;
      if (parsed) {
        span.innerHTML = parsed.innerHTML;
      } else {
        span.innerHTML = guardedContent;
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

  if (import.meta.env.DEV && isAstDebugEnabled()) {
    recordAstTrace({
      type: "snapshot_rebuild",
      messageId,
      nodesCount: nodes?.length || 0,
      registryKeys: Array.from(registry.keys()),
      html: sandbox.innerHTML
    });
  }
}

/**
 * RawHtml 容器的冻结前沿：记录容器内已闭合定型的根级子节点数量。
 *
 * 依据：流式 tail 严格只追加，且 HTML 解析是确定性的——同一文本前缀解析出的已闭合
 * 根级子树逐帧完全相同；开放元素链（含 adoption agency / foster parenting 的全部
 * 作用域）必然挂在最后一个根级子节点上，够不着它之前的兄弟。因此每帧只需把新闭合的
 * 根子节点从 fresh 树物理搬入（appendChild，零重 parse），再 morphdom 收敛最后一个
 * "活跃"根子节点即可，全量 morphdom 的 O(整树)/帧 走查降为 O(活跃子树)/帧。
 *
 * 非追加式变化（epoch reset）会由前端重建容器元素，WeakMap 条目随之失效，天然安全。
 */
const rawHtmlFrozenCounts = new WeakMap<HTMLElement, number>();

/** 活跃区 morphdom 的守卫选项（与全量路径一致：媒体/图片状态保留） */
function morphdomLiveOptions() {
  return {
    childrenOnly: false,
    onBeforeElUpdated: (fromEl: HTMLElement, toEl: HTMLElement) => {
      if (fromEl.isEqualNode(toEl)) return false;
      if (fromEl.tagName === "IMG" && (fromEl as HTMLImageElement).complete) return false;
      if (fromEl.tagName === "VIDEO" || fromEl.tagName === "AUDIO") {
        if (!(fromEl as HTMLMediaElement).paused) return false;
      }
      return true;
    },
  };
}

/**
 * 对 raw_html 容器应用一帧"冻结前沿"增量更新。
 * 返回 false 表示未建立基线或子节点结构对不上，调用方应回退全量 morphdom。
 */
function applyRawHtmlFreezeFrame(container: HTMLElement, fresh: HTMLElement): boolean {
  const freshChildren = Array.from(fresh.childNodes);
  // 最后一个根子节点是活跃区（开放元素链所在），其余全部已闭合定型
  const eligible = freshChildren.length - 1;
  if (eligible < 0) {
    container.textContent = "";
    rawHtmlFrozenCounts.set(container, 0);
    return true;
  }

  const frozen = rawHtmlFrozenCounts.get(container);
  if (frozen === undefined) return false; // 首帧：尚无基线
  if (eligible < frozen || container.childNodes.length !== frozen + 1) {
    return false; // 结构漂移（理论上不会发生），放弃增量
  }

  // 1. 新闭合的根子节点物理搬入活跃子节点之前（节点直接移动，无重 parse）
  const live = container.childNodes[frozen] ?? null;
  for (let i = frozen; i < eligible; i++) {
    container.insertBefore(freshChildren[i], live);
  }

  // 2. 收敛活跃子节点
  const newLive = freshChildren[eligible];
  if (live) {
    if (live.nodeType === Node.TEXT_NODE && newLive.nodeType === Node.TEXT_NODE) {
      if (live.nodeValue !== newLive.nodeValue) live.nodeValue = newLive.nodeValue;
    } else if (
      live.nodeType === Node.ELEMENT_NODE &&
      newLive.nodeType === Node.ELEMENT_NODE &&
      (live as Element).tagName === (newLive as Element).tagName
    ) {
      morphdom(live as HTMLElement, newLive as HTMLElement, morphdomLiveOptions());
    } else {
      container.replaceChild(newLive, live);
    }
  } else {
    container.appendChild(newLive);
  }

  rawHtmlFrozenCounts.set(container, eligible);
  return true;
}

/** 全量 morphdom 之后重建冻结基线：除最后一个根子节点外全部视为已定型。 */
function resetRawHtmlFrontier(container: HTMLElement) {
  rawHtmlFrozenCounts.set(container, Math.max(0, container.childNodes.length - 1));
}

/**
 * 执行单条 AST Mutation 指令，以增量方式更新 DOM
 */
function executeMutation(
  mutation: AstMutation,
  messageId: string,
  sandbox: HTMLElement,
  motion: FrameMotion,
): ExecuteMutationResult {
  const registry = getRegistry(messageId);
  const debugEnabled = import.meta.env.DEV && isAstDebugEnabled();
  if (debugEnabled) {
    astDebugLog(`[AST Mutation Exec] op=${mutation.op}, id=${mutation.id}, parent=${(mutation as any).parent || ''}, chunk=${(mutation as any).chunk || ''}, val=${(mutation as any).value || ''}`);
  }

  let status = "success";
  let detail = "";

  switch (mutation.op) {
    case "append": {
      const node = registry.get(mutation.id);
      if (node && node.nodeType === Node.TEXT_NODE) {
        if (motion.inlineRevealToken) {
          appendStreamTextFragment(
            messageId,
            mutation.id,
            node as Text,
            mutation.chunk,
            motion.inlineRevealToken,
          );
        } else {
          (node as CharacterData).appendData(mutation.chunk);
        }
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

    case "patch_code": {
      const node = registry.get(mutation.id);
      const codeRoot = node instanceof HTMLElement
        ? Array.from(node.children).find((child) =>
            child instanceof HTMLElement
            && child.tagName === "CODE"
            && child.hasAttribute("data-vcp-stream-code")
          )
        : undefined;
      const stable = codeRoot?.querySelector<HTMLElement>("[data-vcp-code-stable]");
      const active = codeRoot?.querySelector<HTMLElement>("[data-vcp-code-active]");

      if (stable && active) {
        // Syntect 已对源码做 HTML 转义，这里只解析后端生成的 span 片段。
        // 完整行永久追加；只有尚未换行的末行会被替换。
        const completedTemplate = document.createElement("template");
        completedTemplate.innerHTML = mutation.completed_html;
        const activeTemplate = document.createElement("template");
        activeTemplate.innerHTML = mutation.active_html;
        stable.appendChild(completedTemplate.content);
        active.replaceChildren(activeTemplate.content);
      } else {
        status = "failed";
        detail = node
          ? "Incremental code anchors not found"
          : "Code block not found in registry";
      }
      break;
    }

    case "patch_raw_html": {
      // 后端 html5ever 树权威切分：冻结域 = 未闭合最外层元素的子节点列表。
      // 容器结构恒为 .vcp-raw-html-container > outer > [frozen × N] + [live?]；
      // 种子帧整棵重建，稳态帧只搬入新冻结段并收敛 live 子节点。
      // 冻结段每个字节全程只 parse 这一次，之后永久挂载、对象身份不变。
      const existing = registry.get(mutation.id);
      let container: HTMLElement;
      if (existing instanceof HTMLElement) {
        container = existing;
      } else {
        container = document.createElement("div");
        container.className = "vcp-raw-html-container";
        sandbox.appendChild(container);
        registry.set(mutation.id, container);
      }

      if (mutation.seed) {
        const seedTemplate = document.createElement("template");
        seedTemplate.innerHTML = filterTrustedRichHtml(mutation.live_html);
        container.replaceChildren(seedTemplate.content);
        break;
      }

      const outer = container.firstElementChild;
      if (!outer) {
        status = "failed";
        detail = "Raw html container missing outer element (no seed applied)";
        break;
      }

      // 1. 搬入新冻结段。插入点 = 旧冻结数 = frozen_total - 本帧新增数
      const frozenTemplate = document.createElement("template");
      frozenTemplate.innerHTML = mutation.frozen_html
        ? filterTrustedRichHtml(mutation.frozen_html)
        : "";
      const newFrozenCount = frozenTemplate.content.childNodes.length;
      const prevFrozen = mutation.frozen_total - newFrozenCount;
      if (
        prevFrozen < 0 ||
        (outer.childNodes.length !== prevFrozen && outer.childNodes.length !== prevFrozen + 1)
      ) {
        status = "failed";
        detail = `Raw html frozen frontier desync (children=${outer.childNodes.length}, prevFrozen=${prevFrozen})`;
        break;
      }
      {
        const live = outer.childNodes[prevFrozen] ?? null;
        for (const child of Array.from(frozenTemplate.content.childNodes)) {
          outer.insertBefore(child, live);
        }
      }

      // 2. 收敛活跃子节点（守卫与全量 morphdom 路径一致）
      const liveTemplate = document.createElement("template");
      liveTemplate.innerHTML = mutation.live_html
        ? filterTrustedRichHtml(mutation.live_html)
        : "";
      const newLive = liveTemplate.content.firstChild ?? null;
      const oldLive = outer.childNodes[mutation.frozen_total] ?? null;
      if (oldLive && newLive) {
        if (oldLive.nodeType === Node.TEXT_NODE && newLive.nodeType === Node.TEXT_NODE) {
          if (oldLive.nodeValue !== newLive.nodeValue) oldLive.nodeValue = newLive.nodeValue;
        } else if (
          oldLive.nodeType === Node.ELEMENT_NODE &&
          newLive.nodeType === Node.ELEMENT_NODE &&
          (oldLive as Element).tagName === (newLive as Element).tagName
        ) {
          morphdom(oldLive as HTMLElement, newLive as HTMLElement, morphdomLiveOptions());
        } else {
          outer.replaceChild(newLive, oldLive);
        }
      } else if (newLive) {
        outer.appendChild(newLive);
      } else if (oldLive) {
        outer.removeChild(oldLive);
      }
      if (outer.childNodes.length !== mutation.frozen_total + (newLive ? 1 : 0)) {
        status = "failed";
        detail = "Raw html live reconcile desync";
      }
      break;
    }

    case "add": {
      const parentNode = mutation.parent === "root"
        ? sandbox
        : registry.get(mutation.parent);
      if (parentNode) {
        const newDom = createDomFromNode(mutation.node, mutation.id, registry);
        if (
          motion.animateBlocks
          && newDom instanceof HTMLElement
          && shouldRevealAddedMarkdownNode(mutation.node)
        ) {
          animateStreamBlock(newDom, parentNode);
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
        parentNode.appendChild(newDom);
      } else {
        status = "failed";
        detail = `Parent node '${mutation.parent}' not found`;
      }
      break;
    }

    case "add_list_item": {
      // 列表项级别增量：在已存活的 <ul>/<ol> 下追加一个 <li>，并按 {id}.b{n} 注册其块级子节点。
      // 与 createDomFromNode 的 list 分支中 <li>/子块的 ID 命名规则保持一致。
      const parentNode = registry.get(mutation.parent);
      if (parentNode) {
        const li = document.createElement("li");
        registry.set(mutation.id, li);
        mutation.children.forEach((child, bIdx) => {
          const childDom = createDomFromNode(child, `${mutation.id}.b${bIdx}`, registry);
          li.appendChild(childDom);
        });
        if (motion.animateBlocks) {
          animateStreamBlock(li, parentNode);
        }
        parentNode.appendChild(li);
      } else {
        status = "failed";
        detail = `List parent node '${mutation.parent}' not found`;
      }
      break;
    }

    case "prop": {
      const node = registry.get(mutation.id);
      if (node instanceof HTMLElement) {
        if (mutation.key === "level" && /^H[1-6]$/i.test(node.tagName)) {
          const level = Math.max(1, Math.min(6, Number(mutation.value) || 1));
          const replacement = document.createElement(`h${level}`);
          // 移动现有子节点而不是 innerHTML 重建，保证 registry 中的后代引用仍指向存活 DOM。
          while (node.firstChild) {
            replacement.appendChild(node.firstChild);
          }
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
          const nodeType = mutation.node.type;

          // 1. 策略 A：代码块原地 innerHTML 覆盖与 Style 原地同步
          if (
            nodeType === "code_block" &&
            oldNode instanceof HTMLElement &&
            oldNode.tagName === "PRE" &&
            mutation.node.highlighted_html
          ) {
            cleanupSubtreeRefs(mutation.id, registry, false); // 保留外层 pre 的 ref
            
            // 🆕 利用原生 DOM 树解析，安全提取 innerHTML 与 style，彻底废除正则剥离
            const temp = document.createElement("div");
            temp.innerHTML = mutation.node.highlighted_html.trim();
            const newPre = temp.firstElementChild as HTMLElement;
            if (newPre && newPre.tagName === "PRE") {
              // 原地同步 style 属性（如背景颜色）与类名
              oldNode.className = newPre.className;
              if (newPre.getAttribute("style")) {
                oldNode.setAttribute("style", newPre.getAttribute("style") || "");
              } else {
                oldNode.removeAttribute("style");
              }
              // 原地覆盖内容（剥离了外层 pre 的内部节点）
              oldNode.innerHTML = newPre.innerHTML;
            } else {
              oldNode.innerHTML = mutation.node.highlighted_html;
            }
            if (debugEnabled) astDebugLog(`[AST replace code_block optimized] id=${mutation.id}`);
            break;
          }

          // 2. 策略 B：Mermaid 图表源码原地覆盖
          if (
            nodeType === "code_block" &&
            mutation.node.lang === "mermaid" &&
            oldNode instanceof HTMLElement &&
            oldNode.classList.contains("mermaid-placeholder")
          ) {
            cleanupSubtreeRefs(mutation.id, registry, false); // 保留外壳的 ref
            oldNode.textContent = mutation.node.code || "";
            if (debugEnabled) astDebugLog(`[AST replace mermaid optimized] id=${mutation.id}`);
            break;
          }

          // 3. 策略 C：RawHtml 和 Table 局部 Morphdom 拦截
          if (
            (nodeType === "raw_html" || nodeType === "table") &&
            oldNode instanceof HTMLElement
          ) {
            const tempRegistry = new Map<string, Node>();
            const newDom = createDomFromNode(mutation.node, mutation.id, tempRegistry);

            // raw_html 走冻结前沿增量：新闭合根子节点物理搬入 + 仅 morphdom 活跃子树。
            // 返回 false（首帧无基线 / 结构漂移）时回退全量 morphdom 并重建基线。
            if (
              nodeType === "raw_html" &&
              applyRawHtmlFreezeFrame(oldNode, newDom as HTMLElement)
            ) {
              cleanupSubtreeRefs(mutation.id, registry, true);
              registry.set(mutation.id, oldNode);
              if (debugEnabled) astDebugLog(`[AST replace raw_html freeze-frontier] id=${mutation.id}`);
              break;
            }

            morphdom(oldNode, newDom, {
              childrenOnly: false,
              onBeforeElUpdated: (fromEl, toEl) => {
                if (fromEl.isEqualNode(toEl)) return false;

                // 保留媒体播放与图片加载状态
                if (fromEl.tagName === 'IMG' && (fromEl as HTMLImageElement).complete) return false;
                if (fromEl.tagName === 'VIDEO' || fromEl.tagName === 'AUDIO') {
                  if (!(fromEl as HTMLMediaElement).paused) return false;
                }
                return true;
              }
            });
            if (nodeType === "raw_html") {
              resetRawHtmlFrontier(oldNode);
            }

            // raw_html / table 是无 AST children 的整体节点（后端永远整块 Replace，绝不对其子树做
            // 增量 child diff），故只需保留根节点引用、清空全部子孙引用，杜绝悬空的 temp 子孙引用。
            cleanupSubtreeRefs(mutation.id, registry, true);
            registry.set(mutation.id, oldNode);
            if (debugEnabled) astDebugLog(`[AST replace morphdom optimized] id=${mutation.id}, type=${nodeType}`);
            break;
          }

          // 4. 默认兜底策略：传统的物理 DOM 树替换
          cleanupSubtreeRefs(mutation.id, registry, true);
          const newDom = createDomFromNode(mutation.node, mutation.id, registry);
          parent.replaceChild(newDom, oldNode);
          if (debugEnabled) astDebugLog(`[AST replace success] id=${mutation.id}`);
        } else {
          status = "failed";
          detail = "Old node has no parentNode";
          if (debugEnabled) astDebugLog(`[AST replace fail - oldNode has no parent] id=${mutation.id}`);
        }
      } else {
        status = "failed";
        detail = "Old node not found in registry";
        if (debugEnabled) astDebugLog(`[AST replace fail - oldNode not found in registry] id=${mutation.id}`);
      }
      break;
    }

    case "replace_inline": {
      const oldNode = registry.get(mutation.id);
      if (oldNode) {
        if (oldNode.parentNode) {
          const parent = oldNode.parentNode;
          const nodeType = mutation.node.type;

          // 1. 策略 A：叶子型行内节点原地 textContent / 属性更新 (Code, Text, InlineMath, HighlightTag, AlertTag)
          if (nodeType === "text" && oldNode.nodeType === Node.TEXT_NODE) {
            oldNode.textContent = mutation.node.value || "";
            break;
          }
          if (nodeType === "code" && oldNode.nodeName === "CODE") {
            oldNode.textContent = mutation.node.value || "";
            break;
          }
          if (
            nodeType === "inline_math" &&
            oldNode instanceof HTMLElement &&
            (oldNode.classList.contains("vcp-math-inline") || oldNode.classList.contains("vcp-math-block"))
          ) {
            oldNode.setAttribute("data-latex", mutation.node.content || "");
            oldNode.textContent = mutation.node.content || "";
            break;
          }
          if (
            nodeType === "vcp_custom" &&
            !mutation.node.children &&
            oldNode instanceof HTMLElement
          ) {
            oldNode.textContent = mutation.node.value || "";
            break;
          }

          // 2. 策略 B：图片属性原地更新，不销毁 DOM
          if (nodeType === "image" && oldNode instanceof HTMLImageElement) {
            const rawSrc =
              mutation.node.needs_asset_conversion && mutation.node.src
                ? convertFileSrc(mutation.node.src)
                : (mutation.node.src || "");
            const src = filterTrustedRichHtmlUrl(rawSrc, "img", "src");
            if (src === null) {
              oldNode.removeAttribute("src");
            } else {
              oldNode.setAttribute("src", src);
            }
            oldNode.alt = mutation.node.alt || "";
            oldNode.title = mutation.node.title || "";
            break;
          }

          // 3. 策略 C：容器/复杂行内节点局部 Morphdom 拦截 (Link, QuotedText, Strong, Emphasis, Strikethrough, RawHtmlInline)
          const isContainerNode = [
            "link",
            "vcp_custom",
            "strong",
            "emphasis",
            "strikethrough",
            "raw_html_inline",
          ].includes(nodeType);
          if (isContainerNode && oldNode instanceof HTMLElement) {
            const tempRegistry = new Map<string, Node>();
            const newDom = createInlineDom(mutation.node, mutation.id, tempRegistry);

            // 在 morphdom 变形前先记录每个子孙 id 相对 newDom 的结构路径。morphdom 会保证 oldNode
            // 变形后结构与 newDom 一致，故变形后沿同一路径即可从存活的 oldNode 子树取回真实节点。
            // 必须在 morphdom 之前计算：变形会把 newDom 的子节点移走/丢弃，事后再走 temp 树已不可靠。
            const childPaths: Array<[string, number[]]> = [];
            if (nodeType !== "raw_html_inline") {
              for (const [id, tempNode] of tempRegistry.entries()) {
                if (id === mutation.id) continue;
                const path = computeChildPath(tempNode, newDom);
                if (path) childPaths.push([id, path]);
              }
            }

            morphdom(oldNode, newDom, {
              childrenOnly: false,
            });

            cleanupSubtreeRefs(mutation.id, registry, true);
            registry.set(mutation.id, oldNode); // 根 ID 永远指向页面上存活的 oldNode
            // link / strong / emphasis / strikethrough / vcp_custom 会被后续 .i{N} 子级 mutation
            // 继续增量更新，必须从 morphdom 后存活的真实 DOM 子树重建子孙 registry，
            // 否则后续子级 mutation 会命中被丢弃的 temp 节点而静默失败。
            // raw_html_inline 无 AST children（childPaths 为空），自然只保留根。
            for (const [id, path] of childPaths) {
              const live = resolveChildPath(oldNode, path);
              if (live) registry.set(id, live);
            }
            break;
          }

          // 4. 默认兜底策略：物理 DOM 树替换
          cleanupSubtreeRefs(mutation.id, registry, true);
          const newDom = createInlineDom(mutation.node, mutation.id, registry);
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
          if (debugEnabled) astDebugLog(`[AST remove success] id=${mutation.id}`);
          cleanupSubtreeRefs(mutation.id, registry, true);
        } else {
          status = "failed";
          detail = "Node has no parentNode";
          if (debugEnabled) astDebugLog(`[AST remove fail - node has no parent] id=${mutation.id}`);
        }
      } else {
        status = "failed";
        detail = "Node not found in registry";
        if (debugEnabled) astDebugLog(`[AST remove fail - node not found in registry] id=${mutation.id}`);
      }
      break;
    }
  }

  if (debugEnabled) {
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
  }

  return status === "success" ? { ok: true } : { ok: false, reason: detail };
}

/**
 * 批量执行当前帧的 mutations 并直推更新至沙箱 DOM 元素
 */
export function applyFrame(
  mutations: AstMutation[],
  messageId: string,
  sandbox: HTMLElement,
  options: ApplyFrameOptions = {},
): ApplyFrameResult {
  const debugEnabled = import.meta.env.DEV && isAstDebugEnabled();
  const beforeHtml = debugEnabled ? sandbox.innerHTML : "";
  let result: ApplyFrameResult = { ok: true, applied: 0 };

  // 平滑关闭时保持原热路径；开启后先整帧判定，避免结构 mutation 与临时 fragment 分叉。
  const appendOnly = options.smoothStreaming === true
    && mutations.length > 0
    && mutations.every((mutation) => mutation.op === "append");
  if (!appendOnly) {
    flushStreamTextFragments(messageId);
    clearStreamElementReveals(sandbox);
  }
  const motion: FrameMotion = {
    inlineRevealToken: appendOnly ? {} : undefined,
    animateBlocks: options.smoothStreaming === true && !appendOnly,
  };

  for (const [index, mutation] of mutations.entries()) {
    const mutationResult = executeMutation(mutation, messageId, sandbox, motion);
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

  if (debugEnabled) {
    const registry = getRegistry(messageId);
    const afterHtml = sandbox.innerHTML;
    astDebugLog(`[AST Executor Frame Done] messageId=${messageId}, ok=${result.ok}, html=${afterHtml}`);

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
  }

  return result;
}
