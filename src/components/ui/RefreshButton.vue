<script setup lang="ts">
/**
 * RefreshButton.vue — 全局统一的刷新图标按钮（原子组件）。
 *
 * 动画契约（「整圈停止」）：
 * - 点击后必转满至少一整圈；
 * - loading 仍为 true 时每到圈界再转一圈；
 * - loading 结束后在当前圈边界（0°）干净停下，不骤停在半圈；
 * - 仅点击触发旋转——后台轮询把 loading 顶成 true 不会自转。
 *
 * 实现：CSS keyframes + animationiteration 事件（不用 WAAPI，保老 WebView）。
 * bare 模式不附带按钮外壳，供自带按钮样式的场景（diary-icon-button 等）复用动画。
 */
import { ref, watch } from 'vue';
import { RefreshCw } from 'lucide-vue-next';

const props = withDefaults(
  defineProps<{
    /** 加载中标识（决定何时请求停止旋转）。 */
    loading?: boolean;
    /** 无障碍与悬浮提示文案（同时作 aria-label 和 title）。 */
    label: string;
    /** 图标尺寸（px）。 */
    size?: number;
    disabled?: boolean;
    /** 不附带默认 40×40 外壳样式（消费方通过 class 自定义）。 */
    bare?: boolean;
  }>(),
  { loading: false, size: 17, disabled: false, bare: false },
);

const emit = defineEmits<{ refresh: [] }>();

const spinning = ref(false);
/** 到下一个圈边界是否停止。 */
let stopRequested = false;

function onClick(): void {
  if (props.disabled) return;
  spinning.value = true;
  // 已在加载中 → 转到加载结束；否则保底一圈即停（除非随后 loading 拉起）
  stopRequested = !props.loading;
  emit('refresh');
}

watch(
  () => props.loading,
  (loading) => {
    if (!spinning.value) return;
    stopRequested = !loading;
  },
);

/** 圈边界：有停止请求则停下（恰好回到 0°）。 */
function onIteration(): void {
  if (stopRequested) spinning.value = false;
}
</script>

<template>
  <button
    type="button"
    :class="bare ? undefined : 'ub-refresh-btn'"
    :aria-label="label"
    :title="label"
    :disabled="disabled"
    @click="onClick"
  >
    <RefreshCw
      :size="size"
      :class="{ 'ub-refresh-spinning': spinning }"
      @animationiteration="onIteration"
    />
  </button>
</template>

<style scoped>
.ub-refresh-btn {
  width: 40px;
  height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.65;
  flex-shrink: 0;
}

.ub-refresh-btn:active {
  opacity: 1;
}

.ub-refresh-btn:disabled {
  opacity: 0.3;
}

.ub-refresh-spinning {
  animation: ub-refresh-rotate 0.8s linear infinite;
  transform-origin: center;
}

@keyframes ub-refresh-rotate {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
</style>
