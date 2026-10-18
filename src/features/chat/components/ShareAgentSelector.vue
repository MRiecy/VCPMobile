<script setup lang="ts">
import { computed, ref, watch, onUnmounted } from "vue";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";
import type { AgentListItemDto } from "../../../core/types/assistant";

const props = defineProps<{
  isOpen: boolean;
  sharedText: string;
  sharedFileCount: number;
  failures: Array<{ itemId: string; fileName: string; sha256?: string; error?: string }>;
  busy: boolean;
  error?: string;
  isDraft: boolean;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "selected", agent: AgentListItemDto, allowPartial: boolean): void;
  (e: "retry"): void;
  (e: "discard"): void;
}>();

const assistantStore = useAssistantStore();
const { registerModal, unregisterModal } = useModalHistory();
const modalId = "ShareAgentSelector";

const availableAgents = computed(() => assistantStore.agents);
const allowPartial = ref(false);
watch(() => props.failures, () => { allowPartial.value = false; });

const previewText = computed(() => {
  if (!props.sharedText) return null;
  return props.sharedText.length > 200
    ? props.sharedText.slice(0, 200) + "..."
    : props.sharedText;
});

const handleSelect = (agent: AgentListItemDto) => {
  if (!props.busy && (!props.failures.length || allowPartial.value)) emit("selected", agent, allowPartial.value);
};

watch(
  () => props.isOpen,
  (newVal) => {
    if (newVal) {
      registerModal(modalId, () => {
        emit("close");
      });
    } else {
      unregisterModal(modalId);
    }
  },
  { immediate: true }
);

onUnmounted(() => {
  unregisterModal(modalId);
});
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩 -->
    <Transition name="fade">
      <div
        v-if="isOpen"
        class="vcp-viewport-surface fixed inset-0 bg-black/40 z-sheet"
        @click="emit('close')"
        @touchmove.prevent
      ></div>
    </Transition>

    <!-- 底部弹窗 -->
    <Transition name="slide-up">
      <div
        v-if="isOpen"
        class="vcp-viewport-bottom fixed bottom-0 left-0 right-0 z-sheet bg-white dark:bg-zinc-900 rounded-t-2xl p-5 flex flex-col border-t border-black/10 dark:border-white/10"
        :style="{
          paddingRight: 'calc(var(--vcp-safe-right, 0px) + 1.25rem)',
          paddingBottom: 'calc(var(--vcp-safe-bottom, 48px) + 12px)',
          paddingLeft: 'calc(var(--vcp-safe-left, 0px) + 1.25rem)',
        }"
      >
        <!-- 拖手线 -->
        <div class="w-10 h-1 bg-black/10 dark:bg-white/15 rounded-full mx-auto mb-4"></div>

        <!-- 标题 -->
        <div class="flex flex-col mb-4 px-1">
          <span class="text-[10px] font-bold text-zinc-400 uppercase tracking-widest leading-none">Share Intent</span>
          <span class="text-[17px] font-extrabold text-zinc-800 dark:text-zinc-100 mt-1">{{ isDraft ? '继续当前分享' : '选择对话助手' }}</span>
        </div>

        <!-- 分享内容预览 -->
        <div
          v-if="previewText || sharedFileCount > 0"
          class="mx-1 mb-4 px-3 py-2.5 rounded-xl bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10"
        >
          <div v-if="previewText" class="text-[13px] text-zinc-600 dark:text-zinc-400 leading-relaxed break-all line-clamp-4">
            {{ previewText }}
          </div>
          <div
            v-if="sharedFileCount > 0"
            class="flex items-center gap-1.5 mt-1.5 text-[12px] font-medium text-blue-500"
          >
            <div class="i-heroicons-paper-clip text-sm"></div>
            <span>{{ sharedFileCount }} 个附件</span>
          </div>
        </div>

        <p v-if="error" class="text-sm text-red-500 mb-3">{{ error }}</p>
        <div v-if="failures.length" class="text-sm mb-3">
          <p v-for="failure in failures" :key="failure.itemId" class="text-red-500">{{ failure.fileName }}：{{ failure.error }}</p>
          <button v-if="failures.some(item => item.sha256)" class="btn mt-2" :disabled="busy" @click="emit('retry')">重试附件导入</button>
          <p v-if="failures.some(item => !item.sha256)" class="text-zinc-500">来源未能提供的项目需要重新分享。</p>
          <label class="flex gap-2 items-center min-h-11"><input v-model="allowPartial" type="checkbox">仅继续文字和成功的附件</label>
        </div>
        <p v-if="busy" class="text-sm text-zinc-500 mb-3">正在准备分享…</p>
        <!-- Agent 列表 -->
        <div class="flex flex-col max-h-[320px] overflow-y-auto px-1 gap-1 scrollbar-none">
          <div v-if="availableAgents.length === 0" class="text-center py-6 text-[14px] text-zinc-400">
            暂无可用助手
          </div>
          <button
            v-for="agent in availableAgents"
            :key="agent.id"
            :disabled="busy || !!error || (!!failures.length && !allowPartial)"
            @click="handleSelect(agent)"
            class="flex items-center gap-3 px-3 py-2.5 rounded-xl hover:bg-black/5 dark:hover:bg-white/5 disabled:opacity-40 transition-colors text-left"
          >
            <VcpAvatar
              :owner-id="agent.id"
              :fallback-name="agent.name"
              :dominant-color="agent.avatarCalculatedColor || undefined"
              size="w-9 h-9"
              owner-type="agent"
            />
            <div class="flex-1 min-w-0">
              <div class="text-[15px] font-semibold text-zinc-800 dark:text-zinc-100 truncate">
                {{ agent.name }}
              </div>
              <div class="text-[12px] text-zinc-400 dark:text-zinc-500 truncate mt-0.5">
                {{ agent.model || '默认模型' }}
              </div>
            </div>
            <div class="i-heroicons-chevron-right text-zinc-400 text-lg shrink-0"></div>
          </button>
        </div>
        <div class="grid grid-cols-2 gap-2 mt-3 border-t border-black/10 dark:border-white/10 pt-3">
          <button
            type="button"
            class="min-h-11 rounded-lg border border-red-500/25 bg-transparent px-3 text-[14px] font-medium text-red-600 dark:text-red-400 transition-colors active:bg-red-500/10 disabled:opacity-40"
            :disabled="busy"
            @click="emit('discard')"
          >丢弃分享</button>
          <button
            type="button"
            class="min-h-11 rounded-lg border border-zinc-800 bg-zinc-800 px-3 text-[14px] font-medium text-white dark:border-zinc-100 dark:bg-zinc-100 dark:text-zinc-900 transition-colors active:bg-zinc-700 dark:active:bg-zinc-200"
            @click="emit('close')"
          >稍后继续</button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
