<script setup lang="ts">
/**
 * DelegationPanel.vue — 异步委托追踪（只读展示 + 取消，见 05 篇决策 5）。
 * 轮询由 store 的 startDelegationWatch/stopDelegationWatch 驱动（Tab 激活才拉取）。
 */
import { computed, onBeforeUnmount, watch } from 'vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import { useTaskCenterStore } from '../taskCenterStore';
import { DELEGATION_STATUS_LABEL, formatDateTime, type DelegationItem } from '../taskTypes';

const props = defineProps<{ active: boolean }>();

const store = useTaskCenterStore();
const overlayStore = useOverlayStore();

watch(
  () => props.active,
  (active) => {
    if (active) store.startDelegationWatch();
    else store.stopDelegationWatch();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  store.stopDelegationWatch();
});

const running = computed(() =>
  store.delegations.filter((d) => ['running', 'waiting', 'cancelling'].includes(d.status)),
);
const recent = computed(() =>
  store.delegations.filter((d) => !['running', 'waiting', 'cancelling'].includes(d.status)),
);

function statusLabel(item: DelegationItem): string {
  return DELEGATION_STATUS_LABEL[item.status] ?? item.status;
}

async function confirmCancel(item: DelegationItem): Promise<void> {
  const ok = await overlayStore.showConfirm({
    title: '取消委托',
    message: `请求取消 ${item.agentName} 的委托任务？\n（取消请求送达后，任务会在下一个心跳点停止）`,
    isDanger: true,
  });
  if (!ok) return;
  await store.cancelDelegation(item.id);
}
</script>

<template>
  <div class="delegation-panel">
    <div v-if="store.delegations.length === 0" class="dp-empty">
      <p class="dp-empty-title">
        {{ store.delegationsLoading ? '正在读取委托…' : '暂无异步委托' }}
      </p>
      <p class="dp-empty-detail">任务开启「异步委托模式」后，其后台执行状态会出现在这里。</p>
    </div>

    <template v-else>
      <h4 class="dp-group-title">运行中（{{ running.length }}）</h4>
      <div v-if="running.length === 0" class="dp-group-empty">暂无运行中的委托</div>
      <article v-for="item in running" :key="item.id" class="dp-row dp-row-active">
        <div class="dp-head">
          <span class="dp-agent">{{ item.agentName }}</span>
          <span class="dp-status">{{ statusLabel(item) }}</span>
        </div>
        <p class="dp-line">{{ formatDateTime(item.createdAt) }} 开始</p>
        <p v-if="item.summary" class="dp-summary">{{ item.summary }}</p>
        <div class="dp-actions">
          <button
            type="button"
            class="dp-cancel-btn"
            :disabled="store.cancellingIds.has(item.id) || item.status === 'cancelling'"
            @click="confirmCancel(item)"
          >
            {{ item.status === 'cancelling' ? '取消中…' : '请求取消' }}
          </button>
        </div>
      </article>

      <h4 class="dp-group-title">最近记录（{{ recent.length }}）</h4>
      <div v-if="recent.length === 0" class="dp-group-empty">暂无最近记录</div>
      <article
        v-for="item in recent"
        :key="item.id"
        class="dp-row"
        :class="`dp-${item.status}`"
      >
        <div class="dp-head">
          <span class="dp-agent">{{ item.agentName }}</span>
          <span class="dp-status">{{ statusLabel(item) }}</span>
        </div>
        <p class="dp-line">{{ formatDateTime(item.updatedAt || item.createdAt) }}</p>
        <p v-if="item.summary" class="dp-summary">{{ item.summary }}</p>
      </article>
    </template>
  </div>
</template>

<style scoped>
.delegation-panel {
  padding: 4px 0;
}

.dp-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 40px 24px;
  text-align: center;
}

.dp-empty-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  opacity: 0.75;
}

.dp-empty-detail {
  margin: 0;
  font-size: 12px;
  opacity: 0.5;
  max-width: 28rem;
}

.dp-group-title {
  margin: 14px 4px 6px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.14em;
  opacity: 0.45;
}

.dp-group-empty {
  padding: 8px 4px;
  font-size: 11px;
  opacity: 0.45;
}

.dp-row {
  border-left: 2px solid var(--border-color);
  border-bottom: 1px solid var(--border-color);
  padding: 9px 10px 9px 12px;
}

.dp-row-active {
  border-left-color: #3b82f6;
}

.dp-row.dp-completed {
  border-left-color: #10b981;
}

.dp-row.dp-failed {
  border-left-color: #ef4444;
}

.dp-row.dp-cancelled {
  border-left-color: #f59e0b;
}

.dp-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.dp-agent {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dp-status {
  font-size: 10px;
  font-weight: 700;
  opacity: 0.65;
  flex-shrink: 0;
}

.dp-line {
  margin: 3px 0 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10.5px;
  opacity: 0.55;
}

.dp-summary {
  margin: 4px 0 0;
  font-size: 11px;
  opacity: 0.7;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.dp-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 6px;
}

.dp-cancel-btn {
  min-height: 32px;
  padding: 0 14px;
  border-radius: 999px;
  border: 1px solid rgba(239, 68, 68, 0.4);
  background: transparent;
  color: #ef4444;
  font-size: 11px;
  font-weight: 700;
}

.dp-cancel-btn:disabled {
  opacity: 0.45;
}
</style>
