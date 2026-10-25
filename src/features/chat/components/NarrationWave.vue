<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted, nextTick } from 'vue';

const props = withDefaults(defineProps<{
  active?: boolean;
}>(), {
  active: true,
});

const canvasRef = ref<HTMLCanvasElement | null>(null);
let animationFrameId: number | null = null;
let resizeObserver: ResizeObserver | null = null;

// 柱子高度数组与阶段
let barsHeights: number[] = [];
let targetHeights: number[] = [];
let numBars = 0;
let phase = 0;
let startTime = 0;

// 跃动持续时间 (毫秒)，跃动后平滑平息到目标随机起伏状态
const JUMP_DURATION_MS = 1300;

// 生成自然且起伏适中的随机声波波形 (高度在 1.2px 到 4.2px 物理像素间，避免遮挡输入框)
const generateTargetHeights = (count: number, height: number, dpr: number) => {
  const targets = new Array<number>(count);
  const minH = 1.2 * dpr;
  const maxH = Math.min(height * 0.75, 4.2 * dpr);
  const range = maxH - minH;

  // 使用多频复合正弦波结合微量随机，生成连续起伏的波浪切片
  const rPhase1 = Math.random() * Math.PI * 2;
  const rPhase2 = Math.random() * Math.PI * 2;
  const freq1 = 1.2 + Math.random() * 0.8;
  const freq2 = 2.6 + Math.random() * 1.2;

  for (let i = 0; i < count; i++) {
    const norm = count > 1 ? i / (count - 1) : 0.5;
    const s1 = Math.sin(norm * Math.PI * 2 * freq1 + rPhase1) * 0.5 + 0.5;
    const s2 = Math.sin(norm * Math.PI * 2 * freq2 + rPhase2) * 0.3 + 0.5;
    const jitter = (Math.random() - 0.5) * 0.15;
    const combined = Math.max(0, Math.min(1, s1 * 0.65 + s2 * 0.35 + jitter));
    targets[i] = minH + combined * range;
  }
  return targets;
};

const stopAnimation = () => {
  if (animationFrameId !== null) {
    cancelAnimationFrame(animationFrameId);
    animationFrameId = null;
  }
};

const startAnimation = () => {
  stopAnimation();
  startTime = performance.now();

  const canvas = canvasRef.value;
  if (!canvas) return;

  const dpr = window.devicePixelRatio || 1;
  const width = canvas.clientWidth * dpr;
  const height = canvas.clientHeight * dpr;
  if (width <= 0 || height <= 0) return;

  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  // 短杠配置：短宽度 (2.2px) + 小间距 (1.5px)，形成高密度的精致短杠音波
  const spacing = 1.5 * dpr;
  const barWidth = 2.2 * dpr;
  const step = barWidth + spacing;
  numBars = Math.max(8, Math.floor((width + spacing) / step));

  targetHeights = generateTargetHeights(numBars, height, dpr);
  barsHeights = new Array(numBars).fill(1.2 * dpr);
  phase = Math.random() * Math.PI;

  const render = (now: number) => {
    animationFrameId = null;
    const cvs = canvasRef.value;
    if (!cvs) return;
    const ctx = cvs.getContext('2d');
    if (!ctx) return;

    const w = cvs.width;
    const h = cvs.height;
    if (w === 0 || h === 0) return;

    ctx.clearRect(0, 0, w, h);

    // 绘制彩虹霓虹渐变
    const grad = ctx.createLinearGradient(0, 0, w, 0);
    grad.addColorStop(0.00, '#a855f7'); // 魅紫
    grad.addColorStop(0.20, '#6366f1'); // 靛蓝
    grad.addColorStop(0.40, '#06b6d4'); // 青绿
    grad.addColorStop(0.60, '#10b981'); // 翠绿
    grad.addColorStop(0.80, '#f59e0b'); // 琥珀黄
    grad.addColorStop(1.00, '#f43f5e'); // 玫瑰桃红
    ctx.fillStyle = grad;

    const elapsed = now - startTime;
    const isJumping = elapsed < JUMP_DURATION_MS;

    if (isJumping) {
      phase += 0.14; // 正弦波流动速度
    }

    let maxDiff = 0;
    const dprVal = window.devicePixelRatio || 1;
    const maxJumpH = Math.min(h * 0.8, 4.6 * dprVal);
    const minJumpH = 1.2 * dprVal;

    for (let i = 0; i < numBars; i++) {
      if (isJumping) {
        // 跃动状态：正弦波行波流动起伏
        const angle = (i / numBars) * Math.PI * 2 * 2.2 - phase;
        const normWave = (Math.sin(angle) + 1) * 0.5; // 0 ~ 1
        const liveTarget = minJumpH + normWave * (maxJumpH - minJumpH);
        barsHeights[i] += (liveTarget - barsHeights[i]) * 0.28;
      } else {
        // 逐渐平息并稳定在随机起伏的静止状态
        const diff = targetHeights[i] - barsHeights[i];
        barsHeights[i] += diff * 0.14;
        maxDiff = Math.max(maxDiff, Math.abs(diff));
      }

      const x = i * step;
      const barH = Math.max(1 * dprVal, barsHeights[i]);
      const y = h - barH;

      // 绘制圆角小短条
      const radius = Math.min(barWidth / 2, barH / 2);
      ctx.beginPath();
      if (typeof ctx.roundRect === 'function') {
        ctx.roundRect(x, y, barWidth, barH, radius);
      } else {
        ctx.rect(x, y, barWidth, barH);
      }
      ctx.fill();
    }

    // 跃动已结束且全部柱子已经平滑收敛到目标形态，停止动画以节省能耗
    if (!isJumping && maxDiff < 0.08 * dprVal) {
      return;
    }

    animationFrameId = requestAnimationFrame(render);
  };

  animationFrameId = requestAnimationFrame(render);
};

const handleResize = () => {
  if (props.active) {
    startAnimation();
  }
};

watch(() => props.active, async (active) => {
  if (active) {
    await nextTick();
    startAnimation();
  } else {
    stopAnimation();
  }
}, { immediate: true });

onMounted(async () => {
  await nextTick();
  if (canvasRef.value) {
    resizeObserver = new ResizeObserver(() => {
      handleResize();
    });
    resizeObserver.observe(canvasRef.value);
  }
  if (props.active) {
    startAnimation();
  }
});

onUnmounted(() => {
  stopAnimation();
  if (resizeObserver) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
});
</script>

<template>
  <div class="vcp-narration-wave w-full h-[5px] relative pointer-events-none overflow-hidden opacity-90">
    <canvas ref="canvasRef" class="w-full h-full block pointer-events-none"></canvas>
  </div>
</template>

<style scoped>
.vcp-narration-wave {
  filter: drop-shadow(0 0 3px rgba(168, 85, 247, 0.25));
}

@media (prefers-reduced-motion: reduce) {
  .vcp-narration-wave {
    transition: none !important;
  }
}
</style>
