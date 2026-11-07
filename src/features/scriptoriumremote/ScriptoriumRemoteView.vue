<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue';
import { ChevronLeft, Download, FileText, GitBranch, PenLine, RefreshCw, Save, Search, Settings2, X } from "@lucide/vue";
import DOMPurify from 'dompurify';
import { marked } from 'marked';
import SlidePage from '../../components/ui/SlidePage.vue';
import { useScriptoriumRemoteStore } from './scriptoriumRemoteStore';
import { BRIDGE_CONFIG_SOURCE_LABELS } from '../../core/bridge/bridgeConfig';
import { saveToDownloads } from 'tauri-plugin-vcp-mobile';

const props = defineProps<{ isOpen: boolean; zIndex: number }>();
const emit = defineEmits<{ close: [] }>();

const store = useScriptoriumRemoteStore();
const query = ref('');
const tab = ref<'doc' | 'lineage'>('doc');
const editing = ref(false);
const editText = ref('');
const saving = ref(false);
const confirmRollbackId = ref('');
const showExport = ref(false);
const showBridgeSettings = ref(false);
const bridgeBaseDraft = ref('');
const bridgeTokenDraft = ref('');
const exporting = ref('');
const saveHint = ref('');
const openedName = ref('');

const bridgeSourceLabel = computed(
  () => BRIDGE_CONFIG_SOURCE_LABELS[store.configSource] || '未知来源',
);

function openBridgeSettings(): void {
  bridgeBaseDraft.value = store.baseUrl;
  bridgeTokenDraft.value = store.token;
  showBridgeSettings.value = true;
}

async function applyBridgeSettings(): Promise<void> {
  store.saveSettings({ baseUrl: bridgeBaseDraft.value, token: bridgeTokenDraft.value });
  showBridgeSettings.value = false;
  await store.loadDocs();
}

const filteredDocs = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return store.docs;
  return store.docs.filter((doc) => doc.name.toLowerCase().includes(q));
});

// —— 文档保真渲染：正文 + document.css + 内联 <style> + mermaid 图表 ——
const READER_BASE_CSS = `
.scriptorium-reader{background:#ffffff;color:#2f2a24;border-radius:12px;padding:14px 16px;font-size:15px;line-height:1.72;overflow-wrap:break-word;}
.scriptorium-reader h1{font-size:24px;font-weight:800;margin:0 0 14px;}
.scriptorium-reader h2{font-size:18px;font-weight:700;margin:22px 0 10px;}
.scriptorium-reader h3{font-size:15.5px;font-weight:700;margin:16px 0 8px;}
.scriptorium-reader p{margin:8px 0;}
.scriptorium-reader ul,.scriptorium-reader ol{margin:8px 0 8px 20px;padding:0;}
.scriptorium-reader li{margin:5px 0;}
.scriptorium-reader ul{list-style:disc;} .scriptorium-reader ol{list-style:decimal;}
.scriptorium-reader blockquote{margin:12px 0;padding:8px 12px;border-left:4px solid #E1D9CC;background:#F9F7F4;color:#6b6055;}
.scriptorium-reader table{width:100%;border-collapse:collapse;margin:12px 0;font-size:13.5px;}
.scriptorium-reader th,.scriptorium-reader td{border:1px solid #E1D9CC;padding:8px 10px;text-align:left;vertical-align:top;}
.scriptorium-reader th{background:#F8F1E4;font-weight:600;}
.scriptorium-reader tr:nth-child(even){background:#FAF8F5;}
.scriptorium-reader code{background:#F5F2EC;padding:2px 6px;border-radius:4px;font-size:13px;color:#8b5a2b;}
.scriptorium-reader pre{background:#F7F4EF;padding:12px 14px;border-radius:8px;overflow-x:auto;margin:10px 0;}
.scriptorium-reader pre code{background:none;padding:0;color:#4a443d;}
.scriptorium-reader strong{color:#8a5f1c;font-weight:600;}
.scriptorium-reader em{font-style:italic;}
.scriptorium-reader hr{border:none;border-top:1px dashed #E1D9CC;margin:24px 0;}
.scriptorium-reader img{max-width:100%;height:auto;border-radius:8px;display:block;margin:10px auto;}
.scriptorium-reader .vcp-doc-mermaid{margin:12px 0;overflow-x:auto;}
.scriptorium-reader .vcp-doc-mermaid svg{max-width:100%;height:auto;display:block;margin:0 auto;}
.scriptorium-reader .mermaid-error{background:#fff4f4;color:#b42318;border-radius:8px;padding:10px;font-size:12px;white-space:pre-wrap;}
.scriptorium-reader .vcp-island-slot{margin:12px 0;}
.scriptorium-reader .vcp-island-slot iframe{width:100%;border:0;display:block;border-radius:8px;background:transparent;}
`;

const docHtml = ref('');
const docCss = ref('');
const docRendering = ref(false);
let renderSeq = 0;

interface IslandDefinition {
  id: string;
  html: string;
}

const islandDefinitions = ref<IslandDefinition[]>([]);
const islandFrames = new Map<HTMLIFrameElement, HTMLElement>();

function buildIslandSrcdoc(island: IslandDefinition): string {
  const documentCss = store.css || '';
  const heightReporter = [
    '<script>',
    '(function(){',
    '  var last = -1;',
    '  function post(){',
    '    try {',
    '      var total = Math.max(',
    '        document.documentElement ? document.documentElement.scrollHeight : 0,',
    '        document.body ? document.body.scrollHeight : 0,',
    '        document.body ? document.body.offsetHeight : 0',
    '      );',
    '      var text = document.body ? (document.body.innerText || "").replace(/\\s+/g, " ").slice(0, 160) : "";',
    '      var canvases = document.querySelectorAll("canvas").length;',
    '      if (total !== last) {',
    '        last = total;',
    '        parent.postMessage({ type: "vcp-island-height", height: total, text: text, canvases: canvases }, "*");',
    '      }',
    '    } catch (e) {}',
    '  }',
    '  window.addEventListener("load", post);',
    '  for (var i = 0; i < 24; i++) { setTimeout(post, i * 150); }',
    '  if (window.ResizeObserver) { try { new ResizeObserver(post).observe(document.documentElement); } catch (e) {} }',
    '  setInterval(post, 1000);',
    '  post();',
    '})();',
    '<\/script>',
  ].join('\n');
  return [
    '<!doctype html><html><head><meta charset="utf-8">',
    '<meta name="viewport" content="width=device-width, initial-scale=1">',
    '<base target="_blank">',
    '<style>',
    'html,body{margin:0;padding:0;background:transparent;color:#2f2a24;',
    "font-family:system-ui,-apple-system,'PingFang SC','Noto Sans SC',sans-serif;",
    'font-size:15px;line-height:1.7;overflow:hidden;}*{box-sizing:border-box;}',
    '</style>',
    `<style>${documentCss}</style>`,
    '</head><body>',
    island.html,
    heightReporter,
    '</body></html>',
  ].join('');
}

function splitIslands(html: string): { mainHtml: string; islands: IslandDefinition[] } {
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  const islands: IslandDefinition[] = [];
  const nodes = Array.from(parsed.querySelectorAll('[data-vdoc-island]'));
  nodes.forEach((node) => {
    const id = node.getAttribute('data-vdoc-island') || `island-${islands.length + 1}`;
    const slot = parsed.createElement('div');
    slot.className = 'vcp-island-slot';
    slot.setAttribute('data-island-id', id);
    slot.setAttribute('data-island-index', String(islands.length));
    islands.push({ id, html: node.outerHTML });
    node.replaceWith(slot);
  });
  return { mainHtml: parsed.body.innerHTML, islands };
}

function mountIslandFrames(): void {
  const reader = document.querySelector('.scriptorium-reader');
  if (!reader) return;
  const slots = Array.from(reader.querySelectorAll<HTMLElement>('.vcp-island-slot'));
  slots.forEach((slot) => {
    const index = Number(slot.getAttribute('data-island-index') || '-1');
    const island = islandDefinitions.value[index];
    if (!island) return;
    if (slot.querySelector('iframe')) return;
    const frame = document.createElement('iframe');
    frame.className = 'vcp-island-frame';
    frame.setAttribute('demo-repo', 'allow-scripts allow-forms allow-modals allow-popups allow-downloads allow-pointer-lock allow-orientation-lock allow-presentation');
    frame.setAttribute('referrerpolicy', 'no-referrer');
    frame.setAttribute('loading', 'lazy');
    frame.style.width = '100%';
    frame.style.border = '0';
    frame.style.display = 'block';
    frame.style.background = 'transparent';
    frame.style.height = '220px';
    frame.setAttribute('data-island-id', island.id);
    frame.srcdoc = buildIslandSrcdoc(island);
    slot.appendChild(frame);
    islandFrames.set(frame, slot);
  });
}

function onIslandMessage(event: MessageEvent): void {
  const data = event.data as { type?: string; height?: number; text?: string; canvases?: number } | null;
  if (!data || data.type !== 'vcp-island-height') return;
  if (!event.source) return;
  for (const [frame, slot] of islandFrames) {
    if (frame.contentWindow !== event.source) continue;
    const height = Math.min(Math.max(Number(data.height) || 0, 60), 2400);
    frame.style.height = `${height}px`;
    slot.dataset.islandHeight = String(height);
    if (typeof data.canvases === 'number') slot.dataset.islandCanvases = String(data.canvases);
    if (data.text) slot.dataset.islandText = String(data.text).slice(0, 160);
    return;
  }
}

onMounted(() => {
  window.addEventListener('message', onIslandMessage);
});

onUnmounted(() => {
  window.removeEventListener('message', onIslandMessage);
  islandFrames.clear();
});

function scopeCss(css: string, scope: string): string {
  if (!css || !css.trim()) return '';
  const out: string[] = [];
  let index = 0;
  while (index < css.length) {
    const brace = css.indexOf('{', index);
    if (brace === -1) {
      out.push(css.slice(index));
      break;
    }
    const selector = css.slice(index, brace).trim();
    let depth = 1;
    let cursor = brace + 1;
    while (cursor < css.length && depth > 0) {
      if (css[cursor] === '{') depth += 1;
      else if (css[cursor] === '}') depth -= 1;
      cursor += 1;
    }
    const body = css.slice(brace + 1, cursor - 1);
    if (selector.startsWith('@')) {
      out.push(`${selector}{${body}}`);
    } else {
      const scoped = selector
        .split(',')
        .map((item) => item.trim())
        .filter(Boolean)
        .map((item) => `${scope} ${item}`)
        .join(',');
      out.push(`${scoped}{${body}}`);
    }
    index = cursor;
  }
  return out.join('\n');
}

function extractStyleBlocks(html: string): { html: string; css: string } {
  let css = '';
  const cleaned = html.replace(/<style[^>]*>([\s\S]*?)<\/style>/gi, (_match, content) => {
    css += `${content}\n`;
    return '';
  });
  return { html: cleaned, css };
}

function escapeHtmlText(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}



async function buildDocView(): Promise<void> {
  const sequence = ++renderSeq;
  const markdown = store.markdown || '';
  if (!markdown) {
    docHtml.value = '';
    docCss.value = '';
    islandDefinitions.value = [];
    return;
  }
  docRendering.value = true;
  try {
    const rawHtml = marked.parse(markdown) as string;
    const split = splitIslands(rawHtml);
    const sanitized = DOMPurify.sanitize(split.mainHtml, {
      ADD_TAGS: ['style'],
      ADD_ATTR: ['data-vdoc-island', 'viewBox', 'fill', 'stroke', 'stroke-width', 'text-anchor', 'font-size', 'font-family', 'rx', 'ry', 'x', 'y', 'x1', 'y1', 'x2', 'y2', 'points', 'd', 'width', 'height', 'role', 'aria-label'],
      FORBID_TAGS: ['script', 'iframe', 'object', 'embed', 'form'],
      ALLOW_DATA_ATTR: true,
    });
    const extracted = extractStyleBlocks(sanitized);
    if (sequence !== renderSeq) return;
    docHtml.value = extracted.html;
    islandDefinitions.value = split.islands;
    docCss.value = `${READER_BASE_CSS}\n${scopeCss(store.css || '', '.scriptorium-reader')}\n${scopeCss(extracted.css, '.scriptorium-reader')}`;
    await nextTick();
    mountIslandFrames();
  } catch (error) {
    if (sequence !== renderSeq) return;
    docHtml.value = `<p>正文解析失败：${escapeHtmlText(String((error as Error)?.message || error))}</p>`;
    docCss.value = READER_BASE_CSS;
  } finally {
    if (sequence === renderSeq) docRendering.value = false;
  }
}

watch(
  [() => store.doc, () => store.markdown, () => store.css],
  () => {
    void buildDocView();
  },
  { immediate: true },
);

function formatSize(bytes: number): string {
  if (!bytes) return '0 KB';
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function formatTime(ms: number): string {
  if (!ms) return '';
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

async function openDoc(name: string): Promise<void> {
  openedName.value = name;
  tab.value = 'doc';
  await store.openDoc(name);
}

async function doExport(format: 'pdf' | 'html-flow' | 'html-paged' | 'project'): Promise<void> {
  if (!openedName.value || exporting.value) return;
  exporting.value = format;
  showExport.value = false;
  try {
    const file = await store.exportDoc(openedName.value, format);
    await saveToDownloads(file.fileName, file.base64, file.mime);
    saveHint.value = `已导出到下载目录：${file.fileName}（${(file.bytes / 1024).toFixed(0)} KB）`;
  } catch (e) {
    saveHint.value = `导出失败：${String((e as Error)?.message || e)}`;
  } finally {
    exporting.value = '';
  }
}

async function rollbackTo(id: string): Promise<void> {
  if (!openedName.value) return;
  const item = store.history.find((entry) => entry.id === id);
  if (item?.isCurrent) {
    saveHint.value = '这已经是当前版本，无需回退';
    confirmRollbackId.value = '';
    return;
  }
  if (confirmRollbackId.value !== id) {
    confirmRollbackId.value = id;
    return;
  }
  try {
    await store.rollback(openedName.value, id);
    confirmRollbackId.value = '';
    saveHint.value = '已回退到所选快照';
    tab.value = 'doc';
  } catch (e) {
    saveHint.value = `回退失败：${String((e as Error)?.message || e)}`;
  }
}

function startEdit(): void {
  editText.value = store.markdown || '';
  editing.value = true;
  saveHint.value = '';
}

async function saveEdit(): Promise<void> {
  if (!openedName.value || saving.value) return;
  saving.value = true;
  saveHint.value = '';
  try {
    const result = await store.applyEdit(openedName.value, editText.value);
    // 桌面侧落盘是异步的（mutationQueue），稍等再回读，避免读到旧内容。
    await new Promise((resolve) => setTimeout(resolve, 1200));
    await store.openDoc(openedName.value);
    editing.value = false;
    saveHint.value = `已保存（${result.mode === 'append' ? '追加' : '整篇替换'}${result.revision !== undefined ? ` · 修订 ${result.revision}` : ''}）`;
  } catch (e) {
    saveHint.value = `保存失败：${String((e as Error)?.message || e)}`;
  } finally {
    saving.value = false;
  }
}

function backToList(): void {
  openedName.value = '';
  store.closeDoc();
}

watch(
  () => props.isOpen,
  async (open) => {
    if (open) {
      backToList();
      await store.loadDocs();
    }
  },
  { immediate: true },
);

function authorName(checkpoint: any): string {
  const author = checkpoint?.author;
  if (!author) return '未知';
  return String(author.name || author.id || '未知');
}
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <main class="h-full min-h-0 flex flex-col bg-[var(--primary-bg)] text-[var(--primary-text)] no-swipe">
      <header class="shrink-0 px-3 flex items-center gap-2 border-b border-black/5 dark:border-white/5" style="padding-top: var(--vcp-safe-top, 24px); min-height: calc(3.5rem + var(--vcp-safe-top, 24px));">
        <button type="button" class="w-9 h-9 shrink-0 flex items-center justify-center rounded-full active:scale-95" aria-label="返回" @click="openedName ? backToList() : emit('close')">
          <ChevronLeft :size="20" />
        </button>
        <div class="flex-1 min-w-0">
          <div class="text-[9px] font-black tracking-[0.18em] opacity-45">SCRIPTORIUM · DESKTOP</div>
          <div class="text-sm font-bold truncate">{{ openedName || '文坊' }}</div>
        </div>
        <button type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="桥接设置" @click="openBridgeSettings">
          <Settings2 :size="16" />
        </button>
        <button v-if="!openedName" type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="刷新" @click="store.loadDocs()">
          <RefreshCw :size="16" />
        </button>
        <button v-if="openedName && !editing" type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="导出" @click="showExport = !showExport">
          <Download :size="16" />
        </button>
        <button v-if="openedName && !editing" type="button" class="w-8 h-8 flex items-center justify-center rounded-full" aria-label="编辑" @click="startEdit">
          <PenLine :size="16" />
        </button>
        <div v-if="openedName && !editing" class="flex rounded-lg overflow-hidden border border-black/10 dark:border-white/10">
          <button type="button" class="px-2 h-8 text-[10px] font-bold" :class="tab === 'doc' ? 'bg-[var(--highlight-text)] text-white' : 'opacity-60'" @click="tab = 'doc'">正文</button>
          <button type="button" class="px-2 h-8 text-[10px] font-bold" :class="tab === 'lineage' ? 'bg-[var(--highlight-text)] text-white' : 'opacity-60'" @click="tab = 'lineage'">文脉</button>
        </div>
      </header>

      <p v-if="store.error" class="mx-3 mt-2 rounded-md bg-red-500/15 text-red-500 text-[10px] px-2 py-1.5">{{ store.error }}</p>

      <!-- 导出菜单 -->
      <div v-if="showExport" class="absolute inset-0 z-30 bg-black/35" @click.self="showExport = false">
        <div class="absolute left-4 right-4 top-1/3 rounded-xl bg-[var(--primary-bg)] border border-black/10 dark:border-white/10 p-3 space-y-2">
          <p class="text-[11px] font-bold px-1">导出 {{ store.title || openedName }}</p>
          <button type="button" class="w-full h-10 rounded-lg bg-[var(--secondary-bg)] text-[12px] font-bold" :disabled="!!exporting" @click="doExport('pdf')">PDF（分页打印）</button>
          <button type="button" class="w-full h-10 rounded-lg bg-[var(--secondary-bg)] text-[12px]" :disabled="!!exporting" @click="doExport('html-flow')">HTML · 连续流</button>
          <button type="button" class="w-full h-10 rounded-lg bg-[var(--secondary-bg)] text-[12px]" :disabled="!!exporting" @click="doExport('html-paged')">HTML · 逐页</button>
          <button type="button" class="w-full h-10 rounded-lg bg-[var(--secondary-bg)] text-[12px]" :disabled="!!exporting" @click="doExport('project')">工程文件（.vdocx）</button>
          <button type="button" class="w-full h-9 rounded-lg text-[11px] opacity-60" @click="showExport = false">取消</button>
        </div>
      </div>

      <!-- 桥接设置（与 V工程 / 远程桌面 共用同一份配置） -->
      <div v-if="showBridgeSettings" class="absolute inset-0 z-30 bg-black/40" @click.self="showBridgeSettings = false">
        <div class="absolute left-4 right-4 top-1/4 rounded-xl bg-[var(--primary-bg)] text-[var(--primary-text)] border border-black/10 dark:border-white/10 p-3.5 space-y-3">
          <div class="flex items-center gap-2">
            <Settings2 :size="15" class="opacity-60" />
            <span class="text-[12px] font-bold flex-1">桌面桥接设置</span>
            <button type="button" class="w-7 h-7 flex items-center justify-center rounded-full opacity-60" aria-label="关闭" @click="showBridgeSettings = false">
              <X :size="15" />
            </button>
          </div>
          <p class="text-[9px] leading-relaxed opacity-60">
            文坊数据来自桌面端 VCPChat 的文坊插件。默认跟随「设置 → 同步」里的桌面地址（当前：{{ bridgeSourceLabel }}）。
          </p>
          <div>
            <p class="text-[10px] font-bold opacity-60 mb-1">Base 地址</p>
            <input
              v-model="bridgeBaseDraft"
              class="w-full h-9 px-2.5 rounded-lg bg-[var(--secondary-bg)] text-[11px] font-mono outline-none"
              placeholder="http://桌面IP:5974/api/mobile-bridge"
              autocapitalize="off"
              spellcheck="false"
            />
          </div>
          <div>
            <p class="text-[10px] font-bold opacity-60 mb-1">Token</p>
            <input
              v-model="bridgeTokenDraft"
              class="w-full h-9 px-2.5 rounded-lg bg-[var(--secondary-bg)] text-[11px] font-mono outline-none"
              placeholder="桌面端 MobilePluginBridge 的 Bearer Token"
              autocapitalize="off"
              spellcheck="false"
            />
          </div>
          <div class="flex items-center gap-2">
            <button type="button" class="flex-1 h-9 rounded-lg bg-[var(--secondary-bg)] text-[11px] font-bold" @click="showBridgeSettings = false">取消</button>
            <button type="button" class="flex-1 h-9 rounded-lg text-[11px] font-bold" style="background-color: var(--highlight-text); color: #fff" @click="applyBridgeSettings">保存并刷新</button>
          </div>
          <p class="text-[9px] opacity-40 leading-relaxed">与「V工程 / 远程桌面」共用同一份桥接配置；只填 http://IP:5974 会自动补全 /api/mobile-bridge。</p>
        </div>
      </div>

      <!-- 文档库 -->
      <section v-if="!openedName" class="flex-1 min-h-0 flex flex-col">
        <div class="px-3 py-2 shrink-0">
          <div class="flex items-center gap-2 h-9 px-3 rounded-lg bg-[var(--secondary-bg)]">
            <Search :size="14" class="opacity-45" />
            <input v-model="query" class="flex-1 min-w-0 bg-transparent outline-none text-[12px]" placeholder="搜索文档" />
          </div>
        </div>
        <div class="flex-1 min-h-0 overflow-y-auto vcp-scrollable px-3 pb-3 space-y-2">
          <button
            v-for="item in filteredDocs"
            :key="item.name"
            type="button"
            class="w-full text-left rounded-xl border border-black/8 dark:border-white/8 bg-[var(--secondary-bg)] px-3 py-2.5 active:opacity-80"
            @click="openDoc(item.name)"
          >
            <div class="flex items-center gap-2">
              <FileText :size="15" class="opacity-50" />
              <span class="flex-1 min-w-0 text-[13px] font-bold truncate">{{ item.name.replace(/\.(vdocx|vpptx)$/i, '') }}</span>
              <span class="shrink-0 text-[9px] px-1.5 py-0.5 rounded border border-black/10 dark:border-white/10 opacity-60 uppercase">{{ item.kind }}</span>
            </div>
            <div class="mt-1 text-[9px] opacity-45 font-mono">{{ formatSize(item.size) }} · {{ formatTime(item.mtime) }}</div>
          </button>
          <p v-if="!filteredDocs.length && !store.loading" class="text-center text-[11px] opacity-50 pt-8">没有匹配的文档</p>
        </div>
      </section>

      <!-- 编辑 -->
      <section v-if="editing" class="flex-1 min-h-0 flex flex-col">
        <div class="px-3 py-2 flex items-center gap-2 shrink-0">
          <span class="text-[10px] font-bold opacity-60">Markdown 源码</span>
          <span class="flex-1" />
          <button type="button" class="h-8 px-2 rounded-md text-[10px] flex items-center gap-1" @click="editing = false">
            <X :size="13" />取消
          </button>
          <button type="button" class="h-8 px-3 rounded-md text-[10px] font-bold flex items-center gap-1" style="background-color: var(--highlight-text); color: #fff" :disabled="saving" @click="saveEdit">
            <Save :size="13" />{{ saving ? '保存中…' : '保存' }}
          </button>
        </div>
        <p v-if="saveHint" class="mx-3 mb-1 text-[10px] opacity-70">{{ saveHint }}</p>
        <textarea v-model="editText" class="flex-1 min-h-0 mx-3 mb-3 p-3 rounded-lg bg-[var(--secondary-bg)] text-[12px] font-mono leading-relaxed outline-none resize-none vcp-scrollable" spellcheck="false"></textarea>
      </section>

      <!-- 阅读 -->
      <section v-else-if="!editing" class="flex-1 min-h-0 flex flex-col">
        <div v-if="tab === 'doc'" class="flex-1 min-h-0 overflow-y-auto vcp-scrollable px-4 py-3">
          <div v-if="docRendering && !docHtml" class="text-[12px] opacity-60 px-1 py-6">正在排版文档…</div>
          <div class="scriptorium-reader" v-html="docHtml"></div>
          <component :is="'style'" v-text="docCss"></component>
        </div>
        <div v-else class="flex-1 min-h-0 overflow-y-auto vcp-scrollable px-3 py-2 space-y-2">
          <div v-if="store.history.length" class="pb-1">
            <p class="text-[9px] font-black tracking-[0.16em] opacity-45 px-1 pb-1">可回退快照（手机记录）</p>
            <button
              v-for="item in store.history"
              :key="item.id"
              type="button"
              class="w-full text-left rounded-lg border px-3 py-2 mb-2 active:opacity-80"
              :class="confirmRollbackId === item.id ? 'border-red-500/50' : 'border-black/8 dark:border-white/8'"
              @click="rollbackTo(item.id)"
            >
              <div class="flex items-center gap-2">
                <GitBranch :size="13" class="opacity-50" />
                <span class="flex-1 min-w-0 text-[11px] font-bold truncate">{{ item.summary }}</span>
                <span class="shrink-0 text-[9px] opacity-50 font-mono">{{ formatTime(item.at) }}</span>
              </div>
              <p class="mt-1 text-[9px] font-mono" :class="item.isCurrent ? 'opacity-35' : 'opacity-45'">
                {{ item.length }} 字符 · {{ item.isCurrent ? '当前版本' : (confirmRollbackId === item.id ? '再点一次确认回退' : '点击回退') }}
              </p>
            </button>
          </div>
          <p class="text-[9px] font-black tracking-[0.16em] opacity-45 px-1 pb-1">桌面文脉刻点</p>
          <p v-if="!store.lineage.length" class="text-[11px] opacity-50 text-center py-6">还没有文脉刻点</p>
          <div
            v-for="checkpoint in store.lineage"
            :key="checkpoint.id"
            class="rounded-lg border border-black/8 dark:border-white/8 px-3 py-2"
          >
            <div class="flex items-center gap-2">
              <GitBranch :size="13" class="opacity-50" />
              <span class="flex-1 min-w-0 text-[11px] font-bold truncate">{{ checkpoint.name || checkpoint.summary || checkpoint.id }}</span>
              <span class="shrink-0 text-[9px] opacity-45 font-mono">r{{ checkpoint.revision }}</span>
            </div>
            <p class="mt-1 text-[10px] opacity-60 truncate">{{ checkpoint.summary || '' }}</p>
            <p class="mt-1 text-[9px] opacity-40 font-mono">{{ authorName(checkpoint) }} · {{ formatTime(Number(checkpoint.createdAt)) }} · {{ checkpoint.status || '' }}</p>
          </div>
        </div>
        <footer class="shrink-0 px-4 pt-2 text-[9px] opacity-45 font-mono border-t border-black/5 dark:border-white/5" style="padding-bottom: calc(0.5rem + var(--vcp-safe-bottom, 0px));">
          {{ saveHint || '编辑走桌面 PR 审批（手机来源自动允许）' }}
        </footer>
      </section>
    </main>
  </SlidePage>
</template>

<style scoped>
.scriptorium-reader :deep(h1),
.scriptorium-reader :deep(h2),
.scriptorium-reader :deep(h3) { font-weight: 800; margin: 0.9em 0 0.45em; line-height: 1.35; }
.scriptorium-reader :deep(h1) { font-size: 1.3em; }
.scriptorium-reader :deep(h2) { font-size: 1.15em; }
.scriptorium-reader :deep(h3) { font-size: 1.02em; }
.scriptorium-reader :deep(p) { margin: 0.5em 0; }
.scriptorium-reader :deep(ul),
.scriptorium-reader :deep(ol) { margin: 0.5em 0; padding-left: 1.2em; list-style: disc; }
.scriptorium-reader :deep(ol) { list-style: decimal; }
.scriptorium-reader :deep(blockquote) { margin: 0.6em 0; padding-left: 0.7em; border-left: 2px solid var(--highlight-text); opacity: 0.85; }
.scriptorium-reader :deep(code) { font-family: ui-monospace, monospace; font-size: 0.92em; background: rgba(127, 127, 127, 0.16); padding: 0.1em 0.35em; border-radius: 4px; }
.scriptorium-reader :deep(pre) { padding: 10px 12px; border-radius: 8px; overflow-x: auto; background: rgba(127, 127, 127, 0.14); }
.scriptorium-reader :deep(table) { width: 100%; border-collapse: collapse; font-size: 0.9em; }
.scriptorium-reader :deep(th),
.scriptorium-reader :deep(td) { border: 1px solid rgba(127, 127, 127, 0.28); padding: 4px 6px; }
</style>
