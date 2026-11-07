<script setup lang="ts">
/**
 * V工程「源码」懒加载文件树。
 *
 * 目录展开/下载由父组件（PfSourcePanel）/ `useProjectForgeSource` 负责，本组件只负责渲染与事件：
 * - 普通模式：`rows` 是已扁平化的树行（含 depth/open/loading），点目录 emit toggle、点文件 emit open；
 * - 搜索模式：`searchQuery` 非空时展示 `results` 扁平命中列表（文件名 + 所在目录）。
 */
import { computed } from 'vue';
import {
  AlertTriangle,
  ChevronRight,
  FileCode2,
  FileText,
  Folder,
  FolderOpen,
  Loader2,
  RefreshCw,
  SearchX,
} from "@lucide/vue";
import type { PfFileEntry } from '../projectForgeTypes';
import type { PfSourceTreeRow } from '../useProjectForgeSource';

const props = withDefaults(
  defineProps<{
    /** 普通模式的扁平树行。 */
    rows?: PfSourceTreeRow[];
    /** 搜索模式的扁平命中。 */
    results?: PfFileEntry[];
    /** 文件名搜索词（空格分隔多关键字，非空时进入搜索模式）。 */
    searchQuery?: string;
    searching?: boolean;
    /** 当前打开文件的工程内相对路径（高亮）。 */
    activePath?: string;
    /** 根目录加载中。 */
    loading?: boolean;
    /** 根目录错误文案。 */
    error?: string;
    emptyText?: string;
  }>(),
  {
    rows: () => [],
    results: () => [],
    searchQuery: '',
    searching: false,
    activePath: '',
    loading: false,
    error: '',
    emptyText: '',
  },
);

const emit = defineEmits<{
  open: [path: string];
  toggle: [path: string];
  retry: [];
}>();

const searchMode = computed(() => props.searchQuery.trim() !== '');
const tokens = computed(() =>
  props.searchQuery
    .trim()
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean),
);

/** 关键字高亮切片（按出现位置切分，不区分大小写）。 */
function highlightParts(text: string): Array<{ text: string; hit: boolean }> {
  const source = String(text ?? '');
  if (!tokens.value.length || !source) return [{ text: source, hit: false }];
  const mask = new Array<boolean>(source.length).fill(false);
  const lower = source.toLowerCase();
  for (const token of tokens.value) {
    let from = 0;
    while (from <= lower.length - token.length) {
      const index = lower.indexOf(token, from);
      if (index < 0) break;
      for (let offset = 0; offset < token.length; offset += 1) mask[index + offset] = true;
      from = index + token.length;
    }
  }
  const parts: Array<{ text: string; hit: boolean }> = [];
  let start = 0;
  while (start < source.length) {
    const hit = mask[start];
    let end = start;
    while (end < source.length && mask[end] === hit) end += 1;
    parts.push({ text: source.slice(start, end), hit });
    start = end;
  }
  return parts;
}

function dirOf(path: string): string {
  const clean = String(path ?? '').replace(/\\/g, '/');
  const index = clean.lastIndexOf('/');
  return index > 0 ? clean.slice(0, index) : '';
}

function fileIcon(path: string) {
  return /\.(?!$)[a-z0-9]{1,8}$/i.test(path) ? FileCode2 : FileText;
}
</script>

<template>
  <div class="pf-tree-wrap">
    <!-- 错误态 -->
    <div v-if="error" class="pf-tree-state pf-tree-error">
      <AlertTriangle :size="18" />
      <span class="pf-tree-state-text">{{ error }}</span>
      <button type="button" class="pf-tree-retry" @click="emit('retry')">
        <RefreshCw :size="14" />
        重试
      </button>
    </div>

    <!-- 搜索模式 -->
    <template v-else-if="searchMode">
      <div v-if="searching" class="pf-tree-state">
        <Loader2 :size="16" class="pf-spin" />
        <span class="pf-tree-state-text">正在搜索文件…</span>
      </div>
      <div v-else-if="!results.length" class="pf-tree-state">
        <SearchX :size="18" />
        <span class="pf-tree-state-text">没有匹配的文件</span>
      </div>
      <ul v-else class="pf-tree-list" role="listbox" aria-label="文件搜索结果">
        <li v-for="item in results" :key="`search:${item.path}`" role="option">
          <button
            type="button"
            class="pf-tree-row pf-tree-row-file"
            :class="{ 'pf-tree-row-active': item.path === activePath }"
            :title="item.path"
            :data-path="item.path"
            @click="emit('open', item.path)"
          >
            <component :is="fileIcon(item.name)" :size="15" class="pf-tree-icon" />
            <span class="pf-tree-main">
              <span class="pf-tree-name">
                <template v-for="(part, index) in highlightParts(item.name)" :key="index">
                  <mark v-if="part.hit">{{ part.text }}</mark>
                  <template v-else>{{ part.text }}</template>
                </template>
              </span>
              <span v-if="dirOf(item.path)" class="pf-tree-sub">
                <template v-for="(part, index) in highlightParts(dirOf(item.path))" :key="index">
                  <mark v-if="part.hit">{{ part.text }}</mark>
                  <template v-else>{{ part.text }}</template>
                </template>
              </span>
            </span>
            <span v-if="typeof item.size === 'number'" class="pf-tree-size">
              {{ item.size < 1024 ? `${item.size} B` : `${(item.size / 1024).toFixed(1)} KB` }}
            </span>
          </button>
        </li>
      </ul>
    </template>

    <!-- 普通模式 -->
    <template v-else>
      <div v-if="loading && !rows.length" class="pf-tree-state">
        <Loader2 :size="16" class="pf-spin" />
        <span class="pf-tree-state-text">正在读取文件树…</span>
      </div>
      <div v-else-if="!rows.length" class="pf-tree-state">
        <Folder :size="18" />
        <span class="pf-tree-state-text">{{ emptyText || '工程根目录为空' }}</span>
      </div>
      <ul v-else class="pf-tree-list" role="tree" aria-label="工程文件树">
        <li
          v-for="row in rows"
          :key="`${row.type}:${row.path}`"
          role="treeitem"
          :aria-expanded="row.type === 'dir' ? row.open : undefined"
        >
          <button
            v-if="row.type === 'dir'"
            type="button"
            class="pf-tree-row pf-tree-row-dir"
            :class="{ 'pf-tree-row-active': row.path === activePath }"
            :style="{ paddingLeft: `${8 + row.depth * 16}px` }"
            :title="row.path"
            :data-dir="row.path"
            @click="emit('toggle', row.path)"
          >
            <ChevronRight :size="14" class="pf-tree-chevron" :class="{ 'pf-tree-chevron-open': row.open }" />
            <component :is="row.open ? FolderOpen : Folder" :size="15" class="pf-tree-icon" />
            <span class="pf-tree-name">{{ row.name }}</span>
            <Loader2 v-if="row.loading" :size="13" class="pf-spin pf-tree-loading" />
          </button>
          <button
            v-else
            type="button"
            class="pf-tree-row pf-tree-row-file"
            :class="{ 'pf-tree-row-active': row.path === activePath }"
            :style="{ paddingLeft: `${8 + row.depth * 16}px` }"
            :title="row.path"
            :data-path="row.path"
            @click="emit('open', row.path)"
          >
            <span class="pf-tree-chevron pf-tree-chevron-placeholder" />
            <component :is="fileIcon(row.name)" :size="15" class="pf-tree-icon" />
            <span class="pf-tree-name">{{ row.name }}</span>
            <span v-if="typeof row.size === 'number'" class="pf-tree-size">
              {{ row.size < 1024 ? `${row.size} B` : `${(row.size / 1024).toFixed(1)} KB` }}
            </span>
          </button>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.pf-tree-wrap {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  color: var(--primary-text);
}

.pf-tree-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
  flex: 1;
  min-height: 0;
}

.pf-tree-row {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  min-height: 44px;
  padding: 6px 10px;
  border: 0;
  border-left: 2px solid transparent;
  background: transparent;
  color: inherit;
  text-align: left;
  font-size: 13px;
  line-height: 1.2;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}

.pf-tree-row:active {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 12%, transparent);
}

.pf-tree-row-active {
  border-left-color: var(--highlight-text, #4f8cff);
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 10%, transparent);
}

.pf-tree-row-dir {
  font-weight: 600;
}

.pf-tree-chevron {
  flex: 0 0 auto;
  opacity: 0.55;
  transition: transform 0.12s ease;
}

.pf-tree-chevron-open {
  transform: rotate(90deg);
}

.pf-tree-chevron-placeholder {
  width: 14px;
  height: 14px;
  flex: 0 0 14px;
}

.pf-tree-icon {
  flex: 0 0 auto;
  opacity: 0.7;
}

.pf-tree-main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  flex: 1;
}

.pf-tree-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-tree-sub {
  font-size: 10px;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  opacity: 0.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-tree-size {
  flex: 0 0 auto;
  font-size: 10px;
  font-family: ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace;
  opacity: 0.4;
}

.pf-tree-name mark,
.pf-tree-sub mark {
  background: color-mix(in srgb, var(--highlight-text, #4f8cff) 35%, transparent);
  color: inherit;
  border-radius: 2px;
}

.pf-tree-state {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 24px 16px;
  font-size: 12px;
  opacity: 0.6;
}

.pf-tree-error {
  flex-direction: column;
  gap: 10px;
  color: var(--pf-danger, #ff6b6b);
  opacity: 0.9;
}

.pf-tree-state-text {
  text-align: center;
  word-break: break-word;
}

.pf-tree-retry {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-height: 44px;
  padding: 0 16px;
  border: 1px solid var(--border-color, rgba(255, 255, 255, 0.14));
  border-radius: 10px;
  background: var(--secondary-bg, rgba(255, 255, 255, 0.06));
  color: inherit;
  font-size: 12px;
  cursor: pointer;
}

.pf-spin {
  animation: pf-spin 0.8s linear infinite;
}

.pf-tree-loading {
  margin-left: auto;
  opacity: 0.6;
}

@keyframes pf-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
