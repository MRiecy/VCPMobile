<script setup lang="ts">
/**
 * LogCenterView.vue — VCP 日志中心页面（SlidePage）。
 *
 * 视觉与交互遵循 UI 美学宪法：高密度线性布局、2px accent bar 状态表达、
 * 无毛玻璃、Monospace 技术值。方案见
 * plan/vcpmobile-more-tools-research/01-日志中心-上游契约与移植方案.md §4。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { useVirtualList } from '@vueuse/core';
import {
  ArrowLeft,
  ArrowDownToLine,
  Ellipsis,
  Pause,
  Play,
  Search,
  ArrowDownUp,
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import RefreshButton from '../../components/ui/RefreshButton.vue';
import BottomSheet, { type ActionItem } from '../../components/ui/BottomSheet.vue';
import { useOverlayStore } from '../../core/stores/overlay';
import { useNotificationStore } from '../../core/stores/notification';
import { useLogCenterStore } from './logCenterStore';
import { levelOf, splitByKeyword } from './logText';

const props = withDefaults(defineProps<{ isOpen?: boolean; zIndex?: number }>(), {
  isOpen: false,
  zIndex: 40,
});

const emit = defineEmits<{ close: [] }>();

const store = useLogCenterStore();
const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();

const toast = (type: 'info' | 'success' | 'warning' | 'error', message: string) => {
  notificationStore.addNotification({
    type,
    title: '日志中心',
    message,
    toastOnly: true,
  });
};

// ---------- 虚拟滚动 ----------
const ROW_HEIGHT = 22;
const { list, containerProps, wrapperProps } = useVirtualList(
  computed(() => store.displayedLines),
  { itemHeight: ROW_HEIGHT, overscan: 15 },
);

const container = ref<HTMLElement | null>(null);
function bindContainerRef(element: unknown): void {
  const htmlElement = element as HTMLElement | null;
  containerProps.ref.value = htmlElement;
  container.value = htmlElement;
}

/** 正序时「末尾」在底部；倒序时最新在顶部，末尾语义反转。 */
const isNearEnd = ref(true);
const showJumpFab = ref(false);

function scrollMetrics() {
  const el = container.value;
  if (!el) return { nearEnd: true };
  const remaining = el.scrollHeight - el.scrollTop - el.clientHeight;
  const nearBottom = remaining < 100;
  const nearTop = el.scrollTop < 100;
  return { nearEnd: store.isReverse ? nearTop : nearBottom };
}

function handleScroll(): void {
  const el = container.value;
  if (el) {
    // 内容收缩后浏览器对 scrollTop 的钳制不一定触发 scroll 事件，主动钳制
    const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight);
    if (el.scrollTop > maxScroll) el.scrollTop = maxScroll;
  }
  containerProps.onScroll();
  const { nearEnd } = scrollMetrics();
  isNearEnd.value = nearEnd;
  showJumpFab.value = !nearEnd;
  if (nearEnd) store.acknowledgeNewLines();
}

async function jumpToEnd(): Promise<void> {
  const el = container.value;
  if (!el) return;
  if (store.isReverse) {
    el.scrollTop = 0;
  } else {
    el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
  }
  containerProps.onScroll();
  store.acknowledgeNewLines();
  showJumpFab.value = false;
}

/**
 * 数据源收缩后的虚拟窗口校正。
 * useVirtualList 的 calculateRange 用旧 scrollTop 计算渲染窗口：当列表变短，
 * start 可能越界（start > end）渲染出空窗口；且浏览器 clamp 未必触发 scroll
 * 事件。这里先归零强制一次有效重算，下一拍再跳到末尾，双重重算保证收敛。
 */
async function recoverScrollWindow(): Promise<void> {
  const el = container.value;
  if (!el) return;
  el.scrollTop = 0;
  containerProps.onScroll();
  await nextTick();
  await jumpToEnd();
}

// 新内容到达：吸底或累计徽标
watch(
  () => store.logVersion,
  async () => {
    await nextTick();
    if (store.autoScroll && isNearEnd.value) {
      await jumpToEnd();
    }
  },
);

// 筛选变化：回到「末尾」（最新处）
watch(
  () => store.filterText,
  () => {
    void jumpToEnd();
  },
);

// 行数限制变化（尤其从大改小）：虚拟列表总高收缩后渲染窗口可能越界白屏。
// 同时监听显示行数（刷新填满缓冲也会改变总长），双重重算校正。
watch(
  () => [store.lineLimit, store.displayedLines.length],
  async (_new, old) => {
    // 仅在收缩场景校正（行数变少）；增量追加由 logVersion 吸底逻辑处理
    if (old && store.displayedLines.length >= old[1] && store.lineLimit >= old[0]) return;
    await nextTick();
    await recoverScrollWindow();
  },
);

// ---------- 会话生命周期 ----------
watch(
  () => props.isOpen,
  (open) => {
    if (open) {
      void store.startSession();
    } else {
      store.stopSession();
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  store.resetSession();
});

// ---------- 溢出菜单 ----------
const isMenuOpen = ref(false);
const isLimitSheetOpen = ref(false);

const LIMIT_OPTIONS = [100, 300, 500, 1000, 3000];

const limitActions = computed<ActionItem[]>(() =>
  LIMIT_OPTIONS.map((value) => ({
    label: `${value} 行${value === store.lineLimit ? '（当前）' : ''}`,
    handler: () => store.setLineLimit(value),
  })),
);

async function copyVisible(): Promise<void> {
  const text = store.displayedLines.join('\n');
  if (!text) {
    toast('info', '当前没有可复制的日志');
    return;
  }
  try {
    await navigator.clipboard.writeText(text);
    toast('success', `已复制 ${store.displayedLines.length} 行可见日志`);
  } catch {
    // 剪贴板 API 失败时的降级：隐藏 textarea + execCommand
    const helper = document.createElement('textarea');
    helper.value = text;
    helper.style.position = 'fixed';
    helper.style.opacity = '0';
    document.body.appendChild(helper);
    helper.select();
    try {
      document.execCommand('copy');
      toast('success', `已复制 ${store.displayedLines.length} 行可见日志`);
    } catch {
      toast('error', '复制失败，请检查剪贴板权限');
    } finally {
      helper.remove();
    }
  }
}

async function confirmClearServer(): Promise<void> {
  const ok = await overlayStore.showConfirm({
    title: '清空服务器日志',
    message:
      '将清空 VCPToolBox 服务器的 ServerLog.txt 本体，影响所有客户端，且不可撤销。\n（服务器 archive 目录中的历史归档不受影响。）',
    isDanger: true,
  });
  if (!ok) return;
  try {
    await store.clearServer();
    toast('success', '服务器日志已清空');
  } catch (e) {
    toast('error', e instanceof Error ? e.message : String(e));
  }
}

const menuActions = computed<ActionItem[]>(() => [
  {
    label: `行数限制：${store.lineLimit} 行`,
    handler: () => {
      isLimitSheetOpen.value = true;
    },
  },
  {
    label: store.autoScroll ? '关闭自动滚动' : '开启自动滚动',
    handler: () => store.toggleAutoScroll(),
  },
  {
    label: '复制可见日志',
    handler: () => {
      void copyVisible();
    },
  },
  {
    label: '清空本地显示',
    handler: () => {
      store.clearLocal();
      toast('info', '本地显示已清空（不影响服务器文件）');
    },
  },
  {
    label: '清空服务器日志…',
    danger: true,
    handler: () => {
      void confirmClearServer();
    },
  },
]);

// ---------- 状态条 ----------
const stateLabel = computed(() => {
  if (store.error) return '连接失败';
  if (store.isPaused) return '已暂停';
  if (store.isLoading) return '拉取中';
  if (store.isPolling) return '监听中';
  return '待机';
});

const stateClass = computed(() => ({
  'is-error': !!store.error,
  'is-paused': store.isPaused && !store.error,
}));

const fileSizeLabel = computed(() => {
  const size = store.fileSize;
  if (!size) return '—';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / 1024 / 1024).toFixed(2)} MB`;
});

const statusLine = computed(() => {
  if (store.error) return store.error;
  if (store.filterText.trim()) {
    return `仅搜索已加载的 ${store.totalBuffered} 行缓冲`;
  }
  return store.logPath || '等待首次拉取…';
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="log-center">
      <!-- 顶栏 -->
      <header class="log-header">
        <button type="button" class="log-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <div class="log-title-block">
          <span class="log-title">日志中心</span>
          <span class="log-state" :class="stateClass">
            <span class="log-state-dot" />{{ stateLabel }}
          </span>
        </div>
        <div class="log-header-actions">
          <button
            type="button"
            class="log-icon-btn"
            :aria-label="store.isPaused ? '继续拉取' : '暂停拉取'"
            :aria-pressed="store.isPaused"
            @click="store.togglePause()"
          >
            <component :is="store.isPaused ? Play : Pause" :size="17" />
          </button>
          <RefreshButton label="全量刷新" :loading="store.isLoading" @refresh="store.refresh()" />
          <button
            type="button"
            class="log-icon-btn"
            aria-label="更多操作"
            @click="isMenuOpen = true"
          >
            <Ellipsis :size="19" />
          </button>
        </div>
      </header>

      <!-- 工具行：筛选 + 倒序 -->
      <div class="log-toolbar">
        <div class="log-search">
          <Search :size="14" class="log-search-icon" />
          <input
            v-model="store.filterText"
            type="search"
            class="log-search-input"
            placeholder="筛选日志内容…"
            enterkeyhint="search"
          />
        </div>
        <button
          type="button"
          class="log-icon-btn log-reverse-btn"
          :class="{ 'is-active': store.isReverse }"
          :aria-pressed="store.isReverse"
          aria-label="切换正序/倒序"
          title="切换正序/倒序"
          @click="store.toggleReverse()"
        >
          <ArrowDownUp :size="16" />
        </button>
      </div>

      <!-- 统计条 -->
      <div class="log-stats" aria-live="polite">
        <span class="log-stat">缓冲 <strong>{{ store.totalBuffered }}</strong></span>
        <span class="log-stat">显示 <strong>{{ store.displayedLines.length }}</strong></span>
        <span v-if="store.filterText.trim()" class="log-stat">
          匹配 <strong>{{ store.matchedCount }}</strong>
        </span>
        <span class="log-stat">文件 <strong>{{ fileSizeLabel }}</strong></span>
      </div>

      <!-- 日志主体 -->
      <div class="log-body">
        <div
          v-if="store.displayedLines.length === 0"
          class="log-empty"
        >
          <p v-if="store.error" class="log-empty-error">{{ store.error }}</p>
          <p v-else-if="store.isLoading">正在拉取日志…</p>
          <p v-else-if="store.filterText.trim()">没有匹配「{{ store.filterText.trim() }}」的日志行</p>
          <p v-else>暂无日志</p>
          <button
            v-if="store.error"
            type="button"
            class="log-retry-btn"
            @click="store.refresh()"
          >
            重试
          </button>
        </div>

        <div
          v-else
          :ref="bindContainerRef"
          :style="containerProps.style"
          class="log-scroll vcp-scrollable no-rubber-band"
          data-logcenter-role="log-scroll"
          @scroll="handleScroll"
        >
          <div v-bind="wrapperProps" class="log-wrapper">
            <div
              v-for="row in list"
              :key="row.index"
              class="log-row"
              :class="`log-level-${levelOf(row.data)}`"
            >
              <span class="log-row-text">
                <template
                  v-for="(part, partIndex) in splitByKeyword(row.data, store.filterText)"
                  :key="partIndex"
                ><mark v-if="part.hit" class="log-hit">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template>
              </span>
            </div>
          </div>
        </div>

        <!-- 跳到最新 FAB（圆形悬浮钮 + 角标） -->
        <Transition name="log-fab">
          <button
            v-if="showJumpFab"
            type="button"
            class="log-jump-fab"
            :class="{ 'log-jump-fab-reverse': store.isReverse }"
            aria-label="跳到最新日志"
            @click="jumpToEnd()"
          >
            <ArrowDownToLine :size="17" />
            <span v-if="store.newLineCount > 0" class="log-jump-badge">
              {{ store.newLineCount > 99 ? '99+' : store.newLineCount }}
            </span>
          </button>
        </Transition>
      </div>

      <!-- 底部状态行 -->
      <footer class="log-footer" :title="store.logPath">{{ statusLine }}</footer>

      <!-- 溢出菜单 -->
      <BottomSheet v-model="isMenuOpen" title="日志选项" :actions="menuActions" compact />
      <BottomSheet v-model="isLimitSheetOpen" title="行数限制" :actions="limitActions" compact />
    </div>
  </SlidePage>
</template>

<style scoped>
.log-center {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.log-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.log-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.log-title {
  font-size: 16px;
  font-weight: 800;
  letter-spacing: 0.02em;
}

.log-state {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 700;
  opacity: 0.6;
  letter-spacing: 0.08em;
}

.log-state-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--highlight-text);
}

.log-state.is-error {
  opacity: 1;
  color: #ef4444;
}

.log-state.is-error .log-state-dot {
  background: #ef4444;
}

.log-state.is-paused .log-state-dot {
  background: #f59e0b;
}

.log-header-actions {
  display: flex;
  align-items: center;
  gap: 2px;
}

.log-icon-btn {
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
  transition: opacity 0.15s ease;
}

.log-icon-btn:active {
  opacity: 1;
}

.log-reverse-btn.is-active {
  opacity: 1;
  color: var(--highlight-text);
}

.log-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.log-search {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
}

.log-search-icon {
  opacity: 0.45;
  flex-shrink: 0;
}

.log-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
}

.log-stats {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 6px 14px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
  overflow-x: auto;
  white-space: nowrap;
}

.log-stat {
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.06em;
  opacity: 0.55;
}

.log-stat strong {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  font-weight: 700;
  opacity: 1;
  color: var(--primary-text);
  margin-left: 2px;
}

.log-body {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.log-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
}

.log-wrapper {
  /* 行内横向滚动：长行不换行（保固定行高虚拟滚动），横滑查看 */
  overflow-x: auto;
}

.log-row {
  display: flex;
  align-items: stretch;
  height: 22px;
  border-left: 2px solid transparent;
}

.log-row-text {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11.5px;
  line-height: 22px;
  white-space: pre;
  padding: 0 10px;
  opacity: 0.88;
}

.log-level-error {
  border-left-color: #ef4444;
  background: rgba(239, 68, 68, 0.07);
}

.log-level-error .log-row-text {
  color: #ef4444;
  opacity: 1;
}

.log-level-warn {
  border-left-color: #f59e0b;
}

.log-level-warn .log-row-text {
  color: #d97706;
}

.log-level-info {
  border-left-color: rgba(59, 130, 246, 0.55);
}

.log-level-debug .log-row-text {
  opacity: 0.5;
}

.log-hit {
  background: rgba(245, 158, 11, 0.35);
  color: inherit;
  border-radius: 2px;
  padding: 0 1px;
}

.log-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px;
  text-align: center;
  font-size: 13px;
  opacity: 0.6;
}

.log-empty-error {
  color: #ef4444;
  opacity: 1;
  font-size: 12px;
  max-width: 32rem;
  word-break: break-all;
}

.log-retry-btn {
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

.log-jump-fab {
  position: absolute;
  right: 18px;
  bottom: 18px;
  width: 42px;
  height: 42px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--highlight-text);
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.16);
  z-index: var(--layer-local);
  transition: opacity 0.15s ease;
}

.log-jump-fab:active {
  opacity: 0.75;
}

/* 倒序时最新在顶部，箭头语义反转 */
.log-jump-fab-reverse :first-child {
  transform: rotate(180deg);
}

.log-jump-badge {
  position: absolute;
  top: -5px;
  right: -5px;
  min-width: 18px;
  height: 18px;
  padding: 0 4px;
  box-sizing: border-box;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 999px;
  background: var(--highlight-text);
  color: #fff;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 800;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.2);
}

.log-fab-enter-active,
.log-fab-leave-active {
  transition:
    opacity 0.15s ease,
    transform 0.15s ease;
}

.log-fab-enter-from,
.log-fab-leave-to {
  opacity: 0;
  transform: translateY(6px);
}

.log-footer {
  flex-shrink: 0;
  padding: 6px 14px calc(var(--vcp-safe-bottom, 48px) + 6px);
  border-top: 1px solid var(--border-color);
  font-size: 10px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  opacity: 0.45;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 平板/宽屏：限制行长保持可读性，统计条与工具行更舒展 */
@media (min-width: 768px) {
  .log-wrapper,
  .log-stats,
  .log-toolbar,
  .log-footer {
    max-width: 1100px;
    width: 100%;
    margin: 0 auto;
  }

  .log-row-text {
    font-size: 12px;
  }
}
</style>
