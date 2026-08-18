<script setup lang="ts">
/**
 * HistoryDetailSheet.vue — 执行历史详情（滑升面板）。
 * 展示一次运行的完整信息：状态、时间、耗时、触发来源、目标/失败 Agent、完整消息。
 */
import { onBeforeUnmount, watch } from 'vue';
import { X } from 'lucide-vue-next';
import { useModalHistory } from '../../../core/composables/useModalHistory';
import {
  RUN_STATUS_LABEL,
  TRIGGER_SOURCE_LABEL,
  formatDateTime,
  formatDuration,
  type RunRecord,
} from '../taskTypes';

const props = defineProps<{ record: RunRecord | null }>();
const emit = defineEmits<{ close: [] }>();

const { registerModal, unregisterModal } = useModalHistory();
const MODAL_ID = 'TaskCenter:HistoryDetail';

watch(
  () => props.record,
  (record) => {
    if (record) {
      registerModal(MODAL_ID, () => emit('close'));
    } else {
      unregisterModal(MODAL_ID);
    }
  },
);

onBeforeUnmount(() => {
  unregisterModal(MODAL_ID);
});

const STATUS_CLASS: Record<string, string> = {
  success: 'is-success',
  partial_success: 'is-partial',
  error: 'is-error',
};
</script>

<template>
  <!-- 原地渲染：absolute 定位于 .task-center 容器内（调度中心页内弹层） -->
  <Transition name="hd-fade">
    <div v-if="record" class="hd-mask" @click="emit('close')" @touchmove.prevent />
  </Transition>
  <Transition name="hd-slide">
    <section v-if="record" class="hd-sheet" role="dialog" aria-label="执行详情">
        <header class="hd-header">
          <div class="hd-title-block">
            <span class="hd-name">{{ record.taskName }}</span>
            <span class="hd-status" :class="STATUS_CLASS[record.status]">
              {{ RUN_STATUS_LABEL[record.status] }}
            </span>
          </div>
          <button type="button" class="hd-close" aria-label="关闭" @click="emit('close')">
            <X :size="18" />
          </button>
        </header>

        <div class="hd-body vcp-scrollable">
          <dl class="hd-grid">
            <div class="hd-item">
              <dt>开始时间</dt>
              <dd class="hd-mono">{{ record.startedAt ? formatDateTime(record.startedAt) : '—' }}</dd>
            </div>
            <div class="hd-item">
              <dt>结束时间</dt>
              <dd class="hd-mono">{{ record.finishedAt ? formatDateTime(record.finishedAt) : '—' }}</dd>
            </div>
            <div class="hd-item">
              <dt>耗时</dt>
              <dd class="hd-mono">{{ formatDuration(record.durationMs) }}</dd>
            </div>
            <div class="hd-item">
              <dt>触发来源</dt>
              <dd>{{ TRIGGER_SOURCE_LABEL[record.triggerSource] ?? record.triggerSource }}</dd>
            </div>
            <div class="hd-item">
              <dt>执行 Agent</dt>
              <dd>{{ record.agents.join('、') || '—' }}</dd>
            </div>
            <div class="hd-item" v-if="record.failedAgents.length">
              <dt>失败 Agent</dt>
              <dd class="hd-error">{{ record.failedAgents.join('、') }}</dd>
            </div>
          </dl>

          <h4 class="hd-section-title">结果消息</h4>
          <pre class="hd-message">{{ record.message || '（无消息）' }}</pre>
        </div>
    </section>
  </Transition>
</template>

<style scoped>
.hd-mask {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  z-index: var(--layer-local);
}

.hd-sheet {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  max-height: 72%;
  display: flex;
  flex-direction: column;
  background: var(--secondary-bg);
  border-top: 1px solid var(--border-color);
  border-radius: 14px 14px 0 0;
  padding-bottom: calc(var(--vcp-safe-bottom, 48px) + 8px);
  z-index: var(--layer-local);
}

.hd-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 14px 14px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.hd-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.hd-name {
  font-size: 15px;
  font-weight: 800;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hd-status {
  font-size: 10px;
  font-weight: 800;
  padding: 2px 8px;
  border-radius: 999px;
  flex-shrink: 0;
}

.hd-status.is-success {
  color: #10b981;
  background: rgba(16, 185, 129, 0.1);
}

.hd-status.is-partial {
  color: #f59e0b;
  background: rgba(245, 158, 11, 0.1);
}

.hd-status.is-error {
  color: #ef4444;
  background: rgba(239, 68, 68, 0.1);
}

.hd-close {
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

.hd-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.hd-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px 14px;
  margin: 0;
}

.hd-item dt {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  opacity: 0.45;
  margin-bottom: 2px;
}

.hd-item dd {
  margin: 0;
  font-size: 12.5px;
  font-weight: 600;
  word-break: break-all;
}

.hd-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.hd-error {
  color: #ef4444;
}

.hd-section-title {
  margin: 16px 0 6px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.14em;
  opacity: 0.45;
}

.hd-message {
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

.hd-fade-enter-active,
.hd-fade-leave-active {
  transition: opacity 0.2s ease;
}

.hd-fade-enter-from,
.hd-fade-leave-to {
  opacity: 0;
}

.hd-slide-enter-active,
.hd-slide-leave-active {
  transition: transform 0.28s cubic-bezier(0.32, 0.72, 0, 1);
}

.hd-slide-enter-from,
.hd-slide-leave-to {
  transform: translateY(100%);
}

@media (min-width: 768px) {
  .hd-sheet {
    left: 50%;
    right: auto;
    width: 560px;
    transform: translateX(-50%);
    border-radius: 14px 14px 0 0;
  }

  .hd-slide-enter-from,
  .hd-slide-leave-to {
    transform: translateX(-50%) translateY(100%);
  }
}
</style>
