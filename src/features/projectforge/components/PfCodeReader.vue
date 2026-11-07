<script setup lang="ts">
/**
 * V工程「源码」只读查看器（对齐桌面源码侧栏的打开文件视图）。
 *
 * 功能：
 * - 行号 / 等宽 / 横向滚动 / 自动换行开关 / 复制路径；
 * - 符号大纲（Outline → 点击跳转行）、FindSymbol 符号搜索（点击打开所在文件）、Trace 链路查询展示；
 * - 文件级操作入口：编辑（emit edit）、删除（emit delete）、移动/重命名（emit move）。
 *
 * 数据全部来自 `useProjectForgeSource`（不直接 fetch）；写操作的强确认在 PfSourcePanel 统一处理。
 */
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import {
  ArrowLeft,
  ChevronRight,
  Copy,
  Loader2,
  PenLine,
  Search,
  Trash2,
  WrapText,
  X,
  FolderInput,
} from "@lucide/vue";
import {
  formatSourceSize,
  languageLabel,
  SOURCE_RENDER_LINE_LIMIT,
  useProjectForgeSource,
} from '../useProjectForgeSource';

const emit = defineEmits<{
  back: [];
  edit: [];
  delete: [];
  move: [];
  'open-file': [path: string];
}>();

const source = useProjectForgeSource();
const {
  currentFile,
  fileLoading,
  fileError,
  outline,
  outlineLoading,
  outlineError,
  loadOutline,
  findSymbolQuery,
  findSymbolHits,
  findSymbolNote,
  findSymbolLoading,
  findSymbolError,
  findSymbol,
  traceTarget,
  traceResult,
  traceLoading,
  traceError,
  trace,
} = source;

const WRAP_STORAGE_KEY = 'vcp.projectforge.source.wrap';

type ToolTab = '' | 'outline' | 'symbol' | 'trace';
const activeTool = ref<ToolTab>('');
const wrap = ref(readWrap());
const copied = ref(false);
const highlightLine = ref(0);
const codeScrollRef = ref<HTMLElement | null>(null);
let highlightTimer: ReturnType<typeof setTimeout> | null = null;
let copyTimer: ReturnType<typeof setTimeout> | null = null;

function readWrap(): boolean {
  try {
    return globalThis.localStorage?.getItem(WRAP_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

function toggleWrap(): void {
  wrap.value = !wrap.value;
  try {
    globalThis.localStorage?.setItem(WRAP_STORAGE_KEY, wrap.value ? '1' : '0');
  } catch {
    // 忽略
  }
}

const file = computed(() => currentFile.value);
const displayLines = computed(() => {
  const lines = file.value?.lines ?? [];
  return lines.slice(0, SOURCE_RENDER_LINE_LIMIT).map((text, index) => ({ no: index + 1, text }));
});
const eolLabel = computed(() => (file.value?.eol === 'CRLF' ? 'CRLF' : 'LF'));
const metaText = computed(() => {
  const target = file.value;
  if (!target) return '';
  const parts = [formatSourceSize(target.size), `${target.lineCount} 行`, languageLabel(target.path), eolLabel.value];
  return parts.filter(Boolean).join(' · ');
});

function openTool(tab: Exclude<ToolTab, ''>): void {
  activeTool.value = activeTool.value === tab ? '' : tab;
  if (activeTool.value === 'outline' && !outline.value.length && file.value) {
    void loadOutline(file.value.path).catch(() => undefined);
  }
}

function jumpToLine(line: number): void {
  if (!line || line < 1) return;
  highlightLine.value = line;
  if (highlightTimer) clearTimeout(highlightTimer);
  highlightTimer = setTimeout(() => {
    highlightLine.value = 0;
  }, 2200);
  void nextTick(() => {
    const container = codeScrollRef.value;
    if (!container) return;
    const target = container.querySelector(`[data-line="${line}"]`) as HTMLElement | null;
    if (target) target.scrollIntoView({ block: 'center' });
  });
}

// FindSymbol 点击：当前文件直接跳行，其它文件交给父组件打开
let pendingJumpLine = 0;
function onSymbolHitClick(path: string, line?: number): void {
  if (!path) return;
  if (path === file.value?.path) {
    if (line) jumpToLine(line);
    return;
  }
  pendingJumpLine = line || 0;
  emit('open-file', path);
}

watch(
  () => file.value?.path,
  () => {
    if (pendingJumpLine) {
      const line = pendingJumpLine;
      pendingJumpLine = 0;
      void nextTick(() => jumpToLine(line));
    }
  },
);

async function copyPath(): Promise<void> {
  const path = file.value?.path ?? '';
  if (!path) return;
  let ok = false;
  try {
    await navigator.clipboard.writeText(path);
    ok = true;
  } catch {
    ok = false;
  }
  if (!ok) {
    try {
      const textarea = document.createElement('textarea');
      textarea.value = path;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      ok = document.execCommand('copy');
      document.body.removeChild(textarea);
    } catch {
      ok = false;
    }
  }
  copied.value = ok;
  if (copyTimer) clearTimeout(copyTimer);
  copyTimer = setTimeout(() => {
    copied.value = false;
  }, 1600);
}

async function runFindSymbol(): Promise<void> {
  try {
    await findSymbol();
  } catch {
    // 错误显示在 findSymbolError
  }
}

async function runTrace(): Promise<void> {
  try {
    await trace();
  } catch {
    // 错误显示在 traceError
  }
}

onBeforeUnmount(() => {
  if (highlightTimer) clearTimeout(highlightTimer);
  if (copyTimer) clearTimeout(copyTimer);
});
</script>

<template>
  <section class="pf-reader" aria-label="源码只读查看">
    <!-- 顶部：返回 + 路径 + 操作 -->
    <header class="pf-reader-head">
      <button type="button" class="pf-icon-btn" title="返回文件树" aria-label="返回文件树" @click="emit('back')">
        <ArrowLeft :size="20" />
      </button>
      <div class="pf-reader-title">
        <code class="pf-reader-path" :title="file?.path">{{ file?.path || '未选择文件' }}</code>
        <span class="pf-reader-meta">{{ metaText }}</span>
      </div>
      <button
        type="button"
        class="pf-icon-btn"
        :title="copied ? '已复制' : '复制路径'"
        aria-label="复制路径"
        :disabled="!file"
        @click="copyPath"
      >
        <Copy :size="17" :class="{ 'pf-copied': copied }" />
      </button>
      <button
        type="button"
        class="pf-icon-btn"
        :class="{ 'pf-icon-btn-active': wrap }"
        :title="wrap ? '关闭自动换行' : '开启自动换行'"
        aria-label="自动换行"
        @click="toggleWrap"
      >
        <WrapText :size="17" />
      </button>
    </header>

    <!-- 工具条：大纲 / 符号 / 链路 -->
    <nav class="pf-reader-tools" aria-label="源码工具">
      <button
        type="button"
        class="pf-tool-btn"
        :class="{ 'pf-tool-btn-active': activeTool === 'outline' }"
        :disabled="!file"
        @click="openTool('outline')"
      >
        大纲
      </button>
      <button
        type="button"
        class="pf-tool-btn"
        :class="{ 'pf-tool-btn-active': activeTool === 'symbol' }"
        @click="openTool('symbol')"
      >
        符号
      </button>
      <button
        type="button"
        class="pf-tool-btn"
        :class="{ 'pf-tool-btn-active': activeTool === 'trace' }"
        @click="openTool('trace')"
      >
        链路
      </button>
      <span v-if="copied" class="pf-reader-copied">已复制路径</span>
    </nav>

    <!-- 工具面板 -->
    <section v-if="activeTool" class="pf-tool-panel" aria-label="源码工具面板">
      <div class="pf-tool-head">
        <strong class="pf-tool-title">
          {{ activeTool === 'outline' ? '符号大纲' : activeTool === 'symbol' ? '符号查找' : '链路查询' }}
        </strong>
        <button type="button" class="pf-icon-btn pf-icon-btn-sm" aria-label="关闭工具面板" @click="activeTool = ''">
          <X :size="16" />
        </button>
      </div>

      <!-- 大纲 -->
      <template v-if="activeTool === 'outline'">
        <div v-if="outlineLoading" class="pf-tool-state">
          <Loader2 :size="14" class="pf-spin" /> 正在解析符号大纲…
        </div>
        <div v-else-if="outlineError" class="pf-tool-state pf-tool-error">{{ outlineError }}</div>
        <div v-else-if="!outline.length" class="pf-tool-state">该文件暂无可用符号（或语言暂不支持大纲）</div>
        <ul v-else class="pf-outline-list">
          <li v-for="(item, index) in outline" :key="`outline:${index}`">
            <button
              type="button"
              class="pf-outline-row"
              :style="{ paddingLeft: `${10 + item.depth * 14}px` }"
              :title="`${item.name} · 第 ${item.start} 行`"
              @click="jumpToLine(item.start)"
            >
              <ChevronRight :size="12" class="pf-outline-caret" />
              <span class="pf-outline-name">{{ item.name }}</span>
              <span v-if="item.kind" class="pf-outline-kind">{{ item.kind }}</span>
              <span class="pf-outline-line">L{{ item.start }}<template v-if="item.end">-{{ item.end }}</template></span>
            </button>
          </li>
        </ul>
      </template>

      <!-- 符号搜索 -->
      <template v-else-if="activeTool === 'symbol'">
        <form class="pf-tool-form" @submit.prevent="runFindSymbol">
          <input
            v-model="findSymbolQuery"
            class="pf-tool-input"
            type="search"
            placeholder="符号名 / 限定名，如 putBlob、Store.putBlob"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="submit" class="pf-tool-submit" :disabled="findSymbolLoading">
            <Loader2 v-if="findSymbolLoading" :size="14" class="pf-spin" />
            <Search v-else :size="14" />
            查找
          </button>
        </form>
        <div v-if="findSymbolError" class="pf-tool-state pf-tool-error">{{ findSymbolError }}</div>
        <div v-else-if="findSymbolNote" class="pf-tool-state pf-tool-note">{{ findSymbolNote }}</div>
        <ul v-if="findSymbolHits.length" class="pf-symbol-list">
          <li v-for="(hit, index) in findSymbolHits" :key="`symbol:${index}`">
            <button type="button" class="pf-symbol-row" :title="hit.raw" @click="onSymbolHitClick(hit.path, hit.start)">
              <span class="pf-symbol-name">{{ hit.name }}</span>
              <span class="pf-symbol-meta">
                <span v-if="hit.kind" class="pf-symbol-kind">{{ hit.kind }}</span>
                <code class="pf-symbol-path">{{ hit.path }}<template v-if="hit.start">:{{ hit.start }}</template></code>
              </span>
            </button>
          </li>
        </ul>
      </template>

      <!-- 链路 -->
      <template v-else>
        <form class="pf-tool-form" @submit.prevent="runTrace">
          <input
            v-model="traceTarget"
            class="pf-tool-input"
            type="search"
            placeholder="ipc:通道名 / global:名称 / page:xxx.html / file:相对路径"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="submit" class="pf-tool-submit" :disabled="traceLoading">
            <Loader2 v-if="traceLoading" :size="14" class="pf-spin" />
            <Search v-else :size="14" />
            查询
          </button>
        </form>
        <div v-if="traceError" class="pf-tool-state pf-tool-error">{{ traceError }}</div>
        <pre v-else-if="traceResult" class="pf-trace-text">{{ traceResult.text }}</pre>
      </template>
    </section>

    <!-- 正文 -->
    <div v-if="fileLoading" class="pf-reader-state">
      <Loader2 :size="18" class="pf-spin" /> 正在读取文件…
    </div>
    <div v-else-if="fileError" class="pf-reader-state pf-reader-error">{{ fileError }}</div>
    <div v-else-if="!file" class="pf-reader-state">选择左侧文件以查看源码</div>
    <div v-else class="pf-reader-body">
      <div v-if="file.renderTruncated" class="pf-reader-truncate">
        文件较大（{{ file.lineCount }} 行），只读视图仅展示前 {{ SOURCE_RENDER_LINE_LIMIT }} 行；编辑可查看全文。
      </div>
      <div ref="codeScrollRef" class="pf-code-scroll" :class="{ 'pf-code-wrap': wrap }">
        <div
          v-for="line in displayLines"
          :key="line.no"
          class="pf-code-line"
          :class="{ 'pf-code-line-hit': highlightLine === line.no }"
          :data-line="line.no"
        >
          <span class="pf-code-gutter">{{ line.no }}</span>
          <span class="pf-code-text">{{ line.text }}</span>
        </div>
      </div>
    </div>

    <!-- 底部操作栏 -->
    <footer v-if="file" class="pf-reader-actions">
      <button type="button" class="pf-action-btn pf-action-primary" @click="emit('edit')">
        <PenLine :size="16" />
        编辑
      </button>
      <button type="button" class="pf-action-btn" @click="emit('move')">
        <FolderInput :size="16" />
        移动
      </button>
      <button type="button" class="pf-action-btn pf-action-danger" @click="emit('delete')">
        <Trash2 :size="16" />
        删除
      </button>
    </footer>
  </section>
</template>

<style scoped>
.pf-reader {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
  background: var(--primary-bg, transparent);
}

.pf-reader-head {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 6px;
  border-bottom: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.08));
  flex: 0 0 auto;
}

.pf-reader-title {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding-left: 2px;
}

.pf-reader-path {
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.pf-reader-meta {
  font-size: 9.5px;
  opacity: 0.45;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  flex: 0 0 44px;
  border: 0;
  border-radius: 10px;
  background: transparent;
  color: inherit;
  opacity: 0.8;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.pf-icon-btn:disabled {
  opacity: 0.3;
}

.pf-icon-btn:active {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 14%, transparent);
}

.pf-icon-btn-sm {
  width: 36px;
  height: 36px;
  flex-basis: 36px;
}

.pf-icon-btn-active {
  color: var(--highlight-text, #4f8cff);
  opacity: 1;
}

.pf-copied {
  color: var(--pf-ok, #3ecf8e);
}

.pf-reader-tools {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px 6px;
  flex: 0 0 auto;
}

.pf-tool-btn {
  min-height: 34px;
  padding: 0 14px;
  border-radius: 999px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.1));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.04));
  color: inherit;
  font-size: 11.5px;
  cursor: pointer;
}

.pf-tool-btn-active {
  border-color: var(--highlight-text, #4f8cff);
  color: var(--highlight-text, #4f8cff);
}

.pf-tool-btn:disabled {
  opacity: 0.4;
}

.pf-reader-copied {
  margin-left: auto;
  font-size: 10px;
  color: var(--pf-ok, #3ecf8e);
}

.pf-tool-panel {
  flex: 0 0 auto;
  max-height: 38vh;
  overflow-y: auto;
  margin: 0 8px 6px;
  padding: 8px 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.1));
  border-radius: 12px;
  background: var(--secondary-bg, rgba(255, 255, 255, 0.04));
}

.pf-tool-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.pf-tool-title {
  font-size: 12px;
}

.pf-tool-state {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 2px;
  font-size: 11.5px;
  opacity: 0.6;
}

.pf-tool-error {
  color: var(--pf-danger, #ff6b6b);
  opacity: 0.9;
}

.pf-tool-note {
  font-size: 10.5px;
}

.pf-tool-form {
  display: flex;
  gap: 8px;
  padding: 8px 0 4px;
}

.pf-tool-input {
  flex: 1;
  min-width: 0;
  min-height: 40px;
  padding: 0 10px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--primary-bg, rgba(0, 0, 0, 0.2));
  color: inherit;
  font-size: 12px;
  outline: none;
}

.pf-tool-submit {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-height: 40px;
  padding: 0 14px;
  border: 0;
  border-radius: 10px;
  background: var(--highlight-text, #4f8cff);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.pf-tool-submit:disabled {
  opacity: 0.6;
}

.pf-outline-list,
.pf-symbol-list {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
}

.pf-outline-row {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  min-height: 40px;
  padding-right: 8px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  text-align: left;
  font-size: 11.5px;
  cursor: pointer;
}

.pf-outline-row:active {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 14%, transparent);
}

.pf-outline-caret {
  flex: 0 0 auto;
  opacity: 0.4;
}

.pf-outline-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-outline-kind {
  font-size: 9.5px;
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 18%, transparent);
  color: var(--highlight-text, #4f8cff);
  flex: 0 0 auto;
}

.pf-outline-line {
  font-size: 9.5px;
  opacity: 0.45;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  flex: 0 0 auto;
}

.pf-symbol-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 100%;
  min-height: 44px;
  padding: 6px 8px;
  border: 0;
  border-radius: 8px;
  background: transparent;
  color: inherit;
  text-align: left;
  cursor: pointer;
}

.pf-symbol-row:active {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 14%, transparent);
}

.pf-symbol-name {
  font-size: 12px;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
}

.pf-symbol-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.pf-symbol-kind {
  font-size: 9px;
  color: var(--highlight-text, #4f8cff);
  flex: 0 0 auto;
}

.pf-symbol-path {
  font-size: 9.5px;
  opacity: 0.5;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl;
  text-align: left;
}

.pf-trace-text {
  margin: 6px 0 0;
  padding: 8px;
  border-radius: 8px;
  background: var(--primary-bg, rgba(0, 0, 0, 0.25));
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  font-size: 10.5px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 26vh;
  overflow: auto;
}

.pf-reader-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  flex: 1;
  padding: 24px 16px;
  font-size: 12px;
  opacity: 0.6;
  text-align: center;
}

.pf-reader-error {
  color: var(--pf-danger, #ff6b6b);
  opacity: 0.9;
  flex-direction: column;
}

.pf-reader-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.pf-reader-truncate {
  flex: 0 0 auto;
  margin: 0 8px 4px;
  padding: 6px 10px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--pf-warn, #f5a623) 14%, transparent);
  color: var(--pf-warn, #f5a623);
  font-size: 10.5px;
}

.pf-code-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  -webkit-overflow-scrolling: touch;
  padding: 4px 0 8px;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, 'Liberation Mono', monospace;
  font-size: 11.5px;
  line-height: 1.55;
  tab-size: 4;
}

.pf-code-line {
  display: flex;
  min-height: 1.55em;
}

.pf-code-line-hit {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 20%, transparent);
}

.pf-code-gutter {
  flex: 0 0 3.4em;
  padding-right: 10px;
  text-align: right;
  opacity: 0.32;
  user-select: none;
  font-size: 10px;
}

.pf-code-text {
  flex: 1;
  min-width: 0;
  white-space: pre;
}

.pf-code-wrap .pf-code-text {
  white-space: pre-wrap;
  word-break: break-word;
}

.pf-reader-actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px calc(8px + var(--vcp-safe-bottom, 0px));
  border-top: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.08));
}

.pf-action-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  flex: 1;
  min-height: 44px;
  border-radius: 10px;
  border: 1px solid var(--vcp-border-subtle, rgba(255, 255, 255, 0.12));
  background: var(--secondary-bg, rgba(255, 255, 255, 0.06));
  color: inherit;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.pf-action-btn:active {
  opacity: 0.8;
}

.pf-action-primary {
  border-color: var(--highlight-text, #4f8cff);
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 18%, transparent);
  color: var(--highlight-text, #4f8cff);
}

.pf-action-danger {
  color: var(--pf-danger, #ff6b6b);
  border-color: color-mix(in srgb, var(--pf-danger, #ff6b6b) 40%, transparent);
}

.pf-spin {
  animation: pf-spin 0.8s linear infinite;
}

@keyframes pf-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
