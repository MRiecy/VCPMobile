<script setup lang="ts">
import { computed, ref, watch, nextTick, onUnmounted } from "vue";
import type {
  ChatMessage,
  ContentBlock,
  InlineNode,
  MarkdownNode,
  TailFrame,
} from "../../core/types/chat";
import { useOverlayStore } from "../../core/stores/overlay";
import { useChatHistoryStore } from "../../core/stores/chatHistoryStore";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useThemeStore } from "../../core/stores/theme";
import { useNotificationStore } from "../../core/stores/notification";
import { useMessageEvents } from "../../core/composables/useMessageEvents";
import { useEmoticonFixer } from "../../core/composables/useEmoticonFixer";
import { renderMarkdownNodes } from "../../core/utils/astRenderer";
import { applyFrame, cleanupRegistry, rebuildSnapshot } from "../../core/utils/astExecutor";
import { useMessageStyleInjector } from "../../core/composables/useMessageStyleInjector";
import { Copy, Edit2, RotateCcw, Trash2, StopCircle } from "lucide-vue-next";
import morphdom from "morphdom";
import {
  canSmoothStreamAppend,
  createStreamRevealController,
} from "./streamRevealScheduler";

const { processEmoticonsInContainer } = useEmoticonFixer();
const mermaidCache = new Map<string, string>();
const MAX_MERMAID_CACHE_SIZE = 30;

function setMermaidCache(key: string, value: string) {
  if (mermaidCache.has(key)) {
    mermaidCache.delete(key);
  } else if (mermaidCache.size >= MAX_MERMAID_CACHE_SIZE) {
    const firstKey = mermaidCache.keys().next().value;
    if (firstKey !== undefined) {
      mermaidCache.delete(firstKey);
    }
  }
  mermaidCache.set(key, value);
}

const renderingMermaids = new Map<string, Promise<string>>();
let mermaidInitialized = false;

// UI Components
import ChatBubble from "./components/ChatBubble.vue";
import MessageHeader from "./components/MessageHeader.vue";
import ThinkingIndicator from "./components/ThinkingIndicator.vue";
import StreamingTag from "./components/StreamingTag.vue";
import AttachmentPreview from "./attachment/AttachmentPreview.vue";

// Interactive Block Components
import ToolBlock from "./blocks/ToolBlock.vue";
import ThoughtBlock from "./blocks/ThoughtBlock.vue";
import HtmlPreviewBlock from "./blocks/HtmlPreviewBlock.vue";
import ToolSummaryBlock from "./blocks/ToolSummaryBlock.vue";
import DiaryBlock from "./blocks/DiaryBlock.vue";
import MermaidFullScreenViewer from "./blocks/MermaidFullScreenViewer.vue";

const props = defineProps<{
  message: ChatMessage;
  agentId?: string;
  depth?: number;
  isBackground?: boolean;
}>();

const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const historyStore = useChatHistoryStore();
const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();
const themeStore = useThemeStore();

function isAstDebugEnabled(): boolean {
  return Boolean(import.meta.env.DEV && (window as any).__VCP_AST_DEBUG__);
}

function astDebugLog(...args: unknown[]): void {
  if (import.meta.env.DEV && isAstDebugEnabled()) {
    console.warn(...args);
  }
}

// === AST Diff Feature Flags & Refs ===
const tailSandboxRef = ref<HTMLElement | null>(null);
const enableAstDiff = ref(true); // Feature Flag, 默认开启
const isAstRecoveryPending = ref(false);
let astRecoveryPromise: Promise<boolean> | null = null;
const isPlainTailFallback = computed(() => {
  const block = props.message.tailBlock;
  return !!block
    && (
      block.render_mode === "plain"
      || (
        block.render_mode === undefined
        && (!block.nodes || block.nodes.length === 0)
      )
    );
});
const thoughtTailBlock = computed(() => (
  props.message.tailBlock?.type === "thought" ? props.message.tailBlock : null
));
const useAstForCurrentTail = computed(() => {
  if (!enableAstDiff.value || isAstRecoveryPending.value) return false;
  if (isPlainTailFallback.value) return false;
  return (
    !!props.message.tailFrame ||
    !!props.message.tailBlock?.nodes ||
    !!props.message.tailSnapshot
  );
});
let lastAppliedFrameSeq = 0;
let lastAcceptedFrameSeq = 0;
let localTailStreamId = -1;
let localTailEpoch = -1;
let localTailRevision = -1;
let astFailureCount = 0;
let lastSandbox: HTMLElement | null = null;
let rendererDisposed = false;

function getTailSnapshotNodes() {
  const frameSnapshot = props.message.tailFrame?.reset
    ? props.message.tailFrame.snapshot
    : undefined;
  return frameSnapshot || props.message.tailBlock?.nodes || [];
}

function rebuildTailSnapshot(sandbox: HTMLElement): void {
  rebuildSnapshot(getTailSnapshotNodes(), props.message.id, sandbox);
  localTailStreamId = props.message.tailFrame?.streamId ?? localTailStreamId;
  localTailEpoch = props.message.tailFrame?.epoch ?? localTailEpoch;
  localTailRevision = props.message.tailFrame?.revision ?? localTailRevision;
}

function requestCurrentTailSnapshot(reason: string): Promise<boolean> {
  if (astRecoveryPromise) return astRecoveryPromise;
  const key = sessionStore.currentConversationKey;
  if (!key) return Promise.resolve(false);

  isAstRecoveryPending.value = true;
  const recovery = streamStore.requestAuroraSnapshot(
    key.ownerId,
    key.ownerType,
    key.topicId,
    props.message.id,
    reason,
  );
  const completion = recovery.finally(() => {
    if (rendererDisposed || astRecoveryPromise !== completion) return;
    isAstRecoveryPending.value = false;
    astRecoveryPromise = null;
  });
  astRecoveryPromise = completion;
  return astRecoveryPromise;
}

function recoverTailSnapshotOrDowngrade(reason: string): void {
  if (rendererDisposed || !enableAstDiff.value) return;
  astFailureCount += 1;
  if (import.meta.env.DEV && isAstDebugEnabled()) {
    astDebugLog(`[AST Diff Recovery] ${props.message.id}: ${reason}. failureCount=${astFailureCount}`);
  }
  void requestCurrentTailSnapshot(reason).then((recovered) => {
    if (rendererDisposed) return;
    if (recovered) {
      astFailureCount = 0;
    } else if (astFailureCount >= 2) {
      enableAstDiff.value = false;
      tailRevealController.cancel();
      cleanupRegistry(props.message.id);
    }
  });
}

function handleAstFrameFailure(_sandbox: HTMLElement | null, reason: string): void {
  recoverTailSnapshotOrDowngrade(reason);
}

function markTailFrameApplied(frame: TailFrame): void {
  lastAppliedFrameSeq = frame.frameSeq;
  lastAcceptedFrameSeq = Math.max(lastAcceptedFrameSeq, frame.frameSeq);
  localTailStreamId = frame.streamId;
  localTailEpoch = frame.epoch;
  localTailRevision = frame.revision;
  astFailureCount = 0;
}

const tailRevealController = createStreamRevealController<TailFrame>({
  apply(targetId, text) {
    const sandbox = tailSandboxRef.value;
    if (!sandbox || sandbox !== lastSandbox || !useAstForCurrentTail.value) {
      return false;
    }
    return applyFrame(
      [{ op: "append", id: targetId, chunk: text }],
      props.message.id,
      sandbox,
    ).ok;
  },
  complete(frame) {
    if (
      frame.streamId === localTailStreamId
      && frame.epoch === localTailEpoch
    ) {
      markTailFrameApplied(frame);
    }
  },
  fail(_frame, reason) {
    handleAstFrameFailure(tailSandboxRef.value, `smooth reveal ${reason}`);
  },
});

function shouldSmoothTailFrame(frame: TailFrame): boolean {
  if (
    !themeStore.smoothStreamingEnabled
    || !isStreaming.value
    || !useAstForCurrentTail.value
    || props.isBackground === true
    || typeof document !== "undefined" && document.hidden
    || typeof window !== "undefined"
      && window.matchMedia("(prefers-reduced-motion: reduce)").matches
    || frame.reset === true
    || frame.snapshot !== undefined
  ) {
    return false;
  }

  const mutations = frame.mutations || [];
  return mutations.length === 1
    && mutations[0].op === "append"
    && canSmoothStreamAppend(mutations[0].chunk)
    && (
      props.message.tailBlock?.type === "markdown"
      || props.message.tailBlock?.type === "thought"
    );
}

// === Mermaid FullScreen States ===
const isMermaidFullScreen = ref(false);
const activeMermaidSvg = ref("");
const activeMermaidSource = ref("");

// === Shell Properties (Pre-computed in Rust) ===
const shell = computed(() => props.message.shell);

// === Streaming State ===

// 数据层面：消息是否处于当前完整会话的活跃流中。
const isMessageInActiveStream = computed(() => {
  const key = sessionStore.currentConversationKey;
  return key
    ? streamStore.isMessageActive(
        key.ownerId,
        key.ownerType,
        key.topicId,
        props.message.id,
      )
    : false;
});

// UI 层面：消息是否在当前视口中显示流式状态
const isStreaming = computed(() => {
  if (shell.value?.isUser) return false;

  const key = sessionStore.currentConversationKey;
  if (!key) return false;

  return streamStore.isMessageActiveInSession(
    key.ownerId,
    key.ownerType,
    key.topicId,
    props.message.id,
  );
});

function isBrkNode(node: MarkdownNode): boolean {
  if (node.type === "raw_html" && node.content) {
    const trimmed = node.content.trim().replace(/\s+/g, "");
    return trimmed === "<!--brk-->";
  }
  return false;
}

function isBrkBlock(block: ContentBlock): boolean {
  if (!isInlineHtmlBlock(block.type)) return false;
  
  if (block.content) {
    const trimmed = block.content.trim().replace(/\s+/g, "");
    if (trimmed === "<!--brk-->") return true;
  }
  
  if (block.nodes && block.nodes.length > 0) {
    const groups = splitMarkdownNodes(block.nodes);
    return groups.length === 0;
  }
  
  return false;
}

function isWhitespaceNode(
  node: MarkdownNode | InlineNode | { type: "softbreak" | "hardbreak" },
): boolean {
  if (!node) return true;
  if (node.type === "text") {
    return !node.value || node.value.trim() === "";
  }
  if (node.type === "paragraph") {
    return !node.children || node.children.length === 0 || node.children.every(isWhitespaceNode);
  }
  if (node.type === "softbreak" || node.type === "hardbreak" || node.type === "break") {
    return true;
  }
  return false;
}

function trimWhitespaceNodes(nodes: MarkdownNode[]): MarkdownNode[] {
  let start = 0;
  while (start < nodes.length && isWhitespaceNode(nodes[start])) {
    start++;
  }
  let end = nodes.length;
  while (end > start && isWhitespaceNode(nodes[end - 1])) {
    end--;
  }
  return nodes.slice(start, end);
}

function splitMarkdownNodes(nodes: MarkdownNode[]): MarkdownNode[][] {
  const result: MarkdownNode[][] = [];
  let currentGroup: MarkdownNode[] = [];
  let htmlDepth = 0;
  
  for (const node of nodes) {
    if (node.type === "raw_html" && node.content) {
      const content = node.content.trim().toLowerCase();
      if (content.startsWith("<div") && !content.endsWith("/>") && !content.includes("</div>")) {
        htmlDepth++;
      }
      if (content.startsWith("</div")) {
        htmlDepth = Math.max(0, htmlDepth - 1);
      }
    }

    if (isBrkNode(node) && htmlDepth === 0) {
      const trimmed = trimWhitespaceNodes(currentGroup);
      if (trimmed.length > 0) {
        result.push(trimmed);
      }
      currentGroup = [];
    } else {
      currentGroup.push(node);
    }
  }
  
  const trimmed = trimWhitespaceNodes(currentGroup);
  if (trimmed.length > 0) {
    result.push(trimmed);
  }
  return result;
}

interface BubbleGroup {
  id: string;
  blocks: ContentBlock[];
  isTail?: boolean;
}

const messageBubbles = computed(() => {
  const list: BubbleGroup[] = [];
  let currentBlocks: ContentBlock[] = [];
  let bubbleIndex = 0;

  const pushCurrentGroup = () => {
    if (currentBlocks.length > 0) {
      list.push({
        id: `${props.message.id}-bubble-${bubbleIndex++}`,
        blocks: [...currentBlocks]
      });
      currentBlocks = [];
    }
  };

  const isUserMsg = shell.value?.isUser;

  if (props.message.blocks && props.message.blocks.length > 0) {
    for (const block of props.message.blocks) {
      if (!isInlineHtmlBlock(block.type) || isUserMsg) {
        currentBlocks.push(block);
        continue;
      }

      // 🆕 优先判定这个块是否整体就是一个 brk 物理分割块 (支持纯文本及 AST 状态双重鉴定)
      if (isBrkBlock(block)) {
        pushCurrentGroup();
        continue; // 过滤掉 <!--brk--> 本身不渲染
      }

      if (block.nodes && block.nodes.length > 0) {
        // 🆕 如果这个 block 的第一个有效节点是 brk 节点，说明它和前一个 bubble 之间有 brk 割裂，
        // 应该在处理这个 block 之前，先把当前气泡推出去！
        let startsWithBrk = false;
        for (const node of block.nodes) {
          if (isBrkNode(node)) {
            startsWithBrk = true;
            break;
          }
          if (isWhitespaceNode(node)) {
            continue;
          }
          break;
        }

        if (startsWithBrk) {
          pushCurrentGroup();
        }

        const nodeGroups = splitMarkdownNodes(block.nodes);
        if (nodeGroups.length > 1) {
          nodeGroups.forEach((groupNodes, idx) => {
            const newBlock: ContentBlock = {
              ...block,
              nodes: groupNodes,
              hash: block.hash !== undefined ? `${block.hash}-split-${idx}` : undefined
            };
            currentBlocks.push(newBlock);
            if (idx < nodeGroups.length - 1) {
              pushCurrentGroup();
            }
          });
        } else if (nodeGroups.length === 0) {
          // 🆕 兜底：如果内部 AST 切分结果为 0 也是纯分割块
          pushCurrentGroup();
        } else {
          currentBlocks.push(block);
        }
      } else {
        currentBlocks.push(block);
      }
    }
  }

  pushCurrentGroup();

  // 🆕 流式状态下，如果最后一个稳定块是个 brk 块，我们需要额外追加一个空的气泡组以供 tailBlock 打字渲染
  const lastBlockIsBrk = props.message.blocks && props.message.blocks.length > 0 && (() => {
    const last = props.message.blocks[props.message.blocks.length - 1];
    return last ? isBrkBlock(last) : false;
  })();

  if (isStreaming.value && props.message.tailBlock && lastBlockIsBrk) {
    list.push({
      id: `${props.message.id}-bubble-${bubbleIndex++}`,
      blocks: []
    });
  }

  // 兜底：如果整个消息 blocks 为空
  if (list.length === 0) {
    list.push({
      id: `${props.message.id}-bubble-0`,
      blocks: []
    });
  }

  return list;
});

// === Event Delegation ===
const messageContentRef = ref<HTMLElement | null>(null);
useMessageEvents(messageContentRef);

// === Block Rendering Helper ===
function isInlineHtmlBlock(type: string): boolean {
  return [
    "markdown",
    "role-divider",
    "button-click",
  ].includes(type);
}

function renderBlockHtml(block: ContentBlock): string {
  switch (block.type) {
    case "markdown":
      if (block.nodes && block.nodes.length > 0) {
        if (
          block.nodes.length === 1 &&
          block.nodes[0].type === "raw_html" &&
          block.nodes[0].content?.trimStart().toLowerCase().startsWith("<style")
        ) {
          const content = block.nodes[0].content;
          let cssContent = "";
          content.replace(/<style\b[^>]*>([\s\S]*?)(?:<\/style>|$)/gi, (_, css) => {
            cssContent += css.trim() + "\n";
            return "";
          });
          if (cssContent.trim().length > 0) {
            injectScopedCss(cssContent, props.message.id);
          }
          return ""; // Keep unclosed style invisible in chat body
        }
        return `<div class="vcp-markdown-block">${renderMarkdownNodes(block.nodes, props.message.id, block.hash)}</div>`;
      }
      return `<div class="vcp-markdown-block"><p>${escapeHtml(block.content || "")}</p></div>`;
    
    case "role-divider":
      const role = block.role || "unknown";
      const roleDisplay = role.charAt(0).toUpperCase() + role.slice(1);
      const actionText = block.is_end ? "[结束]" : "[起始]";
      const roleClass = `role-${role.toLowerCase()}`;
      const typeClass = block.is_end ? "type-end" : "type-start";
      
      return `
        <div class="vcp-role-divider ${roleClass} ${typeClass}">
          <span class="divider-text">角色分界: ${roleDisplay} ${actionText}</span>
        </div>
      `;

    case "button-click": {
      const escapedContent = escapeHtml(block.content || "");
      const finalText = `[[点击按钮:${block.content || ""}]]`;
      return `
        <div class="inline-block px-3 py-1 bg-black/10 dark:bg-white/10 rounded-full text-[10px] font-bold opacity-70 my-1 cursor-pointer active:opacity-40 transition-opacity select-none border border-black/5 dark:border-white/5 active:scale-95 duration-75 transform"
             data-vcp-button="${escapeHtml(finalText)}">
          ${escapedContent}
        </div>
      `;
    }

    case "style":
      return "";

    default:
      return "";
  }
}

function getBlockKey(block: ContentBlock, index: number, bubbleId: string): string {
  if (block.hash !== undefined && block.hash !== null) {
    return `${bubbleId}-${block.type}-${String(block.hash)}-${index}`;
  }
  // Fallback for legacy data (index-based)
  return `${bubbleId}-${block.type}-idx-${index}`;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

const openMermaidFullScreen = (svgHtml: string, sourceCode: string) => {
  activeMermaidSvg.value = svgHtml;
  activeMermaidSource.value = sourceCode;
  isMermaidFullScreen.value = true;
};

function enhanceMermaid(el: HTMLElement, sourceCode: string) {
  if (!el || el.dataset.vcpMermaidEnhanced === 'true') return;
  
  const svg = el.querySelector('svg');
  if (!svg) return;
  
  el.dataset.vcpMermaidEnhanced = 'true';
  
  // 给 SVG 设置基础样式，使其自适应显示
  svg.removeAttribute('style');
  svg.style.maxWidth = '100%';
  svg.style.height = 'auto';
  svg.style.display = 'block';
  svg.style.margin = '0 auto';
  
  // 创建包裹层
  const wrapper = document.createElement('div');
  wrapper.className = 'vcp-mermaid-wrapper group relative my-3 overflow-hidden rounded-xl border border-black/5 dark:border-white/10 bg-black/5 dark:bg-white/5 p-4 transition-all duration-300 active:scale-[0.99] cursor-pointer';
  
  // 创建全屏按钮
  const fullscreenBtn = document.createElement('button');
  fullscreenBtn.type = 'button';
  fullscreenBtn.className = 'absolute top-3 right-3 z-10 flex items-center justify-center w-8 h-8 rounded-lg border border-black/5 dark:border-white/10 bg-white/80 dark:bg-black/80 text-gray-500 dark:text-gray-400 opacity-0 group-hover:opacity-100 active:scale-90 transition-all duration-200 cursor-pointer shadow-sm';
  fullscreenBtn.innerHTML = '<div class="i-ph:arrows-out-bold w-4 h-4"></div>';
  fullscreenBtn.title = '全屏查看图表';
  
  wrapper.addEventListener('click', (e) => {
    e.stopPropagation();
    openMermaidFullScreen(svg.outerHTML, sourceCode);
  });
  
  wrapper.addEventListener('dblclick', (e) => {
    e.stopPropagation();
  });
  
  el.textContent = '';
  wrapper.appendChild(fullscreenBtn);
  wrapper.appendChild(svg);
  el.appendChild(wrapper);
}

// === Heavy Content Rendering (KaTeX inline math + Mermaid) ===
const renderHeavyContent = async () => {
  await nextTick();
  if (!messageContentRef.value) return;

  // 1. KaTeX math (inline + display mode, rendered inside markdown blocks via v-html)
  const mathElements = Array.from(
    messageContentRef.value.querySelectorAll('.vcp-math-inline[data-latex], .vcp-math-block[data-latex]')
  ).filter(el => !el.closest('.streaming-tail'));

  if (mathElements.length > 0) {
    try {
      const katexModule = await import('katex');
      const katex = katexModule.default;
      mathElements.forEach((el) => {
        if (el.querySelector('.katex')) return; // already rendered
        const latex = el.getAttribute('data-latex');
        if (!latex) return;
        const isDisplay = el.classList.contains('vcp-math-block');
        katex.render(latex, el as HTMLElement, {
          throwOnError: false,
          strict: false,
          displayMode: isDisplay,
        });
      });
    } catch (e) {
      console.error('[MessageRenderer] KaTeX render failed:', e);
    }
  }

  // 2. Mermaid diagrams
  const mermaidPlaceholders = Array.from(
    messageContentRef.value.querySelectorAll('.mermaid-placeholder, .mermaid, code.language-mermaid')
  ).filter(el => !el.closest('.streaming-tail'));

  if (mermaidPlaceholders.length > 0) {
    try {
      const mermaidModule = await import('mermaid');
      const mermaid = mermaidModule.default;
      if (!mermaidInitialized) {
        mermaid.initialize({ startOnLoad: false, theme: 'dark' });
        mermaidInitialized = true;
      }
      for (const el of Array.from(mermaidPlaceholders)) {
        const placeholder = el as HTMLElement;
        const wrapper = placeholder.closest('.vcp-mermaid-wrapper');
        if (wrapper && wrapper.querySelector('svg')) continue; // already rendered & enhanced

        // 流式 AST 与异步 Mermaid 渲染可能先完成 SVG 注入、后触发本轮增强。
        // 已有 SVG 只代表“可见”，不代表点击/全屏交互已经绑定。
        if (placeholder.querySelector('svg')) {
          enhanceMermaid(
            placeholder,
            placeholder.dataset.mermaidSource || placeholder.textContent || '',
          );
          continue;
        }
        
        const sourceCode = placeholder.dataset.mermaidSource || placeholder.textContent || '';
        const codeKey = sourceCode;
        // Skip if Vue has replaced this element out of the DOM
        if (!messageContentRef.value.contains(placeholder)) continue;
        
        // Use cache to avoid re-rendering the same diagram
        if (mermaidCache.has(codeKey)) {
          const cachedSvg = mermaidCache.get(codeKey)!;
          placeholder.innerHTML = cachedSvg;
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          enhanceMermaid(placeholder, placeholder.dataset.mermaidSource || '');
          continue;
        }
        
        placeholder.dataset.mermaidSource = sourceCode;
        try {
          let renderPromise = renderingMermaids.get(codeKey);
          if (!renderPromise) {
            const renderId = `vcp-mermaid-${Date.now()}-${Math.random().toString(36).slice(2)}`;
            renderPromise = mermaid.render(renderId, sourceCode).then(result => result.svg);
            renderingMermaids.set(codeKey, renderPromise);
            const release = () => {
              if (renderingMermaids.get(codeKey) === renderPromise) renderingMermaids.delete(codeKey);
            };
            void renderPromise.then(release, release);
          }
          const renderedSvg = await renderPromise;
          setMermaidCache(codeKey, renderedSvg);
          if (!messageContentRef.value.contains(placeholder)) continue;
          placeholder.innerHTML = renderedSvg;
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          enhanceMermaid(placeholder, sourceCode);
        } catch (e: any) {
          const errorMsg = e?.str || e?.message || String(e);
          console.error('[MessageRenderer] Mermaid render failed:', errorMsg, e);
          placeholder.innerHTML = `<div class="text-red-500 text-[10px] p-4 rounded-xl border border-red-500/10 bg-red-500/5">图表渲染失败: ${escapeHtml(errorMsg)}</div>`;
        }
      }
    } catch (e) {
      console.error('[MessageRenderer] Mermaid load failed:', e);
    }
  }

  // 3. Emoticons
  if (messageContentRef.value) {
    processEmoticonsInContainer(messageContentRef.value);
  }
};

// Watch for content changes and trigger heavy rendering
// Note: blocks array reference changes when Rust parser returns new AST,
// so shallow watch is sufficient. Avoid deep watch to prevent O(n) traversal
// on every streaming chunk across all rendered messages.
watch(
  () => props.message.blocks,
  () => {
    renderHeavyContent();
  },
  { immediate: true }
);

// 消息真正离开活跃流后统一执行一次重渲染，确保 KaTeX/Mermaid/Emoticon 正确渲染
watch(
  isMessageInActiveStream,
  (inStream, wasInStream) => {
    if (wasInStream && !inStream) {
      tailRevealController.cancel();
      renderHeavyContent();
    }
  }
);

watch(
  () => themeStore.smoothStreamingEnabled,
  (enabled) => {
    if (!enabled) tailRevealController.flush();
  },
);

watch(
  () => props.message.isReconnecting,
  (isReconnecting) => {
    if (isReconnecting) tailRevealController.flush();
  },
);

watch(
  () => props.isBackground,
  (isBackground) => {
    if (isBackground) tailRevealController.flush();
  },
);

// === Context Menu ===
const showMessageContextMenu = async () => {
  const messageKey = historyStore.captureMessageActionKey(props.message.id);
  if (!messageKey) return;
  const actions: any[] = [];
  const generationActive = streamStore.activeStreamingIds.size > 0;

  if (isStreaming.value && !shell.value?.isUser) {
    actions.push({
      label: "中止回复",
      icon: StopCircle,
      danger: true,
      handler: () => {
        if (historyStore.isMessageActionCurrent(messageKey)) {
          const key = messageKey.conversation;
          streamStore.stopMessage(
            key.ownerId,
            key.ownerType,
            key.topicId,
            props.message.id,
          );
        }
      },
    });
  }

  const getFullText = async () => {
    if (props.message.content) return props.message.content;
    return await historyStore.fetchRawContent(messageKey);
  };

  // 1. 如果不是流式，编辑消息移动到首位
  if (!isStreaming.value) {
    actions.push({
      label: "编辑消息",
      icon: Edit2,
      handler: async () => {
        const fullText = await getFullText();
        if (!historyStore.isMessageActionCurrent(messageKey)) return;
        overlayStore.openEditor({
          initialValue: fullText || "",
          onSave: (newContent: string) => historyStore.updateMessageContent(messageKey, newContent),
        });
      },
    });
  }

  // 2. 复制内容紧随其后
  actions.push({
    label: "复制内容",
    icon: Copy,
    handler: async () => {
      const fullText = await getFullText();
      if (!fullText) return;
      await navigator.clipboard.writeText(fullText);
      notificationStore.addNotification({
        type: "success",
        title: "复制成功",
        message: "内容已复制到剪贴板",
      });
    },
  });

  // 3. 其他非流式操作
  if (!isStreaming.value) {
    actions.push({
      label: "重新渲染",
      icon: RotateCcw,
      handler: async () => {
        try {
          await historyStore.reRenderMessage(messageKey);
          notificationStore.addNotification({
            type: "success",
            title: "重构完成",
            message: "消息内容已完成物理就地重绘与排版刷新",
            toastOnly: true,
          });
        } catch (e) {
          notificationStore.addNotification({
            type: "error",
            title: "重构失败",
            message: String(e),
            toastOnly: true,
          });
        }
      },
    });

    if (!shell.value?.isUser) {
      actions.push({
        label: "重新生成",
        icon: RotateCcw,
        disabled: generationActive,
        handler: () => {
          if (streamStore.activeStreamingIds.size === 0) {
            historyStore.regenerateResponse(messageKey);
          }
        },
      });
    } else {
      actions.push({
        label: "编辑重发",
        icon: Edit2,
        disabled: generationActive,
        handler: () => {
          if (streamStore.activeStreamingIds.size === 0) {
            historyStore.beginEditResend(messageKey);
          }
        },
      });
    }
  }

  actions.push({
    label: "删除消息",
    icon: Trash2,
    danger: true,
    handler: async () => {
      const confirmed = await overlayStore.showConfirm({
        title: "删除消息",
        message: "确定要删除这条消息吗？",
        isDanger: true
      });
      if (confirmed) {
        await historyStore.deleteMessage(messageKey);
      }
    },
  });

  overlayStore.openContextMenu(actions, shell.value?.isUser ? "User" : "Assistant");
};

function formatTime(ts: number) {
  const date = new Date(ts);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  return `${year}-${month}-${day} ${hours}:${minutes}`;
}

// === Style Block CSS Injection ===
const { injectScopedCss, removeScopedCss } = useMessageStyleInjector();

watch(
  () => props.message.blocks,
  (blocks) => {
    if (!blocks) return;
    for (const block of blocks) {
      if (block.type === "style" && block.content) {
        injectScopedCss(block.content, props.message.id);
      }
    }
  },
  { immediate: true }
);

// === Stream Tail Morphdom Smooth Rendering ===
const tailRootRef = ref<HTMLElement | null>(null);
const fallbackTailSignature = computed(() => {
  const block = props.message.tailBlock;
  if (!block) return "";

  const contentSignal = block.hash !== undefined && block.hash !== null
    ? String(block.hash)
    : block.content || "";

  return [
    block.type,
    contentSignal,
    block.nodes?.length ?? 0,
  ].join("|");
});

watch(
  [fallbackTailSignature, useAstForCurrentTail, tailRootRef],
  () => {
    const newTailBlock = props.message.tailBlock;
    if (useAstForCurrentTail.value) return; // 🆕 启用 AST Diff 且有节点时跳过 Morphdom
    if (isPlainTailFallback.value) return; // 超限 tail 直接由 Vue 文本节点渲染，不做 HTML parse/morphdom
    if (!newTailBlock || !isInlineHtmlBlock(newTailBlock.type)) return;
    nextTick(() => {
      if (!tailRootRef.value) return;
      const html = renderBlockHtml(newTailBlock);

      // 实时提取未闭合/已闭合的 <style> 并物理抹除以防 morphdom 崩溃
      let cssContent = "";
      const processedHtml = html.replace(
        /<style\b[^>]*>([\s\S]*?)(?:<\/style>|$)/gi,
        (_, css) => {
          cssContent += css.trim() + "\n";
          return ""; // 从正文 HTML 中抹除 style 标签
        }
      );

      if (cssContent.trim().length > 0) {
        injectScopedCss(cssContent, props.message.id);
      }

      try {
        morphdom(tailRootRef.value, `<div>${processedHtml}</div>`, {
          childrenOnly: true,
          getNodeKey: (node) => {
            if (!node || node.nodeType !== 1) return undefined;
            const el = node as Element;
            return el.id || el.getAttribute('data-vcp-key') || undefined;
          },
          onBeforeElUpdated: (fromEl, toEl) => {
            if (fromEl.isEqualNode(toEl)) return false;

            // 1. 保留可能存在的过渡/动画 class，防止 morphdom 移除它们
            const animClasses = ['vcp-stream-element-fade-in', 'animate-fade-in', 'vcp-stream-content-pulse'];
            for (const cls of animClasses) {
              if (fromEl.classList.contains(cls)) {
                toEl.classList.add(cls);
              }
            }

            // 2. 保留媒体播放状态
            if (fromEl.tagName === 'VIDEO' || fromEl.tagName === 'AUDIO') {
              const mediaEl = fromEl as HTMLMediaElement;
              if (!mediaEl.paused) return false;
            }

            // 3. 保留输入焦点
            if (fromEl === document.activeElement) {
              requestAnimationFrame(() => {
                if (toEl && typeof toEl.focus === 'function') toEl.focus();
              });
            }

            // 4. 保留已加载图片的可见性和状态，防止重新加载闪烁
            if (fromEl.tagName === 'IMG') {
              const fromImg = fromEl as HTMLImageElement;
              const toImg = toEl as HTMLImageElement;
              if (fromImg.onerror && !toImg.onerror) toImg.onerror = fromImg.onerror;
              if (fromImg.onload && !toImg.onload) toImg.onload = fromImg.onload;
              if (fromImg.style.visibility) toImg.style.visibility = fromImg.style.visibility;
              if (fromImg.complete && fromImg.naturalWidth > 0) return false;
            }

            return true;
          }
        });
      } catch (e) {
        console.debug('[TailMorphdom] Skipped frame:', e);
      }
    });
  },
  { immediate: true, flush: 'post' }
);


// === AST Diff Executor ===

watch(
  [
    () => props.message.tailFrame,
    () => props.message.tailSnapshot,
    tailSandboxRef,
  ],
  ([frame, _snapshot, sandbox]) => {
    const debugEnabled = import.meta.env.DEV && isAstDebugEnabled();
    if (debugEnabled) {
      astDebugLog(`[AST Diff Watch] Msg ${props.message.id} stream=${frame?.streamId ?? 'none'}, frame=${frame ? frame.frameSeq : 'none'}, mutations=${frame?.mutations?.length || 0}, sandbox=${sandbox ? 'Ready' : 'Null'}, epoch=${frame?.epoch}, revision=${frame?.revision}`);
    }

    if (!useAstForCurrentTail.value || !sandbox) {
      tailRevealController.cancel();
      if (lastSandbox) {
        cleanupRegistry(props.message.id);
        lastSandbox.innerHTML = '';
        lastSandbox = null;
      }
      return;
    }

    if (lastSandbox !== sandbox) {
      tailRevealController.cancel();
      cleanupRegistry(props.message.id);
      sandbox.innerHTML = '';
      lastAppliedFrameSeq = 0;
      lastAcceptedFrameSeq = 0;
      localTailStreamId = -1;
      localTailEpoch = -1;
      localTailRevision = -1;
      lastSandbox = sandbox;
      if (frame?.reset !== true && props.message.tailBlock?.nodes) {
        rebuildTailSnapshot(sandbox); // 内部已将 localTailEpoch/Revision 同步到当前 frame
        astFailureCount = 0;
        // 认领当前帧，避免下方 reset 分支对同一帧重复重建（新 sandbox 时 localTailEpoch 刚被重置，
        // epochChanged 必为真，会触发第二次全量重建）。重建已用当前完整 tail AST，无需再来一次。
        if (frame) {
          lastAppliedFrameSeq = frame.frameSeq;
          lastAcceptedFrameSeq = frame.frameSeq;
        }
      } else if (
        frame
        && frame.frameSeq > 1
        && !(frame.reset === true && frame.snapshot !== undefined)
      ) {
        recoverTailSnapshotOrDowngrade("sandbox_remount");
        return;
      }
    }

    if (!frame) {
      return;
    }

    const incomingStreamId = frame.streamId ?? 0;
    const incomingEpoch = frame.epoch ?? 0;
    const incomingRevision = frame.revision ?? -1;
    const streamChanged = incomingStreamId !== localTailStreamId;
    const epochChanged = incomingEpoch !== localTailEpoch;
    const explicitReset = frame.reset === true || streamChanged || epochChanged;

    // frameSeq 只在同一 stream/epoch 内有可比性；暖接续的新流必须先接管身份，
    // 不能被上一条流遗留的高序号提前拦截。
    if (!explicitReset && frame.frameSeq <= lastAcceptedFrameSeq) {
      return;
    }

    if (explicitReset) {
      tailRevealController.cancel();
      const snapshot = frame.snapshot ?? props.message.tailBlock?.nodes;
      const canStartFromEmpty = frame.reset !== true
        && frame.frameSeq === 1
        && lastAppliedFrameSeq === 0;
      if (snapshot === undefined && !canStartFromEmpty) {
        recoverTailSnapshotOrDowngrade("reset_without_snapshot");
        return;
      }
      sandbox.innerHTML = '';
      cleanupRegistry(props.message.id);
      localTailStreamId = incomingStreamId;
      localTailEpoch = incomingEpoch;
      localTailRevision = incomingRevision;
      lastAppliedFrameSeq = 0;
      lastAcceptedFrameSeq = 0;
      if (snapshot !== undefined) {
        rebuildSnapshot(snapshot, props.message.id, sandbox);
      }
    }

    const mutations = frame.mutations || [];
    if (mutations.length === 0) {
      if (!tailRevealController.flush()) return;
      markTailFrameApplied(frame);
      return;
    }

    if (!explicitReset && shouldSmoothTailFrame(frame)) {
      const mutation = mutations[0];
      if (mutation.op !== "append") return;
      lastAcceptedFrameSeq = frame.frameSeq;
      tailRevealController.enqueue({
        targetId: mutation.id,
        text: mutation.chunk,
        metadata: frame,
      });
      return;
    }

    // 任何结构变化都是展示屏障：先合并追平已经接收的安全文本，再原子执行本帧。
    if (!tailRevealController.flush()) return;

    if (debugEnabled) {
      astDebugLog(`[AST Diff Apply] Executing frame ${frame.frameSeq} (${mutations.length} mutations) for ${props.message.id}`);
    }
    const result = applyFrame(mutations, props.message.id, sandbox);
    if (result.ok) {
      markTailFrameApplied(frame);
    } else {
      handleAstFrameFailure(sandbox, result.failed?.reason || "applyFrame failed");
    }
  },
  { flush: "post", immediate: true }
);

onUnmounted(() => {
  rendererDisposed = true;
  tailRevealController.dispose();
  removeScopedCss(props.message.id);
  cleanupRegistry(props.message.id);
});
</script>

<template>
  <div ref="messageContentRef" v-longpress="showMessageContextMenu"
    class="vcp-message-item flex flex-col w-full mb-6 animate-fade-in px-1 min-w-0" :data-message-id="message.id"
    :data-role="message.role">
    
    <!-- 统一的气泡循环渲染列表 -->
    <template v-for="(bubble, bubbleIndex) in messageBubbles" :key="bubble.id">
      <template v-if="shell">
        <MessageHeader
          :class="bubbleIndex > 0 ? 'vcp-message-header-repeated' : ''"
          :data-bubble-index="bubbleIndex"
          :is-user="shell.isUser"
          :display-name="shell.displayName"
          :name-style="{ color: shell.avatarColor }"
          :owner-type="shell.isUser ? 'user' : 'agent'"
          :owner-id="shell.isUser ? 'user_avatar' : (message.agentId || agentId)"
          :avatar-dominant-color="shell.avatarColor"
        />

        <ChatBubble 
          :is-user="shell.isUser" 
          :is-streaming="isStreaming && (bubbleIndex === messageBubbles.length - 1)" 
          :bubble-style="{
            '--dynamic-color': shell.avatarColor,
          }"
          :data-bubble-index="bubbleIndex"
          :data-bubble-last="bubbleIndex === messageBubbles.length - 1 ? 'true' : 'false'"
          :class="bubbleIndex > 0 ? 'mt-2' : ''"
        >
          <!-- 初始思考指示灯：仅在此活跃气泡没有任何已确认 blocks，且仍在流式并未吐出 tail 时显示 -->
          <ThinkingIndicator v-if="isStreaming && (bubbleIndex === messageBubbles.length - 1) && (!message.blocks || message.blocks.length === 0) && !message.tailBlock" />

          <div class="vcp-content-blocks space-y-2 min-w-0 w-full overflow-hidden">
            <template v-if="bubble.blocks && bubble.blocks.length > 0">
              <template v-for="(block, index) in bubble.blocks" :key="getBlockKey(block, index, bubble.id)">
                <!-- v-memo 使用含 bubble ID 的稳定 key，避免分条气泡之间复用错误子树 -->
                <div v-memo="[getBlockKey(block, index, bubble.id), isStreaming]">
                  <DiaryBlock
                    v-if="block.type === 'diary' || block.type === 'diary-update'"
                    :block="block"
                    :message-id="message.id"
                  />

                  <div
                    v-else-if="isInlineHtmlBlock(block.type)"
                    v-html="renderBlockHtml(block)"
                  />

                  <ToolBlock
                    v-else-if="block.type === 'tool-use' || block.type === 'tool-result'"
                    :type="block.type"
                    :content="block.content"
                    :block="block"
                    :default-expanded="isMessageInActiveStream"
                  />

                  <ThoughtBlock
                    v-else-if="block.type === 'thought'"
                    :block="block"
                    :message-id="message.id"
                    :default-expanded="isMessageInActiveStream"
                  />

                  <HtmlPreviewBlock
                    v-else-if="block.type === 'html-preview'"
                    :content="block.content || ''"
                    :highlighted-content="block.highlighted_content"
                    :message-id="message.id"
                    :is-streaming="isStreaming"
                  />

                  <ToolSummaryBlock
                    v-else-if="block.type === 'tool-call-summary'"
                    :block="block"
                  />
                </div>
              </template>
            </template>
            <template v-else-if="bubbleIndex === 0 && message.content && (!isStreaming || !message.tailBlock)">
              <div class="vcp-markdown-block select-text">
                <p>{{ message.content }}</p>
              </div>
            </template>

            <!-- 未闭合思维链：仅在活跃 tail 中使用可撤销 Thought 外壳，正文仍复用同一 AST 增量沙箱。 -->
            <ThoughtBlock
              v-if="isStreaming && (bubbleIndex === messageBubbles.length - 1) && thoughtTailBlock"
              :block="thoughtTailBlock"
              :message-id="message.id"
              :default-expanded="true"
              animate-entry
            >
              <div
                v-if="isPlainTailFallback || isAstRecoveryPending || !useAstForCurrentTail"
                :data-tail-render-mode="isAstRecoveryPending ? 'recovery-text' : 'plaintext'"
                class="thought-body vcp-markdown-block whitespace-pre-wrap break-words"
              >{{ thoughtTailBlock.content }}</div>
              <div
                v-else
                :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                class="thought-body vcp-markdown-block vcp-ast-sandbox"
              />
            </ThoughtBlock>

            <!-- 其他尾部流式推测渲染（只对最后一个活跃气泡生效） -->
            <div v-else-if="isStreaming && (bubbleIndex === messageBubbles.length - 1) && message.tailBlock" class="streaming-tail opacity-90">
              <div
                v-if="isPlainTailFallback || isAstRecoveryPending"
                :data-tail-render-mode="isAstRecoveryPending ? 'recovery-text' : 'plaintext'"
                class="vcp-markdown-block whitespace-pre-wrap break-words"
              >{{ message.tailBlock.content || '' }}</div>
              <HtmlPreviewBlock
                v-else-if="message.tailBlock.type === 'html-preview'"
                :content="useAstForCurrentTail ? '' : (message.tailBlock.content || '')"
                :message-id="message.id"
                :is-streaming="isStreaming"
              >
                <template v-if="useAstForCurrentTail" #code>
                  <div
                    :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                    class="vcp-markdown-block vcp-ast-sandbox"
                  />
                </template>
              </HtmlPreviewBlock>
              <div v-else-if="useAstForCurrentTail && isInlineHtmlBlock(message.tailBlock.type)">
                <div
                  :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                  class="vcp-markdown-block vcp-ast-sandbox"
                />
              </div>
              <div
                v-else-if="!useAstForCurrentTail && isInlineHtmlBlock(message.tailBlock.type)"
                :ref="(el) => { tailRootRef = el as HTMLElement | null }"
                class="vcp-markdown-block"
              />
            </div>
            <div v-if="isStreaming && (bubbleIndex === messageBubbles.length - 1) && message.tailContent && message.blocks && message.blocks.length > 0 && (!message.tailBlock || (!isInlineHtmlBlock(message.tailBlock.type) && message.tailBlock.type !== 'html-preview' && message.tailBlock.type !== 'thought'))" class="opacity-70 italic">
              {{ message.tailContent }}
            </div>
          </div>

          <AttachmentPreview 
            v-if="bubbleIndex === 0 && message.attachments && message.attachments.length > 0" 
            :attachments="message.attachments"
            :message-id="message.id"
            :topic-id="message.topicId || sessionStore.currentTopicId || undefined"
            class="pt-3 border-t border-black/5 dark:border-white/5" 
          />

          <StreamingTag v-if="isStreaming && (bubbleIndex === messageBubbles.length - 1)" />

          <template #footer>
            <div class="vcp-message-time text-[9px] mt-1.5 px-1 opacity-50 font-mono tracking-tighter w-full"
              :data-bubble-last="bubbleIndex === messageBubbles.length - 1 ? 'true' : 'false'"
              :class="shell.isUser ? 'text-right' : 'text-left'">
              {{ formatTime(message.timestamp) }}
            </div>
          </template>
        </ChatBubble>
      </template>
    </template>

    <!-- Mermaid FullScreen Viewer -->
    <MermaidFullScreenViewer
      :visible="isMermaidFullScreen"
      :svg-html="activeMermaidSvg"
      :source-code="activeMermaidSource"
      @close="isMermaidFullScreen = false"
    />
  </div>
</template>

<style scoped>
.vcp-message-item {
  /* Native Virtual Scrolling: defers rendering and layout of off-screen messages */
  content-visibility: auto;
  contain-intrinsic-size: auto 100px;
}

.animate-fade-in {
  animation: fadeIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(10px) scale(0.98); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}
</style>
