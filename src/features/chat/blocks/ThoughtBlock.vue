<script setup lang="ts">
import { ref, watch } from "vue";
import { ChevronDown, ChevronUp } from "lucide-vue-next";
import { renderMarkdownNodes } from "../../../core/utils/astRenderer";
import type { ContentBlock } from "../../../core/types/chat";

const props = withDefaults(
  defineProps<{
    block: ContentBlock;
    messageId: string;
    defaultExpanded?: boolean;
    streamEntryFade?: boolean;
  }>(),
  {
    defaultExpanded: false,
    streamEntryFade: false,
  }
);

const isExpanded = ref(props.defaultExpanded);
const entryMotionActive = ref(props.streamEntryFade);

watch(
  () => props.defaultExpanded,
  (newVal) => {
    isExpanded.value = newVal;
  }
);

watch(
  () => props.streamEntryFade,
  (enabled) => {
    if (!enabled) entryMotionActive.value = false;
  },
);

const toggleExpand = () => {
  isExpanded.value = !isExpanded.value;
};

const finishEntryMotion = () => {
  entryMotionActive.value = false;
};

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}
</script>

<template>
  <div
    class="vcp-thought-block"
    :class="{ 'vcp-stream-element-fade-in': entryMotionActive }"
    @animationend.self="finishEntryMotion"
    @animationcancel.self="finishEntryMotion"
  >
    <div class="vcp-thought-header" @click="toggleExpand">
      <span class="vcp-thought-icon">🧠</span>
      <span class="vcp-thought-label flex items-center gap-1">
        {{ block.theme || "元思考链" }}
      </span>
      <component :is="isExpanded ? ChevronUp : ChevronDown" :size="14" class="opacity-40 ml-auto" />
    </div>

    <div
      v-show="isExpanded"
      class="vcp-thought-content"
    >
      <slot>
        <div
          class="thought-body"
          v-html="
            block.nodes && block.nodes.length > 0
              ? renderMarkdownNodes(block.nodes, messageId, block.hash)
              : escapeHtml(block.content || '')
          "
        />
      </slot>
    </div>
  </div>
</template>
