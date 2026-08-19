<script setup lang="ts">
/**
 * AgentMgrView.vue — Agent 管理（AgentAssistant agents CRUD）。
 *
 * 独立「更多」入口（08 篇 §6 裁决）。高密度线性列表 + 滑入子页编辑器；
 * 「全局设置」Tab 编辑 7 个顶层字段。改名/删除前的任务引用扫描在编辑器内完成。
 */
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { ArrowLeft, Bot, ChevronRight, Plus, RefreshCw } from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import AgentEditorView from './AgentEditorView.vue';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useAgentMgrStore } from './agentMgrStore';
import { nameInitial } from '../../core/utils/nameHue';
import {
  emptyAgentDraft,
  type AgentDraft,
  type AgentEntry,
  type GlobalConfig,
} from './agentMgrTypes';

const props = withDefaults(defineProps<{ isOpen?: boolean; zIndex?: number }>(), {
  isOpen: false,
  zIndex: 40,
});

const emit = defineEmits<{ close: [] }>();

const store = useAgentMgrStore();

// 模板插值无法内联包含 }} 的字符串字面量，提示文案走 v-text
const delegationTemplateHint =
  '可用占位符：{{SenderName}}、{{TaskPrompt}}。留空使用内置默认。';

const activeTab = ref<'agents' | 'global'>('agents');

// ---------- 编辑器（滑入子页） ----------
const editorDraft = ref<AgentDraft | null>(null);
const isEditorOpen = ref(false);

const { registerModal, unregisterModal } = useModalHistory();
const EDITOR_MODAL_ID = 'AgentMgr:Editor';

watch(isEditorOpen, (open) => {
  if (open) registerModal(EDITOR_MODAL_ID, () => closeEditor());
  else unregisterModal(EDITOR_MODAL_ID);
});

function openCreateEditor(): void {
  void store.loadModels();
  editorDraft.value = emptyAgentDraft();
  isEditorOpen.value = true;
}

function openEditEditor(entry: AgentEntry): void {
  void store.loadModels();
  editorDraft.value = {
    originalName: entry.chineseName,
    chineseName: entry.chineseName,
    baseName: entry.baseName,
    modelId: entry.modelId,
    description: entry.description,
    systemPrompt: entry.systemPrompt,
    maxOutputTokens: entry.maxOutputTokens,
    temperature: entry.temperature,
    extras: { ...entry.extras },
  };
  isEditorOpen.value = true;
}

function closeEditor(): void {
  isEditorOpen.value = false;
  editorDraft.value = null;
}

// ---------- 全局设置草稿 ----------
const globalDraft = ref<GlobalConfig>({ ...store.globalConfig });

watch(
  () => store.configLoaded,
  (loaded) => {
    if (loaded) globalDraft.value = { ...store.globalConfig };
  },
);

const delegationTimeoutMinutes = computed({
  get: () => Math.round(globalDraft.value.delegationTimeout / 60000),
  set: (minutes: number) => {
    globalDraft.value.delegationTimeout = Math.max(1, Math.round(minutes)) * 60000;
  },
});

function stepGlobal(field: 'maxHistoryRounds' | 'contextTtlHours' | 'delegationMaxRounds', delta: number): void {
  globalDraft.value[field] = Math.max(1, (globalDraft.value[field] || 1) + delta);
}

function stepTimeout(deltaMinutes: number): void {
  delegationTimeoutMinutes.value = delegationTimeoutMinutes.value + deltaMinutes;
}

async function saveGlobal(): Promise<void> {
  await store.saveGlobalConfig({ ...globalDraft.value });
}

// ---------- 会话 ----------
watch(
  () => props.isOpen,
  (open) => {
    if (open) void store.loadConfig();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  unregisterModal(EDITOR_MODAL_ID);
  store.resetSession();
});

// ---------- 空态 ----------
const emptyState = computed(() => {
  if (store.error && !store.configLoaded) {
    const pluginDown = store.error.includes('PLUGIN_UNAVAILABLE');
    return {
      title: pluginDown ? 'AgentAssistant 插件不可用' : '连接失败',
      detail: pluginDown
        ? '请在 VCPToolBox 服务器上确认 AgentAssistant 插件已加载。'
        : store.error,
    };
  }
  return null;
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="agent-mgr">
      <!-- 顶栏 -->
      <header class="am-header">
        <button type="button" class="am-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <div class="am-title-block">
          <span class="am-title">Agent 管理</span>
          <span class="am-subtitle">AgentAssistant</span>
        </div>
        <button
          type="button"
          class="am-icon-btn"
          aria-label="刷新配置"
          title="刷新配置"
          @click="store.loadConfig()"
        >
          <RefreshCw :size="17" :class="{ 'custom-spin': store.isLoading }" />
        </button>
      </header>

      <!-- 分段控件 -->
      <nav class="am-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          class="am-tab"
          :class="{ 'is-active': activeTab === 'agents' }"
          :aria-selected="activeTab === 'agents'"
          @click="activeTab = 'agents'"
        >
          Agent 列表
        </button>
        <button
          type="button"
          role="tab"
          class="am-tab"
          :class="{ 'is-active': activeTab === 'global' }"
          :aria-selected="activeTab === 'global'"
          @click="activeTab = 'global'"
        >
          全局设置
        </button>
      </nav>

      <!-- 整页空态 -->
      <div v-if="emptyState" class="am-empty">
        <Bot :size="28" class="am-empty-icon" />
        <p class="am-empty-title">{{ emptyState.title }}</p>
        <p class="am-empty-detail">{{ emptyState.detail }}</p>
        <button type="button" class="am-retry-btn" @click="store.loadConfig()">重试</button>
      </div>

      <!-- Agent 列表 -->
      <div
        v-else-if="activeTab === 'agents'"
        class="am-scroll vcp-scrollable no-rubber-band"
        data-agentmgr-role="agent-list"
      >
        <div v-if="store.agents.length === 0" class="am-empty">
          <p class="am-empty-title">{{ store.isLoading ? '正在读取配置…' : '尚未配置 Agent' }}</p>
          <p v-if="!store.isLoading" class="am-empty-detail">
            点击下方「新建 Agent」创建第一个可调度 Agent。
          </p>
        </div>

        <button
          v-for="entry in store.agents"
          :key="entry.chineseName"
          type="button"
          class="am-row"
          @click="openEditEditor(entry)"
        >
          <span class="am-avatar" aria-hidden="true">{{ nameInitial(entry.chineseName) }}</span>
          <span class="am-row-main">
            <span class="am-row-name">{{ entry.chineseName }}</span>
            <span class="am-row-model">{{ entry.modelId || '未绑定模型' }}</span>
            <span v-if="entry.description" class="am-row-desc">{{ entry.description }}</span>
            <span class="am-row-meta">
              {{ entry.baseName || entry.chineseName.toUpperCase() }} · tokens {{ entry.maxOutputTokens }} · temp {{ entry.temperature }}
            </span>
          </span>
          <ChevronRight :size="16" class="am-row-chevron" aria-hidden="true" />
        </button>

        <button type="button" class="am-create-btn" @click="openCreateEditor">
          <Plus :size="15" />
          新建 Agent
        </button>
      </div>

      <!-- 全局设置 -->
      <div
        v-else
        class="am-scroll vcp-scrollable no-rubber-band"
        data-agentmgr-role="global-settings"
      >
        <section class="am-section">
          <h3 class="am-section-title">会话</h3>
          <div class="am-field">
            <span class="am-label">历史保留轮数（每 Agent）</span>
            <div class="am-stepper">
              <button type="button" class="am-stepper-btn" @click="stepGlobal('maxHistoryRounds', -1)">−</button>
              <span class="am-stepper-value">{{ globalDraft.maxHistoryRounds }}</span>
              <button type="button" class="am-stepper-btn" @click="stepGlobal('maxHistoryRounds', 1)">＋</button>
            </div>
          </div>
          <div class="am-field">
            <span class="am-label">上下文 TTL（小时）</span>
            <div class="am-stepper">
              <button type="button" class="am-stepper-btn" @click="stepGlobal('contextTtlHours', -1)">−</button>
              <span class="am-stepper-value">{{ globalDraft.contextTtlHours }}</span>
              <button type="button" class="am-stepper-btn" @click="stepGlobal('contextTtlHours', 1)">＋</button>
            </div>
          </div>
          <label class="am-field">
            <span class="am-label">共享系统提示词（追加到每个 Agent 之后）</span>
            <textarea
              v-model="globalDraft.globalSystemPrompt"
              class="am-textarea"
              rows="3"
              placeholder="留空 = 不追加"
            />
          </label>
        </section>

        <section class="am-section">
          <h3 class="am-section-title">异步委托</h3>
          <div class="am-field">
            <span class="am-label">最大唤醒轮数</span>
            <div class="am-stepper">
              <button type="button" class="am-stepper-btn" @click="stepGlobal('delegationMaxRounds', -1)">−</button>
              <span class="am-stepper-value">{{ globalDraft.delegationMaxRounds }}</span>
              <button type="button" class="am-stepper-btn" @click="stepGlobal('delegationMaxRounds', 1)">＋</button>
            </div>
          </div>
          <div class="am-field">
            <span class="am-label">总超时（分钟）</span>
            <div class="am-stepper">
              <button type="button" class="am-stepper-btn" @click="stepTimeout(-1)">−</button>
              <span class="am-stepper-value">{{ delegationTimeoutMinutes }}</span>
              <button type="button" class="am-stepper-btn" @click="stepTimeout(1)">＋</button>
            </div>
          </div>
          <label class="am-field">
            <span class="am-label">委托系统提示词模板</span>
            <textarea
              v-model="globalDraft.delegationSystemPrompt"
              class="am-textarea"
              rows="4"
              placeholder="留空 = 使用后端内置模板"
            />
            <p class="am-hint" v-text="delegationTemplateHint" />
          </label>
          <label class="am-field">
            <span class="am-label">委托催促提示词</span>
            <textarea
              v-model="globalDraft.delegationHeartbeatPrompt"
              class="am-textarea"
              rows="3"
              placeholder="留空 = 使用后端内置模板"
            />
          </label>
        </section>

        <footer class="am-global-footer">
          <button
            type="button"
            class="am-save-btn"
            :disabled="store.saving"
            @click="saveGlobal"
          >
            {{ store.saving ? '保存中…' : '保存全局设置' }}
          </button>
        </footer>
      </div>

      <!-- 编辑器（滑入子页） -->
      <Transition name="am-editor-slide">
        <AgentEditorView
          v-if="isEditorOpen && editorDraft"
          :initial-draft="editorDraft"
          @close="closeEditor"
        />
      </Transition>
    </div>
  </SlidePage>
</template>

<style scoped>
.agent-mgr {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.am-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.am-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.am-title {
  font-size: 16px;
  font-weight: 800;
}

.am-subtitle {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  opacity: 0.45;
  text-transform: uppercase;
}

.am-icon-btn {
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

.am-icon-btn:active {
  opacity: 1;
}

.am-tabs {
  display: flex;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border-color);
  padding: 0 14px;
  gap: 18px;
}

.am-tab {
  padding: 10px 2px;
  border: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
  font-weight: 700;
  opacity: 0.5;
  border-bottom: 2px solid transparent;
}

.am-tab.is-active {
  opacity: 1;
  border-bottom-color: var(--highlight-text);
}

.am-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 10px 12px calc(var(--vcp-safe-bottom, 48px) + 12px);
}

.am-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px 24px;
  text-align: center;
}

.am-empty-icon {
  opacity: 0.35;
}

.am-empty-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  opacity: 0.75;
}

.am-empty-detail {
  margin: 0;
  font-size: 12px;
  opacity: 0.5;
  max-width: 28rem;
  word-break: break-all;
}

.am-retry-btn {
  margin-top: 6px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

/* ---- Agent 卡片行（实体卡：弱化分隔线，按对象分组信息） ---- */
.am-row {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  margin-bottom: 8px;
  padding: 12px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  text-align: left;
  transition: opacity 0.15s ease;
}

.am-row:active {
  opacity: 0.75;
}

.am-avatar {
  width: 42px;
  height: 42px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 12px;
  border: 1px solid var(--border-color);
  background: var(--primary-bg);
  color: var(--highlight-text);
  font-size: 17px;
  font-weight: 800;
}

.am-row-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.am-row-name {
  font-size: 14px;
  font-weight: 800;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.am-row-model {
  align-self: flex-start;
  max-width: 100%;
  padding: 2px 8px;
  border-radius: 6px;
  background: var(--primary-bg);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.75;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.am-row-chevron {
  flex-shrink: 0;
  opacity: 0.3;
}

.am-row-desc {
  font-size: 11.5px;
  opacity: 0.55;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.am-row-meta {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.am-create-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  width: 100%;
  min-height: 44px;
  margin-top: 10px;
  border: 1px dashed var(--border-color);
  border-radius: 10px;
  background: transparent;
  color: var(--highlight-text);
  font-size: 13px;
  font-weight: 700;
}

/* ---- 全局设置表单 ---- */
.am-section {
  margin-bottom: 18px;
}

.am-section-title {
  margin: 0 0 10px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.am-field {
  display: block;
  margin-bottom: 12px;
}

.am-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.6;
  margin-bottom: 6px;
}

.am-hint {
  margin: 6px 0 0;
  font-size: 11px;
  opacity: 0.5;
}

.am-stepper {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  overflow: hidden;
}

.am-stepper-btn {
  width: 44px;
  height: 40px;
  border: none;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 16px;
  font-weight: 700;
}

.am-stepper-value {
  min-width: 56px;
  text-align: center;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 14px;
  font-weight: 700;
}

.am-textarea {
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

.am-global-footer {
  padding-bottom: 8px;
}

.am-save-btn {
  width: 100%;
  min-height: 44px;
  border: none;
  border-radius: 10px;
  background: var(--highlight-text);
  color: #fff;
  font-size: 14px;
  font-weight: 800;
}

.am-save-btn:disabled {
  opacity: 0.4;
}

/* 编辑器滑入动画（内敛：位移 + 透明度） */
.am-editor-slide-enter-active,
.am-editor-slide-leave-active {
  transition:
    transform 0.3s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.3s ease;
}

.am-editor-slide-enter-from,
.am-editor-slide-leave-to {
  transform: translateX(100%);
  opacity: 0.6;
}

@media (min-width: 768px) {
  .am-scroll,
  .am-tabs {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
