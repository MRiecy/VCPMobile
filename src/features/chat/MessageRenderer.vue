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
import { useContentProcessor } from "../../core/composables/useContentProcessor";
import { Copy, Edit2, RotateCcw, Trash2, StopCircle } from "lucide-vue-next";

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
  const itemId = props.message.agentId || props.agentId;
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
        return `<div class="vcp-markdown-block">${renderMarkdownNodes(block.nodes, props.message.id)}</div>`;
      }
      return `<div class="vcp-markdown-block"><p>${escapeHtml(block.content || "")}</p></div>`;
    
    case "diary": {
      const diaryContent = (block.nodes && block.nodes.length > 0)
        ? renderMarkdownNodes(block.nodes, props.message.id)
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

    case "button-click":
      return `
        <div class="inline-block px-3 py-1 bg-black/10 dark:bg-white/10 rounded-full text-[10px] font-bold opacity-70 my-1">
          ${escapeHtml(block.content || "")}
        </div>
      `;

    case "style":
      return "";

    default:
      return "";
  }
}

function getBlockKey(block: ContentBlock, index: number): string {
  const content = block.content || '';
  const nodesStr = block.nodes ? JSON.stringify(block.nodes) : '';
  const stateStr = JSON.stringify({
    s: block.status,
    c: block.is_complete,
    t: block.tool_name,
    f: block.footer,
    d: block.details,
    r: block.role,
    e: block.is_end,
  });
  const combined = content + nodesStr + stateStr;
  const hash = combined.length > 0
    ? combined.split('').reduce((h, c) => ((h << 5) - h) + c.charCodeAt(0), 0).toString(36)
    : '';
  return `${block.type}-${hash}-${index}`;
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// === Heavy Content Rendering (KaTeX inline math + Mermaid) ===
const renderHeavyContent = async () => {
  // 消息处于活跃流中时完全跳过重渲染（KaTeX/Mermaid/Emoticon），
  // 与当前话题无关，确保多并发流式话题切换时行为一致
  if (isMessageInActiveStream.value) return;
  await nextTick();
  if (!messageContentRef.value) return;

  // 1. KaTeX math (inline + display mode, rendered inside markdown blocks via v-html)
  const mathElements = messageContentRef.value.querySelectorAll('.vcp-math-inline[data-latex], .vcp-math-block[data-latex]');
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
  const mermaidPlaceholders = messageContentRef.value.querySelectorAll('.mermaid-placeholder');
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
        if (placeholder.querySelector('svg')) continue; // already rendered
        // Use innerHTML (HTML-escaped code) as stable cache key
        const codeKey = placeholder.innerHTML;
        // Skip if already being rendered by a concurrent call
        if (renderingMermaids.has(codeKey)) continue;
        // Skip if Vue has replaced this element out of the DOM
        if (!messageContentRef.value.contains(placeholder)) continue;
        // Use cache to avoid re-rendering the same diagram
        if (mermaidCache.has(codeKey)) {
          placeholder.innerHTML = mermaidCache.get(codeKey)!;
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          continue;
        }
        renderingMermaids.add(codeKey);
        try {
          placeholder.classList.remove('mermaid-placeholder');
          placeholder.classList.add('mermaid');
          await mermaid.run({ nodes: [placeholder] });
          mermaidCache.set(codeKey, placeholder.innerHTML);
        } catch (e: any) {
          const errorMsg = e?.str || e?.message || String(e);
          console.error('[MessageRenderer] Mermaid render failed:', errorMsg, e);
          placeholder.innerHTML = `<div class="text-red-500 text-[10px]">图表渲染失败: ${escapeHtml(errorMsg)}</div>`;
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
    if (isMessageInActiveStream.value) return;
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
  return new Date(ts).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

// === Block Compilation Fallback ===
// 当消息无 blocks 且不在流式中时，向后端请求预渲染
const blocksRequested = ref(false);
watch(
  () => props.message.blocks,
  (blocks) => {
    if (!blocks && props.message.content && !blocksRequested.value && !isStreaming.value) {
      blocksRequested.value = true;
      historyStore.compileMessageBlocks(props.message.id);
    }
  },
  { immediate: true }
);

// === Style Block CSS Injection ===
const { injectScopedCss, removeScopedCss } = useContentProcessor();

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

onUnmounted(() => {
  removeScopedCss(props.message.id);
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
            />

            <ThoughtBlock
              v-else-if="block.type === 'thought'"
              :block="block"
              :message-id="message.id"
            />

            <HtmlPreviewBlock
              v-else-if="block.type === 'html-preview'"
              :content="block.content || ''"
              :message-id="message.id"
            />
          </div>
        </template>
        
        <!-- 流式尾部快速渲染 (Aurora 路径) -->
        <div v-if="isStreaming && message.tailContent" class="opacity-70 italic animate-pulse">
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
    </ChatBubble>
  </div>
</template>

<style scoped>
.animate-fade-in {
  animation: fadeIn 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

@keyframes fadeIn {
  from { opacity: 0; transform: translateY(10px) scale(0.98); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}
</style>
