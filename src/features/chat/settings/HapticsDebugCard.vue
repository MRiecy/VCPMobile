<script setup lang="ts">
import { ref, reactive } from 'vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import { triggerNativeHaptic, type NativeHapticFeedbackResult } from '../../../core/utils/nativeHaptics';

defineProps<{ active?: boolean }>();

const isExpanded = ref(true);
const customDuration = ref(15);
const customAmplitude = ref(60);

const lastLog = reactive<{
  title: string;
  type: string;
  detail: string;
  time: string;
  success: boolean;
}>({
  title: '等待测试',
  type: 'none',
  detail: '点击下方按钮体验不同触感',
  time: '--:--:--',
  success: true,
});

function formatTime(): string {
  const now = new Date();
  return now.toTimeString().split(' ')[0];
}

async function testViewHaptic(name: string, label: string) {
  const res: NativeHapticFeedbackResult = await triggerNativeHaptic({
    type: 'view',
    effect: name,
  });
  lastLog.title = label;
  lastLog.type = `View Haptics (${name})`;
  lastLog.detail = `performed: ${res.performed ?? false}, constant: ${res.constant ?? 'N/A'}${res.fallback ? ' (fallback web)' : ''}`;
  lastLog.time = formatTime();
  lastLog.success = res.performed !== false;
}

async function testPredefinedHaptic(name: string, label: string) {
  const res: NativeHapticFeedbackResult = await triggerNativeHaptic({
    type: 'predefined',
    effect: name,
  });
  lastLog.title = label;
  lastLog.type = `Predefined Effect (${name})`;
  lastLog.detail = `performed: ${res.performed ?? false}, effectId: ${res.effectId ?? 'N/A'}${res.fallback ? ' (fallback duration)' : ''}`;
  lastLog.time = formatTime();
  lastLog.success = res.performed !== false;
}

async function testOneShot(durationMs: number, amplitude: number, label: string) {
  const res: NativeHapticFeedbackResult = await triggerNativeHaptic({
    type: 'one_shot',
    durationMs,
    amplitude,
  });
  lastLog.title = label;
  lastLog.type = `OneShot (${durationMs}ms / amp ${amplitude})`;
  lastLog.detail = `performed: ${res.performed ?? false}, ampControl: ${res.hasAmplitudeControl ? '支持' : '不支持'}`;
  lastLog.time = formatTime();
  lastLog.success = res.performed !== false;
}

function testWebVibrate(ms: number, label: string) {
  let performed = false;
  if (typeof navigator !== 'undefined' && navigator.vibrate) {
    performed = navigator.vibrate(ms);
  }
  lastLog.title = label;
  lastLog.type = `Web API (navigator.vibrate)`;
  lastLog.detail = `duration: ${ms}ms, performed: ${performed}`;
  lastLog.time = formatTime();
  lastLog.success = performed;
}
</script>

<template>
  <div class="haptics-debug-wrapper mt-4">
    <div class="flex items-center justify-between py-2 px-1">
      <div class="flex items-center gap-2">
        <span class="inline-block w-2 h-2 rounded-full bg-[var(--highlight-text)] animate-pulse"></span>
        <h4 class="text-xs font-black tracking-wider uppercase opacity-80">系统级触感调试实验室 (Debug)</h4>
      </div>
      <button
        type="button"
        class="text-xs font-mono text-[var(--highlight-text)] hover:underline active:opacity-60 transition-opacity"
        @click="isExpanded = !isExpanded"
      >
        {{ isExpanded ? '[ 收起 ]' : '[ 展开测试 ]' }}
      </button>
    </div>

    <SettingsCard v-if="isExpanded">
      <div class="space-y-4 text-xs">
        <!-- 实时监控状态 -->
        <div class="p-2.5 rounded-lg bg-black/10 dark:bg-white/5 border border-black/5 dark:border-white/10 font-mono">
          <div class="flex items-center justify-between text-[11px] opacity-70 mb-1">
            <span>最近触发: <strong class="text-[var(--primary-text)] font-sans">{{ lastLog.title }}</strong></span>
            <span>{{ lastLog.time }}</span>
          </div>
          <div class="text-[10px] break-all text-[var(--secondary-text)]">
            <div>类别: <span class="text-[var(--highlight-text)]">{{ lastLog.type }}</span></div>
            <div>状态: <span>{{ lastLog.detail }}</span></div>
          </div>
        </div>

        <!-- 分类 1: 系统视图触觉 (View Haptics) -->
        <div>
          <div class="text-[11px] font-bold text-[var(--primary-text)] mb-1.5 flex items-center gap-1.5">
            <span>1. 系统视图触感 (View Haptics)</span>
            <span class="text-[10px] font-mono px-1 py-0.2 rounded bg-blue-500/10 text-blue-400">极轻·打字同款</span>
          </div>
          <p class="text-[10px] text-[var(--secondary-text)] mb-2">Android 系统输入法/原生控件同款单瞬态触感，几乎无马达嗡鸣：</p>
          <div class="grid grid-cols-2 gap-2">
            <button
              type="button"
              class="haptic-btn border-blue-500/30 font-semibold"
              @click="testViewHaptic('keyboard_tap', '键盘轻点 (KEYBOARD_TAP)')"
            >
              <span>键盘轻点 (TAP)</span>
              <small>打字同款·极轻脆</small>
            </button>
            <button
              type="button"
              class="haptic-btn border-blue-500/30 font-semibold"
              @click="testViewHaptic('clock_tick', '时钟刻度 (CLOCK_TICK)')"
            >
              <span>时钟刻度 (TICK)</span>
              <small>微弱齿轮刻度感</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testViewHaptic('virtual_key', '虚拟按键 (VIRTUAL_KEY)')"
            >
              <span>虚拟按键 (KEY)</span>
              <small>经典系统按键感</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testViewHaptic('confirm', '操作确认 (CONFIRM)')"
            >
              <span>操作确认 (CONFIRM)</span>
              <small>Android 11+ 完成感</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testViewHaptic('reject', '操作拒绝 (REJECT)')"
            >
              <span>操作拒绝 (REJECT)</span>
              <small>Android 11+ 双顿挫</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testViewHaptic('context_click', '上下文点击 (CONTEXT)')"
            >
              <span>长按/菜单 (CONTEXT)</span>
              <small>上下文菜单反馈</small>
            </button>
          </div>
        </div>

        <!-- 分类 2: 马达预定义波形 (Predefined VibrationEffect) -->
        <div class="pt-2 border-t border-[var(--vcp-border-subtle)]">
          <div class="text-[11px] font-bold text-[var(--primary-text)] mb-1.5 flex items-center gap-1.5">
            <span>2. 预定义原生波形 (Predefined Effect)</span>
            <span class="text-[10px] font-mono px-1 py-0.2 rounded bg-emerald-500/10 text-emerald-400">Android 10+</span>
          </div>
          <p class="text-[10px] text-[var(--secondary-text)] mb-2">直接调用底层硬件 HAL 预定义波形：</p>
          <div class="grid grid-cols-2 gap-2">
            <button
              type="button"
              class="haptic-btn border-emerald-500/30 font-semibold"
              @click="testPredefinedHaptic('tick', '滴答轻触 (EFFECT_TICK)')"
            >
              <span>滴答 (EFFECT_TICK)</span>
              <small>极短微弱脉冲</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testPredefinedHaptic('click', '轻击 (EFFECT_CLICK)')"
            >
              <span>轻击 (CLICK)</span>
              <small>标准单击</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testPredefinedHaptic('heavy_click', '重击 (EFFECT_HEAVY_CLICK)')"
            >
              <span>重击 (HEAVY_CLICK)</span>
              <small>扎实偏重</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testPredefinedHaptic('double_click', '双击 (EFFECT_DOUBLE_CLICK)')"
            >
              <span>双击 (DOUBLE_CLICK)</span>
              <small>连续两下</small>
            </button>
          </div>
        </div>

        <!-- 分类 3: 振幅调控微震 (OneShot with Amplitude) -->
        <div class="pt-2 border-t border-[var(--vcp-border-subtle)]">
          <div class="text-[11px] font-bold text-[var(--primary-text)] mb-1.5 flex items-center gap-1.5">
            <span>3. 振幅微调控制 (OneShot Amplitude)</span>
            <span class="text-[10px] font-mono px-1 py-0.2 rounded bg-purple-500/10 text-purple-400">功率降级</span>
          </div>
          <p class="text-[10px] text-[var(--secondary-text)] mb-2">降低马达驱动功率（若硬件马达支持振幅调控）：</p>
          <div class="grid grid-cols-3 gap-1.5 mb-2.5">
            <button
              type="button"
              class="haptic-btn"
              @click="testOneShot(10, 30, '极柔微弱 (10ms / 振幅30)')"
            >
              <span>10ms @ 30</span>
              <small>极柔微弱</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testOneShot(15, 60, '轻柔微震 (15ms / 振幅60)')"
            >
              <span>15ms @ 60</span>
              <small>轻柔点触</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testOneShot(20, 120, '中度适中 (20ms / 振幅120)')"
            >
              <span>20ms @ 120</span>
              <small>中度适中</small>
            </button>
          </div>

          <!-- 自定义滑动条 -->
          <div class="bg-black/5 dark:bg-white/5 p-2 rounded-lg space-y-2">
            <div class="flex items-center justify-between text-[10px]">
              <span class="text-[var(--secondary-text)]">自定义时长: <strong class="text-[var(--primary-text)] font-mono">{{ customDuration }}ms</strong></span>
              <input type="range" min="5" max="60" step="5" v-model.number="customDuration" class="w-32 h-1 accent-[var(--highlight-text)]" />
            </div>
            <div class="flex items-center justify-between text-[10px]">
              <span class="text-[var(--secondary-text)]">自定义振幅: <strong class="text-[var(--primary-text)] font-mono">{{ customAmplitude }}/255</strong></span>
              <input type="range" min="15" max="255" step="15" v-model.number="customAmplitude" class="w-32 h-1 accent-[var(--highlight-text)]" />
            </div>
            <button
              type="button"
              class="w-full py-1.5 px-3 rounded text-[11px] font-bold bg-[var(--highlight-text)] text-black dark:text-white active:opacity-80 transition-opacity"
              @click="testOneShot(customDuration, customAmplitude, `自定义 (${customDuration}ms / 振幅${customAmplitude})`)"
            >
              测试当前滑动参数微震
            </button>
          </div>
        </div>

        <!-- 分类 4: Web API 时长对照 (navigator.vibrate) -->
        <div class="pt-2 border-t border-[var(--vcp-border-subtle)]">
          <div class="text-[11px] font-bold text-[var(--primary-text)] mb-1.5 flex items-center gap-1.5">
            <span>4. Web API 时长对照 (navigator.vibrate)</span>
            <span class="text-[10px] font-mono px-1 py-0.2 rounded bg-amber-500/10 text-amber-400">全功率蜂鸣对照</span>
          </div>
          <p class="text-[10px] text-[var(--secondary-text)] mb-2">对比不同毫秒数下的全功率连续震动感：</p>
          <div class="grid grid-cols-2 gap-2">
            <button
              type="button"
              class="haptic-btn"
              @click="testWebVibrate(10, 'Web API (10ms 超短脉冲)')"
            >
              <span>10ms 脉冲</span>
              <small>短脉冲微触</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testWebVibrate(15, 'Web API (15ms 轻短脉冲)')"
            >
              <span>15ms 脉冲</span>
              <small>轻短脉冲</small>
            </button>
            <button
              type="button"
              class="haptic-btn"
              @click="testWebVibrate(25, 'Web API (25ms 当前发送消息)')"
            >
              <span>25ms 发送</span>
              <small>当前【发送】默认</small>
            </button>
            <button
              type="button"
              class="haptic-btn border-red-500/30 text-red-400"
              @click="testWebVibrate(40, 'Web API (40ms 当前收到回复)')"
            >
              <span>40ms 回复</span>
              <small>当前【收到回复】默认 (震感大)</small>
            </button>
          </div>
        </div>
      </div>
    </SettingsCard>
  </div>
</template>

<style scoped>
.haptic-btn {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  justify-content: center;
  padding: 0.45rem 0.6rem;
  border: 1px solid var(--vcp-border-subtle);
  border-radius: 0.5rem;
  background: rgba(0, 0, 0, 0.03);
  text-align: left;
  transition: all 0.15s ease;
}

:global(.dark) .haptic-btn {
  background: rgba(255, 255, 255, 0.04);
}

.haptic-btn:active {
  background: var(--vcp-highlight-bg-10, rgba(59, 130, 246, 0.15));
  border-color: var(--highlight-text, #3b82f6);
  transform: scale(0.98);
}

.haptic-btn span {
  font-size: 0.6875rem;
  font-weight: 700;
  color: var(--primary-text);
  line-height: 1.25;
}

.haptic-btn small {
  font-size: 0.625rem;
  color: var(--secondary-text);
  margin-top: 0.15rem;
  line-height: 1.2;
}
</style>
