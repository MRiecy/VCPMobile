<script setup lang="ts">
import { ref, reactive, watch, onScopeDispose } from 'vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import { triggerNativeHaptic, type NativeHapticFeedbackPayload } from '../../../core/utils/nativeHaptics';
import { isAndroid, isIOS } from '../../../core/utils/platform';

const props = withDefaults(defineProps<{ active?: boolean }>(), { active: true });

const isExpanded = ref(true);
const customDuration = ref(15);
const customAmplitude = ref(60);
const intensity = ref(0.5);
const impactStyles = ['light', 'medium', 'heavy', 'soft', 'rigid'] as const;
const notificationEffects = ['success', 'warning', 'error'] as const;
const busy = ref(false);
let generation = 0;
watch(() => props.active, (active) => {
  if (!active) { generation += 1; busy.value = false; }
}, { flush: 'sync' });
onScopeDispose(() => { generation += 1; });

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

async function testHaptic(payload: NativeHapticFeedbackPayload, label: string) {
  if (!props.active || busy.value) return;
  busy.value = true;
  const request = ++generation;
  try {
    const result = await triggerNativeHaptic(payload);
    if (!props.active || request !== generation) return;
    lastLog.title = label;
    lastLog.type = `${payload.type}${payload.effect ? ` (${payload.effect})` : ''}`;
    lastLog.detail = `${result.submitted ? '已提交' : '未提交'} · ${JSON.stringify(result)}`;
    lastLog.time = formatTime();
    lastLog.success = result.submitted;
  } finally {
    if (request === generation) busy.value = false;
  }
}

const testViewHaptic = (effect: string, label: string) => testHaptic({ type: 'view', effect }, label);
const testPredefinedHaptic = (effect: string, label: string) => testHaptic({ type: 'predefined', effect }, label);
const testOneShot = (durationMs: number, amplitude: number, label: string) =>
  testHaptic({ type: 'one_shot', durationMs, amplitude }, label);

function testWebVibrate(ms: number, label: string) {
  if (!props.active || busy.value) return;
  let performed = false;
  if (typeof navigator !== 'undefined' && navigator.vibrate) {
    try { performed = navigator.vibrate(ms); } catch { performed = false; }
  }
  lastLog.title = label;
  lastLog.type = `Web API (navigator.vibrate)`;
  lastLog.detail = `duration: ${ms}ms, performed: ${performed}`;
  lastLog.time = formatTime();
  lastLog.success = performed;
}
</script>

<template>
  <div :inert="!active" class="haptics-debug-wrapper">
    <div class="flex items-center justify-between py-2 px-1">
      <div class="flex items-center gap-2">
        <h3 class="text-sm font-semibold">消息触感与效果测试</h3>
      </div>
      <button
        type="button"
        :disabled="!active"
        class="min-h-11 px-2 text-xs text-[var(--highlight-text)] active:opacity-60 transition-opacity"
        @click="isExpanded = !isExpanded"
      >
        {{ isExpanded ? '[ 收起 ]' : '[ 展开测试 ]' }}
      </button>
    </div>

    <p class="mb-3 text-xs text-[var(--secondary-text)]">测试仅试听效果，不修改聊天偏好。系统与设备决定是否实际播放触感。</p>
    <SettingsCard v-if="isExpanded">
      <fieldset :disabled="!active || busy || (!isAndroid && !isIOS)" class="border-0 p-0 m-0 min-w-0">
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

        <div>
          <h4 class="text-xs font-semibold mb-2">正式消息触感</h4>
          <div class="grid grid-cols-2 gap-2">
            <button type="button" class="haptic-btn" @click="testHaptic({ type: 'message', effect: 'send' }, '发送消息')">
              <span>发送消息</span><small>试听发送反馈</small>
            </button>
            <button type="button" class="haptic-btn" @click="testHaptic({ type: 'message', effect: 'complete' }, '回复完整生成')">
              <span>回复完整生成</span><small>试听完成反馈</small>
            </button>
          </div>
        </div>

        <div v-if="isIOS" class="pt-2 border-t border-[var(--vcp-border-subtle)] space-y-3">
          <h4 class="text-xs font-semibold">iOS 原生触感</h4>
          <label class="flex min-h-11 items-center gap-3">
            <span class="shrink-0">强度 <span class="font-mono">{{ intensity.toFixed(1) }}</span></span>
            <input v-model.number="intensity" type="range" min="0" max="1" step="0.1" class="min-w-0 flex-1 accent-[var(--highlight-text)]" />
          </label>
          <div class="grid grid-cols-2 gap-2">
            <button v-for="style in impactStyles" :key="style" type="button" class="haptic-btn"
              @click="testHaptic({ type: 'impact', effect: style, intensity }, `Impact ${style}`)">
              <span class="font-mono">{{ style }}</span><small>Impact · {{ intensity.toFixed(1) }}</small>
            </button>
            <button type="button" class="haptic-btn" @click="testHaptic({ type: 'selection' }, '选择变化')">
              <span>选择变化</span><small>Selection</small>
            </button>
            <button v-for="effect in notificationEffects" :key="effect" type="button" class="haptic-btn"
              @click="testHaptic({ type: 'notification', effect }, `Notification ${effect}`)">
              <span class="font-mono">{{ effect }}</span><small>Notification</small>
            </button>
          </div>
        </div>

        <!-- 分类 1: 系统视图触觉 (View Haptics) -->
        <div v-if="isAndroid">
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
        <div v-if="isAndroid" class="pt-2 border-t border-[var(--vcp-border-subtle)]">
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
        <div v-if="isAndroid" class="pt-2 border-t border-[var(--vcp-border-subtle)]">
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
        <div v-if="isAndroid" class="pt-2 border-t border-[var(--vcp-border-subtle)]">
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
              @click="testWebVibrate(25, 'Web API (25ms)')"
            >
              <span>25ms 脉冲</span>
              <small>历史发送效果对照</small>
            </button>
            <button
              type="button"
              class="haptic-btn border-red-500/30 text-red-400"
              @click="testWebVibrate(40, 'Web API (40ms)')"
            >
              <span>40ms 脉冲</span>
              <small>历史回复效果对照</small>
            </button>
          </div>
        </div>
      </div>
      </fieldset>
      <p v-if="!isAndroid && !isIOS" class="mt-2 text-xs text-[var(--secondary-text)]">请在移动设备上测试原生触感。</p>
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
  min-height: 44px;
  border: 1px solid var(--vcp-border-subtle);
  border-radius: 0.5rem;
  background: rgba(0, 0, 0, 0.03);
  text-align: left;
  transition: background-color 0.15s ease, border-color 0.15s ease;
}

:global(.dark) .haptic-btn {
  background: rgba(255, 255, 255, 0.04);
}

.haptic-btn:active {
  background: var(--vcp-highlight-bg-10, rgba(59, 130, 246, 0.15));
  border-color: var(--highlight-text, #3b82f6);
}

.haptic-btn:disabled { opacity: 0.45; }

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
