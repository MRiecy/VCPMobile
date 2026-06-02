<script setup lang="ts">
import { ref, watch, nextTick } from "vue";
import { marked } from "marked";
import type { ChatMessage } from "../../core/types/chat";

const props = defineProps<{
  message: ChatMessage;
}>();

const contentRef = ref<HTMLElement | null>(null);
const renderedHtml = ref("");

// Configure marked
marked.setOptions({
  breaks: true,
  gfm: true,
});

function renderMarkdown(text: string): string {
  if (!text) return "";
  try {
    return marked.parse(text) as string;
  } catch {
    return escapeHtml(text);
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Watch content changes and re-render
watch(
  () => props.message.content,
  (content) => {
    renderedHtml.value = renderMarkdown(content || "");
  },
  { immediate: true }
);

// Lazy KaTeX rendering for math formulas
watch(renderedHtml, async () => {
  await nextTick();
  if (!contentRef.value) return;

  const mathElements = contentRef.value.querySelectorAll(
    ".katex-inline, .katex-block"
  );
  if (mathElements.length > 0) {
    try {
      const katex = (await import("katex")).default;
      mathElements.forEach((el) => {
        if (el.querySelector(".katex")) return;
        const latex = el.getAttribute("data-latex");
        if (!latex) return;
        const isDisplay = el.classList.contains("katex-block");
        try {
          katex.render(latex, el as HTMLElement, {
            throwOnError: false,
            strict: false,
            displayMode: isDisplay,
          });
        } catch {
          // KaTeX render failed for this element, leave as-is
        }
      });
    } catch {
      // KaTeX not available
    }
  }
});
</script>

<template>
  <div
    class="flex flex-col w-full mb-5 px-1 min-w-0"
    :class="message.role === 'user' ? 'items-end' : 'items-start'"
  >
    <!-- Bubble -->
    <div
      class="message-bubble rounded-2xl transition-all relative min-w-[60px]"
      :class="
        message.role === 'user'
          ? 'p-3 w-fit max-w-[85%] bg-blue-500 text-white'
          : 'p-1.5 w-fit max-w-[100%] min-w-[1rem] bg-black/5 dark:bg-white/5 text-primary-text border border-black/5 dark:border-white/5'
      "
    >
      <!-- Thinking indicator -->
      <div
        v-if="message.isThinking"
        class="flex items-center space-x-1 py-1 opacity-70"
      >
        <span
          class="w-1.5 h-1.5 bg-current rounded-full animate-bounce"
          style="animation-delay: 0ms"
        ></span>
        <span
          class="w-1.5 h-1.5 bg-current rounded-full animate-bounce"
          style="animation-delay: 150ms"
        ></span>
        <span
          class="w-1.5 h-1.5 bg-current rounded-full animate-bounce"
          style="animation-delay: 300ms"
        ></span>
      </div>

      <!-- Rendered content -->
      <div
        v-if="renderedHtml"
        ref="contentRef"
        class="vcp-markdown-block select-text min-w-0 w-full overflow-hidden"
        v-html="renderedHtml"
      />
      <pre
        v-else-if="message.content"
        class="font-sans whitespace-pre-wrap break-all m-0 select-text"
      >{{ message.content }}</pre>
    </div>

    <!-- Timestamp -->
    <div
      class="text-[9px] mt-1.5 px-1 opacity-40 font-mono tracking-tighter w-full"
      :class="message.role === 'user' ? 'text-right' : 'text-left'"
    >
      {{ new Date(message.timestamp).toLocaleString("zh-CN", {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      }) }}
    </div>
  </div>
</template>

<style scoped>
.message-bubble {
  position: relative;
  word-break: break-word;
}
</style>
