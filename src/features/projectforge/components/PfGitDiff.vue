<script setup lang="ts">
/**
 * Git 差异渲染（对齐桌面 VProjectForge「Git 差异」面板）。
 *
 * 输入是 `git diff --no-color` 的原始 unified diff 文本，按行类型着色：
 *   新增（+）绿色、删除（-）红色、hunk 头（@@）蓝色、文件头/索引等灰色；
 *   左侧显示旧/新行号（等宽），支持横滑与「自动换行」开关（记住偏好）。
 * 本组件纯展示，不发起任何请求；数据由 PfGitPanel / useProjectForgeGit 提供。
 */
import { computed, ref } from 'vue';

interface Props {
  /** unified diff 原文。 */
  text?: string;
  /** 是否正在读取差异。 */
  loading?: boolean;
  /** 读取失败提示。 */
  error?: string;
  /** 文件路径/标题。 */
  title?: string;
  /** 分组说明（如「暂存的更改」）。 */
  groupLabel?: string;
  /** 无差异时的提示文案。 */
  emptyHint?: string;
}

const props = withDefaults(defineProps<Props>(), {
  text: '',
  loading: false,
  error: '',
  title: '',
  groupLabel: '',
  emptyHint: '',
});

type DiffLineKind = 'file' | 'hunk' | 'add' | 'del' | 'context' | 'meta';

interface DiffLine {
  kind: DiffLineKind;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

/** 最多渲染的行数，避免超大 diff 卡住 WebView。 */
const MAX_RENDER_LINES = 4000;
/** 自动换行偏好（与桌面端 diff 换行开关同语义）。 */
const WRAP_STORAGE_KEY = 'vcp.projectForge.git.diffWrap.v1';

const wrap = ref(readWrapPreference());
const truncated = ref(false);

function readWrapPreference(): boolean {
  try {
    return window.localStorage.getItem(WRAP_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

function toggleWrap(): void {
  wrap.value = !wrap.value;
  try {
    window.localStorage.setItem(WRAP_STORAGE_KEY, wrap.value ? '1' : '0');
  } catch {
    // localStorage 不可用时仅本次生效
  }
}

function signOf(kind: DiffLineKind): string {
  if (kind === 'add') return '+';
  if (kind === 'del') return '-';
  if (kind === 'context') return ' ';
  if (kind === 'hunk') return '@';
  return ' ';
}

const lines = computed<DiffLine[]>(() => {
  truncated.value = false;
  const text = String(props.text ?? '');
  if (!text.trim()) return [];
  const parsed: DiffLine[] = [];
  let oldNo = 0;
  let newNo = 0;
  let truncatedFlag = false;
  const rawLines = text.split('\n');
  // git diff 输出通常以换行结尾，split 后末尾会多出一个空串，避免渲染成幽灵上下文行
  if (rawLines.length > 1 && rawLines[rawLines.length - 1] === '') rawLines.pop();
  for (const rawLine of rawLines) {
    if (parsed.length >= MAX_RENDER_LINES) {
      truncatedFlag = true;
      break;
    }
    const line = rawLine.replace(/\r$/, '');
    if (line.startsWith('diff --git ') || line.startsWith('diff --cc ')) {
      parsed.push({ kind: 'file', text: line, oldNo: null, newNo: null });
      continue;
    }
    if (line.startsWith('@@')) {
      const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      oldNo = match ? Number(match[1]) : 0;
      newNo = match ? Number(match[2]) : 0;
      parsed.push({ kind: 'hunk', text: line, oldNo: null, newNo: null });
      continue;
    }
    if (
      line.startsWith('--- ') ||
      line.startsWith('+++ ') ||
      line.startsWith('index ') ||
      line.startsWith('old mode ') ||
      line.startsWith('new mode ') ||
      line.startsWith('new file mode ') ||
      line.startsWith('deleted file mode ') ||
      line.startsWith('similarity index ') ||
      line.startsWith('dissimilarity index ') ||
      line.startsWith('rename from ') ||
      line.startsWith('rename to ') ||
      line.startsWith('copy from ') ||
      line.startsWith('copy to ') ||
      line.startsWith('Binary files ') ||
      line.startsWith('GIT binary patch') ||
      line.startsWith('\\ No newline')
    ) {
      parsed.push({ kind: 'meta', text: line, oldNo: null, newNo: null });
      continue;
    }
    if (line.startsWith('+')) {
      parsed.push({ kind: 'add', text: line.slice(1), oldNo: null, newNo });
      newNo += 1;
      continue;
    }
    if (line.startsWith('-')) {
      parsed.push({ kind: 'del', text: line.slice(1), oldNo, newNo: null });
      oldNo += 1;
      continue;
    }
    // 上下文行（unified diff 里以空格开头；个别工具会去掉行首空格，按上下文处理）
    parsed.push({ kind: 'context', text: line.startsWith(' ') ? line.slice(1) : line, oldNo, newNo });
    oldNo += 1;
    newNo += 1;
  }
  truncated.value = truncatedFlag;
  return parsed;
});

const hasContent = computed(() => lines.value.length > 0);
</script>

<template>
  <div class="pf-git-diff" aria-label="Git 差异">
    <div class="pf-git-diff-toolbar">
      <div class="pf-git-diff-heading">
        <span v-if="title" class="pf-git-diff-title">{{ title }}</span>
        <span v-if="groupLabel" class="pf-git-diff-group">{{ groupLabel }}</span>
      </div>
      <button
        type="button"
        class="pf-git-diff-wrap-btn"
        :aria-pressed="wrap"
        :title="wrap ? '关闭自动换行' : '开启自动换行'"
        @click="toggleWrap"
      >
        {{ wrap ? '换行开' : '自动换行' }}
      </button>
    </div>

    <div v-if="loading" class="pf-git-diff-state">正在读取差异…</div>
    <div v-else-if="error" class="pf-git-diff-state error">{{ error }}</div>
    <div v-else-if="!hasContent" class="pf-git-diff-state">
      {{ emptyHint || '没有可显示的差异（可能是二进制文件、空文件或仅权限/模式变化）。' }}
    </div>
    <div v-else class="pf-git-diff-body" :class="{ 'is-wrap': wrap }">
      <div v-for="(line, index) in lines" :key="index" class="pf-git-diff-line" :class="`is-${line.kind}`">
        <span class="pf-git-diff-ln">{{ line.oldNo ?? '' }}</span>
        <span class="pf-git-diff-ln">{{ line.newNo ?? '' }}</span>
        <span class="pf-git-diff-sign">{{ signOf(line.kind) }}</span>
        <span class="pf-git-diff-code">{{ line.text }}</span>
      </div>
      <div v-if="truncated" class="pf-git-diff-truncated">
        差异过大，仅显示前 {{ MAX_RENDER_LINES }} 行；完整内容请在桌面端查看。
      </div>
    </div>
  </div>
</template>

<style scoped>
.pf-git-diff {
  display: flex;
  flex-direction: column;
  min-height: 0;
  flex: 1;
  background: var(--primary-bg, #1c1c1e);
  color: var(--primary-text, #e0e0e0);
}

.pf-git-diff-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border-color, #3a3a3e);
  flex-shrink: 0;
}

.pf-git-diff-heading {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.pf-git-diff-title {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-git-diff-group {
  font-size: 11px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.6));
  flex-shrink: 0;
}

.pf-git-diff-wrap-btn {
  min-height: 44px;
  min-width: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
  font-size: 12px;
  cursor: pointer;
  flex-shrink: 0;
}

.pf-git-diff-wrap-btn[aria-pressed='true'] {
  border-color: var(--highlight-text, #3498db);
  color: var(--highlight-text, #3498db);
}

.pf-git-diff-state {
  padding: 16px 12px;
  font-size: 13px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.6));
  line-height: 1.6;
}

.pf-git-diff-state.error {
  color: var(--danger-color, #f85149);
}

.pf-git-diff-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  -webkit-overflow-scrolling: touch;
  padding: 6px 0 16px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  line-height: 1.5;
}

.pf-git-diff-line {
  display: flex;
  align-items: flex-start;
  min-width: max-content;
  white-space: pre;
}

.pf-git-diff-body.is-wrap .pf-git-diff-line {
  min-width: 0;
  white-space: pre-wrap;
  word-break: break-all;
}

.pf-git-diff-ln {
  flex-shrink: 0;
  width: 40px;
  padding: 0 6px 0 0;
  text-align: right;
  color: rgba(150, 150, 160, 0.7);
  user-select: none;
  border-right: 1px solid rgba(128, 128, 128, 0.15);
}

.pf-git-diff-sign {
  flex-shrink: 0;
  width: 16px;
  text-align: center;
  opacity: 0.75;
  user-select: none;
}

.pf-git-diff-code {
  flex: 1;
  padding-right: 12px;
}

.pf-git-diff-line.is-add {
  background: rgba(46, 160, 67, 0.16);
}

.pf-git-diff-line.is-add .pf-git-diff-sign,
.pf-git-diff-line.is-add .pf-git-diff-code {
  color: #3fb950;
}

.pf-git-diff-line.is-del {
  background: rgba(248, 81, 73, 0.16);
}

.pf-git-diff-line.is-del .pf-git-diff-sign,
.pf-git-diff-line.is-del .pf-git-diff-code {
  color: #f85149;
}

.pf-git-diff-line.is-hunk {
  background: rgba(56, 139, 253, 0.12);
  color: #58a6ff;
  padding-top: 2px;
  padding-bottom: 2px;
}

.pf-git-diff-line.is-file {
  background: rgba(139, 148, 158, 0.12);
  color: #8b949e;
  font-weight: 600;
  padding-top: 4px;
}

.pf-git-diff-line.is-meta {
  color: #8b949e;
}

.pf-git-diff-truncated {
  padding: 10px 12px;
  color: #d29922;
  font-size: 12px;
}
</style>
