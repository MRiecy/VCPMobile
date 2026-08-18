<script setup lang="ts">
/**
 * DelegationDetailSheet.vue — 异步委托详情（滑升面板）。
 * 与 HistoryDetailSheet 同构；活跃委托可在详情内请求取消。
 */
import { onBeforeUnmount, watch } from 'vue';
import { X } from 'lucide-vue-next';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useTaskCenterStore } from '../taskCenterStore';
import {
  DELEGATION_STATUS_LABEL,
  formatDateTime,
  formatDuration,
  type DelegationItem,
} from '../taskTypes';

const props = defineProps<{ item: DelegationItem | null }>();
const emit = defineEmits<{ close: [] }>();

const store = useTaskCenterStore();
const overlayStore = useOverlayStore();

const { registerModal, unregisterModal } = useModalHistory();
const MODAL_ID = 'TaskCenter:DelegationDetail';

watch(
  () => props.item,
  (item) => {
    if (item) registerModal(MODAL_ID, () => emit('close'));
    else unregisterModal(MODAL_ID);
  },
);

onBeforeUnmount(() => {
  unregisterModal(MODAL_ID);
});

const STATUS_CLASS: Record<string, string> = {
  running: 'is-running',
  waiting: 'is-running',
  cancelling: 'is-cancelling',
  completed: 'is-completed',
  failed: 'is-failed',
  cancelled: 'is-cancelled',
};

function isActive(item: DelegationItem): boolean {
  return ['running', 'waiting', 'cancelling'].includes(item.status);
}

async function confirmCancel(item: DelegationItem): Promise<void> {
  const ok = await overlayStore.showConfirm({
    title: '取消委托',
    message: `请求取消 ${item.agentName} 的委托任务？\n（取消请求送达后，任务会在下一个心跳点停止）`,
    isDanger: true,
  });
  if (!ok) return;
  await store.cancelDelegation(item.id);
  emit('close');
}
</script>

<template>
  <Transition name="dd-fade">
    <div v-if="item" class="dd-mask" @click="emit('close')" @touchmove.prevent />
  </Transition>
  <Transition name="dd-slide">
    <section v-if="item" class="dd-sheet" role="dialog" aria-label="委托详情">
      <header class="dd-header">
        <div class="dd-title-block">
          <span class="dd-name">{{ item.agentName }}</span>
          <span class="dd-status" :class="STATUS_CLASS[item.status]">
            {{ DELEGATION_STATUS_LABEL[item.status] ?? item.status }}
          </span>
        </div>
        <button type="button" class="dd-close" aria-label="关闭" @click="emit('close')">
          <X :size="18" />
        </button>
      </header>

      <div class="dd-body vcp-scrollable">
        <dl class="dd-grid">
          <div class="dd-item">
            <dt>委托 ID</dt>
            <dd class="dd-mono">{{ item.id }}</dd>
          </div>
          <div class="dd-item">
            <dt>开始时间</dt>
            <dd class="dd-mono">{{ item.createdAt ? formatDateTime(item.createdAt) : '—' }}</dd>
          </div>
          <div class="dd-item">
            <dt>最近更新</dt>
            <dd class="dd-mono">{{ item.updatedAt ? formatDateTime(item.updatedAt) : '—' }}</dd>
          </div>
          <div class="dd-item">
            <dt>已耗时</dt>
            <dd class="dd-mono">{{ formatDuration(item.elapsedMs) }}</dd>
          </div>
          <div class="dd-item" v-if="item.currentRound">
            <dt>执行轮次</dt>
            <dd class="dd-mono">
              第 {{ item.currentRound }}{{ item.maxRounds ? ` / ${item.maxRounds}` : '' }} 轮
            </dd>
          </div>
          <div class="dd-item" v-if="item.cancelRequested">
            <dt>取消请求</dt>
            <dd class="dd-warn">已发出，等待心跳点生效</dd>
          </div>
        </dl>

        <template v-if="item.promptPreview">
          <h4 class="dd-section-title">委托内容</h4>
          <pre class="dd-message">{{ item.promptPreview }}</pre>
        </template>

        <template v-if="item.responsePreview">
          <h4 class="dd-section-title">最近响应</h4>
          <pre class="dd-message">{{ item.responsePreview }}</pre>
        </template>

        <template v-if="item.reportPreview">
          <h4 class="dd-section-title">最终报告</h4>
          <pre class="dd-message">{{ item.reportPreview }}</pre>
        </template>
      </div>

      <footer v-if="isActive(item)" class="dd-footer">
        <button
          type="button"
          class="dd-cancel-btn"
          :disabled="store.cancellingIds.has(item.id) || item.status === 'cancelling'"
          @click="confirmCancel(item)"
        >
          {{ item.status === 'cancelling' ? '取消中…' : '请求取消委托' }}
        </button>
      </footer>
    </section>
  </Transition>
</template>

<style scoped>
.dd-mask {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  z-index: var(--layer-local);
}

.dd-sheet {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  max-height: 76%;
  display: flex;
  flex-direction: column;
  background: var(--secondary-bg);
  border-top: 1px solid var(--border-color);
  border-radius: 14px 14px 0 0;
  z-index: var(--layer-local);
}

.dd-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 14px 14px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.dd-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.dd-name {
  font-size: 15px;
  font-weight: 800;
}

.dd-status {
  font-size: 10px;
  font-weight: 800;
  padding: 2px 8px;
  border-radius: 999px;
  flex-shrink: 0;
}

.dd-status.is-running {
  color: #3b82f6;
  background: rgba(59, 130, 246, 0.1);
}

.dd-status.is-cancelling,
.dd-status.is-cancelled {
  color: #f59e0b;
  background: rgba(245, 158, 11, 0.1);
}

.dd-status.is-completed {
  color: #10b981;
  background: rgba(16, 185, 129, 0.1);
}

.dd-status.is-failed {
  color: #ef4444;
  background: rgba(239, 68, 68, 0.1);
}

.dd-close {
  width: 36px;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.6;
}

.dd-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.dd-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px 14px;
  margin: 0;
}

.dd-item dt {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  opacity: 0.45;
  margin-bottom: 2px;
}

.dd-item dd {
  margin: 0;
  font-size: 12.5px;
  font-weight: 600;
  word-break: break-all;
}

.dd-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.dd-warn {
  color: #f59e0b;
}

.dd-section-title {
  margin: 16px 0 6px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.14em;
  opacity: 0.45;
}

.dd-message {
  margin: 0;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  font-size: 11.5px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  opacity: 0.85;
  font-family: inherit;
}

.dd-footer {
  flex-shrink: 0;
  padding: 10px 14px calc(var(--vcp-safe-bottom, 48px) + 10px);
  border-top: 1px solid var(--border-color);
}

.dd-cancel-btn {
  width: 100%;
  min-height: 42px;
  border-radius: 10px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: transparent;
  color: #ef4444;
  font-size: 13px;
  font-weight: 800;
}

.dd-cancel-btn:disabled {
  opacity: 0.45;
}

.dd-fade-enter-active,
.dd-fade-leave-active {
  transition: opacity 0.2s ease;
}

.dd-fade-enter-from,
.dd-fade-leave-to {
  opacity: 0;
}

.dd-slide-enter-active,
.dd-slide-leave-active {
  transition: transform 0.28s cubic-bezier(0.32, 0.72, 0, 1);
}

.dd-slide-enter-from,
.dd-slide-leave-to {
  transform: translateY(100%);
}

@media (min-width: 768px) {
  .dd-sheet {
    left: 50%;
    right: auto;
    width: 560px;
    transform: translateX(-50%);
  }

  .dd-slide-enter-from,
  .dd-slide-leave-to {
    transform: translateX(-50%) translateY(100%);
  }
}
</style>
