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
import { useChatPreferencesStore } from "./preferences/store";
import { useNotificationStore } from "../../core/stores/notification";
import { useMessageEvents } from "../../core/composables/useMessageEvents";
import { useEmoticonFixer } from "../../core/composables/useEmoticonFixer";
import { MERMAID_CONTENT_SELECTOR, renderMarkdownNodes } from "../../core/utils/astRenderer";
import { shouldRevealAddedMarkdownNode } from "../../core/utils/astExecutor";
import {
  clearStreamElementReveals,
  discardStreamTextFragments,
  prefersReducedStreamMotion,
  STREAM_ELEMENT_REVEAL_CLASS,
  supportsStreamRevealMotion,
} from "../../core/utils/streamTextFade";
import { useMessageStyleInjector } from "../../core/composables/useMessageStyleInjector";
import { Copy, Edit2, RotateCcw, Trash2, StopCircle, Square, Volume2 } from "@lucide/vue";
import { useBubbleNarration } from './voice/useBubbleNarration';
import { useMessageTailRenderer } from './composables/useMessageTailRenderer';
import { separateMessageStyles, stableMessageStyles, standaloneStyleSource } from './utils/messageStyles';
import { instantiateMermaidSvg, renderMermaidTemplate } from './utils/mermaidRenderer';
import { allowChatMutation, chatMutationBlockReason } from "./chatMutationPolicy";
import type { OverlayActionItem } from "../../core/types/overlay";
import { isIOS } from "../../core/utils/platform";

const { processEmoticonsInContainer } = useEmoticonFixer();
let heavyController: AbortController | null = null;

// UI Components
import ChatBubble from "./components/ChatBubble.vue";
import MessageHeader from "./components/MessageHeader.vue";
import ThinkingIndicator from "./components/ThinkingIndicator.vue";
import StreamingTag from "./components/StreamingTag.vue";
import AttachmentPreview from "./attachment/AttachmentPreview.vue";

// Interactive Block Components
import ToolBlock from "./blocks/ToolBlock.vue";
import ThoughtBlock from "./blocks/ThoughtBlock.vue";
import VoiceGuidanceBlock from "./blocks/VoiceGuidanceBlock.vue";
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
const chatPreferences = useChatPreferencesStore();
const bubbleNarration = useBubbleNarration();

let rendererDisposed = false;
const {
  tailSandboxRef,
  tailRootRef,
  thoughtTailSandboxRef,
  isPlainTailFallback,
  thoughtTailBlock,
  voiceGuidanceTailBlock,
  useAstForCurrentTail,
  isAstRecoveryPending,
  finishTailMotion,
} = useMessageTailRenderer({
  getMessage: () => props.message,
  renderTailHtml: () => tailProjection.value.html,
  isInlineHtmlBlock,
  shouldAnimateFrame: shouldAnimateTailFrame,
  requestSnapshot: (messageId, reason) => {
    const key = sessionStore.currentConversationKey;
    return key ? streamStore.requestAuroraSnapshot(
      key.ownerId, key.ownerType, key.topicId, messageId, reason,
    ) : null;
  },
});

function finishStreamMotion(): void {
  pendingStableRevealBlocks = new WeakSet();
  finishTailMotion(messageContentRef.value);
}

function isSmoothStreamMotionEnabled(): boolean {
  return !(
    !chatPreferences.smoothStreamingEnabled
    || !isStreaming.value
    || !useAstForCurrentTail.value
    || props.isBackground === true
    || typeof document !== "undefined" && document.hidden
    || prefersReducedStreamMotion()
  );
}

function shouldAnimateTailFrame(frame: TailFrame): boolean {
  return isSmoothStreamMotionEnabled()
    && frame.reset !== true
    && frame.snapshot === undefined
    && props.message.tailBlock?.type === "markdown";
}

// === Mermaid FullScreen States ===
const isMermaidFullScreen = ref(false);
const activeMermaidSvg = ref("");
const activeMermaidSource = ref("");

// === Shell Properties (Pre-computed in Rust) ===
const shell = computed(() => props.message.shell);

const isNarratingCurrentMessage = computed(() => {
  if (shell.value?.isUser) return false;
  const activeKey = bubbleNarration.activeMessageKey;
  if (!activeKey || activeKey.messageId !== props.message.id) return false;
  const currentConv = sessionStore.currentConversationKey;
  if (!currentConv) return false;
  return activeKey.conversation.ownerId === currentConv.ownerId &&
    activeKey.conversation.ownerType === currentConv.ownerType &&
    activeKey.conversation.topicId === currentConv.topicId;
});

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

const expandToolsByDefault = computed(() =>
  isMessageInActiveStream.value && !chatPreferences.collapseToolsWhileStreaming,
);

// 终态渲染失败时继续展示已收到的尾部；展示引用不延长生成活动或操作占用。
const showMessageTail = computed(() => isStreaming.value || Boolean(
  props.message.finishReason && (props.message.tailBlock || props.message.tailContent),
));

const streamStatusLabel = computed(() => {
  if (props.message.isStopping) return "停止待确认";
  if (props.message.streamRecoveryState === "waiting_retry") return "接续失败，等待重试";
  if (props.message.streamRecoveryState === "reconnecting") return "正在接续";
  return props.message.finishReason === "cancelled_by_user" ? "已停止" : "";
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

// Tail chunks only change the last host; they must not re-project the stable AST.
const stableBubbleProjection = computed(() => {
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
            inheritStableReveal(block, newBlock);
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

  return { bubbles: list, lastBlockIsBrk };
});

const needsTailBubble = computed(() => showMessageTail.value && Boolean(props.message.tailBlock)
  && stableBubbleProjection.value.lastBlockIsBrk);
const messageBubbles = computed(() => {
  const { bubbles } = stableBubbleProjection.value;
  if (needsTailBubble.value) {
    return [...bubbles, { id: `${props.message.id}-bubble-${bubbles.length}`, blocks: [] }];
  }
  return bubbles.length ? bubbles : [{ id: `${props.message.id}-bubble-0`, blocks: [] }];
});

// === Event Delegation ===
const messageContentRef = ref<HTMLElement | null>(null);
useMessageEvents(messageContentRef, openMermaidFullScreen);

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
        if (standaloneStyleSource(block) !== null) return "";
        return `<div class="vcp-markdown-block">${renderMarkdownNodes(block.nodes, props.message.id, block.hash)}</div>`;
      }
      return `<div class="vcp-markdown-block"><p>${escapeHtml(block.content || "")}</p></div>`;
    
    case "role-divider": {
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
    }
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

// Stable blocks are immutable projections. Each message owns its HTML cache; the
// nested bubble loops cannot share Vue's single v-memo slot as their cache owner.
const stableBlockHtml = new WeakMap<ContentBlock, { messageId: string; html: string }>();
function renderStableBlockHtml(block: ContentBlock): string {
  const cached = stableBlockHtml.get(block);
  if (cached?.messageId === props.message.id) return cached.html;
  const html = renderBlockHtml(block);
  stableBlockHtml.set(block, { messageId: props.message.id, html });
  return html;
}

type ToolContentBlock = Extract<ContentBlock, { type: "tool-use" | "jev-tool-use" | "tool-result" }>;

function sameToolContent(previous: ToolContentBlock, current: ToolContentBlock): boolean {
  if (previous === current) return true;
  if (previous.type !== current.type) return false;
  if (previous.tool_name !== current.tool_name) return false;
  if (previous.type !== "tool-result" && current.type !== "tool-result") {
    return previous.content === current.content;
  }
  if (previous.type === "tool-result" && current.type === "tool-result") {
    return previous.status === current.status
      && previous.footer === current.footer
      && previous.details.length === current.details.length
      && previous.details.every((detail, index) =>
        detail.key === current.details[index].key && detail.value === current.details[index].value,
      );
  }
  return false;
}

// 流式与终态 hash 不同，但两条解析链按相同顺序产生工具块。
// 只对齐工具次序，排除 markdown 合并造成的位置变化；语义变化则建立新交互身份。
// 真正插入/删除工具改变次序时，后续无法同位匹配的工具按新结构恢复默认。
// 上一版只保留块引用和短 key，不复制可能包含大段正文/base64 的工具结果。
let previousToolBlocks: ToolContentBlock[] = [];
let previousToolKeys: string[] = [];
let previousToolMessageId = "";
let nextToolIdentity = 0;
const toolBlockKeys = computed(() => {
  const currentBlocks: ToolContentBlock[] = [];
  const currentKeys: string[] = [];
  const keys = new Map<ContentBlock, string>();
  for (const block of props.message.blocks ?? []) {
    if (block.type !== "tool-use" && block.type !== "jev-tool-use" && block.type !== "tool-result") continue;
    const index = currentBlocks.length;
    const previous = previousToolBlocks[index];
    const key = previousToolMessageId === props.message.id && previous && sameToolContent(previous, block)
      ? previousToolKeys[index]
      : `${props.message.id}-tool-${nextToolIdentity++}`;
    currentBlocks.push(block);
    currentKeys.push(key);
    keys.set(block, key);
  }
  previousToolBlocks = currentBlocks;
  previousToolKeys = currentKeys;
  previousToolMessageId = props.message.id;
  return keys;
});

// 尾部与稳定区属于不同组件位置，展开意图随当前消息保留，闭合时按指导次序交接。
const guidanceExpansion = ref<Record<string, boolean>>({});
const guidanceKey = (ordinal: number) => `${props.message.id}-voice-guidance-${ordinal}`;
const tailGuidanceKey = computed(() => guidanceKey(
  props.message.blocks?.filter(block => block.type === "voice-guidance").length ?? 0,
));
watch(() => props.message.id, () => { guidanceExpansion.value = {}; });

function getBlockKey(block: ContentBlock, index: number, bubbleId: string): string {
  if (block.type === "voice-guidance") {
    const ordinal = props.message.blocks?.filter(candidate => candidate.type === "voice-guidance").indexOf(block) ?? index;
    return guidanceKey(ordinal);
  }
  const toolKey = toolBlockKeys.value.get(block);
  if (toolKey !== undefined) return toolKey;
  if (block.hash !== undefined && block.hash !== null) {
    return `${bubbleId}-${block.type}-${String(block.hash)}-${index}`;
  }
  // Fallback for legacy data (index-based)
  return `${bubbleId}-${block.type}-idx-${index}`;
}

let pendingStableRevealBlocks = new WeakSet<ContentBlock>();

function shouldRevealStableBlock(block: ContentBlock): boolean {
  if (!pendingStableRevealBlocks.has(block) || block.type !== "markdown") return false;
  if (isBrkBlock(block)) return false;
  if (!block.nodes || block.nodes.length === 0) return !!block.content?.trim();
  return block.nodes.every(shouldRevealAddedMarkdownNode);
}

function inheritStableReveal(source: ContentBlock, clone: ContentBlock): void {
  if (pendingStableRevealBlocks.has(source)) pendingStableRevealBlocks.add(clone);
}

function finishStableBlockReveal(event: Event, block: ContentBlock): void {
  if (event.target !== event.currentTarget) return;
  pendingStableRevealBlocks.delete(block);
  (event.currentTarget as HTMLElement).classList.remove(STREAM_ELEMENT_REVEAL_CLASS);
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

function openMermaidFullScreen(svgHtml: string, sourceCode: string) {
  activeMermaidSvg.value = svgHtml;
  activeMermaidSource.value = sourceCode;
  isMermaidFullScreen.value = true;
}

// 只整理外观；图表的点击由消息根代理始终负责。
function decorateMermaid(el: HTMLElement) {
  const svg = el.querySelector('svg');
  if (!svg) return;
  if (svg.parentElement?.classList.contains('vcp-mermaid-wrapper')) return;
  
  // 给 SVG 设置基础样式，使其自适应显示
  // 保留 Pele 根节点内联的 --_* 主题变量声明，仅设置自适应尺寸样式
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
  
  el.textContent = '';
  wrapper.appendChild(fullscreenBtn);
  wrapper.appendChild(svg);
  el.appendChild(wrapper);
}

// === Heavy Content Rendering (KaTeX inline math + Mermaid) ===
const renderHeavyContent = async (refreshMermaid = false) => {
  heavyController?.abort();
  const controller = new AbortController();
  heavyController = controller;


  const messageId = props.message.id;
  const host = messageContentRef.value;
  const isCurrent = () => !rendererDisposed && !controller.signal.aborted && props.message.id === messageId && messageContentRef.value === host;
  if (!host || !isCurrent()) return;

  // 稳定块包含正文、Thought、Diary、Tool；活动 AST/morphdom 尾部不在这些区域内。
  const stableRegions = Array.from(host.querySelectorAll<HTMLElement>('[data-message-render-region="stable"]'));
  const queryStable = (selector: string) => stableRegions.flatMap(region => Array.from(region.querySelectorAll<HTMLElement>(selector)));
  if (refreshMermaid) {
    for (const diagram of queryStable(MERMAID_CONTENT_SELECTOR)) {
      const source = diagram.dataset.mermaidSource;
      if (source === undefined) continue;
      diagram.textContent = source;
      diagram.classList.remove('mermaid');
      diagram.classList.add('mermaid-placeholder');
    }
  }

  // 1. KaTeX math (inline + display mode, rendered inside markdown blocks via v-html)
  const mathElements = queryStable('.vcp-math-inline[data-latex], .vcp-math-block[data-latex]');

  if (mathElements.length > 0) {
    try {
      const katexModule = await import('katex');
      if (!isCurrent()) return;
      const katex = katexModule.default;
      mathElements.forEach((el) => {
        if (!host.contains(el)) return;
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
  const mermaidPlaceholders = queryStable(MERMAID_CONTENT_SELECTOR);

  if (mermaidPlaceholders.length > 0) {
    try {
      for (const placeholder of mermaidPlaceholders) {
        if (!isCurrent()) return;
        if (!host.contains(placeholder)) continue;
        placeholder.dataset.contentKind = 'mermaid';

        // 历史 raw HTML 可直接提供 SVG；不能把 SVG 标签文字当作 Mermaid 源码。
        if (placeholder.querySelector('svg')) {
          decorateMermaid(placeholder);
          continue;
        }
        
        const sourceCode = placeholder.dataset.mermaidSource ?? placeholder.textContent ?? '';
        placeholder.dataset.mermaidSource = sourceCode;
        const canCommit = () => isCurrent() && host.contains(placeholder) && placeholder.dataset.mermaidSource === sourceCode;
        try {
          const renderedSvg = await renderMermaidTemplate(sourceCode, controller.signal);
          if (!canCommit()) continue;
          placeholder.innerHTML = instantiateMermaidSvg(renderedSvg);
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          decorateMermaid(placeholder);
        } catch (e: any) {
          if (!canCommit()) continue;
          const errorMsg = e?.str || e?.message || String(e);
          console.error('[MessageRenderer] Mermaid render failed:', errorMsg, e);
          placeholder.innerHTML = `<div class="text-red-500 text-[10px] p-4 rounded-xl border border-red-500/10 bg-red-500/5">图表渲染失败: ${escapeHtml(errorMsg)}</div>`;
        }
      }
    } catch (e) {
      console.error('[MessageRenderer] Mermaid load failed:', e);
    }
  }

  // 3. 保留尾部图片的 URL 修复与表情标识；等待 AST 的 post 更新完成。
  await nextTick();
  if (isCurrent()) {
    void processEmoticonsInContainer(host);
  }
};

// Watch for content changes and trigger heavy rendering
// Note: blocks array reference changes when Rust parser returns new AST,
// so shallow watch is sufficient. Avoid deep watch to prevent O(n) traversal
// on every streaming chunk across all rendered messages.
watch(
  [
    () => props.message.blocks,
    () => props.message.tailBlock,
    () => props.message.tailFrame,
  ],
  ([blocks, _tailBlock, frame], [previousBlocks, previousTailBlock, previousFrame]) => {
    if (blocks === previousBlocks) return;
    pendingStableRevealBlocks = new WeakSet();

    const previous = previousBlocks ?? [];
    if (
      !blocks
      || blocks.length <= previous.length
      || !chatPreferences.smoothStreamingEnabled
      || !isStreaming.value
      || props.isBackground === true
      || props.message.isReconnecting === true
      || typeof document !== "undefined" && document.hidden
      || prefersReducedStreamMotion()
      || !supportsStreamRevealMotion()
      || !frame?.reset
      || !previousFrame
      || frame.streamId !== previousFrame.streamId
      || frame.epoch <= previousFrame.epoch
      || !previous.every((block, index) => blocks[index] === block)
    ) {
      return;
    }

    const appended = blocks.slice(previous.length);
    const hadVisibleTail = previousTailBlock !== undefined && previousTailBlock !== null;
    if (hadVisibleTail) {
      // stable 内容将在同一 Vue patch 接管；旧 fragment 必须丢弃，不能回写旧 Text node。
      discardStreamTextFragments(props.message.id);
      if (messageContentRef.value) clearStreamElementReveals(messageContentRef.value);

      // 一个 tail 段可能被 button 标记切为 markdown/button/markdown；无 Wire 来源映射时整批静态更安全。
      if (appended.some((block) => block.type === "button-click")) return;
    }

    for (const block of appended.slice(hadVisibleTail ? 1 : 0)) {
      if (block.type === "markdown") pendingStableRevealBlocks.add(block);
    }
  },
  { flush: "pre" },
);

// DOM patch 后统一处理内容、主题和终态变化；同一轮更新不重复取消/启动渲染。
watch(
  [messageContentRef, () => props.message.blocks, isMessageInActiveStream],
  () => {
    void renderHeavyContent();
  },
  { flush: 'post' },
);

// 消息离开活跃流后收束文字动画；重内容由上面的 post watcher 统一调度。
watch(
  isMessageInActiveStream,
  (inStream, wasInStream) => {
    if (wasInStream && !inStream) {
      finishStreamMotion();
    }
  }
);

watch(
  () => chatPreferences.smoothStreamingEnabled,
  (enabled) => {
    if (!enabled) finishStreamMotion();
  },
);

watch(
  () => props.message.isReconnecting,
  (isReconnecting) => {
    if (isReconnecting) finishStreamMotion();
  },
);

watch(
  () => props.isBackground,
  (isBackground) => {
    if (isBackground) finishStreamMotion();
  },
);

// === Context Menu ===
const showMessageContextMenu = async () => {
  const messageKey = historyStore.captureMessageActionKey(props.message.id);
  if (!messageKey) return;
  const actions: OverlayActionItem[] = [];
  const mutationBlocked = () => chatMutationBlockReason(messageKey.conversation) !== null;

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

  const getCachedFullText = () => props.message.id === messageKey.messageId
    ? props.message.content
    : undefined;
  const getFullText = async () => {
    if (!historyStore.isMessageActionCurrent(messageKey)) return "";
    const cachedText = getCachedFullText();
    if (cachedText) return cachedText;
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
      let contentError: unknown;
      const requireCurrentText = (text?: string) => {
        if (!historyStore.isMessageActionCurrent(messageKey)) {
          throw new Error("会话已切换，请重新选择要复制的消息。");
        }
        if (!text) throw new Error("未能读取消息正文，或此消息没有可复制的文字。");
        return text;
      };
      try {
        if (!historyStore.isMessageActionCurrent(messageKey)) {
          throw new Error("会话已切换，请重新选择要复制的消息。");
        }
        const clipboard = navigator.clipboard;
        if (!clipboard?.writeText) throw new Error("当前环境无法访问剪贴板。");
        const cachedText = getCachedFullText();
        if (cachedText) {
          // Keep already available text synchronous with the click on both platforms.
          await clipboard.writeText(requireCurrentText(cachedText));
        } else if (isIOS) {
          if (typeof ClipboardItem === "undefined" || !clipboard.write) {
            throw new Error("当前系统不支持异步全文复制，请先打开编辑消息加载正文后重试。");
          }
          const content = getFullText()
            .then(text => new Blob([requireCurrentText(text)], { type: "text/plain" }))
            .catch(error => { contentError = error; throw error; });
          // Observe a late read rejection even if ClipboardItem/write rejects before
          // consuming it. The original promise still rejects the clipboard write.
          void content.catch(() => undefined);
          // WebKit must receive write() in the click, before the original-text IPC settles.
          await clipboard.write([new ClipboardItem({ "text/plain": content })]);
        } else {
          // Older Android WebViews need not support promise-backed ClipboardItem.
          await clipboard.writeText(requireCurrentText(await getFullText()));
        }
        notificationStore.addNotification({
          type: "success",
          title: "复制成功",
          message: "内容已复制到剪贴板",
        });
      } catch (error) {
        const cause = contentError ?? error;
        notificationStore.addNotification({
          type: "error",
          title: "复制失败",
          message: cause instanceof Error ? cause.message : String(cause),
          toastOnly: true,
        });
      }
    },
  });

  // 3. 其他非流式操作
  if (!isStreaming.value) {
    if (!shell.value?.isUser) {
      const capturedTtsId = bubbleNarration.isNarrating(messageKey) ? bubbleNarration.activeTtsId : null;
      actions.push({
        label: capturedTtsId ? '停止朗读' : '朗读气泡',
        icon: capturedTtsId ? Square : Volume2,
        danger: Boolean(capturedTtsId),
        handler: async () => {
          if (capturedTtsId) {
            await bubbleNarration.stopMessage(messageKey, capturedTtsId);
            return;
          }
          const key = messageKey.conversation;
          await bubbleNarration.startBubbleNarration(messageKey, getFullText, () =>
            historyStore.isMessageActionCurrent(messageKey)
            && !streamStore.isMessageActiveInSession(key.ownerId, key.ownerType, key.topicId, messageKey.messageId),
          );
        },
      });
    }
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
        danger: true,
        get disabled() { return mutationBlocked(); },
        handler: async () => {
          if (allowChatMutation(messageKey.conversation)) {
            const confirmed = await overlayStore.showConfirm({
              title: "重新生成",
              message: "确定要重新生成这条消息吗？",
              isDanger: true,
            });
            if (confirmed) {
              await historyStore.regenerateResponse(messageKey);
            }
          }
        },
      });
    } else {
      actions.push({
        label: "编辑重发",
        icon: Edit2,
        danger: true,
        get disabled() { return mutationBlocked(); },
        handler: async () => {
          if (allowChatMutation(messageKey.conversation)) {
            const confirmed = await overlayStore.showConfirm({
              title: "编辑重发",
              message: "确定要编辑重发这条消息吗？",
              isDanger: true,
            });
            if (confirmed) {
              historyStore.beginEditResend(messageKey);
            }
          }
        },
      });
    }
  }

  actions.push({
    label: "删除消息",
    icon: Trash2,
    danger: true,
    get disabled() { return mutationBlocked(); },
    handler: async () => {
      if (!allowChatMutation(messageKey.conversation)) return;
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

// Stable CSS only depends on stable content; tail updates never rescan its AST.
const stableCss = computed(() => stableBubbleProjection.value.bubbles
  .map(bubble => stableMessageStyles(bubble.blocks)).filter(Boolean).join('\n'));
const emptyTailProjection = { html: '', css: '' };
const tailProjection = computed(() => {
  const block = props.message.tailBlock;
  if (!showMessageTail.value || !block || isAstRecoveryPending.value || useAstForCurrentTail.value || isPlainTailFallback.value || !isInlineHtmlBlock(block.type)) {
    return emptyTailProjection;
  }
  const styleSource = standaloneStyleSource(block);
  if (styleSource !== null) return { html: '', css: separateMessageStyles(styleSource).css };
  return separateMessageStyles(renderBlockHtml(block));
});
const { injectScopedCss } = useMessageStyleInjector();
watch(
  [() => props.message.id, stableCss, () => tailProjection.value.css],
  ([messageId, stable, tail]) => injectScopedCss([stable, tail].filter(Boolean).join('\n'), messageId),
  { immediate: true, flush: 'post' },
);

onUnmounted(() => {
  rendererDisposed = true;
  heavyController?.abort();
});
</script>

<template>
  <div ref="messageContentRef" v-longpress.message="showMessageContextMenu"
    class="vcp-message-item flex flex-col w-full mb-6 px-1 min-w-0" :data-message-id="message.id"
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
          :is-narrating="isNarratingCurrentMessage"
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
          <ThinkingIndicator v-if="isStreaming && !streamStatusLabel && (bubbleIndex === messageBubbles.length - 1) && (!message.blocks || message.blocks.length === 0) && !message.tailBlock" />

          <div class="vcp-content-blocks space-y-2 min-w-0 w-full overflow-hidden">
            <template v-if="bubble.blocks && bubble.blocks.length > 0">
                <!-- memo 与循环位于同一节点，按列表项缓存；保留所有实际展示输入。 -->
                <!-- eslint-disable vue/valid-v-memo -- The rule tracks the outer bubble loop; Vue compiles memo on this inner v-for into renderList's per-item cache. -->
                <div
                  v-for="(block, index) in bubble.blocks"
                  :key="getBlockKey(block, index, bubble.id)"
                  v-memo="[getBlockKey(block, index, bubble.id), block, isStreaming, isMessageInActiveStream, expandToolsByDefault, shouldRevealStableBlock(block), guidanceExpansion[getBlockKey(block, index, bubble.id)]]"
                  data-message-render-region="stable"
                  :class="{ [STREAM_ELEMENT_REVEAL_CLASS]: shouldRevealStableBlock(block) }"
                  @animationend.self="finishStableBlockReveal($event, block)"
                  @animationcancel.self="finishStableBlockReveal($event, block)"
                >
                  <!-- eslint-enable vue/valid-v-memo -->
                  <DiaryBlock
                    v-if="block.type === 'diary' || block.type === 'diary-update'"
                    :block="block"
                    :message-id="message.id"
                  />

                  <!-- eslint-disable vue/no-v-html -- AST output uses filterTrustedRichHtml; generated divider/button markup uses parser roles and escaped text. -->
                  <div
                    v-else-if="isInlineHtmlBlock(block.type)"
                    data-message-content
                    v-html="renderStableBlockHtml(block)"
                  />
                  <!-- eslint-enable vue/no-v-html -->

                  <ToolBlock
                    v-else-if="block.type === 'tool-use' || block.type === 'jev-tool-use' || block.type === 'tool-result'"
                    :type="block.type"
                    :content="block.content"
                    :block="block"
                    :default-expanded="expandToolsByDefault"
                    @content-mounted="renderHeavyContent()"
                  />

                  <ThoughtBlock
                    v-else-if="block.type === 'thought'"
                    :block="block"
                    :message-id="message.id"
                    :default-expanded="isMessageInActiveStream"
                  />

                  <VoiceGuidanceBlock
                    v-else-if="block.type === 'voice-guidance'"
                    :block="block"
                    v-model:expanded="guidanceExpansion[getBlockKey(block, index, bubble.id)]"
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
            <template v-else-if="bubbleIndex === 0 && message.content && !showMessageTail">
              <div class="vcp-markdown-block select-text">
                <p>{{ message.content }}</p>
              </div>
            </template>

            <VoiceGuidanceBlock
              v-if="showMessageTail && (bubbleIndex === messageBubbles.length - 1) && voiceGuidanceTailBlock"
              :key="tailGuidanceKey"
              data-message-render-region="tail"
              :block="voiceGuidanceTailBlock"
              :is-streaming="isStreaming"
              v-model:expanded="guidanceExpansion[tailGuidanceKey]"
            />

            <!-- 思维链尾部复用同一 AST 沙箱；终态解析失败时保留已有展示。 -->
            <ThoughtBlock
              v-else-if="showMessageTail && (bubbleIndex === messageBubbles.length - 1) && thoughtTailBlock"
              data-message-render-region="tail"
              :block="thoughtTailBlock"
              :message-id="message.id"
              :default-expanded="true"
            >
              <div
                v-if="isPlainTailFallback || isAstRecoveryPending || !useAstForCurrentTail"
                :ref="(el) => { thoughtTailSandboxRef = el as HTMLElement | null }"
                data-message-content
                :data-tail-render-mode="isAstRecoveryPending ? 'recovery-text' : 'plaintext'"
                class="thought-body vcp-markdown-block whitespace-pre-wrap break-words"
              />
              <div
                v-else
                :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                data-message-content
                class="thought-body vcp-markdown-block vcp-ast-sandbox"
              />
            </ThoughtBlock>

            <!-- 其他尾部展示只归属最后一个气泡，完整 blocks 接管后移除。 -->
            <div v-else-if="showMessageTail && (bubbleIndex === messageBubbles.length - 1) && message.tailBlock" data-message-render-region="tail" class="streaming-tail opacity-90">
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
                  <!-- 此沙箱只承载流式代码 pre，刻意不挂 vcp-markdown-block：
                       其 15px 正文字号会截断代码区 10px 容器的 font-size 继承链，
                       导致流式渲染字体明显大于静态完成态。 -->
                  <div
                    :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                    class="vcp-ast-sandbox"
                  />
                </template>
              </HtmlPreviewBlock>
              <div v-else-if="useAstForCurrentTail && isInlineHtmlBlock(message.tailBlock.type)">
                <div
                  :ref="(el) => { tailSandboxRef = el as HTMLElement | null }"
                  data-message-content
                  class="vcp-markdown-block vcp-ast-sandbox"
                />
              </div>
              <div
                v-else-if="!useAstForCurrentTail && isInlineHtmlBlock(message.tailBlock.type)"
                :ref="(el) => { tailRootRef = el as HTMLElement | null }"
                data-message-content
                class="vcp-markdown-block"
              />
            </div>
            <div v-if="showMessageTail && (bubbleIndex === messageBubbles.length - 1) && message.tailContent && message.blocks && message.blocks.length > 0 && (!message.tailBlock || (!isInlineHtmlBlock(message.tailBlock.type) && message.tailBlock.type !== 'html-preview' && message.tailBlock.type !== 'thought' && message.tailBlock.type !== 'voice-guidance'))" class="opacity-70 italic">
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

          <template v-if="bubbleIndex === messageBubbles.length - 1">
            <div v-if="streamStatusLabel" role="status" class="pt-2 text-xs opacity-60">
              {{ streamStatusLabel }}
            </div>
            <StreamingTag v-else-if="isStreaming" />
          </template>

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
</style>
