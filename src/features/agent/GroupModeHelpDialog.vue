<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

/**
 * 群聊模式帮助说明弹窗。
 *
 * 入口：群组设置「群聊模式」区块标题右侧的问号按钮。
 * 内容覆盖四种发言模式及必要的使用说明。
 */
const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits(['update:modelValue']);

const close = () => emit('update:modelValue', false);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'GroupModeHelpDialog';
const surfaceRef = ref<HTMLElement | null>(null);

watch(
  () => props.modelValue,
  (val) => {
    if (val) {
      registerModal(modalId, close, () => surfaceRef.value);
    } else {
      unregisterModal(modalId);
    }
  },
  { immediate: true },
);
onUnmounted(() => unregisterModal(modalId));

const modes = [
  {
    label: '顺序发言',
    text: '成员按设定顺序轮流回复，可用上下箭头调整次序。',
  },
  {
    label: '自然随机',
    text: '优先回应 @提及和匹配的触发标签，也会随机邀请成员参与。',
  },
  {
    label: '邀请发言',
    text: '点击成员或发送 @名字 即可邀请发言，其他成员不会自动回复。',
  },
  {
    label: 'JEV 智能群聊',
    text: '根据话题和成员特点选择谁来回应，讨论结束后自动停下。「开始」让一位成员开场，「继续」接着当前话题聊。',
  },
];
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="props.modelValue"
        class="vcp-viewport-surface fixed inset-0 z-dialog bg-black/50 flex items-center justify-center p-4"
        @click.self="close"
        @touchmove.prevent
      >
        <div
          ref="surfaceRef"
          class="w-full max-w-md max-h-[80%] flex flex-col bg-[var(--primary-bg)] border border-[var(--border-color)] rounded-xl overflow-hidden"
          role="dialog"
          aria-modal="true"
          aria-label="发言模式说明"
        >
          <!-- 标题栏 -->
          <div class="flex items-center justify-between px-4 py-3 border-b border-black/5 dark:border-white/5 shrink-0">
            <span class="text-sm font-bold text-primary-text">发言模式说明</span>
            <button
              class="w-11 h-11 flex items-center justify-center rounded-lg text-primary-text opacity-60 active:opacity-40 transition-opacity"
              aria-label="关闭"
              @click="close"
            >
              <div class="i-heroicons-x-mark text-base"></div>
            </button>
          </div>

          <!-- 说明正文 -->
          <div class="flex-1 overflow-y-auto px-4 py-3 space-y-4 vcp-scrollable" @touchmove.stop>
            <div v-for="mode in modes" :key="mode.label" class="flex gap-2.5">
              <div class="w-0.5 shrink-0 bg-[var(--border-color)]"></div>
              <div class="min-w-0">
                <div class="text-[13px] font-bold text-primary-text">{{ mode.label }}</div>
                <p class="text-xs leading-relaxed text-[var(--secondary-text)] mt-0.5">{{ mode.text }}</p>
              </div>
            </div>

            <div class="border-t border-black/5 dark:border-white/5 pt-3 space-y-2">
              <div class="text-xs font-semibold text-primary-text">补充说明</div>
              <ul class="text-xs leading-relaxed text-[var(--secondary-text)] space-y-2 list-disc pl-4">
                <li>标签匹配：严格模式遇到匹配标签就回应；自然模式减少引用历史消息造成的重复回应。</li>
                <li>@所有人 仅在自然随机模式下有效。</li>
                <li>JEV 群聊中，可随时邀请成员或发送新消息补充话题。</li>
                <li class="text-red-500 font-medium">停止群聊会立即停止所有成员回复。</li>
              </ul>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.25s cubic-bezier(0.16, 1, 0.3, 1);
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
