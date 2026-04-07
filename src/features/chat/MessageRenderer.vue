<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { ChatMessage } from "../../core/stores/chatManager";
import { useAssistantStore } from "../../core/stores/assistant";
import { useSettingsStore } from "../../core/stores/settings";
import {
  useContentProcessor,
  type ContentBlock,
} from "../../core/composables/useContentProcessor";
import { useAvatarTheme } from "../../core/composables/useAvatarTheme";
import { useOverlayStore } from "../../core/stores/overlay";
import { useChatManagerStore } from "../../core/stores/chatManager";
import { useNotificationStore } from "../../core/stores/notification";
import { Copy, Edit2, RotateCcw, Trash2, StopCircle } from "lucide-vue-next";

// Import block components
import MarkdownBlock from "./blocks/MarkdownBlock.vue";
import ToolBlock from "./blocks/ToolBlock.vue";
import DiaryBlock from "./blocks/DiaryBlock.vue";
import ThoughtBlock from "./blocks/ThoughtBlock.vue";
import HtmlPreviewBlock from "./blocks/HtmlPreviewBlock.vue";
import ChatBubble from "./components/ChatBubble.vue";
import MessageHeader from "./components/MessageHeader.vue";
import ThinkingIndicator from "./components/ThinkingIndicator.vue";
import StreamingTag from "./components/StreamingTag.vue";
import AttachmentPreview from "../../components/ui/AttachmentPreview.vue";

const props = defineProps<{
  message: ChatMessage;
  agentId?: string;
  depth?: number;
}>();

const assistantStore = useAssistantStore();
const settingsStore = useSettingsStore();
const { processMessageContent, removeScopedCss } = useContentProcessor();
const { extractAndSaveColor } = useAvatarTheme();
const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();

const chatStore = useChatManagerStore();

const isUser = computed(() => props.message.role === "user");
const isStreaming = computed(() => {
  if (isUser.value) return false;
  if (props.message.isThinking) return true;

  // 检查当前消息是否在所属会话的活动流中
  const itemId =
    props.message.extra?.agentId ||
    props.message.extra?.groupId ||
    props.agentId;
  const topicId = chatStore.currentTopicId;
  if (!itemId || !topicId) return false;

  const key = `${itemId}:${topicId}`;
  const streams = chatStore.sessionActiveStreams?.get(key);
  return streams ? streams.has(props.message.id) : false;
});

// 获取当前消息实际对应的 Agent ID (对于群聊，从 extra 中读取)
const actualAgentId = computed(() => {
  return props.message.extra?.agentId || props.agentId;
});

// 获取当前 Agent 的配置
const agentConfig = computed(() => {
  if (isUser.value) return null;

  // 1. 优先按 ID 查找
  if (actualAgentId.value) {
    const agent = assistantStore.agents.find(
      (a) => a.id === actualAgentId.value,
    );
    if (agent) return agent;
  }

  // 2. 针对群聊历史数据，可能只有名称没有 ID，尝试按名称查找
  if (props.message.name) {
    const agent = assistantStore.agents.find(
      (a) => a.name === props.message.name,
    );
    if (agent) return agent;
  }

  return null;
});

// 获取头像 URL
const resolvedAvatarUrl = computed(() => {
  if (isUser.value) return "vcp-avatar://user/default";

  // 优先使用匹配到的 Agent ID
  if (actualAgentId.value) {
    return `vcp-avatar://agent/${actualAgentId.value}`;
  }

  // 如果没有 ID 只有名称，尝试按名称匹配 (兼容旧数据)
  if (props.message.name) {
    const agent = assistantStore.agents.find(
      (a) => a.name === props.message.name,
    );
    if (agent) return `vcp-avatar://agent/${agent.id}`;
  }

  return null;
});

onMounted(() => {
  // If color is missing, extract it
  if (
    !isUser.value &&
    actualAgentId.value &&
    resolvedAvatarUrl.value &&
    !agentConfig.value?.avatarCalculatedColor
  ) {
    extractAndSaveColor(
      actualAgentId.value,
      resolvedAvatarUrl.value,
    ).then((color) => {
      if (agentConfig.value && color) {
        agentConfig.value.avatarCalculatedColor = color;
      }
    });
  }
});

onUnmounted(() => {
  // 彻底防止 Scoped CSS 在组件销毁后泄漏内存或污染全局
  if (props.message && props.message.id) {
    removeScopedCss(props.message.id);
  }
});

// 响应式消息块 (AST 树)
const contentBlocks = ref<ContentBlock[]>([]);
// 流式传输专用原始文本
const streamContent = ref<string>("");

// 过渡状态：用于在流式结束、等待 Rust AST 解析完成前，保持流式视图不消失，防止闪烁
const isTransitioning = ref(false);

// 决定当前 UI 显示哪个视图：只要在流式中，或者正在过渡中，就显示流式纯文本视图
const showStreamView = computed(
  () => isStreaming.value || isTransitioning.value,
);

// 节流状态
let isProcessing = false;
let pendingText: string | null = null;

// 核心解析逻辑
const updateContentBlocks = async (text: string) => {
  if (!text && props.message.isThinking) {
    contentBlocks.value = [];
    streamContent.value = "";
    return;
  }

  const options = {
    role: props.message.role,
    depth: props.depth || 0,
    rules: (agentConfig.value as any)?.stripRegexes,
    messageId: props.message.id,
    isStreaming: isStreaming.value,
  };

  if (isStreaming.value) {
    // 流式状态：跳过 Rust AST，全量正则生成混合 Markdown
    const blocks = await processMessageContent(text || "", options);
    streamContent.value = blocks[0]?.content || "";
  } else {
    // 静态完成状态：走严格的 AST 拆分 (Rust)
    isTransitioning.value = true;
    try {
      contentBlocks.value = await processMessageContent(text || "", options);
    } finally {
      // 确保无论解析成功失败，都能解除过渡状态
      isTransitioning.value = false;
    }
  }
};

// 监听文本变化或流状态变化，加入节流机制 (Throttle) 防止流式输出卡顿
watch(
  [
    () =>
      props.message.processedContent ||
      props.message.displayedContent ||
      props.message.content,
    () => isStreaming.value,
  ],
  async ([newText, streaming]) => {
    if (isProcessing) {
      // 如果正在处理，则将最新文本存入 pending
      pendingText = (newText as string) || "";
      return;
    }

    try {
      isProcessing = true;
      await updateContentBlocks((newText as string) || "");

      // [优化] 流式状态时放宽到 33ms (约 30fps) 以减轻渲染主线程负担
      // 如果是非流式（例如切换话题、历史加载），保持 50ms 响应性
      const throttleTime = streaming ? 33 : 50;
      await new Promise((resolve) => setTimeout(resolve, throttleTime));
    } catch (e) {
      console.error("[MessageRenderer] Watcher error:", e);
    } finally {
      // [关键修复] 必须在 finally 中释放锁，防止发生错误后永久死锁
      isProcessing = false;

      // 消费积压的文本
      if (pendingText !== null) {
        const textToProcess = pendingText;
        pendingText = null;
        updateContentBlocks(textToProcess);
      }
    }
  },
  { immediate: true },
);

// 计算气泡背景颜色
const bubbleStyle = computed(() => {
  if (isUser.value)
    return {
      backgroundColor: "var(--user-bubble-bg, rgba(145, 109, 51, 0.573))",
      color: "var(--user-text, #e8e8e8)",
      borderBottomRightRadius: "4px",
    };

  const color =
    agentConfig.value?.avatarCalculatedColor || props.message.avatarColor;
  const baseStyle: any = {
    backgroundColor: "var(--assistant-bubble-bg, rgba(44, 62, 74, 0.577))",
    color: "var(--agent-text, #e8e8e8)",
    borderBottomLeftRadius: "4px",
    border: "1px solid rgba(128, 128, 128, 0.15)", // Subtle frosted border instead of solid heavy line
  };

  if (color) {
    baseStyle["--dynamic-color"] = color;
    baseStyle.borderColor = `${color}30`; // Adjust to very subtle 18% opacity
    baseStyle.boxShadow = `0 4px 12px ${color}15`;
  }

  return baseStyle;
});

// 计算名称颜色
const nameStyle = computed(() => {
  if (isUser.value) return { color: "var(--secondary-text)" };
  const color =
    agentConfig.value?.avatarCalculatedColor || props.message.avatarColor;
  return { color: color || "var(--highlight-text)" };
});

const displayName = computed(() => {
  if (!props.message.name && !agentConfig.value?.name && !isUser.value)
    return null;
  return isUser.value
    ? settingsStore.settings?.userName || "ME"
    : props.message.name || agentConfig.value?.name;
});

const avatarFallbackText = computed(() => {
  return isUser.value
    ? settingsStore.settings?.userName || "ME"
    : props.message.name || "AI";
});

const avatarBorderColor = computed(() => {
  return isUser.value
    ? undefined
    : agentConfig.value?.avatarCalculatedColor ||
        props.message.avatarColor ||
        "transparent";
});

// 长按菜单触发逻辑
const showMessageContextMenu = () => {
  const chatStore = useChatManagerStore();

  const actions: any[] = [];

  // 1. 如果正在流式生成，提供强制中止功能 (最高优先级)
  if (isStreaming.value && !isUser.value) {
    actions.push({
      label: "中止回复",
      icon: StopCircle,
      danger: true,
      handler: () => {
        chatStore.stopMessage(props.message.id);
      },
    });
  }

  // 2. 复制文本 (所有状态可用，除了纯占位符)
  const fullText = props.message.content || streamContent.value;
  if (fullText) {
    actions.push({
      label: "复制内容",
      icon: Copy,
      handler: async () => {
        try {
          if (navigator.clipboard && navigator.clipboard.writeText) {
            await navigator.clipboard.writeText(fullText);
          } else {
            // Fallback for some old webviews
            const textarea = document.createElement("textarea");
            textarea.value = fullText;
            document.body.appendChild(textarea);
            textarea.select();
            document.execCommand("copy");
            document.body.removeChild(textarea);
          }
          notificationStore.addNotification({
            type: "success",
            title: "复制成功",
            message: "内容已复制到剪贴板",
            duration: 2000,
          });
        } catch (e) {
          console.error("[MessageContextMenu] Copy failed:", e);
        }
      },
    });
  }

  // 3. 编辑消息 (非流式状态下支持全屏编辑)
  if (!isStreaming.value) {
    actions.push({
      label: "编辑消息",
      icon: Edit2,
      handler: () => {
        overlayStore.openEditor({
          initialValue: props.message.content || streamContent.value || "",
          onSave: (newContent) => handleSaveEdit(newContent),
        });
      },
    });
  }

  // 4. 用户特权操作 (编辑重发)
  if (isUser.value) {
    actions.push({
      label: "编辑重发",
      icon: Edit2,
      handler: () => {
        // 将内容填入全局编辑状态供 InputEnhancer 读取
        chatStore.editMessageContent = props.message.content;
      },
    });
  }

  // 5. AI 重新生成 (非流式状态下可用)
  if (!isUser.value && !isStreaming.value) {
    actions.push({
      label: "重新生成",
      icon: RotateCcw,
      handler: () => {
        chatStore.regenerateResponse(props.message.id);
      },
    });
  }

  // 6. 删除 (万能操作)
  actions.push({
    label: "删除消息",
    icon: Trash2,
    danger: true,
    handler: () => {
      if (confirm("确定要删除这条消息吗？")) {
        chatStore.deleteMessage(props.message.id);
      }
    },
  });

  overlayStore.openContextMenu(
    actions,
    isUser.value ? "User Message" : "Assistant Message",
  );
};

const handleSaveEdit = async (newContent: string) => {
  const chatStore = useChatManagerStore();
  if (newContent !== props.message.content) {
    await chatStore.updateMessageContent(props.message.id, newContent);
    // 立即重新触发正则渲染
    await updateContentBlocks(newContent);
  }
};
</script>

<template>
  <div
    v-longpress="showMessageContextMenu"
    class="vcp-message-item flex flex-col w-full mb-6 animate-fade-in px-1 min-w-0"
    :data-message-id="message.id"
    :data-role="message.role"
  >
    <MessageHeader
      :is-user="isUser"
      :display-name="displayName"
      :name-style="nameStyle"
      :avatar-url="resolvedAvatarUrl"
      :avatar-border-color="avatarBorderColor"
      :avatar-fallback-text="avatarFallbackText"
    />

    <ChatBubble
      :is-user="isUser"
      :is-streaming="isStreaming"
      :bubble-style="bubbleStyle"
    >
      <ThinkingIndicator v-if="message.isThinking && streamContent === ''" />

      <template v-if="!showStreamView">
        <div
          class="vcp-content-blocks space-y-2 min-w-0 w-full overflow-hidden"
        >
          <template v-for="(block, index) in contentBlocks" :key="index">
            <MarkdownBlock
              v-if="block.type === 'markdown'"
              :content="block.content"
              :is-streaming="false"
            />
            <ToolBlock
              v-else-if="block.type === 'tool-use'"
              :type="block.type"
              :content="block.content"
              :block="block"
            />
            <ToolBlock
              v-else-if="block.type === 'tool-result'"
              :type="block.type"
              :block="block"
            />
            <DiaryBlock
              v-else-if="block.type === 'diary'"
              :content="block.content"
              :block="block"
            />
            <ThoughtBlock
              v-else-if="block.type === 'thought'"
              :content="block.content"
              :block="block"
            />
            <HtmlPreviewBlock
              v-else-if="block.type === 'html-preview'"
              :content="block.content"
              :message-id="message.id"
            />
            <div
              v-else-if="block.type === 'button-click'"
              class="inline-block px-3 py-1 bg-black/10 dark:bg-white/10 rounded-full text-[10px] font-bold opacity-70 my-1"
            >
              {{ block.content }}
            </div>
          </template>
        </div>
      </template>
      <template v-else>
        <div
          class="vcp-content-blocks space-y-2 min-w-0 w-full overflow-hidden"
        >
          <MarkdownBlock :content="streamContent" :is-streaming="true" />
        </div>
      </template>

      <AttachmentPreview
        v-if="message.attachments && message.attachments.length > 0"
        :attachments="message.attachments"
        class="pt-3 border-t border-black/5 dark:border-white/5"
      />

      <StreamingTag v-if="message.isThinking && streamContent !== ''" />

      <template #footer>
        <div
          class="text-[9px] mt-1.5 px-1 opacity-50 font-mono tracking-tighter w-full"
          :class="isUser ? 'text-right' : 'text-left'"
          :style="
            isUser
              ? { color: 'var(--secondary-text)' }
              : { color: 'var(--secondary-text)' }
          "
        >
          {{
            new Date(message.timestamp).toLocaleTimeString([], {
              hour: "2-digit",
              minute: "2-digit",
              second: "2-digit",
            })
          }}
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
  from {
    opacity: 0;
    transform: translateY(10px) scale(0.98);
  }
  to {
    opacity: 1;
    transform: translateY(0) scale(1);
  }
}
</style>
