<script setup lang="ts">
import { watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

/**
 * 群聊模式帮助说明弹窗。
 *
 * 入口：群组设置「群聊模式」区块标题右侧的问号按钮。
 * 内容覆盖三种发言模式的触发规则、Tag 匹配模式差异、以及
 * 「新成员默认 Tag 为名字」等隐式行为——让这些规则不读代码也可知。
 */
const props = defineProps<{
  modelValue: boolean;
}>();

const emit = defineEmits(['update:modelValue']);

const close = () => emit('update:modelValue', false);

const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'GroupModeHelpDialog';

watch(
  () => props.modelValue,
  (val) => {
    if (val) {
      registerModal(modalId, close);
    } else {
      unregisterModal(modalId);
    }
  },
);

const modes = [
  {
    label: '顺序发言',
    accent: 'bg-blue-500',
    text: '每条消息触发全体成员按列表顺序依次轮流发言，一人一段，直到所有成员说完。',
  },
  {
    label: '自然随机',
    accent: 'bg-purple-500',
    text: '按优先级决定谁发言：@名字 ＞ Tag 命中 ＞ @所有人 ＞ 15% 随机概率 ＞ 保底一人。Tag 在成员列表中设置，多个 Tag 用逗号分隔。',
  },
  {
    label: '邀请发言',
    accent: 'bg-orange-500',
    text: '成员不会自动回复。点按输入框上方「邀其发言」横条指定成员，或在消息中 @成员 后发送——被提及的成员将按出现顺序自动受邀发言。',
  },
];
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="props.modelValue"
        class="fixed inset-0 z-dialog bg-black/50 flex items-center justify-center p-6"
        @click.self="close"
        @touchmove.prevent
      >
        <div
          class="w-full max-w-sm max-h-[75vh] flex flex-col bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl shadow-2xl overflow-hidden"
          role="dialog"
          aria-label="发言模式说明"
        >
          <!-- 标题栏 -->
          <div class="flex items-center justify-between px-4 py-3 border-b border-black/5 dark:border-white/5 shrink-0">
            <span class="text-sm font-bold text-primary-text">发言模式说明</span>
            <button
              class="w-7 h-7 flex items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-primary-text opacity-60 active:scale-90 transition-all"
              aria-label="关闭"
              @click="close"
            >
              <div class="i-heroicons-x-mark text-base"></div>
            </button>
          </div>

          <!-- 说明正文 -->
          <div class="flex-1 overflow-y-auto px-4 py-3 space-y-4">
            <div v-for="mode in modes" :key="mode.label" class="flex gap-2.5">
              <div class="w-0.5 rounded-full shrink-0" :class="mode.accent"></div>
              <div class="min-w-0">
                <div class="text-[13px] font-bold text-primary-text">{{ mode.label }}</div>
                <p class="text-[11px] leading-relaxed text-primary-text opacity-60 mt-0.5">{{ mode.text }}</p>
              </div>
            </div>

            <div class="border-t border-black/5 dark:border-white/5 pt-3 space-y-2">
              <div class="text-[10px] font-bold uppercase tracking-wider text-primary-text opacity-40">补充说明</div>
              <ul class="text-[11px] leading-relaxed text-primary-text opacity-60 space-y-1.5 list-disc pl-4">
                <li><span class="font-bold opacity-100 text-purple-500">严格模式</span>：上下文或消息包含 Tag 即触发，无需 @。</li>
                <li><span class="font-bold opacity-100 text-purple-500">自然模式</span>：区分 Tag 来源，避免 Agent 因引用自己的历史发言而循环触发。</li>
                <li>新加入的成员默认以「名字」作为 Tag（可手动清空），因此严格模式下不带 @ 的名字提及也会触发该成员。</li>
                <li>@所有人 仅在自然随机模式下有效。</li>
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
