<script setup lang="ts">
/**
 * GuideOverlay — 全局唯一教学引导覆盖层（z-guide = 95）
 *
 * 职责：
 * - 黑纱 + 聚光高亮（box-shadow 挖洞，无 backdrop-filter / SVG mask）；
 * - 指引卡片（[n/m] 计数、标题、正文、「下一步 / 我知道了」两枚按钮）；
 * - 几何管线：scrollIntoView 居中 → rect 稳定采样 → 四向方位自动翻转，
 *   约束在 --vcp-safe-* insets 内；ResizeObserver / resize / 键盘 Insets /
 *   生命周期 resume 均触发重算；
 * - 目标不可解析：waitFor 轮询 100ms / waitTimeoutMs 超时静默越过该步；
 * - 全屏拦截指针事件（touch-action: none），教学期间用户手势零副作用；
 *   步骤可配置 perform（真实安全业务：打开菜单/面板）与 undo（整场收尾清理）。
 *
 * 由 App.vue 在 lifecycle READY 后挂载；mounted 时调用 guideStore.init()。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useGuideStore } from '../stores/guideStore';
import { resolveTarget } from '../registry';
import '../guides';
import GuideDemoAnimation from './GuideDemoAnimation.vue';
import type { GuidePlacement, GuideStep } from '../types';

const WAIT_POLL_MS = 100;
const STABILITY_GAP_MS = 120;
const CARD_GAP = 12;
const CARD_MAX_WIDTH = 320;
const SAFE_MARGIN = 8;

interface SpotRect {
  top: number;
  left: number;
  width: number;
  height: number;
  right: number;
  bottom: number;
}

const guideStore = useGuideStore();

const isActive = computed(() => guideStore.activeGuideId !== null);
const step = computed(() => guideStore.activeStep);
const stepKey = computed(
  () => `${guideStore.activeGuideId ?? 'none'}:${guideStore.activeStepIndex}`,
);
const isLastStep = computed(() => {
  const guide = guideStore.activeGuide;
  return !!guide && guideStore.activeStepIndex >= guide.steps.length - 1;
});

const cardEl = ref<HTMLElement | null>(null);
const spotRect = ref<SpotRect | null>(null);
const cardStyle = ref<Record<string, string>>({ top: '-9999px', left: '-9999px' });
/** 卡片完成首次定位后才启用位置过渡（避免 -9999px 占位处飞入）。 */
const cardPlaced = ref(false);
const fallbackVeil = computed(() => isActive.value && spotRect.value === null);

const spotStyle = computed<Record<string, string>>(() => {
  const r = spotRect.value;
  if (!r) return {} as Record<string, string>;
  return {
    top: `${r.top - 4}px`,
    left: `${r.left - 4}px`,
    width: `${r.width + 8}px`,
    height: `${r.height + 8}px`,
  };
});

let targetEl: HTMLElement | null = null;
let waitTimer: ReturnType<typeof setTimeout> | null = null;
let stabilityTimer: ReturnType<typeof setTimeout> | null = null;
let stepToken = 0;
let waitDeadline = 0;
let stepTimeoutMs = 3000;
let lastSample: SpotRect | null = null;
let targetObserver: ResizeObserver | null = null;
let cardObserver: ResizeObserver | null = null;
let lifecycleUnlisten: UnlistenFn | null = null;
let roInitialCallback = false;
/** 本场指引已执行 perform 的步骤 undo 栈（整场结束时逆序执行）。 */
let undoStack: Array<() => void> = [];

const runUndoStack = () => {
  const stack = undoStack;
  undoStack = [];
  for (let i = stack.length - 1; i >= 0; i -= 1) {
    try {
      stack[i]();
    } catch (e) {
      console.warn('[Guide] undo() failed:', e);
    }
  }
};

const clearTimers = () => {
  if (waitTimer !== null) {
    clearTimeout(waitTimer);
    waitTimer = null;
  }
  if (stabilityTimer !== null) {
    clearTimeout(stabilityTimer);
    stabilityTimer = null;
  }
};

const rectOf = (el: HTMLElement): SpotRect => {
  const r = el.getBoundingClientRect();
  return {
    top: r.top,
    left: r.left,
    width: r.width,
    height: r.height,
    right: r.right,
    bottom: r.bottom,
  };
};

const sameRect = (a: SpotRect, b: SpotRect): boolean =>
  Math.abs(a.top - b.top) < 1 &&
  Math.abs(a.left - b.left) < 1 &&
  Math.abs(a.width - b.width) < 1 &&
  Math.abs(a.height - b.height) < 1;

const isInViewport = (r: SpotRect): boolean =>
  r.top >= -1 &&
  r.left >= -1 &&
  r.right <= window.innerWidth + 1 &&
  r.bottom <= window.innerHeight + 1;

const readSafeInsets = () => {
  const style = getComputedStyle(document.documentElement);
  const parse = (name: string): number => {
    const raw = style.getPropertyValue(name);
    const value = parseFloat(raw);
    return Number.isNaN(value) ? 0 : value;
  };
  return {
    top: parse('--vcp-safe-top'),
    right: parse('--vcp-safe-right'),
    bottom: parse('--vcp-safe-bottom'),
    left: parse('--vcp-safe-left'),
  };
};

const scrollIntoViewCenter = (el: HTMLElement) => {
  if (typeof el.scrollIntoView !== 'function') return;
  try {
    el.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'auto' });
  } catch {
    /* 旧 WebView fallback：滚动失败不阻塞教学 */
  }
};

const targetIdOf = (): string | null => {
  const s = step.value;
  if (!s) return null;
  try {
    return typeof s.target === 'function' ? s.target() : s.target;
  } catch {
    return null;
  }
};

const clampX = (x: number, cardW: number, insets: { left: number; right: number }): number => {
  const maxLeft = Math.max(
    insets.left + SAFE_MARGIN,
    window.innerWidth - insets.right - SAFE_MARGIN - cardW,
  );
  return Math.min(Math.max(x, insets.left + SAFE_MARGIN), maxLeft);
};

const clampY = (y: number, cardH: number, insets: { top: number; bottom: number }): number => {
  const maxTop = Math.max(
    insets.top + SAFE_MARGIN,
    window.innerHeight - insets.bottom - SAFE_MARGIN - cardH,
  );
  return Math.min(Math.max(y, insets.top + SAFE_MARGIN), maxTop);
};

/** 卡片方位：首选 → 四向翻转 → 全不满足时取空间最大侧并钳制进安全区。 */
const positionCard = (r: SpotRect | null) => {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const insets = readSafeInsets();
  const measuredW = cardEl.value?.offsetWidth ?? 0;
  const measuredH = cardEl.value?.offsetHeight ?? 0;
  const cardW =
    measuredW > 0
      ? measuredW
      : Math.min(CARD_MAX_WIDTH, Math.max(120, vw - insets.left - insets.right - SAFE_MARGIN * 2));
  const cardH = measuredH > 0 ? measuredH : 140;

  if (!r) {
    cardStyle.value = {
      top: `${Math.max(insets.top + SAFE_MARGIN, (vh - cardH) / 2)}px`,
      left: `${clampX((vw - cardW) / 2, cardW, insets)}px`,
    };
    return;
  }

  const preferred: GuidePlacement = step.value?.placement ?? 'bottom';
  const order: GuidePlacement[] =
    preferred === 'top'
      ? ['top', 'bottom', 'left', 'right']
      : preferred === 'left'
        ? ['left', 'right', 'bottom', 'top']
        : preferred === 'right'
          ? ['right', 'left', 'bottom', 'top']
          : ['bottom', 'top', 'left', 'right'];

  const fitsAt = (p: GuidePlacement): boolean => {
    if (p === 'bottom') return r.bottom + CARD_GAP + cardH <= vh - insets.bottom - SAFE_MARGIN;
    if (p === 'top') return r.top - CARD_GAP - cardH >= insets.top + SAFE_MARGIN;
    if (p === 'right') return r.right + CARD_GAP + cardW <= vw - insets.right - SAFE_MARGIN;
    return r.left - CARD_GAP - cardW >= insets.left + SAFE_MARGIN;
  };

  let chosen = order.find(fitsAt);
  if (!chosen) {
    const spaces: Record<GuidePlacement, number> = {
      top: r.top - insets.top,
      bottom: vh - r.bottom - insets.bottom,
      left: r.left - insets.left,
      right: vw - r.right - insets.right,
    };
    let best: GuidePlacement = order[0];
    for (const p of order) {
      if (spaces[p] > spaces[best]) best = p;
    }
    chosen = best;
  }

  let top = 0;
  let left = 0;
  if (chosen === 'bottom') {
    top = r.bottom + CARD_GAP;
    left = clampX(r.left + r.width / 2 - cardW / 2, cardW, insets);
  } else if (chosen === 'top') {
    top = Math.max(insets.top + SAFE_MARGIN, r.top - CARD_GAP - cardH);
    left = clampX(r.left + r.width / 2 - cardW / 2, cardW, insets);
  } else if (chosen === 'left') {
    left = Math.max(insets.left + SAFE_MARGIN, r.left - CARD_GAP - cardW);
    top = clampY(r.top + r.height / 2 - cardH / 2, cardH, insets);
  } else {
    left = r.right + CARD_GAP;
    top = clampY(r.top + r.height / 2 - cardH / 2, cardH, insets);
  }
  cardStyle.value = { top: `${top}px`, left: `${left}px` };
};

const skipStep = (token: number) => {
  if (token !== stepToken) return;
  clearTimers();
  guideStore.next();
};

const schedulePoll = (token: number, id: string, s: GuideStep) => {
  if (Date.now() >= waitDeadline) {
    skipStep(token);
    return;
  }
  waitTimer = setTimeout(() => pollTarget(token, id, s), WAIT_POLL_MS);
};

const observeTarget = (el: HTMLElement, token: number, s: GuideStep) => {
  targetObserver?.disconnect();
  targetObserver = null;
  if (typeof ResizeObserver === 'undefined') return;
  roInitialCallback = true;
  const observer = new ResizeObserver(() => {
    // 观察建立时的首次回调不触发重采样，防止「重采样 → 重建观察」死循环。
    if (roInitialCallback) {
      roInitialCallback = false;
      return;
    }
    if (token !== stepToken) return;
    clearTimers();
    lastSample = null;
    startStabilitySampling(token, s);
  });
  observer.observe(el);
  targetObserver = observer;
};

const startStabilitySampling = (token: number, s: GuideStep) => {
  const sample = () => {
    if (token !== stepToken) return;
    if (!targetEl || !targetEl.isConnected) {
      const id = targetIdOf();
      if (id) schedulePoll(token, id, s);
      else skipStep(token);
      return;
    }
    const r = rectOf(targetEl);
    if (lastSample && sameRect(lastSample, r) && isInViewport(r)) {
      spotRect.value = r;
      observeTarget(targetEl, token, s);
      positionCard(r);
      return;
    }
    lastSample = r;
    if (Date.now() >= waitDeadline) {
      skipStep(token);
      return;
    }
    stabilityTimer = setTimeout(sample, STABILITY_GAP_MS);
  };
  sample();
};

const pollTarget = (token: number, id: string, s: GuideStep) => {
  if (token !== stepToken) return;
  if (s.waitFor) {
    let gateOk = false;
    try {
      gateOk = s.waitFor() === true;
    } catch {
      gateOk = false;
    }
    if (!gateOk) {
      schedulePoll(token, id, s);
      return;
    }
  }
  const el = resolveTarget(id);
  if (!el) {
    schedulePoll(token, id, s);
    return;
  }
  targetEl = el;
  scrollIntoViewCenter(el);
  startStabilitySampling(token, s);
};

const beginStep = () => {
  stepToken += 1;
  const token = stepToken;
  clearTimers();
  targetEl = null;
  lastSample = null;
  // 保留旧 spotRect：步间推进不闪黑纱，新目标就绪后经 CSS 过渡滑移聚焦。
  targetObserver?.disconnect();
  targetObserver = null;

  const s = step.value;
  if (!s) {
    skipStep(token);
    return;
  }
  // 步骤级真实业务（安全类：打开菜单/面板），先于目标轮询执行；
  // 异常吞掉不阻塞教学；undo 入栈，整场结束时统一逆序清理。
  try {
    s.perform?.();
  } catch (e) {
    console.warn('[Guide] perform() failed:', e);
  }
  if (s.undo) undoStack.push(s.undo);

  const id = targetIdOf();
  if (!id) {
    skipStep(token);
    return;
  }
  stepTimeoutMs = s.waitTimeoutMs ?? 3000;
  waitDeadline = Date.now() + stepTimeoutMs;
  pollTarget(token, id, s);
};

const restartMeasure = () => {
  if (!step.value || !targetEl) return;
  const s = step.value;
  const token = stepToken;
  clearTimers();
  waitDeadline = Date.now() + stepTimeoutMs;
  lastSample = null;
  startStabilitySampling(token, s);
};

const bindCardRef = (el: unknown) => {
  const node = el as HTMLElement | null;
  // Vue 对函数 ref 在每次元素补丁时都会回调（无身份短路）；
  // 同元素重复回调必须直接返回，否则 positionCard 每次改写 cardStyle
  // 会形成「渲染 → 回调 → 改写 → 渲染」的递归更新。
  if (cardEl.value === node) return;
  cardEl.value = node;
  cardObserver?.disconnect();
  cardObserver = null;
  if (node && typeof ResizeObserver !== 'undefined') {
    const observer = new ResizeObserver(() => {
      if (isActive.value) positionCard(spotRect.value);
    });
    observer.observe(node);
    cardObserver = observer;
    // 挂载后立即定位一次（消除初始 -9999px 占位），随后启用位置过渡。
    positionCard(spotRect.value);
    cardPlaced.value = true;
  }
};

watch(
  () => `${guideStore.activeGuideId ?? 'none'}:${guideStore.activeStepIndex}`,
  () => {
    if (guideStore.activeGuideId !== null) beginStep();
  },
);

watch(isActive, (active) => {
  // 注意：active 分支不做任何 undo 栈清理——本 watcher 在 stepKey
  // watcher（beginStep → perform 入栈）之后运行，此处清栈会误删首步 undo；
  // 上一场结束时 runUndoStack 已保证开播前栈为空。
  if (active) return;
  stepToken += 1;
  clearTimers();
  targetEl = null;
  lastSample = null;
  spotRect.value = null;
  cardPlaced.value = false;
  targetObserver?.disconnect();
  targetObserver = null;
  // 真实业务收尾：关闭本场打开过的菜单/面板/滑开状态。
  runUndoStack();
});

onMounted(() => {
  guideStore.init();
  window.addEventListener('resize', restartMeasure);
  window.addEventListener('vcp-keyboard-inset', restartMeasure);
  listen<{ state: string }>('vcp-lifecycle-changed', (event) => {
    if (event.payload?.state === 'resume') restartMeasure();
  })
    .then((unlisten) => {
      lifecycleUnlisten = unlisten;
    })
    .catch(() => {
      /* 非 Tauri 环境无生命周期事件，忽略 */
    });
});

onBeforeUnmount(() => {
  window.removeEventListener('resize', restartMeasure);
  window.removeEventListener('vcp-keyboard-inset', restartMeasure);
  lifecycleUnlisten?.();
  clearTimers();
  targetObserver?.disconnect();
  cardObserver?.disconnect();
});
</script>

<template>
  <Transition name="guide-fade">
    <div
      v-if="isActive"
      class="guide-overlay fixed inset-0 z-guide select-none"
      @click.prevent
      @contextmenu.prevent
      @wheel.prevent
      @touchstart.prevent
      @touchmove.prevent
      @touchend.prevent
      @touchcancel.prevent
    >
      <!-- 目标未就绪时的兜底暗纱（仅在本场指引尚未解析出任何 rect 前显示；
           出现与让位均走淡入淡出，配合聚光收敛避免瞬间全黑闪烁）
           注意：条件兄弟节点必须带 key，防止无 key 索引匹配在重渲染时
           卸载/重挂载卡片 vnode，进而通过模板 ref 改写 cardStyle 形成递归更新。 -->
      <Transition name="guide-veil-fade">
        <div
          v-if="fallbackVeil"
          key="guide-veil"
          class="guide-veil absolute inset-0 pointer-events-none"
        />
      </Transition>

      <!-- 聚光高亮：veil 层（box-shadow 挖洞 + 入场收敛动画）
           + frame 层（2px Accent 描边，不随收敛缩放保持清晰）；
           步间推进时两层经 top/left/width/height 过渡平滑滑移到新目标。 -->
      <div v-if="spotRect" key="guide-spot" class="guide-spot pointer-events-none" :style="spotStyle">
        <div class="guide-spot-veil" />
        <div class="guide-spot-frame" />
        <GuideDemoAnimation
          v-if="step && step.demo"
          :key="stepKey"
          :demo="step.demo"
        />
      </div>

      <!-- 指引卡片：pointer-events 独立于目标解析状态，按钮始终可点 -->
      <div
        v-if="step"
        key="guide-card"
        :ref="bindCardRef"
        class="guide-card pointer-events-auto"
        :class="{ 'guide-card--placed': cardPlaced }"
        :style="cardStyle"
        @click.stop
        @touchstart.stop
        @touchmove.stop
        @touchend.stop
        @touchcancel.stop
      >
        <div class="guide-card-head">
          <span class="guide-card-step">[{{ guideStore.activeStepIndex + 1 }}/{{ guideStore.activeGuide?.steps.length }}]</span>
          <span class="guide-card-title">{{ step.title }}</span>
        </div>
        <p class="guide-card-content">{{ step.content }}</p>
        <div class="guide-card-actions">
          <button v-if="!isLastStep" class="guide-btn" @click="guideStore.next()">下一步</button>
          <button v-else class="guide-btn guide-btn--primary" @click="guideStore.finish()">我知道了</button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
.guide-overlay {
  touch-action: none;
  overscroll-behavior: contain;
}

.guide-spot {
  position: absolute;
  /* 步间/目标移动时平滑滑移：阴影逐渐位移聚焦到新目标 */
  transition:
    top 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    left 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    width 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    height 0.26s cubic-bezier(0.4, 0, 0.2, 1);
}

/* 挖洞暗化层：入场时洞从约 8 倍大小收敛到目标，
   阴影自四周逐渐聚焦到元素上（transform/box-shadow 合成动画）。 */
.guide-spot-veil {
  position: absolute;
  inset: 0;
  border-radius: 12px;
  transform-origin: center;
  animation: guide-spot-converge 0.42s ease-out both;
}

@keyframes guide-spot-converge {
  from {
    transform: scale(8);
    box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0);
  }
  55% {
    box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.4);
  }
  to {
    transform: scale(1);
    box-shadow: 0 0 0 9999px rgba(0, 0, 0, 0.55);
  }
}

/* 2px Accent 描边层：不参与缩放，收敛期间淡入，保持边缘清晰 */
.guide-spot-frame {
  position: absolute;
  inset: 0;
  border: 2px solid var(--highlight-text, #3b82f6);
  border-radius: 12px;
  animation: guide-frame-in 0.26s ease-out both;
}

@keyframes guide-frame-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

.guide-veil {
  background: rgba(0, 0, 0, 0.55);
}

/* 兜底暗纱淡入 + 让位时淡出（与收敛动画叠加，暗度平滑转移而非瞬间切换） */
.guide-veil-fade-enter-active {
  transition: opacity 0.15s ease-out;
}

.guide-veil-fade-leave-active {
  transition: opacity 0.35s ease-out;
}

.guide-veil-fade-enter-from,
.guide-veil-fade-leave-to {
  opacity: 0;
}

.guide-card {
  position: absolute;
  width: min(320px, calc(100vw - var(--vcp-safe-left, 0px) - var(--vcp-safe-right, 0px) - 16px));
  background: var(--vcp-panel-bg-97, var(--secondary-bg));
  border: 1px solid var(--border-color);
  border-radius: 12px;
  padding: 12px 14px;
  box-shadow: 0 6px 16px rgba(0, 0, 0, 0.16);
  color: var(--primary-text);
  animation: guide-card-in 0.2s ease-out both;
}

/* 完成首次定位后启用位置过渡（与聚光同步滑移，避免 -9999px 飞入） */
.guide-card--placed {
  transition:
    top 0.26s cubic-bezier(0.4, 0, 0.2, 1),
    left 0.26s cubic-bezier(0.4, 0, 0.2, 1);
}

@keyframes guide-card-in {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.guide-card-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.guide-card-step {
  font-family: monospace;
  font-size: 10px;
  opacity: 0.6;
  letter-spacing: 0.05em;
  white-space: nowrap;
}

.guide-card-title {
  font-weight: 700;
  font-size: 13px;
}

.guide-card-content {
  margin: 6px 0 12px;
  font-size: 12px;
  line-height: 1.6;
  opacity: 0.85;
}

.guide-card-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.guide-btn {
  padding: 7px 16px;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  transition: opacity 0.15s;
  cursor: pointer;
}

.guide-btn:active {
  opacity: 0.7;
}

.guide-btn--primary {
  background: var(--highlight-text, #3b82f6);
  border-color: transparent;
  color: #fff;
}

.guide-fade-enter-active {
  transition: opacity 0.15s ease-out;
}

.guide-fade-leave-active {
  transition: opacity 0.2s ease-in;
}

.guide-fade-enter-from,
.guide-fade-leave-to {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .guide-card {
    animation: none;
  }
  .guide-spot,
  .guide-card--placed {
    transition: none;
  }
  .guide-spot-veil,
  .guide-spot-frame {
    animation: none;
    opacity: 1;
  }
}
</style>
