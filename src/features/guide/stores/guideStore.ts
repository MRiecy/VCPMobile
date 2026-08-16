/**
 * 教学引导引擎状态机（Pinia）
 *
 * 职责：
 * - 激活/推进/收尾指引；返回键在指引激活期间完全禁止（registerModal close → false）；
 * - 前置条件评估（requires 依赖链 + predicates 响应式谓词 + settleMs 稳定期）；
 * - pendingQueue 串行播放（同一时刻至多一场）；
 * - 完成态持久化（persist.pick 仅 completed / lastSeenAppVersion）；
 * - 版本门控：introducedIn > lastSeenAppVersion 的指引入队 + Toast 提示。
 */
import { computed, ref, watchEffect } from 'vue';
import { defineStore } from 'pinia';
import { getVersion } from '@tauri-apps/api/app';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useNotificationStore } from '../../../core/stores/notification';
import { allGuides, getGuide } from '../registry';
import type { GuideDefinition } from '../types';

export const DEFAULT_SETTLE_MS = 600;
export const QUEUE_RESUME_DELAY_MS = 250;
export const REPLAY_PREPARE_DELAY_MS = 350;

/** 逐段数值比较版本号：1.9 < 1.10；1.10 == 1.10.0；非数字段按 0 处理。 */
export function compareVersions(a: string, b: string): number {
  const segA = a.split('.').map((s) => {
    const n = parseInt(s, 10);
    return Number.isNaN(n) ? 0 : n;
  });
  const segB = b.split('.').map((s) => {
    const n = parseInt(s, 10);
    return Number.isNaN(n) ? 0 : n;
  });
  const length = Math.max(segA.length, segB.length);
  for (let i = 0; i < length; i += 1) {
    const na = segA[i] ?? 0;
    const nb = segB[i] ?? 0;
    if (na !== nb) return na < nb ? -1 : 1;
  }
  return 0;
}

export const useGuideStore = defineStore('guide', () => {
  const { registerModal, unregisterModal } = useModalHistory();

  const activeGuideId = ref<string | null>(null);
  const activeStepIndex = ref(0);
  const pendingQueue = ref<string[]>([]);
  const completed = ref<string[]>([]);
  const lastSeenAppVersion = ref<string | null>(null);

  let initialized = false;
  /** guideId → settle 定时器（触发评估期间的条件稳定期）。 */
  const settleTimers = new Map<string, ReturnType<typeof setTimeout>>();

  const activeGuide = computed<GuideDefinition | null>(() => {
    if (!activeGuideId.value) return null;
    return getGuide(activeGuideId.value) ?? null;
  });

  const activeStep = computed(() => {
    const guide = activeGuide.value;
    if (!guide) return null;
    return guide.steps[activeStepIndex.value] ?? null;
  });

  const isCompleted = (id: string): boolean => completed.value.includes(id);

  const isPlaying = (id: string): boolean =>
    activeGuideId.value === id || pendingQueue.value.includes(id);

  // ---------- 播放状态机 ----------

  /** 直接开播（仅供队列恢复内部使用，绕过 FIFO 守卫）。 */
  const startImmediately = (id: string) => {
    const def = getGuide(id);
    if (!def) {
      console.warn(`[Guide] startImmediately() with unknown guide id: ${id}`);
      return;
    }
    if (activeGuideId.value) {
      if (!pendingQueue.value.includes(id)) pendingQueue.value.push(id);
      return;
    }
    activeGuideId.value = id;
    activeStepIndex.value = 0;
    // 返回键在指引激活期间完全禁止：close 返回 false 时 useModalHistory
    // 自动补回 history entry（唯一退出出口是末步「我知道了」）。
    registerModal(`Guide:${id}`, () => false);
  };

  /**
   * 严格 FIFO：有指引在播或排队时一律入队，保证播放顺序 =
   * 进入队列的顺序（进入队列的顺序 = settle 计时器触发顺序 =
   * 注册表顺序，settleMs 不同则以先到者先入）。
   */
  const start = (id: string) => {
    if (!getGuide(id)) {
      console.warn(`[Guide] start() with unknown guide id: ${id}`);
      return;
    }
    if (activeGuideId.value === id) return;
    if (activeGuideId.value !== null || pendingQueue.value.length > 0) {
      if (!pendingQueue.value.includes(id)) pendingQueue.value.push(id);
      return;
    }
    startImmediately(id);
  };

  const next = () => {
    const guide = activeGuide.value;
    if (!guide) return;
    if (activeStepIndex.value >= guide.steps.length - 1) {
      finish();
      return;
    }
    activeStepIndex.value += 1;
  };

  const finish = () => {
    const id = activeGuideId.value;
    if (id) {
      if (!completed.value.includes(id)) completed.value.push(id);
      unregisterModal(`Guide:${id}`);
    }
    activeGuideId.value = null;
    activeStepIndex.value = 0;

    if (pendingQueue.value.length > 0) {
      // 队头保留在队列中直到恢复定时器触发（不在 finish 时提前 shift），
      // 保证恢复间隙（250ms）内 FIFO 守卫仍能看到队列非空，防止插队。
      setTimeout(() => {
        const nextId = pendingQueue.value.shift();
        if (nextId) startImmediately(nextId);
      }, QUEUE_RESUME_DELAY_MS);
    }
  };

  /** 设置页回放：绕过触发器；先退出设置页并执行 prepare 钩子，等页面退出动画稳定后开播。 */
  const replay = (id: string) => {
    const def = getGuide(id);
    if (!def) return;
    const overlayStore = useOverlayStore();
    if (overlayStore.isSettingsOpen) overlayStore.closeSettings();
    try {
      def.prepare?.();
    } catch (e) {
      console.error(`[Guide] prepare() failed for ${id}:`, e);
    }
    setTimeout(() => start(id), REPLAY_PREPARE_DELAY_MS);
  };

  /** 整场不可解析时的防死循环收尾：记录完成态并清场。 */
  const finishUnresolvable = () => {
    const id = activeGuideId.value;
    if (id && !completed.value.includes(id)) completed.value.push(id);
    finish();
  };

  /**
   * 重置全部指引进度（反复测试 / 用户重温）：恢复首次状态。
   * 完成态与版本门控一并清空；persist.pick 自动落盘。
   */
  const resetProgress = () => {
    // 若有指引在播先清场（finish 会把当前 id 写回 completed，随后整体清空）。
    if (activeGuideId.value) finish();
    pendingQueue.value = [];
    completed.value = [];
    lastSeenAppVersion.value = null;
  };

  // ---------- 前置条件评估 ----------

  const isTriggerSatisfied = (def: GuideDefinition): boolean => {
    const trigger = def.trigger;
    if (!trigger) return false;
    const requiresOk = (trigger.requires ?? []).every((r) => completed.value.includes(r));
    if (!requiresOk) return false;
    return (trigger.predicates ?? []).every((p) => {
      try {
        return p.check() === true;
      } catch (e) {
        console.warn(`[Guide] predicate "${p.name}" of ${def.id} threw:`, e);
        return false;
      }
    });
  };

  const clearSettleTimer = (id: string) => {
    const timer = settleTimers.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      settleTimers.delete(id);
    }
  };

  /** 评估所有含 trigger 的未完成指引；任一相关响应式状态变化都会触发重评估。 */
  const evaluateTriggers = () => {
    for (const def of allGuides()) {
      if (!def.trigger) continue;
      if (completed.value.includes(def.id)) continue;
      if (isPlaying(def.id)) continue;

      const satisfied = isTriggerSatisfied(def);
      if (!satisfied) {
        clearSettleTimer(def.id);
        continue;
      }
      if (settleTimers.has(def.id)) continue;

      const settleMs = def.trigger.settleMs ?? DEFAULT_SETTLE_MS;
      const timer = setTimeout(() => {
        settleTimers.delete(def.id);
        // 触发前复查，防稳定期内条件翻转的竞态。
        if (isTriggerSatisfied(def) && !isPlaying(def.id)) {
          start(def.id);
        }
      }, settleMs);
      settleTimers.set(def.id, timer);
    }
  };

  // ---------- 版本门控 ----------

  const checkVersionGates = async () => {
    try {
      const currentVersion = await getVersion();
      const last = lastSeenAppVersion.value ?? '0.0.0';
      const notificationStore = useNotificationStore();
      let hasNew = false;
      for (const def of allGuides()) {
        if (!def.introducedIn) continue;
        if (completed.value.includes(def.id)) continue;
        if (compareVersions(def.introducedIn, last) <= 0) continue;
        if (!pendingQueue.value.includes(def.id)) pendingQueue.value.push(def.id);
        hasNew = true;
      }
      if (hasNew) {
        notificationStore.addNotification({
          id: `guide-new-features-${currentVersion}`,
          toastOnly: true,
          title: '发现新功能指引',
          message: '已为你准备更新说明，稍后开始播放。',
          type: 'info',
          duration: 4000,
        });
      }
      lastSeenAppVersion.value = currentVersion;
    } catch (e) {
      // 非 Tauri 环境或版本读取失败：静默降级，不影响指引触发。
      console.warn('[Guide] Version gating skipped:', e);
    }
  };

  /** 幂等初始化：挂载触发评估 watchEffect 并执行一次版本门控。由 GuideOverlay mounted 时调用。 */
  const init = () => {
    if (initialized) return;
    initialized = true;
    watchEffect(evaluateTriggers);
    void checkVersionGates();
  };

  return {
    activeGuideId,
    activeStepIndex,
    pendingQueue,
    completed,
    lastSeenAppVersion,
    activeGuide,
    activeStep,
    init,
    start,
    next,
    finish,
    finishUnresolvable,
    replay,
    resetProgress,
    isCompleted,
    isPlaying,
  };
}, {
  persist: {
    pick: ['completed', 'lastSeenAppVersion'],
  },
});
