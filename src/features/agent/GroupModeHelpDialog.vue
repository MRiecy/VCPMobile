<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue';
import { useModalHistory } from '../../core/composables/useModalHistory';

/**
 * 群聊模式帮助说明弹窗。
 *
 * 入口：群组设置「群聊模式」区块标题右侧的问号按钮。
 * 内容覆盖四种发言模式的触发规则、Tag 匹配模式差异、以及
 * 「新成员默认 Tag 为名字」等隐式行为——让这些规则不读代码也可知。
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
    text: '每条消息触发成员按设置中的发言次序依次回复。使用上移、下移调整次序；新加入的成员排在末尾。',
  },
  {
    label: '自然随机',
    text: '按优先级决定谁发言：@名字 ＞ Tag 命中 ＞ @所有人 ＞ 15% 随机概率 ＞ 保底一人。Tag 在成员列表中设置，多个 Tag 用逗号分隔。',
  },
  {
    label: '邀请发言',
    text: '成员不会自动回复。点按输入框上方「邀其发言」横条指定成员，或在消息中 @成员 后发送——被提及的成员将按出现顺序自动受邀发言。',
  },
  {
    label: 'JEV 智能群聊',
    text: 'JEV 根据最近对话与成员风格，选择有必要发言的成员并判断何时结束。发起群聊先由一位成员开场，继续群聊重新裁决当前历史，点击成员可加入发言队列。需要先在全局设置的「JEV 决策」中配置服务。',
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
                <li><span class="font-semibold text-primary-text">严格模式</span>：上下文或消息包含 Tag 即触发，无需 @。</li>
                <li><span class="font-semibold text-primary-text">自然模式</span>：区分 Tag 来源，避免 Agent 因引用自己的历史发言而循环触发。</li>
                <li>新加入的成员默认以「名字」作为 Tag（可手动清空），因此严格模式下不带 @ 的名字提及也会触发该成员。</li>
                <li>@所有人 仅在自然随机模式下有效。</li>
                <li>本机模式、发言次序和 JEV 策略只影响这台设备。切换模式会保留其他模式的设置。</li>
                <li>JEV 裁决历史提供给裁判；成员模型上下文提供给实际回复的助手。两者独立，均不删除完整聊天记录。</li>
                <li>停止群组发言会中断当前回复、正在进行的裁决以及后续队列。</li>
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
