<script setup lang="ts">
/**
 * PfReportTab.vue — 工程详情「验收报告」tab。
 *
 * 对齐桌面 VCPChat ProjectForge：报告 Markdown 只读渲染（提交者 / 工程状态）+
 * 提交 / 更新验收报告表单（conclusion 必填、issues 可选、status review/accepted/active，
 * 二次确认，写后由 store.submitReport 回读）。
 *
 * 报告来自 Agent 生成的 Markdown：marked 渲染后用 DOMPurify 清洗（与 Scriptorium 阅读器一致）。
 */
import { computed, ref, watch } from 'vue';
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import { AlertTriangle, FileCheck2, Loader2, Send } from "@lucide/vue";
import { useNotificationStore } from '../../../core/stores/notification';
import { useProjectForgeStore } from '../projectForgeStore';
import type { PfProject } from '../projectForgeTypes';

const props = withDefaults(defineProps<{ projectId?: string }>(), { projectId: '' });

const emit = defineEmits<{ submitted: [] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();

const STATUS_LABELS: Record<string, string> = {
  active: '施工中',
  review: '待验收',
  accepted: '已验收',
};

const STATUS_OPTIONS = [
  { value: 'review', label: 'review（待验收）' },
  { value: 'accepted', label: 'accepted（已验收）' },
  { value: 'active', label: 'active（退回继续施工）' },
];

const project = computed<PfProject | null>(() => (store.detail as any)?.project ?? null);
const report = computed(() => String(project.value?.report || ''));
const statusLabelText = computed(() => {
  const status = String(project.value?.status || 'active');
  return STATUS_LABELS[status] || status;
});

const reportHtml = computed(() => {
  const markdown = report.value || '';
  if (!markdown.trim()) return '';
  try {
    const rendered = marked.parse(markdown, { async: false }) as string;
    return DOMPurify.sanitize(rendered, {
      FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'form', 'style', 'link', 'meta'],
      FORBID_ATTR: ['onerror', 'onload', 'onclick'],
      ALLOW_DATA_ATTR: false,
    });
  } catch {
    return '';
  }
});

// —— 提交 / 更新表单 ——
const formOpen = ref(false);
const conclusion = ref('');
const issues = ref('');
const status = ref<'review' | 'accepted' | 'active'>('review');
const formError = ref('');
const submitting = ref(false);
const confirmOpen = ref(false);

const submittingBusy = computed(() => submitting.value || Boolean(store.loading?.action));

const confirmStatusLabel = computed(
  () => STATUS_OPTIONS.find((option) => option.value === status.value)?.label || status.value,
);

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  const message = (error as any)?.message;
  return message ? String(message) : String(error ?? '');
}

function notify(type: 'success' | 'error' | 'info', message: string): void {
  notificationStore.addNotification({
    id: `pf-report-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
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

function toggleForm(): void {
  formOpen.value = !formOpen.value;
  formError.value = '';
  if (formOpen.value) {
    // 已有报告时用现有结论/遗留问题预填，便于更新
    issues.value = '';
    conclusion.value = '';
    status.value = String(project.value?.status || 'review') === 'accepted' ? 'accepted' : 'review';
  }
}

function validate(): boolean {
  formError.value = '';
  if (!conclusion.value.trim()) {
    formError.value = '验收结论（conclusion）为必填项';
    return false;
  }
  if (conclusion.value.trim().length > 20000) {
    formError.value = '验收结论过长（最多 20000 字）';
    return false;
  }
  return true;
}

function requestSubmit(): void {
  if (!validate()) return;
  confirmOpen.value = true;
}

async function confirmSubmit(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  formError.value = '';
  try {
    await store.submitReport({
      conclusion: conclusion.value.trim(),
      issues: issues.value.trim(),
      status: status.value,
    });
    confirmOpen.value = false;
    formOpen.value = false;
    conclusion.value = '';
    issues.value = '';
    notify('success', '验收报告已提交');
    emit('submitted');
  } catch (error) {
    formError.value = errorText(error);
    notify('error', `提交验收报告失败：${formError.value}`);
    confirmOpen.value = false;
  } finally {
    submitting.value = false;
  }
}

watch(
  () => props.projectId,
  () => {
    formOpen.value = false;
    confirmOpen.value = false;
    formError.value = '';
  },
);
</script>

<template>
  <div class="pf-report">
    <div v-if="!project" class="pf-state">暂无工程数据</div>

    <template v-else>
      <!-- 报告只读展示 -->
      <section class="pf-report-view">
        <div class="pf-report-meta">
          <template v-if="report">
            <span>提交者 <span class="pf-maid">{{ project.report_by ? `@${project.report_by}` : '@未署名' }}</span></span>
            <span class="pf-muted">·</span>
            <span>状态 <span class="pf-status">{{ statusLabelText }}</span></span>
            <span class="pf-muted">·</span>
            <span class="pf-muted">更新于 {{ formatTime(project.updated_at) }}</span>
          </template>
          <template v-else>
            <span class="pf-muted">尚未提交验收报告。</span>
          </template>
        </div>

        <div v-if="reportHtml" class="pf-markdown" v-html="reportHtml" />
        <div v-else-if="!report" class="pf-report-empty">
          <FileCheck2 :size="28" aria-hidden="true" />
          <p>Agent 提交验收报告后会显示在这里；你也可以在下方手动提交 / 更新报告。</p>
        </div>
      </section>

      <!-- 提交 / 更新表单 -->
      <div class="pf-report-actions">
        <button type="button" class="pf-btn" @click="toggleForm">
          <Send :size="15" aria-hidden="true" />
          <span>{{ formOpen ? '收起表单' : report ? '更新验收报告' : '提交验收报告' }}</span>
        </button>
      </div>

      <form v-if="formOpen" class="pf-report-form" @submit.prevent="requestSubmit">
        <div class="pf-section-title">{{ report ? '更新验收报告' : '提交验收报告' }}</div>

        <label class="pf-field">
          <span class="pf-field-label">验收结论 <em>*</em></span>
          <textarea
            v-model="conclusion"
            rows="5"
            maxlength="20000"
            placeholder="填写验收结论（必填）：完成了什么、是否通过、依据是什么"
            aria-label="验收结论"
          />
        </label>

        <label class="pf-field">
          <span class="pf-field-label">遗留问题</span>
          <textarea v-model="issues" rows="3" maxlength="10000" placeholder="可选：未解决的问题、后续建议" aria-label="遗留问题" />
        </label>

        <label class="pf-field">
          <span class="pf-field-label">工程状态</span>
          <select v-model="status">
            <option v-for="option in STATUS_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
          </select>
        </label>

        <p v-if="formError" class="pf-form-error">{{ formError }}</p>

        <div class="pf-form-actions">
          <button type="button" class="pf-btn" @click="toggleForm">取消</button>
          <button type="submit" class="pf-btn primary" :disabled="submittingBusy">
            <Loader2 v-if="submittingBusy" class="pf-spin" :size="15" aria-hidden="true" />
            <span>{{ submittingBusy ? '提交中…' : '提交报告' }}</span>
          </button>
        </div>
      </form>

      <!-- 二次确认 -->
      <Teleport to="body">
        <div v-if="confirmOpen" class="pf-confirm-mask" @click.self="confirmOpen = false">
          <div class="pf-confirm" role="dialog" aria-modal="true" aria-label="确认提交验收报告">
            <h3 class="pf-confirm-title">确认提交验收报告</h3>
            <p class="pf-confirm-line">目标状态：<strong>{{ confirmStatusLabel }}</strong></p>
            <p class="pf-confirm-line">结论：{{ conclusion.trim().slice(0, 120) }}{{ conclusion.trim().length > 120 ? '…' : '' }}</p>
            <p v-if="issues.trim()" class="pf-confirm-line">遗留问题：{{ issues.trim().slice(0, 120) }}{{ issues.trim().length > 120 ? '…' : '' }}</p>
            <p class="pf-confirm-warn">
              <AlertTriangle :size="14" aria-hidden="true" />
              <span>提交后会更新该工程的验收报告与状态。</span>
            </p>
            <div class="pf-confirm-actions">
              <button type="button" class="pf-btn" @click="confirmOpen = false">取消</button>
              <button type="button" class="pf-btn primary" :disabled="submittingBusy" @click="confirmSubmit">
                {{ submittingBusy ? '提交中…' : '确认提交' }}
              </button>
            </div>
          </div>
        </div>
      </Teleport>
    </template>
  </div>
</template>

<style scoped>
.pf-report {
  --pf-danger: #e57373;
  padding: 0 0 calc(var(--vcp-safe-bottom, 0px) + 16px);
  color: var(--primary-text);
  font-size: 14px;
}

.pf-state {
  padding: 32px 16px;
  text-align: center;
  font-size: 13px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-report-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 6px;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  margin-bottom: 8px;
}

.pf-maid {
  color: var(--highlight-text, var(--primary-text));
  font-weight: 500;
}

.pf-status {
  color: var(--highlight-text, var(--primary-text));
}

.pf-muted {
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.75;
}

.pf-markdown {
  line-height: 1.7;
  font-size: 14px;
  word-break: break-word;
  overflow-wrap: anywhere;
}

.pf-markdown :deep(h1),
.pf-markdown :deep(h2),
.pf-markdown :deep(h3) {
  margin: 14px 0 6px;
  font-size: 16px;
  line-height: 1.4;
}

.pf-markdown :deep(p) {
  margin: 8px 0;
}

.pf-markdown :deep(ul),
.pf-markdown :deep(ol) {
  padding-left: 20px;
  margin: 8px 0;
}

.pf-markdown :deep(code) {
  font-family: Consolas, 'Cascadia Code', ui-monospace, monospace;
  font-size: 12px;
  background: var(--secondary-bg);
  border-radius: 4px;
  padding: 1px 4px;
}

.pf-markdown :deep(pre) {
  margin: 8px 0;
  padding: 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  overflow-x: auto;
}

.pf-markdown :deep(pre code) {
  background: transparent;
  padding: 0;
}

.pf-markdown :deep(table) {
  border-collapse: collapse;
  width: 100%;
  margin: 8px 0;
  display: block;
  overflow-x: auto;
}

.pf-markdown :deep(th),
.pf-markdown :deep(td) {
  border: 1px solid var(--border-color);
  padding: 5px 8px;
  text-align: left;
  font-size: 13px;
}

.pf-markdown :deep(blockquote) {
  margin: 8px 0;
  padding: 4px 10px;
  border-left: 2px solid var(--border-color);
  color: var(--secondary-text, var(--primary-text));
}

.pf-report-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 24px 16px;
  border: 1px dashed var(--border-color);
  border-radius: 10px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
  font-size: 13px;
  text-align: center;
}

.pf-report-empty p {
  margin: 0;
  max-width: 300px;
  line-height: 1.6;
}

.pf-report-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 12px;
}

.pf-report-form {
  margin-top: 10px;
  padding: 10px 12px 12px;
  border: 1px solid var(--border-color);
  border-radius: 10px;
}

.pf-section-title {
  font-size: 13px;
  font-weight: 600;
  margin-bottom: 8px;
}

.pf-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 8px;
}

.pf-field-label {
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
  opacity: 0.8;
}

.pf-field-label em {
  color: var(--pf-danger);
  font-style: normal;
}

.pf-field textarea,
.pf-field select {
  width: 100%;
  box-sizing: border-box;
  min-height: 44px;
  padding: 8px 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: inherit;
  font: inherit;
  outline: none;
}

.pf-field textarea:focus,
.pf-field select:focus {
  border-color: var(--highlight-text, var(--primary-text));
}

.pf-form-error {
  margin: 0 0 8px;
  padding: 6px 10px;
  border: 1px solid var(--pf-danger);
  border-radius: 6px;
  font-size: 12px;
  color: var(--pf-danger);
}

.pf-form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.pf-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14px;
}

.pf-btn.primary {
  background: var(--highlight-text, var(--primary-text));
  border-color: transparent;
  color: var(--primary-bg);
  font-weight: 600;
}

.pf-btn:disabled {
  opacity: 0.5;
}

.pf-spin {
  animation: pf-rotate 0.9s linear infinite;
}

@keyframes pf-rotate {
  to {
    transform: rotate(360deg);
  }
}

/* ---------- 确认弹层 ---------- */
.pf-confirm-mask {
  position: fixed;
  inset: 0;
  z-index: var(--layer-dialog, 60);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.55);
}

.pf-confirm {
  width: 100%;
  max-width: 360px;
  padding: 16px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
}

.pf-confirm-title {
  margin: 0 0 8px;
  font-size: 16px;
}

.pf-confirm-line {
  margin: 0 0 6px;
  font-size: 13px;
  line-height: 1.6;
  word-break: break-word;
}

.pf-confirm-warn {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 8px 0;
  font-size: 12px;
  color: var(--secondary-text, var(--primary-text));
}

.pf-confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 12px;
}

@media (prefers-reduced-motion: reduce) {
  .pf-spin {
    animation: none;
  }
}
</style>
