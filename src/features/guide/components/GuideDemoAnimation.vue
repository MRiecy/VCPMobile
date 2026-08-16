<script setup lang="ts">
/**
 * GuideDemoAnimation — 手势演示动画原语（图标化版本）
 *
 * 设计约束（真机反馈轮修订）：
 * - 手指改为现成图标（@iconify-json/ph 手势图标，走 UnoCSS 现有管线），
 *   不再手绘"圆+长柱"；图标置于柔和光晕圆底上（radial-gradient，无边框盒）；
 * - 删除假示意卡 / 假行 / 假设置 chip——真实业务（若步骤配置 perform）
 *   由真实 UI 呈现，演示层不再叠加"多框"视觉元素；
 * - press-hold 进度环 600ms 对齐 v-longpress 阈值；
 * - 纯 CSS keyframes + transform/opacity only；循环 2 次后定格；
 * - 全程 pointer-events: none；受 prefers-reduced-motion 降级。
 */
import type { GuideDemo } from '../types';

defineProps<{
  demo: GuideDemo;
}>();
</script>

<template>
  <div class="guide-demo pointer-events-none" aria-hidden="true">
    <!-- 长按：手势图标 + 600ms 进度环 -->
    <template v-if="demo === 'press-hold'">
      <div class="demo-icon demo-icon--press">
        <div class="i-ph-hand-tap demo-icon-glyph" />
      </div>
      <svg class="demo-ring" viewBox="0 0 48 48" width="48" height="48">
        <circle class="demo-ring-track" cx="24" cy="24" r="21" />
        <circle class="demo-ring-progress" cx="24" cy="24" r="21" />
      </svg>
    </template>

    <!-- 单击：手势图标短按 pulse -->
    <template v-else-if="demo === 'tap'">
      <div class="demo-icon demo-icon--tap">
        <div class="i-ph-hand-tap demo-icon-glyph" />
      </div>
    </template>

    <!-- 右滑：手势图标右移（真实行由 perform 驱动真实滑开） -->
    <template v-else-if="demo === 'swipe-right'">
      <div class="demo-icon demo-icon--swipe">
        <div class="i-ph-hand-swipe-right demo-icon-glyph" />
      </div>
    </template>

    <!-- 纵向拖拽：手势图标上下轨迹 -->
    <template v-else-if="demo === 'drag-vertical'">
      <div class="demo-icon demo-icon--drag">
        <div class="i-ph-hand-fist demo-icon-glyph" />
      </div>
    </template>
  </div>
</template>

<style scoped>
.guide-demo {
  position: absolute;
  inset: 0;
  overflow: visible;
}

.demo-icon {
  position: absolute;
  left: 50%;
  top: 50%;
  z-index: 2;
  opacity: 0;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* 柔和光晕圆底：增强图标在亮/暗背景上的可读性（非边框盒） */
.demo-icon::before {
  content: '';
  position: absolute;
  inset: -9px;
  border-radius: 50%;
  background: radial-gradient(
    circle,
    rgba(59, 130, 246, 0.3) 0%,
    rgba(59, 130, 246, 0.12) 55%,
    rgba(59, 130, 246, 0) 72%
  );
  z-index: -1;
}

.demo-icon-glyph {
  width: 34px;
  height: 34px;
  color: #ffffff;
  filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.55));
}

.demo-ring {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  z-index: 1;
}

.demo-ring-track {
  fill: none;
  stroke: rgba(200, 200, 200, 0.3);
  stroke-width: 2;
}

.demo-ring-progress {
  fill: none;
  stroke: var(--highlight-text, #3b82f6);
  stroke-width: 2;
  stroke-linecap: round;
  stroke-dasharray: 132;
  stroke-dashoffset: 132;
  transform: rotate(-90deg);
  transform-origin: center;
  animation: guide-demo-ring 3s linear 2 both;
}

/* 600ms 走满一圈对齐 v-longpress 阈值，其余时间保持满环 */
@keyframes guide-demo-ring {
  0%, 22% { stroke-dashoffset: 132; }
  42%, 100% { stroke-dashoffset: 0; }
}

.demo-icon--press {
  transform: translate(-50%, -50%);
  animation: guide-demo-press-finger 3s ease-in-out 2 both;
}

@keyframes guide-demo-press-finger {
  0% { opacity: 0; transform: translate(-50%, -50%) translateY(-36px); }
  10% { opacity: 1; transform: translate(-50%, -50%) translateY(-36px); }
  22% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
  90%, 100% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
}

.demo-icon--tap {
  transform: translate(-50%, -50%);
  animation: guide-demo-tap 1.4s ease-in-out 2 both;
}

@keyframes guide-demo-tap {
  0% { opacity: 0; transform: translate(-50%, -50%) scale(1.15); }
  30% { opacity: 1; transform: translate(-50%, -50%) scale(0.92); }
  45%, 100% { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

.demo-icon--swipe {
  transform: translate(-50%, -50%);
  animation: guide-demo-swipe-finger 1.6s ease-in-out 2 both;
}

@keyframes guide-demo-swipe-finger {
  0% { opacity: 0; transform: translate(-50%, -50%) translateX(-10px); }
  8% { opacity: 1; transform: translate(-50%, -50%) translateX(-10px); }
  45%, 60% { opacity: 1; transform: translate(-50%, -50%) translateX(72px); }
  80%, 100% { opacity: 0; transform: translate(-50%, -50%) translateX(0); }
}

.demo-icon--drag {
  transform: translate(-50%, -50%);
  animation: guide-demo-drag-finger 3.2s ease-in-out 2 both;
}

@keyframes guide-demo-drag-finger {
  0% { opacity: 0; transform: translate(-50%, -50%) translateY(-32px); }
  8% { opacity: 1; transform: translate(-50%, -50%) translateY(-32px); }
  18% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
  35% { opacity: 1; transform: translate(-50%, -50%) translateY(56px); }
  50% { opacity: 1; transform: translate(-50%, -50%) translateY(-56px); }
  65%, 100% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
}

@media (prefers-reduced-motion: reduce) {
  .demo-ring-progress,
  .demo-icon--press,
  .demo-icon--tap,
  .demo-icon--swipe,
  .demo-icon--drag {
    animation: none;
    opacity: 1;
  }
}
</style>
