<script setup lang="ts">
import { computed, ref, watch, nextTick, onUnmounted } from "vue";
import type { ChatMessage, ContentBlock } from "../../core/types/chat";
import { useOverlayStore } from "../../core/stores/overlay";
import { useChatHistoryStore } from "../../core/stores/chatHistoryStore";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useNotificationStore } from "../../core/stores/notification";
import { useMessageEvents } from "../../core/composables/useMessageEvents";
import { useEmoticonFixer } from "../../core/composables/useEmoticonFixer";
import { renderMarkdownNodes } from "../../core/utils/astRenderer";
import { applyFrame, cleanupRegistry, rebuildSnapshot } from "../../core/utils/astExecutor";
import { useMessageStyleInjector } from "../../core/composables/useMessageStyleInjector";
import { Copy, Edit2, RotateCcw, Trash2, StopCircle } from "lucide-vue-next";
import morphdom from "morphdom";

const { processEmoticonsInContainer } = useEmoticonFixer();
const mermaidCache = new Map<string, string>();
const renderingMermaids = new Set<string>();
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
import MermaidFullScreenViewer from "./blocks/MermaidFullScreenViewer.vue";

const props = defineProps<{
  message: ChatMessage;
  agentId?: string;
  depth?: number;
}>();

const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const historyStore = useChatHistoryStore();
const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();

function isAstDebugEnabled(): boolean {
  return Boolean(import.meta.env.DEV && (window as any).__VCP_AST_DEBUG__);
}

function astDebugLog(...args: unknown[]): void {
  if (isAstDebugEnabled()) {
    console.warn(...args);
  }
}

// === AST Diff Feature Flags & Refs ===
const tailSandboxRef = ref<HTMLElement | null>(null);
const enableAstDiff = ref(true); // Feature Flag, 默认开启
const useAstForCurrentTail = computed(() => {
  return enableAstDiff.value && !!props.message.tailBlock?.nodes;
});
let appliedMutationsCount = 0;
let localTailEpoch = -1;
let localTailRevision = -1;
let astFailureCount = 0;
let lastSandbox: HTMLElement | null = null;

function getTailSnapshotNodes() {
  return props.message.tailSnapshot || props.message.tailBlock?.nodes || [];
}

function rebuildTailSnapshot(sandbox: HTMLElement): void {
  rebuildSnapshot(getTailSnapshotNodes(), props.message.id, sandbox);
  appliedMutationsCount = props.message.tailMutations?.length || 0;
  localTailEpoch = props.message.tailEpoch ?? localTailEpoch;
  localTailRevision = props.message.tailRevision ?? localTailRevision;
}

function handleAstFrameFailure(sandbox: HTMLElement, reason: string): void {
  astFailureCount += 1;
  astDebugLog(`[AST Diff Recovery] ${props.message.id}: ${reason}. failureCount=${astFailureCount}`);
  if (getTailSnapshotNodes().length > 0) {
    rebuildTailSnapshot(sandbox);
    return;
  }
  if (astFailureCount >= 2) {
    enableAstDiff.value = false;
    cleanupRegistry(props.message.id);
  }
}

// === Mermaid FullScreen States ===
const isMermaidFullScreen = ref(false);
const activeMermaidSvg = ref("");
const activeMermaidSource = ref("");

// === Shell Properties (Pre-computed in Rust) ===
const shell = computed(() => props.message.shell);

// === Streaming State ===

// 数据层面：消息是否处于任意活跃流中（不依赖当前话题）
const isMessageInActiveStream = computed(() => {
  for (const streams of Object.values(streamStore.sessionActiveStreams || {})) {
    if (streams.includes(props.message.id)) return true;
  }
  return false;
});

// UI 层面：消息是否在当前视口中显示流式状态
const isStreaming = computed(() => {
  if (shell.value?.isUser) return false;

  const isGroup = !!props.message.isGroupMessage || !!props.message.groupId || sessionStore.currentSelectedItem?.type === "group";
  const itemId = isGroup
    ? (props.message.groupId || sessionStore.currentSelectedItem?.id)
    : (props.message.agentId || props.agentId);

  const topicId = sessionStore.currentTopicId;
  if (!itemId || !topicId) return false;

  const key = `${itemId}:${topicId}`;
  const streams = streamStore.sessionActiveStreams?.[key];
  return streams ? streams.includes(props.message.id) : false;
});

// === Event Delegation ===
const messageContentRef = ref<HTMLElement | null>(null);
useMessageEvents(messageContentRef);

// === Block Rendering Helper ===
function isPlainBlock(type: string): boolean {
  return [
    "markdown",
    "diary",
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
    
    case "diary": {
      const diaryContent = (block.nodes && block.nodes.length > 0)
        ? renderMarkdownNodes(block.nodes, props.message.id, block.hash)
        : escapeHtml(block.content || "");
      return `
        <div class="vcp-diary-block">
          <div class="vcp-diary-header">
            <span class="vcp-diary-title">Maid's Diary</span>
            ${block.date ? `<span class="vcp-diary-date">${escapeHtml(block.date)}</span>` : ''}
          </div>
          ${block.maid ? `
            <div class="vcp-diary-maid-info">
              <span class="diary-maid-label">Maid:</span>
              <span class="vcp-diary-maid-name">${escapeHtml(block.maid)}</span>
            </div>
          ` : ''}
          <div class="vcp-diary-content vcp-markdown-block">${diaryContent}</div>
        </div>
      `;
    }
    
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

function getBlockKey(block: ContentBlock, index: number): string {
  if (block.hash !== undefined && block.hash !== null) {
    return `${block.type}-${String(block.hash)}-${index}`;
  }
  // Fallback for legacy data (index-based)
  return `${block.type}-idx-${index}`;
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
    messageContentRef.value.querySelectorAll('.mermaid-placeholder, pre.mermaid, code.language-mermaid')
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
        if (placeholder.querySelector('svg')) continue; // already rendered
        
        // Use innerHTML as stable cache key
        const codeKey = placeholder.innerHTML;
        // Skip if already being rendered by a concurrent call
        if (renderingMermaids.has(codeKey)) continue;
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
        
        renderingMermaids.add(codeKey);
        try {
          const sourceCode = placeholder.textContent || '';
          placeholder.dataset.mermaidSource = sourceCode; // 保存原始源码
          
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          await mermaid.run({ nodes: [placeholder] });
          
          const renderedSvg = placeholder.innerHTML;
          mermaidCache.set(codeKey, renderedSvg); // 缓存纯 SVG
          
          enhanceMermaid(placeholder, sourceCode);
        } catch (e: any) {
          const errorMsg = e?.str || e?.message || String(e);
          console.error('[MessageRenderer] Mermaid render failed:', errorMsg, e);
          placeholder.innerHTML = `<div class="text-red-500 text-[10px] p-4 rounded-xl border border-red-500/10 bg-red-500/5">图表渲染失败: ${escapeHtml(errorMsg)}</div>`;
        } finally {
          renderingMermaids.delete(codeKey);
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
      renderHeavyContent();
    }
  }
);

// === Context Menu ===
const showMessageContextMenu = async () => {
  const actions: any[] = [];

  if (isStreaming.value && !shell.value?.isUser) {
    actions.push({
      label: "中止回复",
      icon: StopCircle,
      danger: true,
      handler: () => streamStore.stopMessage(props.message.id),
    });
  }

  const getFullText = async () => {
    if (props.message.content) return props.message.content;
    return await historyStore.fetchRawContent(props.message.id);
  };

  // 1. 如果不是流式，编辑消息移动到首位
  if (!isStreaming.value) {
    actions.push({
      label: "编辑消息",
      icon: Edit2,
      handler: async () => {
        const fullText = await getFullText();
        overlayStore.openEditor({
          initialValue: fullText || "",
          onSave: (newContent: string) => historyStore.updateMessageContent(props.message.id, newContent),
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
          await historyStore.reRenderMessage(
            props.message.id,
            props.message.topicId || props.message.topic_id || sessionStore.currentTopicId || ""
          );
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
        handler: () => historyStore.regenerateResponse(props.message.id),
      });
    } else {
      actions.push({
        label: "编辑重发",
        icon: Edit2,
        handler: async () => {
          historyStore.editMessageContent = (await getFullText()) || "";
          historyStore.editingOriginalMessageId = props.message.id;
        },
      });
    }
  }

  actions.push({
    label: "删除消息",
    icon: Trash2,
    danger: true,
    handler: () => {
      if (confirm("确定要删除这条消息吗？")) {
        historyStore.deleteMessage(props.message.id);
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

watch(
  () => props.message.tailBlock,
  (newTailBlock) => {
    if (useAstForCurrentTail.value) return; // 🆕 启用 AST Diff 且有节点时跳过 Morphdom
    if (!newTailBlock || !isPlainBlock(newTailBlock.type)) return;
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
  { deep: true, immediate: true, flush: 'post' }
);


// === AST Diff Executor ===

watch(
  [
    () => props.message.tailMutations,
    () => props.message.tailEpoch,
    () => props.message.tailRevision,
    () => props.message.tailReset,
    () => props.message.tailSnapshot,
    tailSandboxRef,
  ],
  ([mutations, epoch, revision, reset, _snapshot, sandbox]) => {
    astDebugLog(`[AST Diff Watch] Msg ${props.message.id} update: mutations=${mutations ? mutations.length : 0}, sandbox=${sandbox ? 'Ready' : 'Null'}, applied=${appliedMutationsCount}, epoch=${epoch}, revision=${revision}`);

    if (!useAstForCurrentTail.value || !sandbox) {
      if (lastSandbox) {
        cleanupRegistry(props.message.id);
        lastSandbox.innerHTML = '';
        lastSandbox = null;
      }
      if (!mutations || mutations.length === 0) {
        appliedMutationsCount = 0;
      }
      return;
    }

    if (lastSandbox !== sandbox) {
      cleanupRegistry(props.message.id);
      sandbox.innerHTML = '';
      appliedMutationsCount = 0;
      localTailEpoch = -1;
      localTailRevision = -1;
      lastSandbox = sandbox;
      if (getTailSnapshotNodes().length > 0) {
        rebuildTailSnapshot(sandbox);
        astFailureCount = 0;
      }
    }

    const incomingEpoch = epoch ?? 0;
    const incomingRevision = revision ?? -1;
    const epochChanged = incomingEpoch !== localTailEpoch;
    const explicitReset = reset === true || epochChanged;

    if (explicitReset) {
      sandbox.innerHTML = '';
      cleanupRegistry(props.message.id);
      appliedMutationsCount = 0;
      localTailEpoch = incomingEpoch;
      localTailRevision = incomingRevision;
      astFailureCount = 0;

      if (getTailSnapshotNodes().length > 0) {
        rebuildTailSnapshot(sandbox);
        return;
      }
    }

    if (!mutations || mutations.length === 0) {
      if (reset) {
        appliedMutationsCount = 0;
      }
      return;
    }

    if (mutations.length > appliedMutationsCount) {
      const pending = mutations.slice(appliedMutationsCount);
      astDebugLog(`[AST Diff Apply] Executing ${pending.length} new mutations for ${props.message.id}`);
      const result = applyFrame(pending, props.message.id, sandbox);
      if (result.ok) {
        appliedMutationsCount = mutations.length;
        localTailRevision = incomingRevision;
        astFailureCount = 0;
      } else {
        handleAstFrameFailure(sandbox, result.failed?.reason || "applyFrame failed");
      }
    } else if (mutations.length < appliedMutationsCount) {
      astDebugLog(`[AST Diff Reset] Mutations length shrunk from ${appliedMutationsCount} to ${mutations.length}`);
      sandbox.innerHTML = '';
      cleanupRegistry(props.message.id);
      appliedMutationsCount = 0;
      if (getTailSnapshotNodes().length > 0) {
        rebuildTailSnapshot(sandbox);
      } else if (mutations.length > 0) {
        const result = applyFrame(mutations, props.message.id, sandbox);
        if (result.ok) {
          appliedMutationsCount = mutations.length;
        } else {
          handleAstFrameFailure(sandbox, result.failed?.reason || "reset applyFrame failed");
        }
      }
    }
  },
  { flush: "post", immediate: true }
);

onUnmounted(() => {
  removeScopedCss(props.message.id);
  cleanupRegistry(props.message.id);
});
</script>

<template>
  <div v-longpress="showMessageContextMenu"
    class="vcp-message-item flex flex-col w-full mb-6 animate-fade-in px-1 min-w-0" :data-message-id="message.id"
    :data-role="message.role">
    
    <MessageHeader
      v-if="shell"
      :is-user="shell.isUser"
      :display-name="shell.displayName"
      :name-style="{ color: shell.avatarColor }"
      :owner-type="shell.isUser ? 'user' : 'agent'"
      :owner-id="shell.isUser ? 'user_avatar' : (message.agentId || agentId)"
      :avatar-dominant-color="shell.avatarColor"
    />

    <ChatBubble 
      v-if="shell"
      :is-user="shell.isUser" 
      :is-streaming="isStreaming" 
      :bubble-style="{
        '--dynamic-color': shell.avatarColor,
      }"
    >
      <ThinkingIndicator v-if="isStreaming && (!message.blocks || message.blocks.length === 0)" />

      <div ref="messageContentRef" class="vcp-content-blocks space-y-2 min-w-0 w-full overflow-hidden">
        <template v-if="message.blocks && message.blocks.length > 0">
          <template v-for="(block, index) in message.blocks" :key="getBlockKey(block, index)">
            <!-- v-memo=[index] 保证已稳定块零开销：Vue 缓存 VNode 子树，不重渲染、不触碰 DOM -->
            <div v-memo="[getBlockKey(block, index)]">
              <div
                v-if="isPlainBlock(block.type)"
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
                :is-active-stream="isMessageInActiveStream"
              />

              <ToolSummaryBlock
                v-else-if="block.type === 'tool-call-summary'"
                :block="block"
              />
            </div>
          </template>
        </template>
        <template v-else-if="message.content && (!isStreaming || !message.tailBlock)">
          <div class="vcp-markdown-block select-text">
            <p>{{ message.content }}</p>
          </div>
        </template>
        
        <!-- 流式尾部高画质推测渲染 (Speculative Rendering) -->
        <div v-if="isStreaming && message.tailBlock" class="streaming-tail opacity-90">
          <div v-if="useAstForCurrentTail && isPlainBlock(message.tailBlock.type)">
            <div
              ref="tailSandboxRef"
              class="vcp-markdown-block vcp-ast-sandbox"
            />
          </div>
          <div
            v-else-if="!useAstForCurrentTail && isPlainBlock(message.tailBlock.type)"
            ref="tailRootRef"
            class="vcp-markdown-block"
          />
        </div>
        <div v-else-if="isStreaming && message.tailContent && message.blocks && message.blocks.length > 0" class="opacity-70 italic animate-pulse">
          {{ message.tailContent }}
        </div>
      </div>

      <AttachmentPreview 
        v-if="message.attachments && message.attachments.length > 0" 
        :attachments="message.attachments"
        class="pt-3 border-t border-black/5 dark:border-white/5" 
      />

      <StreamingTag v-if="isStreaming" />

      <template #footer>
        <div class="text-[9px] mt-1.5 px-1 opacity-50 font-mono tracking-tighter w-full"
          :class="shell.isUser ? 'text-right' : 'text-left'">
          {{ formatTime(message.timestamp) }}
        </div>
      </template>

      <!-- Mermaid FullScreen Viewer -->
      <MermaidFullScreenViewer
        :visible="isMermaidFullScreen"
        :svg-html="activeMermaidSvg"
        :source-code="activeMermaidSource"
        @close="isMermaidFullScreen = false"
      />
    </ChatBubble>
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
