<script setup lang="ts">
/**
 * PfProjectDetail.vue — 工程详情 shell。
 *
 * 对齐桌面 VCPChat ProjectForge 主区：返回键、工程头（名称 / 状态 / ID / 工作区 / 根目录 /
 * 创建者 / 时间 / 只读或已删除警告）、横向 tab 条（概览 / 开发脉络 / 改动文件 / 搜索历史 /
 * 验收报告，可横滑）、署名与刷新、承载子 tab 组件、节点 / 批次 diff 弹层（PfDiffSheet）。
 *
 * 导航与模式切换由页面 shell（Lead）负责；本组件只接收 projectId、emit back。
 */
import { computed, onMounted, ref, watch } from 'vue';
import { AlertTriangle, ArrowLeft, Loader2, RefreshCw } from "@lucide/vue";
import { useNotificationStore } from '../../../core/stores/notification';
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfBatchSummary, PfHistoryRow, PfProject } from '../projectForgeTypes';
import PfOverviewTab from './PfOverviewTab.vue';
import PfTimelineTab from './PfTimelineTab.vue';
import PfFilesTab from './PfFilesTab.vue';
import PfHistoryTab from './PfHistoryTab.vue';
import PfReportTab from './PfReportTab.vue';
import PfDiffSheet from './PfDiffSheet.vue';

type TabKey = 'overview' | 'timeline' | 'files' | 'history' | 'report';
type DiffTarget = { kind: 'node'; node: PfHistoryRow } | { kind: 'batch'; batch: PfBatchSummary };
type HistorySeed = { byMaid?: string; file?: string; _stamp?: number };

const props = defineProps<{ projectId: string }>();
const emit = defineEmits<{ back: [] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();

const TABS: { key: TabKey; label: string }[] = [
  { key: 'overview', label: '概览' },
  { key: 'timeline', label: '开发脉络' },
  { key: 'files', label: '改动文件' },
  { key: 'history', label: '搜索历史' },
  { key: 'report', label: '验收报告' },
];

const STATUS_LABELS: Record<string, string> = {
  active: '施工中',
  review: '待验收',
  accepted: '已验收',
};

const activeTab = ref<TabKey>('overview');
const historySeed = ref<HistorySeed | null>(null);
const diffOpen = ref(false);
const diffTarget = ref<DiffTarget | null>(null);
const loadError = ref('');
const signatureDraft = ref(String((store as any).signature || ''));

const detail = computed(() => store.detail);
const project = computed<PfProject | null>(() => detail.value?.project ?? (store as any).currentProject ?? null);
const loading = computed(() => Boolean(store.loading?.detail));
const detailError = computed(() => {
  const specific = String((store as any).detailError || '');
  return specific || String(store.error || '');
});

const isReadonly = computed(() => {
  const value = project.value;
  if (!value) return true;
  if ((value as any).deleted_at) return true;
  return (value as any).rootInfo?.writable === false;
});

const statusLabelText = computed(() => {
  const status = String(project.value?.status || 'active');
  return STATUS_LABELS[status] || status;
});

const rootInfo = computed<any>(() => (project.value as any)?.rootInfo || {});

const workspaceLabel = computed(() =>
  String(rootInfo.value.workspaceAlias || project.value?.workspace_alias || '—'),
);

const rootPath = computed(() => String(rootInfo.value.root || project.value?.root || '—'));

const createdByLabel = computed(() => (project.value?.created_by ? `@${project.value.created_by}` : '@未署名'));

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function notify(type: 'success' | 'error' | 'info', message: string): void {
  notificationStore.addNotification({
    id: `pf-detail-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: 'V工程',
    message,
    type,
    duration: type === 'error' ? 4200 : 2400,
    toastOnly: true,
  });
}

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return String(value);
  const pad = (input: number) => String(input).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

// ---------------------------------------------------------------------------
// 详情加载 / 刷新
// ---------------------------------------------------------------------------

async function load(force = false): Promise<void> {
  loadError.value = '';
  try {
    await store.openProject(props.projectId, { force } as any);
  } catch (error) {
    loadError.value = errorText(error);
    notify('error', `读取工程失败：${loadError.value}`);
  }
}

function reload(): void {
  void load(true);
}

onMounted(() => {
  const currentId = String((store as any).currentProjectId || '');
  if (currentId !== props.projectId || !store.detail) void load();
});

watch(
  () => props.projectId,
  (nextId, previousId) => {
    if (!nextId || nextId === previousId) return;
    activeTab.value = 'overview';
    historySeed.value = null;
    closeDiff();
    void load();
  },
);

// ---------------------------------------------------------------------------
// 署名（回退操作记录用）
// ---------------------------------------------------------------------------

let signatureTimer: ReturnType<typeof setTimeout> | null = null;

watch(
  () => (store as any).signature,
  (value) => {
    const next = String(value || '');
    if (next !== signatureDraft.value) signatureDraft.value = next;
  },
);

function onSignatureInput(): void {
  if (signatureTimer) clearTimeout(signatureTimer);
  signatureTimer = setTimeout(() => {
    signatureTimer = null;
    store.setSignature(String(signatureDraft.value || '').trim());
  }, 250);
}

function commitSignature(): void {
  if (signatureTimer) clearTimeout(signatureTimer);
  signatureTimer = null;
  store.setSignature(String(signatureDraft.value || '').trim());
}

// ---------------------------------------------------------------------------
// tab / 跨 tab 跳转
// ---------------------------------------------------------------------------

function selectTab(tab: TabKey): void {
  activeTab.value = tab;
}

function openHistory(filter: HistorySeed): void {
  historySeed.value = { ...filter, _stamp: Date.now() };
  activeTab.value = 'history';
}

function openDiff(target: DiffTarget): void {
  diffTarget.value = target;
  diffOpen.value = true;
}

function closeDiff(): void {
  diffOpen.value = false;
  diffTarget.value = null;
}

// T1 的 rollback / submitReport 内部已完成写后回读（refreshProjectViews / openProject force），
// 这里只需关闭弹层；store.detail 更新后模板会自动重渲染，避免重复打桥接。
function onRolledBack(): void {
  closeDiff();
}

function onReportSubmitted(): void {
  /* store.submitReport 已回读，无需重复拉取 */
}
</script>

<template>
  <div class="pf-detail">
    <!-- 顶栏：返回 + 标题 + 刷新 -->
    <header class="pf-detail-header">
      <button type="button" class="pf-icon-btn" aria-label="返回工程列表" @click="emit('back')">
        <ArrowLeft :size="20" />
      </button>
      <div class="pf-title-block">
        <div class="pf-title-row">
          <h2 class="pf-title">{{ project?.name || '工程详情' }}</h2>
          <span v-if="project" class="pf-badge" :class="`is-${project.status || 'active'}`">{{ statusLabelText }}</span>
        </div>
        <div class="pf-title-sub">ProjectForge · 施工图</div>
      </div>
      <button
        type="button"
        class="pf-icon-btn"
        :disabled="loading"
        aria-label="刷新工程数据"
        title="刷新工程数据"
        @click="reload"
      >
        <RefreshCw :size="18" :class="{ 'pf-spin': loading }" />
      </button>
    </header>

    <!-- 加载 / 错误 / 空 -->
    <div v-if="!detail" class="pf-state">
      <template v-if="loading">
        <Loader2 class="pf-spin" :size="18" aria-hidden="true" />
        <span>正在加载工程数据…</span>
      </template>
      <template v-else-if="detailError || loadError">
        <AlertTriangle :size="18" aria-hidden="true" />
        <p class="pf-state-text">{{ detailError || loadError }}</p>
        <button type="button" class="pf-btn" @click="reload">重试</button>
      </template>
      <template v-else>
        <p class="pf-state-text">未找到该工程的数据</p>
        <button type="button" class="pf-btn" @click="reload">重试</button>
      </template>
    </div>

    <template v-else-if="project">
      <!-- 工程头信息 -->
      <section class="pf-project-head">
        <div class="pf-meta-grid">
          <span class="pf-meta-label">ID</span>
          <code class="pf-mono pf-meta-value">{{ project.id }}</code>
          <span class="pf-meta-label">工作区</span>
          <span class="pf-meta-value">{{ workspaceLabel }}</span>
          <span class="pf-meta-label">根目录</span>
          <code class="pf-mono pf-meta-value" :title="rootPath">{{ rootPath }}</code>
          <span class="pf-meta-label">创建者</span>
          <span class="pf-meta-value">{{ createdByLabel }}</span>
          <span class="pf-meta-label">创建于</span>
          <span class="pf-meta-value">{{ formatTime(project.created_at) }}</span>
          <span class="pf-meta-label">最近变动</span>
          <span class="pf-meta-value">{{ formatTime((project.stats as any)?.lastAt || project.updated_at) }}</span>
        </div>

        <p v-if="rootInfo.writable === false" class="pf-warn-line">
          ⚠ {{ rootInfo.blockedReason || '工程只读' }}（无法回退）
        </p>
        <p v-if="project.deleted_at" class="pf-warn-line">
          ⚠ 工程已被 {{ project.deleted_by ? `@${project.deleted_by}` : '' }} 删除（仅数据库记录）
        </p>

        <!-- 署名 -->
        <div class="pf-signature-row">
          <span class="pf-signature-label">署名</span>
          <input
            v-model="signatureDraft"
            type="text"
            class="pf-signature-input"
            maxlength="100"
            placeholder="回退操作将以此署名记录"
            aria-label="回退署名"
            autocomplete="off"
            @input="onSignatureInput"
            @change="commitSignature"
            @blur="commitSignature"
          />
          <span v-if="!signatureDraft.trim()" class="pf-signature-warn">未设置署名时无法回退</span>
        </div>
      </section>

      <!-- 横向 tab 条 -->
      <nav class="pf-tab-bar" role="tablist" aria-label="工程详情分页">
        <button
          v-for="tab in TABS"
          :key="tab.key"
          type="button"
          role="tab"
          class="pf-tab"
          :class="{ 'is-active': activeTab === tab.key }"
          :aria-selected="activeTab === tab.key"
          @click="selectTab(tab.key)"
        >
          {{ tab.label }}
        </button>
      </nav>

      <!-- tab 内容 -->
      <div class="pf-tab-content">
        <PfOverviewTab v-if="activeTab === 'overview'" @open-history="openHistory" />
        <PfTimelineTab v-else-if="activeTab === 'timeline'" @open-diff="openDiff" />
        <PfFilesTab
          v-else-if="activeTab === 'files'"
          @open-history="openHistory"
          @open-diff="openDiff"
        />
        <PfHistoryTab v-else-if="activeTab === 'history'" :initial-filter="historySeed" @open-diff="openDiff" />
        <PfReportTab v-else-if="activeTab === 'report'" :project-id="props.projectId" @submitted="onReportSubmitted" />
      </div>
    </template>

    <!-- 节点 / 批次 diff 弹层 -->
    <PfDiffSheet
      :open="diffOpen"
      :project-id="props.projectId"
      :target="diffTarget"
      :readonly="isReadonly"
      @close="closeDiff"
      @rolled-back="onRolledBack"
    />
  </div>
</template>

<style scoped>
.pf-detail {
  --pf-ok: #4caf7d;
  --pf-warn: #e0a84a;
  --pf-danger: #e57373;
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
  font-size: 14px;
}

/* ---------- 顶栏 ---------- */
.pf-detail-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px 4px;
  flex-shrink: 0;
}

.pf-icon-btn {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  display: grid;
  place-items: center;
  border: none;
  background: transparent;
  color: inherit;
  opacity: 0.8;
}

.pf-icon-btn:active {
  opacity: 1;
  background: var(--secondary-bg);
}

.pf-icon-btn:disabled {
  opacity: 0.4;
}

.pf-title-block {
  flex: 1;
  min-width: 0;
}

.pf-title-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.pf-title {
  margin: 0;
  font-size: 17px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-title-sub {
  margin-top: 1px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.6;
}

.pf-badge {
  flex-shrink: 0;
  padding: 1px 8px;
  border: 1px solid var(--border-color);
  border-radius: 999px;
  font-size: 11px;
  color: var(--secondary-text, var(--primary-text));
}

.pf-badge.is-active {
  color: var(--highlight-text, var(--primary-text));
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-badge.is-review {
  color: var(--pf-warn);
  border-color: var(--pf-warn);
}

.pf-badge.is-accepted {
  color: var(--pf-ok);
  border-color: var(--pf-ok);
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 状态 ---------- */
.pf-state {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px 20px;
  text-align: center;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.85;
}

.pf-state-text {
  margin: 0;
  max-width: 320px;
  line-height: 1.6;
  word-break: break-word;
}

/* ---------- 工程头 ---------- */
.pf-project-head {
  flex-shrink: 0;
  padding: 4px 12px 8px;
  border-bottom: 1px solid var(--border-color);
}

.pf-meta-grid {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 3px 10px;
  font-size: 12px;
  align-items: baseline;
}

.pf-meta-label {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.7;
  white-space: nowrap;
}

.pf-meta-value {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-mono {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
}

.pf-warn-line {
  margin: 6px 0 0;
  font-size: 12px;
  line-height: 1.5;
  color: var(--pf-warn);
}

.pf-signature-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 8px;
}

.pf-signature-label {
  flex-shrink: 0;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
}

.pf-signature-input {
  flex: 1;
  min-width: 0;
  min-height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  font-size: 13px;
  outline: none;
}

.pf-signature-input:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-signature-warn {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--pf-warn);
}

/* ---------- tab 条（可横滑） ---------- */
.pf-tab-bar {
  display: flex;
  flex-shrink: 0;
  overflow-x: auto;
  overflow-y: hidden;
  border-bottom: 1px solid var(--border-color);
  scrollbar-width: none;
  -webkit-overflow-scrolling: touch;
}

.pf-tab-bar::-webkit-scrollbar {
  display: none;
}

.pf-tab {
  position: relative;
  flex-shrink: 0;
  min-height: 44px;
  padding: 0 14px;
  border: none;
  background: transparent;
  color: var(--secondary-text, var(--primary-text));
  font: inherit;
  font-size: 13px;
  opacity: 0.8;
  white-space: nowrap;
}

.pf-tab.is-active {
  color: var(--primary-text);
  opacity: 1;
  font-weight: 600;
}

.pf-tab.is-active::after {
  content: '';
  position: absolute;
  left: 14px;
  right: 14px;
  bottom: 0;
  height: 2px;
  border-radius: 1px;
  background: var(--highlight-text, var(--primary-text));
}

/* ---------- tab 内容 ---------- */
.pf-tab-content {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
  padding: 12px 12px 0;
}

.pf-btn {
  min-height: 44px;
  padding: 0 16px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14px;
}

.pf-btn:active {
  background: var(--secondary-bg);
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }
}
</style>
