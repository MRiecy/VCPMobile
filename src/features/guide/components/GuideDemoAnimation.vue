<script setup lang="ts">
/**
 * GuideDemoAnimation — 纯视觉演示动画原语
 *
 * 设计约束（研究 01/05 文档）：
 * - 纯 CSS keyframes + 内联 SVG，零依赖；transform/opacity only；
 * - 动画循环 2 次后定格（iteration-count: 2 + fill both）；
 * - 全程 pointer-events: none，不触发任何真实业务；
 * - 受全局 `vcp-paused-animations` 暂停类控制（前后台自动暂停）。
 */
import { Settings } from 'lucide-vue-next';
import type { GuideDemo } from '../types';

defineProps<{
  demo: GuideDemo;
  hint?: string[];
}>();
</script>

<template>
  <div class="guide-demo pointer-events-none" aria-hidden="true">
    <!-- 长按：手指按下 + 600ms 进度环 + 示意小卡 -->
    <template v-if="demo === 'press-hold'">
      <div class="demo-finger demo-finger--press">
        <svg viewBox="0 0 40 40" width="34" height="34">
          <rect x="16" y="3" width="8" height="17" rx="4" class="demo-finger-shape" />
          <ellipse cx="20" cy="23" rx="13" ry="12" class="demo-finger-shape" />
        </svg>
      </div>
      <svg class="demo-ring" viewBox="0 0 48 48" width="48" height="48">
        <circle class="demo-ring-track" cx="24" cy="24" r="21" />
        <circle class="demo-ring-progress" cx="24" cy="24" r="21" />
      </svg>
      <div v-if="hint && hint.length" class="demo-hint-card">
        <span v-for="item in hint" :key="item" class="demo-hint-chip">{{ item }}</span>
      </div>
    </template>

    <!-- 右滑：手指右移 + 行位移露出设置入口（示意，真实行不动） -->
    <template v-else-if="demo === 'swipe-right'">
      <div class="demo-swipe-settings"><Settings :size="16" /></div>
      <div class="demo-ghost-row demo-ghost-row--swipe"></div>
      <div class="demo-finger demo-finger--swipe">
        <svg viewBox="0 0 40 40" width="34" height="34">
          <rect x="16" y="3" width="8" height="17" rx="4" class="demo-finger-shape" />
          <ellipse cx="20" cy="23" rx="13" ry="12" class="demo-finger-shape" />
        </svg>
      </div>
    </template>

    <!-- 纵向拖拽：按住 → 行上浮 → 上下轨迹 → 松手落位 -->
    <template v-else-if="demo === 'drag-vertical'">
      <div class="demo-ghost-row demo-ghost-row--drag"></div>
      <div class="demo-finger demo-finger--drag">
        <svg viewBox="0 0 40 40" width="34" height="34">
          <rect x="16" y="3" width="8" height="17" rx="4" class="demo-finger-shape" />
          <ellipse cx="20" cy="23" rx="13" ry="12" class="demo-finger-shape" />
        </svg>
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

.demo-finger-shape {
  fill: rgba(200, 200, 200, 0.6);
  stroke: rgba(0, 0, 0, 0.35);
  stroke-width: 1.5;
}

.demo-finger {
  position: absolute;
  left: 50%;
  top: 50%;
  opacity: 0;
  z-index: 2;
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

@keyframes guide-demo-ring {
  0%, 22% { stroke-dashoffset: 132; }
  42%, 100% { stroke-dashoffset: 0; }
}

.demo-finger--press {
  transform: translate(-50%, -50%);
  animation: guide-demo-press-finger 3s ease-in-out 2 both;
}

@keyframes guide-demo-press-finger {
  0% { opacity: 0; transform: translate(-50%, -50%) translateY(-36px); }
  10% { opacity: 1; transform: translate(-50%, -50%) translateY(-36px); }
  22% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
  90%, 100% { opacity: 1; transform: translate(-50%, -50%) translateY(0); }
}

.demo-hint-card {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, calc(-100% - 14px));
  display: flex;
  gap: 4px;
  padding: 6px 8px;
  border-radius: 8px;
  background: var(--vcp-panel-bg-97, var(--secondary-bg));
  border: 1px solid var(--border-color);
  opacity: 0;
  z-index: 3;
  animation: guide-demo-hint 3s ease-in-out 2 both;
}

.demo-hint-chip {
  font-size: 10px;
  line-height: 1;
  padding: 3px 6px;
  border-radius: 4px;
  background: var(--vcp-accent-bg-25, rgba(59, 130, 246, 0.25));
  color: var(--primary-text);
  font-family: monospace;
  white-space: nowrap;
}

@keyframes guide-demo-hint {
  0%, 42% { opacity: 0; transform: translate(-50%, calc(-100% - 6px)); }
  52%, 100% { opacity: 1; transform: translate(-50%, calc(-100% - 14px)); }
}

.demo-ghost-row {
  position: absolute;
  inset: 0;
  border-radius: 10px;
  background: rgba(160, 160, 160, 0.16);
  border: 1px solid rgba(160, 160, 160, 0.45);
  opacity: 0;
  z-index: 0;
}

.demo-ghost-row--swipe {
  animation: guide-demo-swipe-row 1.6s ease-in-out 2 both;
}

@keyframes guide-demo-swipe-row {
  0% { opacity: 0; transform: translateX(0); }
  8% { opacity: 0.75; transform: translateX(0); }
  45% { opacity: 0.75; transform: translateX(80px); }
  65% { opacity: 0.75; transform: translateX(80px); }
  85%, 100% { opacity: 0; transform: translateX(0); }
}

.demo-swipe-settings {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 8px;
  color: var(--highlight-text, #3b82f6);
  background: rgba(59, 130, 246, 0.14);
  opacity: 0;
  z-index: 1;
  animation: guide-demo-swipe-settings 1.6s ease-in-out 2 both;
}

@keyframes guide-demo-swipe-settings {
  0%, 30% { opacity: 0; }
  45%, 65% { opacity: 1; }
  90%, 100% { opacity: 0; }
}

.demo-finger--swipe {
  transform: translate(-50%, -50%);
  animation: guide-demo-swipe-finger 1.6s ease-in-out 2 both;
}

@keyframes guide-demo-swipe-finger {
  0% { opacity: 0; transform: translate(-50%, -50%) translateX(-10px); }
  8% { opacity: 1; transform: translate(-50%, -50%) translateX(-10px); }
  45%, 60% { opacity: 1; transform: translate(-50%, -50%) translateX(72px); }
  80%, 100% { opacity: 0; transform: translate(-50%, -50%) translateX(0); }
}

.demo-ghost-row--drag {
  animation: guide-demo-drag-row 3.2s ease-in-out 2 both;
}

@keyframes guide-demo-drag-row {
  0% { opacity: 0; transform: translateY(0); }
  10% { opacity: 0.8; transform: translateY(-6px); }
  35% { opacity: 0.8; transform: translateY(56px); }
  50% { opacity: 0.8; transform: translateY(-56px); }
  65% { opacity: 0.8; transform: translateY(-4px); }
  75%, 100% { opacity: 0.8; transform: translateY(0); }
}

.demo-finger--drag {
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
  .demo-finger--press,
  .demo-hint-card,
  .demo-ghost-row--swipe,
  .demo-swipe-settings,
  .demo-finger--swipe,
  .demo-ghost-row--drag,
  .demo-finger--drag {
    animation: none;
  }
}
</style>
