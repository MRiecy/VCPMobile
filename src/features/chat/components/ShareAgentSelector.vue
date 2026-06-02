<script setup lang="ts">
import { computed } from "vue";
import { useAssistantStore, type AgentConfig } from "../../../core/stores/assistant";
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";

const props = defineProps<{
  isOpen: boolean;
  sharedText: string;
  sharedFileCount: number;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "selected", agent: AgentConfig): void;
}>();

const assistantStore = useAssistantStore();

const availableAgents = computed(() => assistantStore.agents);

const previewText = computed(() => {
  if (!props.sharedText) return null;
  return props.sharedText.length > 200
    ? props.sharedText.slice(0, 200) + "..."
    : props.sharedText;
});

const handleSelect = (agent: AgentConfig) => {
  emit("selected", agent);
};
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩 -->
    <Transition name="fade">
      <div
        v-if="isOpen"
        class="fixed inset-0 bg-black/40 z-sheet"
        @click="emit('close')"
        @touchmove.prevent
      ></div>
    </Transition>

    <!-- 底部弹窗 -->
    <Transition name="slide-up">
      <div
        v-if="isOpen"
        class="fixed bottom-0 left-0 right-0 z-sheet bg-white/95 dark:bg-zinc-900/95 backdrop-blur-xl rounded-t-[1.8rem] shadow-2xl p-5 flex flex-col border-t border-white/20 dark:border-white/5"
        :style="{ paddingBottom: 'calc(env(safe-area-inset-bottom, 20px) + 12px)' }"
      >
        <!-- 拖手线 -->
        <div class="w-10 h-1 bg-black/10 dark:bg-white/15 rounded-full mx-auto mb-4"></div>

        <!-- 标题 -->
        <div class="flex flex-col mb-4 px-1">
          <span class="text-[10px] font-bold text-zinc-400 uppercase tracking-widest leading-none">Share Intent</span>
          <span class="text-[17px] font-extrabold text-zinc-800 dark:text-zinc-100 mt-1">选择对话助手</span>
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

        <!-- Agent 列表 -->
        <div class="flex flex-col max-h-[320px] overflow-y-auto px-1 gap-1 scrollbar-none">
          <div v-if="availableAgents.length === 0" class="text-center py-6 text-[14px] text-zinc-400">
            暂无可用助手
          </div>
          <button
            v-for="agent in availableAgents"
            :key="agent.id"
            @click="handleSelect(agent)"
            class="flex items-center gap-3 px-3 py-2.5 rounded-xl hover:bg-black/5 dark:hover:bg-white/5 active:scale-[0.98] transition-all text-left"
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
      </div>
    </Transition>
  </Teleport>
</template>
