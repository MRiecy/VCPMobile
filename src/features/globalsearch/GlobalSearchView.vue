<script setup lang="ts">
/**
 * GlobalSearchView.vue — 全局消息搜索页。
 *
 * 分层定位：SlidePage 页面栈成员（overlay 类型 'globalSearch'），
 * 检索层走本地 SQLite FTS5（trigram），离线可用。
 *
 * 首开时检测 FTS 索引覆盖率（决策 G），不足则触发后台分批回填并展示进度。
 * 结果项点击 → 关闭搜索页 → 切换会话 → 锚点窗口加载 → scrollIntoView + 高亮闪烁。
 */
import { computed, nextTick, ref, watch } from 'vue';
import { ArrowLeft, ArrowUpDown, ChevronDown, Clock, Search, SlidersHorizontal, X } from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import { useOverlayStore, type GlobalSearchOpenTarget } from '../../core/stores/overlay';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useChatHistoryStore } from '../../core/stores/chatHistoryStore';
import { useAssistantStore } from '../../core/stores/assistant';
import { useTopicStore } from '../../core/stores/topicListManager';
import { useNotificationStore } from '../../core/stores/notification';
import { scrollToMessageById } from '../../core/utils/scrollToMessage';
import { useGlobalSearchStore } from './globalSearchStore';
import {
  ROLE_LABELS,
  SEARCH_MIN_CHARS,
  TIME_LABELS,
  type FtsSearchResultItem,
  type RoleFilter,
  type TimeFilter,
} from './types';

const props = defineProps<{
  isOpen: boolean;
  zIndex: number;
  openTarget?: GlobalSearchOpenTarget | null;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'target-consumed'): void;
}>();

const store = useGlobalSearchStore();
const overlayStore = useOverlayStore();
const sessionStore = useChatSessionStore();
const historyStore = useChatHistoryStore();
const assistantStore = useAssistantStore();
const topicStore = useTopicStore();
const notificationStore = useNotificationStore();

const searchInput = ref<HTMLInputElement | null>(null);
let debounceTimer: ReturnType<typeof setTimeout> | null = null;

// ---------- 生命周期 ----------
watch(
  () => props.isOpen,
  async (open) => {
    if (!open) {
      store.reset();
      return;
    }
    if (props.openTarget) {
      store.applyOpenTarget(props.openTarget);
      emit('target-consumed');
    }
    void store.ensureIndex();
    await nextTick();
    searchInput.value?.focus();
  },
  // 组件由首开 latch 挂载时 isOpen 已为 true，必须 immediate 才能触发首开逻辑
  { immediate: true },
);

// ---------- 输入防抖（275ms，对齐日记搜索） ----------
watch(
  () => store.query,
  () => {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => void store.search(), 275);
  },
);

// 过滤器变更立即重搜（不走防抖）
watch(
  [
    () => store.scope,
    () => store.scopeTopicId,
    () => store.scopeOwnerId,
    () => store.role,
    () => store.timeRange,
    () => store.sort,
  ],
  () => void store.search(),
);

// ---------- 过滤器交互 ----------
const scopeLabel = computed(() => {
  if (store.scope === 'topic') return store.scopeTopicTitle || '当前话题';
  if (store.scope === 'owner') return store.scopeOwnerLabel || '指定助手';
  return '全部';
});

const currentTopicAvailable = computed(() => !!sessionStore.currentConversationKey);

const setScopeAll = () => {
  store.scope = 'all';
};

const setScopeCurrentTopic = () => {
  const key = sessionStore.currentConversationKey;
  if (!key) return;
  const topic = topicStore.topics.find((t) => t.id === key.topicId);
  store.scope = 'topic';
  store.scopeTopicId = key.topicId;
  store.scopeTopicTitle = topic?.name ?? '当前话题';
};

const openOwnerPicker = () => {
  const actions = assistantStore.combinedItems.map((item) => ({
    label: item.name,
    selected: store.scope === 'owner' && store.scopeOwnerId === item.id,
    handler: () => {
      store.scope = 'owner';
      store.scopeOwnerId = item.id;
      store.scopeOwnerType = item.type;
      store.scopeOwnerLabel = item.name;
    },
  }));
  if (actions.length === 0) return;
  overlayStore.openContextMenu(actions, '选择助手 / 群组');
};

const setRole = (r: RoleFilter) => {
  store.role = r;
};

const setTimeRange = (t: TimeFilter) => {
  store.timeRange = t;
};

const toggleSort = () => {
  store.sort = store.sort === 'time' ? 'rank' : 'time';
  notificationStore.addNotification({
    type: 'info',
    title: '全局搜索',
    message:
      store.sort === 'rank'
        ? '已切换为按相关度排序（bm25，仅对 ≥3 字关键词生效）'
        : '已切换为按时间倒序',
    toastOnly: true,
    duration: 2000,
  });
};

/**
 * 与后端 split_trigram_terms 同规则：任一空格分隔词 ≥3 字即走 FTS，bm25 才有意义。
 * 纯短词查询后端强制时间倒序，此时 rank 模式结果与 time 完全一致，需提示用户。
 */
const queryHasLongTerm = computed(() =>
  store.query
    .trim()
    .split(/\s+/)
    .some((t) => [...t].length >= 3),
);
const rankIneffective = computed(
  () =>
    store.sort === 'rank' &&
    store.hasSearched &&
    store.query.trim().length >= SEARCH_MIN_CHARS &&
    !queryHasLongTerm.value,
);

const roleFilters: RoleFilter[] = ['all', 'user', 'assistant', 'system'];
const timeFilters: TimeFilter[] = ['all', 'today', 'week', 'month'];

// ---------- 筛选面板（摘要条 + 展开分组） ----------
const filterOpen = ref(false);

const scopeSummary = computed(() =>
  store.scope === 'all' ? '全部范围' : scopeLabel.value,
);
const filterSummary = computed(
  () =>
    `${scopeSummary.value} · ${ROLE_LABELS[store.role]} · ${TIME_LABELS[store.timeRange]}`,
);
const activeFilterCount = computed(
  () =>
    (store.scope !== 'all' ? 1 : 0) +
    (store.role !== 'all' ? 1 : 0) +
    (store.timeRange !== 'all' ? 1 : 0),
);

// ---------- 结果渲染 ----------
interface SnippetSegment {
  text: string;
  hit: boolean;
}

/** snippet 只含 FTS5 注入的 <mark> 标记；按标记切分后用模板渲染，正文天然转义 */
const snippetSegments = (snippet: string): SnippetSegment[] => {
  return snippet
    .split(/<\/?mark>/g)
    .map((text, i) => ({ text, hit: i % 2 === 1 }))
    .filter((s) => s.text.length > 0);
};

const ownerName = (item: FtsSearchResultItem): string => {
  if (item.ownerType === 'agent') {
    return assistantStore.agents.find((a) => a.id === item.ownerId)?.name ?? '';
  }
  return assistantStore.groups.find((g) => g.id === item.ownerId)?.name ?? '';
};

const roleLabel = (role: string): string =>
  (ROLE_LABELS as Record<string, string>)[role] ?? role;

const fmtTime = (ts: number): string => {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

// ---------- 跳转定位闭环 ----------
const jumpToResult = async (item: FtsSearchResultItem) => {
  try {
    const key = sessionStore.currentConversationKey;
    const sameConversation =
      key &&
      key.ownerId === item.ownerId &&
      key.ownerType === item.ownerType &&
      key.topicId === item.topicId;

    emit('close');

    // 已在当前会话且消息在窗口内：直接定位
    if (sameConversation && historyStore.currentChatHistory.some((m) => m.id === item.msgId)) {
      await nextTick();
      scrollToMessageById(item.msgId);
      return;
    }

    // 切换会话（ChatView 的 watcher 会触发常规首屏加载，随后的锚点加载以 loadId 竞态胜出）
    await sessionStore.selectTopicById(item.ownerId, item.topicId);
    await nextTick();
    const result = await historyStore.loadHistoryAround(
      item.ownerId,
      item.ownerType,
      item.topicId,
      item.msgId,
    );
    await nextTick();

    if (result.ok || !result.anchorMissing) {
      scrollToMessageById(item.msgId);
    }
    if (result.anchorMissing) {
      notificationStore.addNotification({
        type: 'warning',
        title: '全局搜索',
        message: '目标消息已不存在或无法定位',
        toastOnly: true,
      });
    }
  } catch (e) {
    console.error('[GlobalSearch] Jump failed:', e);
    notificationStore.addNotification({
      type: 'error',
      title: '全局搜索',
      message: `跳转失败: ${String(e)}`,
      toastOnly: true,
    });
  }
};
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="gs-page">
      <!-- 顶栏 -->
      <header class="gs-header">
        <button type="button" class="gs-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <span class="gs-title">全局搜索</span>
        <button
          type="button"
          class="gs-sort-btn"
          :aria-label="store.sort === 'time' ? '当前：时间倒序，点击切换为相关度' : '当前：相关度，点击切换为时间倒序'"
          @click="toggleSort"
        >
          <component :is="store.sort === 'time' ? Clock : ArrowUpDown" :size="14" />
          <span class="gs-sort-label">{{ store.sort === 'time' ? '时间' : '相关度' }}</span>
        </button>
      </header>

      <!-- 搜索框 -->
      <div class="gs-toolbar">
        <div class="gs-search">
          <Search :size="14" class="gs-search-icon" />
          <input
            ref="searchInput"
            v-model="store.query"
            type="search"
            class="gs-search-input"
            :placeholder="`搜索全部消息内容（至少 ${SEARCH_MIN_CHARS} 个字符）…`"
            enterkeyhint="search"
          />
          <button
            v-if="store.query"
            type="button"
            class="gs-search-clear"
            aria-label="清空搜索"
            @click="store.query = ''"
          >
            <X :size="14" />
          </button>
        </div>
      </div>

      <!-- 筛选：摘要条 + 展开分组面板（替代旧单行横滑 chips） -->
      <div class="gs-filter">
        <button
          type="button"
          class="gs-filter-toggle"
          :aria-expanded="filterOpen"
          aria-controls="gs-filter-panel"
          @click="filterOpen = !filterOpen"
        >
          <SlidersHorizontal :size="13" class="gs-filter-icon" />
          <span class="gs-filter-summary">{{ filterSummary }}</span>
          <span v-if="activeFilterCount" class="gs-filter-badge">{{ activeFilterCount }}</span>
          <ChevronDown :size="14" class="gs-filter-chevron" :class="{ open: filterOpen }" />
        </button>

        <div v-if="filterOpen" id="gs-filter-panel" class="gs-filter-panel">
          <div class="gs-filter-group">
            <span class="gs-filter-label">范围</span>
            <div class="gs-filter-row" role="group" aria-label="搜索范围">
              <button
                type="button" class="gs-chip" :class="{ active: store.scope === 'all' }"
                @click="setScopeAll"
              >全部</button>
              <button
                type="button" class="gs-chip" :class="{ active: store.scope === 'topic' }"
                :disabled="!currentTopicAvailable"
                @click="setScopeCurrentTopic"
              >当前话题</button>
              <button
                type="button" class="gs-chip gs-chip-ellipsis" :class="{ active: store.scope === 'owner' }"
                @click="openOwnerPicker"
              >{{ store.scope === 'owner' ? scopeLabel : '指定助手' }}</button>
            </div>
          </div>

          <div class="gs-filter-group">
            <span class="gs-filter-label">角色</span>
            <div class="gs-filter-row" role="group" aria-label="消息角色">
              <button
                v-for="r in roleFilters" :key="r"
                type="button" class="gs-chip" :class="{ active: store.role === r }"
                @click="setRole(r)"
              >{{ ROLE_LABELS[r] }}</button>
            </div>
          </div>

          <div class="gs-filter-group">
            <span class="gs-filter-label">时间</span>
            <div class="gs-filter-row" role="group" aria-label="时间范围">
              <button
                v-for="t in timeFilters" :key="t"
                type="button" class="gs-chip" :class="{ active: store.timeRange === t }"
                @click="setTimeRange(t)"
              >{{ TIME_LABELS[t] }}</button>
            </div>
          </div>
        </div>
      </div>

      <!-- 索引构建中提示条 -->
      <div
        v-if="store.indexStatus && !store.indexReady"
        class="gs-index-banner"
        role="status"
      >
        <template v-if="store.indexStatus.rebuilding">
          正在构建搜索索引… {{ store.indexStatus.indexedMessages }} / {{ store.indexStatus.totalMessages }}
          （{{ store.indexProgressPct }}%），当前结果可能不全
        </template>
        <template v-else>
          搜索索引尚未完成（{{ store.indexStatus.indexedMessages }} / {{ store.indexStatus.totalMessages }}），结果可能不全
        </template>
      </div>

      <!-- 结果主体 -->
      <div class="gs-body vcp-scrollable no-rubber-band">
        <div v-if="store.error" class="gs-state">
          <p class="gs-state-error">{{ store.error }}</p>
          <button type="button" class="gs-retry-btn" @click="store.search()">重试</button>
        </div>
        <div v-else-if="!store.canSearch" class="gs-state">
          <p>输入至少 {{ SEARCH_MIN_CHARS }} 个字符开始搜索全部消息</p>
          <p class="gs-state-hint">支持多关键词（空格分隔）、范围 / 角色 / 时间组合过滤</p>
        </div>
        <div v-else-if="store.searching && store.results.length === 0" class="gs-state">
          <p>正在搜索…</p>
        </div>
        <div v-else-if="store.hasSearched && store.results.length === 0" class="gs-state">
          <p>没有匹配的消息</p>
          <p class="gs-state-hint">换个关键词，或放宽范围 / 时间过滤</p>
        </div>

        <template v-else>
          <p v-if="rankIneffective" class="gs-limit-hint">
            相关度排序仅对 ≥3 字的关键词生效，当前查询已按时间倒序展示
          </p>
          <button
            v-for="item in store.results"
            :key="`${item.topicId}:${item.msgId}`"
            type="button"
            class="gs-item"
            @click="jumpToResult(item)"
          >
            <div class="gs-item-top">
              <span class="gs-item-topic">{{ item.topicTitle }}</span>
              <span v-if="ownerName(item)" class="gs-item-owner">{{ ownerName(item) }}</span>
              <span class="gs-item-role">{{ roleLabel(item.role) }}</span>
              <time class="gs-item-time">{{ fmtTime(item.timestamp) }}</time>
            </div>
            <div class="gs-item-snippet">
              <template v-for="(seg, i) in snippetSegments(item.snippet)" :key="i">
                <mark v-if="seg.hit" class="gs-mark">{{ seg.text }}</mark>
                <template v-else>{{ seg.text }}</template>
              </template>
            </div>
          </button>

          <button
            v-if="store.limited && store.sort === 'time'"
            type="button"
            class="gs-load-more"
            :disabled="store.loadingMore"
            @click="store.loadMore()"
          >
            {{ store.loadingMore ? '加载中…' : '加载更多' }}
          </button>
          <p v-if="store.limited && store.sort === 'rank'" class="gs-limit-hint">
            相关度排序仅展示前 {{ store.results.length }} 条，请缩小范围或细化关键词
          </p>
        </template>
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
/* 全局搜索页：密排线性布局，无大圆角 / 无背景模糊 / 灰度优先 */
.gs-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.gs-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 0px) + 10px) 12px 10px;
  border-bottom: 1px solid rgba(128, 128, 128, 0.15);
  flex-shrink: 0;
}

.gs-title {
  flex: 1;
  font-size: 16px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.gs-icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border-radius: 8px;
  color: var(--primary-text);
  opacity: 0.75;
  transition: opacity 0.15s ease, background 0.15s ease;
}

.gs-icon-btn:active {
  opacity: 1;
  background: rgba(128, 128, 128, 0.12);
}

.gs-icon-btn.is-active {
  opacity: 1;
  color: var(--accent-bg, #3b82f6);
}

.gs-sort-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 30px;
  padding: 0 10px;
  border-radius: 8px;
  border: 1px solid rgba(128, 128, 128, 0.25);
  color: var(--primary-text);
  transition: background 0.15s ease;
  flex-shrink: 0;
}

.gs-sort-btn:active {
  background: rgba(128, 128, 128, 0.12);
}

.gs-sort-label {
  font-size: 12px;
  line-height: 1;
}

.gs-toolbar {
  padding: 10px 12px 6px;
  flex-shrink: 0;
}

.gs-search {
  position: relative;
  display: flex;
  align-items: center;
}

.gs-search-icon {
  position: absolute;
  left: 10px;
  opacity: 0.5;
  pointer-events: none;
}

.gs-search-input {
  width: 100%;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 14px;
  border-radius: 8px;
  padding: 9px 32px 9px 30px;
  outline: none;
  border: 1px solid rgba(128, 128, 128, 0.15);
  transition: border-color 0.15s ease;
  /* 隐藏 type=search 聚焦时的原生清除按钮，避免与自定义 X 叠加 */
  -webkit-appearance: none;
  appearance: none;
}

.gs-search-input::-webkit-search-cancel-button,
.gs-search-input::-webkit-search-decoration {
  display: none;
  -webkit-appearance: none;
}

.gs-search-input:focus {
  border-color: var(--accent-bg, #3b82f6);
}

.gs-search-clear {
  position: absolute;
  right: 8px;
  display: flex;
  align-items: center;
  padding: 4px;
  border-radius: 50%;
  color: var(--secondary-text);
  opacity: 0.7;
}

/* 筛选：摘要条常驻可见（当前过滤状态一目了然），点击展开分组面板 */
.gs-filter {
  padding: 4px 12px 8px;
  flex-shrink: 0;
}

.gs-filter-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  border-radius: 6px;
  background: var(--secondary-bg);
  color: var(--secondary-text);
  font-size: 12px;
  text-align: left;
  transition: background 0.15s ease;
}

.gs-filter-toggle:active {
  background: rgba(128, 128, 128, 0.18);
}

.gs-filter-icon {
  flex-shrink: 0;
  opacity: 0.6;
}

.gs-filter-summary {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.gs-filter-badge {
  flex-shrink: 0;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 8px;
  background: var(--accent-bg, #3b82f6);
  color: #fff;
  font-size: 10px;
  line-height: 16px;
  text-align: center;
  font-family: monospace;
}

.gs-filter-chevron {
  flex-shrink: 0;
  opacity: 0.6;
  transition: transform 0.15s ease;
}

.gs-filter-chevron.open {
  transform: rotate(180deg);
}

.gs-filter-panel {
  margin-top: 6px;
  padding: 8px 10px 4px;
  border-radius: 6px;
  background: var(--secondary-bg);
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.gs-filter-group {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}

.gs-filter-label {
  flex-shrink: 0;
  width: 28px;
  padding-top: 5px;
  font-size: 11px;
  color: var(--secondary-text);
  opacity: 0.7;
}

.gs-filter-row {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  min-width: 0;
}

.gs-chip {
  flex-shrink: 0;
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 6px;
  background: var(--primary-bg);
  color: var(--secondary-text);
  border: 1px solid transparent;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.gs-chip.active {
  color: var(--accent-bg, #3b82f6);
  border-color: var(--accent-bg, #3b82f6);
  background: transparent;
}

.gs-chip:disabled {
  opacity: 0.35;
}

.gs-chip-ellipsis {
  max-width: 120px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.gs-index-banner {
  margin: 0 12px 8px;
  padding: 6px 10px;
  font-size: 12px;
  border-radius: 6px;
  background: var(--secondary-bg);
  color: var(--secondary-text);
  border-left: 2px solid var(--accent-bg, #3b82f6);
  flex-shrink: 0;
}

.gs-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 0 12px 16px;
}

.gs-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 24px;
  gap: 8px;
  font-size: 13px;
  color: var(--secondary-text);
  text-align: center;
}

.gs-state-hint {
  font-size: 12px;
  opacity: 0.6;
}

.gs-state-error {
  color: #ef4444;
}

.gs-retry-btn {
  margin-top: 4px;
  padding: 6px 16px;
  font-size: 13px;
  border-radius: 6px;
  background: var(--secondary-bg);
  color: var(--primary-text);
}

/* 结果项：密排线性行，左侧 2px accent bar 作为 hover/active 反馈 */
.gs-item {
  display: block;
  width: 100%;
  text-align: left;
  padding: 8px 10px;
  border-left: 2px solid transparent;
  border-bottom: 1px solid rgba(128, 128, 128, 0.1);
  transition: border-color 0.15s ease, background 0.15s ease;
}

.gs-item:active {
  border-left-color: var(--accent-bg, #3b82f6);
  background: rgba(128, 128, 128, 0.06);
}

.gs-item-top {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.gs-item-topic {
  font-size: 13px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-shrink: 1;
  min-width: 0;
}

.gs-item-owner {
  font-size: 11px;
  color: var(--secondary-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-shrink: 2;
  min-width: 0;
}

.gs-item-role {
  font-size: 10px;
  color: var(--secondary-text);
  border: 1px solid rgba(128, 128, 128, 0.25);
  border-radius: 4px;
  padding: 0 4px;
  flex-shrink: 0;
}

.gs-item-time {
  margin-left: auto;
  font-family: ui-monospace, monospace;
  font-size: 11px;
  color: var(--secondary-text);
  flex-shrink: 0;
}

.gs-item-snippet {
  margin-top: 3px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--secondary-text);
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-all;
}

.gs-mark {
  background: transparent;
  color: var(--accent-bg, #3b82f6);
  font-weight: 600;
}

.gs-load-more {
  display: block;
  width: 100%;
  margin: 10px 0;
  padding: 8px;
  font-size: 13px;
  border-radius: 6px;
  background: var(--secondary-bg);
  color: var(--secondary-text);
}

.gs-load-more:disabled {
  opacity: 0.5;
}

.gs-limit-hint {
  padding: 10px 4px;
  font-size: 12px;
  color: var(--secondary-text);
  opacity: 0.7;
  text-align: center;
}
</style>
