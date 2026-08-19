<script setup lang="ts">
/**
 * AgentEditorView.vue — Agent 编辑器（新建/编辑共用，滑入子页）。
 *
 * 7 字段表单：chineseName（必填+唯一）、baseName（留空回退大写）、modelId
 * （模型选择器 + 可手输）、description、systemPrompt（占位符 chips）、
 * maxOutputTokens、temperature。
 * 改名/删除前扫描任务调度中心的引用并列出受影响任务（引用完整性，08 篇 §6）。
 */
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue';
import { ArrowLeft, Plus, Search, Trash2, X } from 'lucide-vue-next';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useOverlayStore } from '../../core/stores/overlay';
import { useAgentMgrStore } from './agentMgrStore';
import { validateAgentDraft, type AgentDraft } from './agentMgrTypes';

const props = defineProps<{ initialDraft: AgentDraft }>();
const emit = defineEmits<{ close: [] }>();

const store = useAgentMgrStore();
const overlayStore = useOverlayStore();

const draft = reactive<AgentDraft>({ ...props.initialDraft, extras: { ...props.initialDraft.extras } });
const isEditing = computed(() => draft.originalName !== null);
const isRenaming = computed(
  () => isEditing.value && draft.chineseName.trim() !== draft.originalName,
);

// ---------- 模型选择器 ----------
const isModelPickerOpen = ref(false);
const modelSearch = ref('');

const { registerModal, unregisterModal } = useModalHistory();
const PICKER_MODAL_ID = 'AgentMgr:ModelPicker';

watch(isModelPickerOpen, (open) => {
  if (open) registerModal(PICKER_MODAL_ID, () => closeModelPicker());
  else unregisterModal(PICKER_MODAL_ID);
});

onBeforeUnmount(() => {
  unregisterModal(PICKER_MODAL_ID);
});

const filteredModels = computed(() => {
  const keyword = modelSearch.value.trim().toLowerCase();
  if (!keyword) return store.models;
  return store.models.filter((model) => model.toLowerCase().includes(keyword));
});

function openModelPicker(): void {
  void store.loadModels();
  modelSearch.value = '';
  isModelPickerOpen.value = true;
}

function closeModelPicker(): void {
  isModelPickerOpen.value = false;
}

function pickModel(modelId: string): void {
  draft.modelId = modelId;
  closeModelPicker();
}

// ---------- 占位符 chips ----------
const promptTextarea = ref<HTMLTextAreaElement | null>(null);
const PLACEHOLDER_CHIPS = ['{{MaidName}}', '{{角色卡别名}}'];
// 模板插值无法内联包含 }} 的字符串字面量，提示文案走 v-text
const systemPromptHint =
  '{{MaidName}} 运行时替换为中文名；{{角色卡别名}} 引用服务器 Agent 目录的角色卡。';

function insertPlaceholder(placeholder: string): void {
  const textarea = promptTextarea.value;
  if (!textarea) {
    draft.systemPrompt += placeholder;
    return;
  }
  const start = textarea.selectionStart ?? draft.systemPrompt.length;
  const end = textarea.selectionEnd ?? start;
  draft.systemPrompt =
    draft.systemPrompt.slice(0, start) + placeholder + draft.systemPrompt.slice(end);
  requestAnimationFrame(() => {
    textarea.focus();
    const cursor = start + placeholder.length;
    textarea.setSelectionRange(cursor, cursor);
  });
}

// ---------- 数值控件 ----------
function stepTemperature(delta: number): void {
  const next = Math.round(((draft.temperature || 0) + delta) * 10) / 10;
  draft.temperature = Math.min(2, Math.max(0, next));
}

// ---------- 校验 ----------
const otherNames = computed(() =>
  store.agents
    .map((entry) => entry.chineseName)
    .filter((name) => name !== draft.originalName),
);
const validationError = computed(() => validateAgentDraft(draft, otherNames.value));

// ---------- 保存（改名时先引用扫描） ----------
async function save(force = false): Promise<void> {
  if (validationError.value) return;

  if (isRenaming.value && !force) {
    const affected = await store.scanTaskReferences(draft.originalName!);
    const detail =
      affected.length > 0
        ? `以下任务的目标 Agent 引用了「${draft.originalName}」，改名后将静默失效：\n${affected.join('、')}\n\n建议改名后同步更新这些任务。确定继续吗？`
        : `改名会使派发标识从「${draft.originalName}」变为「${draft.chineseName.trim()}」。当前没有任务引用旧名。确定继续吗？`;
    const confirmed = await overlayStore.showConfirm({
      title: '确认改名',
      message: detail,
    });
    if (!confirmed) return;
  }

  const result = await store.saveAgent(draft, { force });
  if (result === 'ok') {
    emit('close');
    return;
  }
  if (result === 'conflict') {
    const overwrite = await overlayStore.showConfirm({
      title: '配置已被他端修改',
      message: 'Agent 配置在你编辑期间已被其他客户端修改。继续保存将覆盖对方的更改，确定吗？',
      isDanger: true,
    });
    if (overwrite) await save(true);
  }
}

// ---------- 删除（引用扫描 + 二次确认） ----------
async function remove(): Promise<void> {
  const affected = await store.scanTaskReferences(draft.chineseName.trim());
  const detail =
    affected.length > 0
      ? `以下任务的目标 Agent 引用了「${draft.chineseName.trim()}」，删除后这些任务将静默失效：\n${affected.join('、')}\n\n此操作不可撤销，确定删除吗？`
      : `确定删除 Agent「${draft.chineseName.trim()}」吗？此操作不可撤销。`;
  const confirmed = await overlayStore.showConfirm({
    title: '删除 Agent',
    message: detail,
    isDanger: true,
  });
  if (!confirmed) return;

  const result = await store.deleteAgent(draft.originalName ?? draft.chineseName.trim());
  if (result === 'ok') {
    emit('close');
    return;
  }
  if (result === 'conflict') {
    const overwrite = await overlayStore.showConfirm({
      title: '配置已被他端修改',
      message: 'Agent 配置在你编辑期间已被其他客户端修改。仍要删除该 Agent 吗？',
      isDanger: true,
    });
    if (overwrite) {
      const retry = await store.deleteAgent(draft.originalName ?? draft.chineseName.trim(), {
        force: true,
      });
      if (retry === 'ok') emit('close');
    }
  }
}
</script>

<template>
  <div class="agent-editor">
    <header class="ae-header">
      <button type="button" class="ae-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <span class="ae-title">{{ isEditing ? '编辑 Agent' : '新建 Agent' }}</span>
      <button
        v-if="isEditing"
        type="button"
        class="ae-icon-btn ae-delete-btn"
        aria-label="删除 Agent"
        :disabled="store.deleting"
        @click="remove()"
      >
        <Trash2 :size="17" />
      </button>
    </header>

    <div class="ae-scroll vcp-scrollable no-rubber-band">
      <!-- 标识 -->
      <section class="ae-section">
        <h3 class="ae-section-title">标识</h3>
        <label class="ae-field">
          <span class="ae-label">中文名（chineseName · 派发唯一标识）</span>
          <input
            v-model="draft.chineseName"
            type="text"
            class="ae-input"
            placeholder="例如：小娜"
          />
        </label>
        <label class="ae-field">
          <span class="ae-label">内部标识（baseName · 选填）</span>
          <input
            v-model="draft.baseName"
            type="text"
            class="ae-input ae-mono"
            :placeholder="`留空 = ${(draft.chineseName.trim() || '中文名').toUpperCase()}`"
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
          />
        </label>
        <label class="ae-field">
          <span class="ae-label">角色描述（供其他 Agent 了解它）</span>
          <input
            v-model="draft.description"
            type="text"
            class="ae-input"
            placeholder="例如：擅长代码审查与重构"
          />
        </label>
      </section>

      <!-- 模型 -->
      <section class="ae-section">
        <h3 class="ae-section-title">模型</h3>
        <div class="ae-field">
          <span class="ae-label">绑定模型（modelId）</span>
          <div class="ae-model-row">
            <input
              v-model="draft.modelId"
              type="text"
              class="ae-input ae-mono ae-model-input"
              placeholder="default / gpt-4o / …"
              autocapitalize="off"
              autocorrect="off"
              spellcheck="false"
            />
            <button type="button" class="ae-model-pick" @click="openModelPicker">
              <Search :size="14" />
              <span>选择</span>
            </button>
          </div>
          <p v-if="store.modelsError" class="ae-hint">模型列表不可用，请手动输入。</p>
        </div>
        <div class="ae-field">
          <span class="ae-label">maxOutputTokens（默认 40000）</span>
          <input
            v-model.number="draft.maxOutputTokens"
            type="number"
            min="1"
            class="ae-input ae-mono"
          />
        </div>
        <div class="ae-field">
          <span class="ae-label">temperature（0 ~ 2，默认 0.7）</span>
          <div class="ae-stepper">
            <button type="button" class="ae-stepper-btn" @click="stepTemperature(-0.1)">−</button>
            <span class="ae-stepper-value">{{ draft.temperature.toFixed(1) }}</span>
            <button type="button" class="ae-stepper-btn" @click="stepTemperature(0.1)">＋</button>
          </div>
        </div>
      </section>

      <!-- 系统提示词 -->
      <section class="ae-section">
        <h3 class="ae-section-title">系统提示词</h3>
        <div class="ae-chips">
          <button
            v-for="chip in PLACEHOLDER_CHIPS"
            :key="chip"
            type="button"
            class="ae-ph-chip"
            @click="insertPlaceholder(chip)"
          >
            <Plus :size="11" />
            <span>{{ chip }}</span>
          </button>
        </div>
        <textarea
          ref="promptTextarea"
          v-model="draft.systemPrompt"
          class="ae-textarea"
          rows="7"
          placeholder="留空 = You are a helpful AI assistant named {{MaidName}}."
        />
        <p class="ae-hint" v-text="systemPromptHint" />
      </section>

      <div class="ae-footer-spacer" />
    </div>

    <!-- 底部操作条 -->
    <footer class="ae-footer">
      <p v-if="validationError" class="ae-validation">{{ validationError }}</p>
      <p v-else-if="isRenaming" class="ae-rename-hint">
        改名：{{ draft.originalName }} → {{ draft.chineseName.trim() || '…' }}
      </p>
      <button
        type="button"
        class="ae-save-btn"
        :disabled="!!validationError || store.saving"
        @click="save()"
      >
        {{ store.saving ? '保存中…' : isEditing ? '保存更改' : '创建 Agent' }}
      </button>
    </footer>

    <!-- 模型选择器（滑升面板） -->
    <Transition name="ae-fade">
      <div v-if="isModelPickerOpen" class="ae-picker-mask" @click="closeModelPicker" @touchmove.prevent />
    </Transition>
    <Transition name="ae-picker-slide">
      <section v-if="isModelPickerOpen" class="ae-picker" role="dialog" aria-label="选择模型">
        <header class="ae-picker-header">
          <span class="ae-picker-title">选择模型</span>
          <button type="button" class="ae-icon-btn" aria-label="关闭" @click="closeModelPicker">
            <X :size="17" />
          </button>
        </header>
        <div class="ae-picker-search">
          <Search :size="14" class="ae-picker-search-icon" />
          <input
            v-model="modelSearch"
            type="search"
            class="ae-picker-search-input"
            placeholder="搜索模型 ID…"
            enterkeyhint="search"
          />
        </div>
        <div class="ae-picker-list vcp-scrollable">
          <div v-if="filteredModels.length === 0" class="ae-picker-empty">
            {{ store.models.length === 0 ? '模型列表不可用，请手动输入' : '没有匹配的模型' }}
          </div>
          <button
            v-for="model in filteredModels"
            :key="model"
            type="button"
            class="ae-picker-row"
            :class="{ 'is-active': model === draft.modelId }"
            @click="pickModel(model)"
          >
            <span class="ae-picker-row-name ae-mono">{{ model }}</span>
          </button>
        </div>
      </section>
    </Transition>
  </div>
</template>

<style scoped>
.agent-editor {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.ae-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.ae-title {
  flex: 1;
  font-size: 16px;
  font-weight: 800;
}

.ae-icon-btn {
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

.ae-delete-btn {
  color: #ef4444;
  opacity: 0.9;
}

.ae-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.ae-section {
  margin-bottom: 18px;
}

.ae-section-title {
  margin: 0 0 10px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.ae-field {
  display: block;
  margin-bottom: 12px;
}

.ae-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.6;
  margin-bottom: 6px;
}

.ae-input {
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

.ae-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.ae-hint {
  margin: 6px 0 0;
  font-size: 11px;
  opacity: 0.5;
}

.ae-model-row {
  display: flex;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.ae-model-input {
  flex: 1;
  border: none !important;
  border-radius: 0 !important;
  background: var(--secondary-bg);
}

.ae-model-pick {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-width: 72px;
  padding: 0 12px;
  border: none;
  border-left: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--highlight-text);
  font-size: 12px;
  font-weight: 700;
}

.ae-stepper {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.ae-stepper-btn {
  width: 44px;
  height: 40px;
  border: none;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 16px;
  font-weight: 700;
}

.ae-stepper-value {
  min-width: 56px;
  text-align: center;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 14px;
  font-weight: 700;
}

/* 占位符注入按钮：token 风格（与任务编辑器同族） */
.ae-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 8px;
}

.ae-ph-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-height: 30px;
  padding: 0 11px;
  border-radius: 6px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--highlight-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  font-weight: 600;
  line-height: 1;
}

.ae-ph-chip:active {
  opacity: 0.75;
}

@supports (background-color: color-mix(in srgb, black, transparent)) {
  .ae-ph-chip {
    border-color: color-mix(in srgb, var(--highlight-text) 40%, transparent);
    background: color-mix(in srgb, var(--highlight-text) 7%, transparent);
  }
}

.ae-textarea {
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

.ae-footer-spacer {
  height: 8px;
}

.ae-footer {
  flex-shrink: 0;
  padding: 10px 14px calc(var(--vcp-safe-bottom, 48px) + 10px);
  border-top: 1px solid var(--border-color);
}

.ae-validation {
  margin: 0 0 8px;
  font-size: 11px;
  color: #f59e0b;
}

.ae-rename-hint {
  margin: 0 0 8px;
  font-size: 11px;
  color: var(--highlight-text);
  opacity: 0.8;
}

.ae-save-btn {
  width: 100%;
  min-height: 44px;
  border: none;
  border-radius: 10px;
  background: var(--highlight-text);
  color: #fff;
  font-size: 14px;
  font-weight: 800;
}

.ae-save-btn:disabled {
  opacity: 0.4;
}

/* ---- 模型选择器滑升面板（与任务编辑器选择器同款） ---- */
.ae-picker-mask {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  z-index: var(--layer-local);
}

.ae-picker {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  max-height: 68%;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  border-top: 1px solid var(--border-color);
  border-radius: 14px 14px 0 0;
  padding-bottom: calc(var(--vcp-safe-bottom, 48px) + 8px);
  z-index: var(--layer-local);
}

.ae-picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.ae-picker-title {
  font-size: 13px;
  font-weight: 800;
}

.ae-picker-search {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 10px 14px 6px;
  height: 38px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  flex-shrink: 0;
}

.ae-picker-search-icon {
  opacity: 0.45;
  flex-shrink: 0;
}

.ae-picker-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
}

.ae-picker-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px 8px 8px;
}

.ae-picker-empty {
  padding: 28px 16px;
  text-align: center;
  font-size: 12px;
  opacity: 0.5;
}

.ae-picker-row {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 10px;
  border: none;
  border-left: 2px solid transparent;
  border-bottom: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.ae-picker-row.is-active {
  border-left-color: var(--highlight-text);
}

.ae-picker-row-name {
  font-size: 12.5px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ae-fade-enter-active,
.ae-fade-leave-active {
  transition: opacity 0.2s ease;
}

.ae-fade-enter-from,
.ae-fade-leave-to {
  opacity: 0;
}

.ae-picker-slide-enter-active,
.ae-picker-slide-leave-active {
  transition: transform 0.28s cubic-bezier(0.32, 0.72, 0, 1);
}

.ae-picker-slide-enter-from,
.ae-picker-slide-leave-to {
  transform: translateY(100%);
}

@media (min-width: 768px) {
  .ae-scroll,
  .ae-footer {
    max-width: 640px;
    width: 100%;
    margin: 0 auto;
  }

  .ae-picker {
    left: 50%;
    right: auto;
    width: 520px;
    transform: translateX(-50%);
  }

  .ae-picker-slide-enter-from,
  .ae-picker-slide-leave-to {
    transform: translateX(-50%) translateY(100%);
  }
}
</style>
