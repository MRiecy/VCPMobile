<script setup lang="ts">
/**
 * TaskEditorView.vue — 任务编辑器（S2b）。
 *
 * 全屏分节表单（替代桌面端 600px 模态框），作为 TaskCenterView 内部的
 * 滑入子页。randomN 魔法字符串控件化为「随机抽取」开关 + 人数步进器；
 * 占位符 chips 点击插入光标处（解决软键盘输入 {{...}} 的痛苦）。
 */
import { computed, reactive, ref } from 'vue';
import { ArrowLeft, Plus, Trash2, X } from 'lucide-vue-next';
import BottomSheet, { type ActionItem } from '../../components/ui/BottomSheet.vue';
import SettingsSwitch from '../../components/settings/SettingsSwitch.vue';
import { useOverlayStore } from '../../core/stores/overlay';
import { useTaskCenterStore } from './taskCenterStore';
import {
  CRON_PRESETS,
  MIN_INTERVAL_MINUTES,
  draftToPayload,
  emptyDraft,
  validateDraft,
  type TaskDraft,
} from './taskTypes';

const props = defineProps<{
  /** 编辑既有任务时的完整草稿；新建时为 null。 */
  initialDraft: TaskDraft | null;
}>();

const emit = defineEmits<{ close: [] }>();

const store = useTaskCenterStore();
const overlayStore = useOverlayStore();

const draft = reactive<TaskDraft>(
  props.initialDraft ? { ...props.initialDraft, schedule: { ...props.initialDraft.schedule }, agents: [...props.initialDraft.agents] } : emptyDraft(),
);
const isEditing = computed(() => !!draft.id);

// ---------- 调度 ----------
const SCHEDULE_MODES = [
  { value: 'interval', label: '间隔' },
  { value: 'once', label: '定时' },
  { value: 'cron', label: 'CRON' },
  { value: 'manual', label: '手动' },
] as const;

function stepInterval(delta: number): void {
  const next = (draft.schedule.intervalMinutes || MIN_INTERVAL_MINUTES) + delta;
  draft.schedule.intervalMinutes = Math.max(MIN_INTERVAL_MINUTES, next);
}

/** datetime-local 输入值 ↔ ISO 字符串 */
const runAtInput = computed({
  get: () => {
    if (!draft.schedule.runAt) return '';
    const timestamp = Date.parse(draft.schedule.runAt);
    if (!Number.isFinite(timestamp)) return '';
    const date = new Date(timestamp);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  },
  set: (value: string) => {
    draft.schedule.runAt = value ? new Date(value).toISOString() : null;
  },
});

// ---------- 目标 Agent ----------
const isAgentPickerOpen = ref(false);
const manualAgentName = ref('');

const agentPickerActions = computed<ActionItem[]>(() =>
  store.agentOptions
    .filter((option) => !draft.agents.includes(option.chineseName))
    .map((option) => ({
      label: option.description
        ? `${option.chineseName} — ${option.description.slice(0, 24)}`
        : option.chineseName,
      handler: () => {
        draft.agents.push(option.chineseName);
      },
    })),
);

function openAgentPicker(): void {
  void store.loadAgentOptions();
  isAgentPickerOpen.value = true;
}

function addManualAgent(): void {
  const name = manualAgentName.value.trim();
  if (!name || draft.agents.includes(name)) return;
  draft.agents.push(name);
  manualAgentName.value = '';
}

function removeAgent(name: string): void {
  draft.agents = draft.agents.filter((agent) => agent !== name);
}

function stepRandom(delta: number): void {
  const current = draft.randomCount ?? 1;
  draft.randomCount = Math.min(Math.max(1, current + delta), 30);
}

// ---------- 占位符 chips ----------
const promptTextarea = ref<HTMLTextAreaElement | null>(null);

const placeholderChips = computed<string[]>(() => {
  if (draft.type === 'forum_patrol') {
    const placeholder = draft.forumListPlaceholder.trim() || '{{forum_post_list}}';
    return placeholder === '{{forum_post_list}}'
      ? ['{{forum_post_list}}']
      : [placeholder, '{{forum_post_list}}'];
  }
  return [];
});

function insertPlaceholder(placeholder: string): void {
  const textarea = promptTextarea.value;
  if (!textarea) {
    draft.promptTemplate += placeholder;
    return;
  }
  const start = textarea.selectionStart ?? draft.promptTemplate.length;
  const end = textarea.selectionEnd ?? start;
  draft.promptTemplate =
    draft.promptTemplate.slice(0, start) + placeholder + draft.promptTemplate.slice(end);
  // 恢复焦点并将光标移到插入内容之后
  requestAnimationFrame(() => {
    textarea.focus();
    const cursor = start + placeholder.length;
    textarea.setSelectionRange(cursor, cursor);
  });
}

// ---------- 保存 / 删除 ----------
const validationError = computed(() => validateDraft(draft));

async function save(): Promise<void> {
  if (validationError.value) return;
  const ok = await store.saveTask(draft.id, draftToPayload(draft));
  if (ok) emit('close');
}

async function remove(): Promise<void> {
  const confirmed = await overlayStore.showConfirm({
    title: '删除任务',
    message: `确定删除任务「${draft.name}」吗？此操作不可撤销。`,
    isDanger: true,
  });
  if (!confirmed) return;
  const ok = await store.deleteTask(draft.id);
  if (ok) emit('close');
}
</script>

<template>
  <div class="tc-editor">
    <header class="tce-header">
      <button type="button" class="tce-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <span class="tce-title">{{ isEditing ? '编辑任务' : '新建任务' }}</span>
      <button
        v-if="isEditing"
        type="button"
        class="tce-icon-btn tce-delete-btn"
        aria-label="删除任务"
        :disabled="store.deletingId === draft.id"
        @click="remove()"
      >
        <Trash2 :size="17" />
      </button>
    </header>

    <div class="tce-scroll vcp-scrollable no-rubber-band">
      <!-- 基本 -->
      <section class="tce-section">
        <h3 class="tce-section-title">基本</h3>
        <label class="tce-field">
          <span class="tce-label">任务名称</span>
          <input v-model="draft.name" type="text" class="tce-input" placeholder="例如：晨间论坛巡航" />
        </label>
        <div class="tce-field">
          <span class="tce-label">任务类型</span>
          <div class="tce-segmented">
            <button
              type="button"
              class="tce-segment"
              :class="{ 'is-active': draft.type === 'forum_patrol' }"
              @click="draft.type = 'forum_patrol'"
            >
              论坛巡航
            </button>
            <button
              type="button"
              class="tce-segment"
              :class="{ 'is-active': draft.type === 'custom_prompt' }"
              @click="draft.type = 'custom_prompt'"
            >
              通用提示词
            </button>
          </div>
          <p class="tce-hint">
            {{ draft.type === 'forum_patrol'
              ? '预读取论坛帖子列表并填充进提示词模板。'
              : '直接向目标 Agent 派发自定义提示词。' }}
          </p>
        </div>
      </section>

      <!-- 调度 -->
      <section class="tce-section">
        <h3 class="tce-section-title">调度</h3>
        <div class="tce-segmented">
          <button
            v-for="mode in SCHEDULE_MODES"
            :key="mode.value"
            type="button"
            class="tce-segment"
            :class="{ 'is-active': draft.schedule.mode === mode.value }"
            @click="draft.schedule.mode = mode.value"
          >
            {{ mode.label }}
          </button>
        </div>

        <div v-if="draft.schedule.mode === 'interval'" class="tce-field">
          <span class="tce-label">间隔分钟（下限 {{ MIN_INTERVAL_MINUTES }}）</span>
          <div class="tce-stepper">
            <button type="button" class="tce-stepper-btn" @click="stepInterval(-10)">−</button>
            <span class="tce-stepper-value">{{ draft.schedule.intervalMinutes }}</span>
            <button type="button" class="tce-stepper-btn" @click="stepInterval(10)">＋</button>
          </div>
        </div>

        <div v-else-if="draft.schedule.mode === 'once'" class="tce-field">
          <span class="tce-label">执行时间（执行后自动禁用）</span>
          <input v-model="runAtInput" type="datetime-local" class="tce-input" />
        </div>

        <div v-else-if="draft.schedule.mode === 'cron'" class="tce-field">
          <span class="tce-label">CRON 表达式（秒 分 时 日 月 周）</span>
          <input
            v-model="draft.schedule.cronValue"
            type="text"
            class="tce-input tce-mono"
            placeholder="0 0 9 * * *"
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
          />
          <div class="tce-chips">
            <button
              v-for="preset in CRON_PRESETS"
              :key="preset.value"
              type="button"
              class="tce-chip"
              @click="draft.schedule.cronValue = preset.value"
            >
              {{ preset.label }}
            </button>
          </div>
        </div>

        <p v-else class="tce-hint">手动模式不参与定时调度，只能通过「立即触发」执行。</p>
      </section>

      <!-- 目标 Agent -->
      <section class="tce-section">
        <h3 class="tce-section-title">目标 Agent</h3>
        <div class="tce-agent-chips">
          <span v-for="agent in draft.agents" :key="agent" class="tce-agent-chip">
            {{ agent }}
            <button type="button" aria-label="移除" @click="removeAgent(agent)">
              <X :size="12" />
            </button>
          </span>
          <button type="button" class="tce-chip" @click="openAgentPicker">
            <Plus :size="13" /> 选择
          </button>
        </div>
        <div class="tce-manual-agent">
          <input
            v-model="manualAgentName"
            type="text"
            class="tce-input"
            placeholder="手动输入 Agent 中文名"
            @keyup.enter="addManualAgent"
          />
          <button type="button" class="tce-chip" @click="addManualAgent">添加</button>
        </div>

        <div class="tce-switch-row">
          <span class="tce-label">随机抽取</span>
          <SettingsSwitch
            :model-value="draft.randomCount !== null"
            @update:model-value="(on: boolean) => (draft.randomCount = on ? 1 : null)"
          />
        </div>
        <div v-if="draft.randomCount !== null" class="tce-field">
          <span class="tce-label">从候选中随机抽取人数</span>
          <div class="tce-stepper">
            <button type="button" class="tce-stepper-btn" @click="stepRandom(-1)">−</button>
            <span class="tce-stepper-value">{{ draft.randomCount }}</span>
            <button type="button" class="tce-stepper-btn" @click="stepRandom(1)">＋</button>
          </div>
          <p class="tce-hint">将从上方候选 Agent 中随机抽取 {{ draft.randomCount }} 人执行。</p>
        </div>
      </section>

      <!-- 派发选项 -->
      <section class="tce-section">
        <h3 class="tce-section-title">派发选项</h3>
        <label class="tce-field">
          <span class="tce-label">派发署名（Maid）</span>
          <input v-model="draft.maid" type="text" class="tce-input" placeholder="VCP系统" />
        </label>
        <div class="tce-switch-row">
          <span class="tce-label">临时通讯（不写入长期记忆）</span>
          <SettingsSwitch v-model="draft.temporaryContact" />
        </div>
        <div class="tce-switch-row">
          <span class="tce-label">异步委托模式（长任务后台执行）</span>
          <SettingsSwitch v-model="draft.taskDelegation" />
        </div>
      </section>

      <!-- 提示词 -->
      <section class="tce-section">
        <h3 class="tce-section-title">提示词模板</h3>
        <div v-if="placeholderChips.length" class="tce-chips">
          <button
            v-for="chip in placeholderChips"
            :key="chip"
            type="button"
            class="tce-chip tce-mono"
            @click="insertPlaceholder(chip)"
          >
            {{ chip }}
          </button>
        </div>
        <textarea
          ref="promptTextarea"
          v-model="draft.promptTemplate"
          class="tce-textarea"
          rows="7"
          placeholder="输入提示词模板…"
        />

        <template v-if="draft.type === 'forum_patrol'">
          <div class="tce-switch-row">
            <span class="tce-label">附带论坛帖子列表</span>
            <SettingsSwitch v-model="draft.includeForumPostList" />
          </div>
          <label v-if="draft.includeForumPostList" class="tce-field">
            <span class="tce-label">最多帖子数</span>
            <input
              v-model.number="draft.maxPosts"
              type="number"
              min="1"
              class="tce-input tce-mono"
            />
          </label>
        </template>
      </section>

      <div class="tce-footer-spacer" />
    </div>

    <!-- 底部操作条 -->
    <footer class="tce-footer">
      <p v-if="validationError" class="tce-validation">{{ validationError }}</p>
      <button
        type="button"
        class="tce-save-btn"
        :disabled="!!validationError || store.saving"
        @click="save()"
      >
        {{ store.saving ? '保存中…' : isEditing ? '保存更改' : '创建任务' }}
      </button>
    </footer>

    <BottomSheet v-model="isAgentPickerOpen" title="选择目标 Agent" :actions="agentPickerActions" />
  </div>
</template>

<style scoped>
.tc-editor {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.tce-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.tce-title {
  flex: 1;
  font-size: 16px;
  font-weight: 800;
}

.tce-icon-btn {
  width: 40px;
  height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.65;
}

.tce-delete-btn {
  color: #ef4444;
  opacity: 0.9;
}

.tce-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.tce-section {
  margin-bottom: 18px;
}

.tce-section-title {
  margin: 0 0 10px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.tce-field {
  display: block;
  margin-bottom: 12px;
}

.tce-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.6;
  margin-bottom: 6px;
}

.tce-input {
  width: 100%;
  box-sizing: border-box;
  height: 40px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 13px;
  outline: none;
}

.tce-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.tce-hint {
  margin: 6px 0 0;
  font-size: 11px;
  opacity: 0.5;
}

.tce-segmented {
  display: flex;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.tce-segment {
  flex: 1;
  min-height: 38px;
  border: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
  opacity: 0.55;
}

.tce-segment.is-active {
  opacity: 1;
  background: var(--secondary-bg);
  color: var(--highlight-text);
}

.tce-stepper {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.tce-stepper-btn {
  width: 44px;
  height: 40px;
  border: none;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 16px;
  font-weight: 700;
}

.tce-stepper-value {
  min-width: 56px;
  text-align: center;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 14px;
  font-weight: 700;
}

.tce-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}

.tce-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-height: 32px;
  padding: 0 12px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 11px;
  font-weight: 700;
}

.tce-agent-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 10px;
}

.tce-agent-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 32px;
  padding: 0 6px 0 12px;
  border-radius: 999px;
  border: 1px solid var(--highlight-text);
  color: var(--highlight-text);
  font-size: 12px;
  font-weight: 700;
}

.tce-agent-chip button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: inherit;
  opacity: 0.7;
}

.tce-manual-agent {
  display: flex;
  gap: 8px;
  margin-bottom: 12px;
}

.tce-manual-agent .tce-input {
  flex: 1;
}

.tce-switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 0;
}

.tce-switch-row .tce-label {
  margin-bottom: 0;
}

.tce-textarea {
  width: 100%;
  box-sizing: border-box;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 13px;
  line-height: 1.6;
  resize: vertical;
  outline: none;
  font-family: inherit;
}

.tce-footer-spacer {
  height: 8px;
}

.tce-footer {
  flex-shrink: 0;
  padding: 10px 14px calc(var(--vcp-safe-bottom, 48px) + 10px);
  border-top: 1px solid var(--border-color);
}

.tce-validation {
  margin: 0 0 8px;
  font-size: 11px;
  color: #f59e0b;
}

.tce-save-btn {
  width: 100%;
  min-height: 44px;
  border: none;
  border-radius: 10px;
  background: var(--highlight-text);
  color: #fff;
  font-size: 14px;
  font-weight: 800;
}

.tce-save-btn:disabled {
  opacity: 0.4;
}

@media (min-width: 768px) {
  .tce-scroll,
  .tce-footer {
    max-width: 640px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
