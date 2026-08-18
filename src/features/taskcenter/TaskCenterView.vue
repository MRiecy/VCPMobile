<script setup lang="ts">
/**
 * TaskCenterView.vue — 任务调度中心（S2a：只读 + 启停 + 触发 + 历史 + 全局开关）。
 *
 * 高密度线性布局 + 2px accent bar 状态表达（绿=启用 / 蓝=运行中 / 红=上次失败 / 灰=禁用）。
 * 方案见 plan/vcpmobile-more-tools-research/02 §5。
 */
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import {
  ArrowLeft,
  CalendarClock,
  CircleAlert,
  Play,
  RefreshCw,
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import SettingsSwitch from '../../components/settings/SettingsSwitch.vue';
import { useTaskCenterStore } from './taskCenterStore';
import {
  RUN_STATUS_LABEL,
  TASK_TYPE_LABEL,
  TRIGGER_SOURCE_LABEL,
  formatDateTime,
  formatDuration,
  scheduleSummary,
  splitRandomTag,
  type RunRecord,
  type TaskItem,
} from './taskTypes';

const props = withDefaults(defineProps<{ isOpen?: boolean; zIndex?: number }>(), {
  isOpen: false,
  zIndex: 40,
});

const emit = defineEmits<{ close: [] }>();

const store = useTaskCenterStore();

const activeTab = ref<'tasks' | 'history'>('tasks');

watch(
  () => props.isOpen,
  (open) => {
    if (open) void store.startSession();
    else store.stopSession();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  store.resetSession();
});

// ---------- 任务卡片状态 ----------
type CardState = 'running' | 'error' | 'enabled' | 'disabled';

function cardState(task: TaskItem): CardState {
  if (task.runtime.running) return 'running';
  if (!task.enabled) return 'disabled';
  if (task.runtime.lastResult?.startsWith('error')) return 'error';
  return 'enabled';
}

function agentsLabel(task: TaskItem): string {
  const { agents, randomCount } = splitRandomTag(task.agents);
  const base = agents.join('、') || '未配置目标';
  return randomCount !== null ? `${base}（随机抽 ${randomCount} 人）` : base;
}

function lastResultLabel(task: TaskItem): string {
  const runtime = task.runtime;
  if (runtime.running) return '正在派发…';
  if (!runtime.lastResult) return '尚未运行';
  const time = runtime.lastFinishTime ? formatDateTime(runtime.lastFinishTime) : '';
  const duration = formatDuration(runtime.lastDurationMs);
  return `${time} · ${duration} · ${runtime.lastResult}`;
}

function nextRunLabel(task: TaskItem): string {
  const next = task.runtime.nextRunTime;
  if (!next) return '—';
  const parsed = Date.parse(next);
  return Number.isFinite(parsed) ? formatDateTime(next) : next;
}

// ---------- 历史 ----------
function historyStateClass(record: RunRecord): string {
  return `history-${record.status}`;
}

// ---------- 空态 ----------
const emptyState = computed(() => {
  if (store.pluginUnavailable) {
    return {
      title: 'TaskAssistant 插件未加载',
      detail: '请在 VCPToolBox 服务器上启用 VCPTaskAssistant 插件后重试。',
      retry: true,
    };
  }
  if (store.error && !store.configLoaded) {
    return { title: '连接失败', detail: store.error, retry: true };
  }
  return null;
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="task-center">
      <!-- 顶栏 -->
      <header class="tc-header">
        <button type="button" class="tc-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <div class="tc-title-block">
          <span class="tc-title">任务调度</span>
          <span class="tc-subtitle">TaskAssistant</span>
        </div>
        <button
          type="button"
          class="tc-icon-btn"
          aria-label="刷新"
          @click="store.refresh()"
        >
          <RefreshCw :size="17" />
        </button>
      </header>

      <!-- 状态条：全局开关 + 统计 -->
      <section class="tc-status">
        <div class="tc-global-row">
          <span class="tc-global-dot" :class="{ 'is-on': store.globalEnabled }" />
          <span class="tc-global-label">全局调度{{ store.globalEnabled ? '运行中' : '已暂停' }}</span>
          <SettingsSwitch
            :model-value="store.globalEnabled"
            :disabled="store.globalToggling"
            @update:model-value="store.setGlobalEnabled"
          />
        </div>
        <div class="tc-stats">
          <span class="tc-stat">定时器 <strong>{{ store.activeTimerCount }}</strong></span>
          <span class="tc-stat">任务 <strong>{{ store.tasks.length }}</strong></span>
          <span class="tc-stat">启用 <strong>{{ store.enabledCount }}</strong></span>
          <span class="tc-stat">运行中 <strong>{{ store.runningCount }}</strong></span>
          <span class="tc-stat tc-stat-error" v-if="store.errorCount > 0">
            失败 <strong>{{ store.errorCount }}</strong>
          </span>
        </div>
      </section>

      <!-- 分段控件 -->
      <nav class="tc-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          class="tc-tab"
          :class="{ 'is-active': activeTab === 'tasks' }"
          :aria-selected="activeTab === 'tasks'"
          @click="activeTab = 'tasks'"
        >
          任务
        </button>
        <button
          type="button"
          role="tab"
          class="tc-tab"
          :class="{ 'is-active': activeTab === 'history' }"
          :aria-selected="activeTab === 'history'"
          @click="activeTab = 'history'"
        >
          执行历史
        </button>
      </nav>

      <!-- 错误横幅（已加载配置后的轮询失败不打断浏览） -->
      <div v-if="store.error && store.configLoaded" class="tc-error-banner" role="alert">
        <CircleAlert :size="14" />
        <span>{{ store.error }}</span>
      </div>

      <!-- 整页空态 -->
      <div v-if="emptyState" class="tc-empty">
        <CalendarClock :size="28" class="tc-empty-icon" />
        <p class="tc-empty-title">{{ emptyState.title }}</p>
        <p class="tc-empty-detail">{{ emptyState.detail }}</p>
        <button
          v-if="emptyState.retry"
          type="button"
          class="tc-retry-btn"
          @click="store.refresh()"
        >
          重试
        </button>
      </div>

      <!-- 任务列表 -->
      <div
        v-else-if="activeTab === 'tasks'"
        class="tc-scroll vcp-scrollable no-rubber-band"
        data-taskcenter-role="task-list"
      >
        <div v-if="store.tasks.length === 0" class="tc-empty">
          <p class="tc-empty-title">{{ store.isLoading ? '正在读取任务…' : '尚未配置任务' }}</p>
          <p class="tc-empty-detail" v-if="!store.isLoading">
            任务编辑器将在后续版本提供；当前可查看与触发既有任务。
          </p>
        </div>

        <article
          v-for="task in store.tasks"
          :key="task.id"
          class="tc-card"
          :class="`tc-card-${cardState(task)}`"
        >
          <div class="tc-card-head">
            <div class="tc-card-title-block">
              <span class="tc-card-name">{{ task.name }}</span>
              <span class="tc-card-type">{{ TASK_TYPE_LABEL[task.type] }}</span>
            </div>
            <SettingsSwitch
              :model-value="task.enabled"
              :disabled="store.togglingIds.has(task.id)"
              @update:model-value="(value: boolean) => store.setTaskEnabled(task.id, value)"
            />
          </div>

          <p class="tc-card-line">{{ scheduleSummary(task) }} · {{ agentsLabel(task) }}</p>
          <p class="tc-card-line tc-card-result" :class="{ 'is-error': cardState(task) === 'error' }">
            {{ lastResultLabel(task) }}
          </p>
          <p class="tc-card-line tc-card-meta">
            下次 {{ nextRunLabel(task) }} · 运行 {{ task.runtime.runCount }} 次
            · 成功 {{ task.runtime.successCount }} · 失败 {{ task.runtime.errorCount }}
          </p>

          <div class="tc-card-actions">
            <button
              type="button"
              class="tc-trigger-btn"
              :disabled="store.triggeringIds.has(task.id) || task.runtime.running"
              @click="store.triggerTask(task.id)"
            >
              <Play :size="13" />
              {{ store.triggeringIds.has(task.id) ? '派发中…' : '立即触发' }}
            </button>
          </div>
        </article>
      </div>

      <!-- 执行历史 -->
      <div
        v-else
        class="tc-scroll vcp-scrollable no-rubber-band"
        data-taskcenter-role="history-list"
      >
        <div v-if="store.history.length === 0" class="tc-empty">
          <p class="tc-empty-title">暂无执行记录</p>
        </div>

        <article
          v-for="record in store.history"
          :key="record.id"
          class="tc-history-row"
          :class="historyStateClass(record)"
        >
          <div class="tc-history-head">
            <span class="tc-history-name">{{ record.taskName }}</span>
            <span class="tc-history-status">{{ RUN_STATUS_LABEL[record.status] }}</span>
          </div>
          <p class="tc-history-line">
            {{ formatDateTime(record.startedAt) }} · {{ formatDuration(record.durationMs) }}
            · {{ TRIGGER_SOURCE_LABEL[record.triggerSource] ?? record.triggerSource }}
            · {{ record.agents.join('、') || '—' }}
          </p>
          <p v-if="record.message" class="tc-history-message">{{ record.message }}</p>
        </article>
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.task-center {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.tc-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.tc-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.tc-title {
  font-size: 16px;
  font-weight: 800;
}

.tc-subtitle {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  opacity: 0.45;
  text-transform: uppercase;
}

.tc-icon-btn {
  width: 40px;
  height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.65;
  cursor: pointer;
}

.tc-icon-btn:active {
  opacity: 1;
}

.tc-status {
  flex-shrink: 0;
  padding: 10px 14px 8px;
  border-bottom: 1px solid var(--border-color);
}

.tc-global-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.tc-global-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--border-color);
}

.tc-global-dot.is-on {
  background: #10b981;
}

.tc-global-label {
  flex: 1;
  font-size: 12px;
  font-weight: 700;
}

.tc-stats {
  display: flex;
  gap: 14px;
  overflow-x: auto;
  white-space: nowrap;
}

.tc-stat {
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.06em;
  opacity: 0.55;
}

.tc-stat strong {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  margin-left: 2px;
  opacity: 1;
  color: var(--primary-text);
}

.tc-stat-error strong {
  color: #ef4444;
}

.tc-tabs {
  display: flex;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border-color);
  padding: 0 14px;
  gap: 18px;
}

.tc-tab {
  padding: 10px 2px;
  border: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
  font-weight: 700;
  opacity: 0.5;
  border-bottom: 2px solid transparent;
  cursor: pointer;
}

.tc-tab.is-active {
  opacity: 1;
  border-bottom-color: var(--highlight-text);
}

.tc-error-banner {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  font-size: 11px;
  color: #ef4444;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.tc-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 10px 12px calc(var(--vcp-safe-bottom, 48px) + 12px);
}

.tc-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px 24px;
  text-align: center;
}

.tc-empty-icon {
  opacity: 0.35;
}

.tc-empty-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  opacity: 0.75;
}

.tc-empty-detail {
  margin: 0;
  font-size: 12px;
  opacity: 0.5;
  max-width: 28rem;
}

.tc-retry-btn {
  margin-top: 6px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

/* ---- 任务卡片（高密度线性 + 2px accent bar） ---- */
.tc-card {
  border-left: 2px solid transparent;
  border-bottom: 1px solid var(--border-color);
  padding: 10px 10px 10px 12px;
}

.tc-card-enabled {
  border-left-color: #10b981;
}

.tc-card-running {
  border-left-color: #3b82f6;
}

.tc-card-error {
  border-left-color: #ef4444;
}

.tc-card-disabled {
  opacity: 0.55;
}

.tc-card-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tc-card-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.tc-card-name {
  font-size: 14px;
  font-weight: 800;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tc-card-type {
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.08em;
  padding: 2px 6px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  opacity: 0.6;
  flex-shrink: 0;
}

.tc-card-line {
  margin: 3px 0 0;
  font-size: 11.5px;
  opacity: 0.7;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tc-card-result.is-error {
  color: #ef4444;
  opacity: 1;
}

.tc-card-meta {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10.5px;
  opacity: 0.5;
}

.tc-card-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 6px;
}

.tc-trigger-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-height: 32px;
  padding: 0 14px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--highlight-text);
  font-size: 11px;
  font-weight: 700;
}

.tc-trigger-btn:disabled {
  opacity: 0.45;
}

/* ---- 执行历史 ---- */
.tc-history-row {
  border-left: 2px solid transparent;
  border-bottom: 1px solid var(--border-color);
  padding: 9px 10px 9px 12px;
}

.tc-history-row.history-success {
  border-left-color: #10b981;
}

.tc-history-row.history-partial_success {
  border-left-color: #f59e0b;
}

.tc-history-row.history-error {
  border-left-color: #ef4444;
}

.tc-history-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.tc-history-name {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tc-history-status {
  font-size: 10px;
  font-weight: 700;
  opacity: 0.65;
  flex-shrink: 0;
}

.tc-history-line {
  margin: 3px 0 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10.5px;
  opacity: 0.55;
}

.tc-history-message {
  margin: 3px 0 0;
  font-size: 11px;
  opacity: 0.7;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 平板/宽屏：内容限宽居中 */
@media (min-width: 768px) {
  .tc-scroll,
  .tc-status,
  .tc-tabs {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
