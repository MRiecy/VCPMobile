<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import Sortable from "sortablejs";
import {
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  Folder,
  GripVertical,
  Search,
  Trash2,
  X,
} from "lucide-vue-next";
import type { DiaryFolderCategory } from "../types";
import { diaryFolderCategory } from "../types";

type SheetMode = "select" | "manage" | "move";

const props = defineProps<{
  open: boolean;
  mode: SheetMode;
  folders: string[];
  hiddenFolders: string[];
  collapsedCategories: DiaryFolderCategory[];
  selectedFolder: string;
}>();

const emit = defineEmits<{
  close: [];
  select: [folder: string];
  move: [folder: string];
  hide: [folder: string];
  restore: [folder: string];
  toggleCategory: [category: DiaryFolderCategory];
  reorder: [folders: string[]];
  deleteFolder: [folder: string];
}>();

const query = ref("");
const sortableRoot = ref<HTMLElement | null>(null);
let sortable: Sortable | null = null;

const title = computed(() => {
  if (props.mode === "manage") return "管理文件夹";
  if (props.mode === "move") return "移动到文件夹";
  return "选择文件夹";
});

const filteredFolders = computed(() => {
  const term = query.value.trim().toLocaleLowerCase("zh-CN");
  return props.folders.filter((folder) => {
    if (props.mode === "select" && props.hiddenFolders.includes(folder)) return false;
    return !term || folder.toLocaleLowerCase("zh-CN").includes(term);
  });
});

const groupedFolders = computed(() => ({
  diary: filteredFolders.value.filter((folder) => diaryFolderCategory(folder) === "diary"),
  cluster: filteredFolders.value.filter((folder) => diaryFolderCategory(folder) === "cluster"),
}));

function destroySortable(): void {
  sortable?.destroy();
  sortable = null;
}

async function initializeSortable(): Promise<void> {
  destroySortable();
  if (!props.open || props.mode !== "manage" || query.value.trim()) return;
  await nextTick();
  if (!sortableRoot.value) return;

  sortable = Sortable.create(sortableRoot.value, {
    animation: 120,
    handle: ".diary-folder-drag-handle",
    delay: 180,
    delayOnTouchOnly: true,
    touchStartThreshold: 3,
    forceFallback: true,
    ghostClass: "diary-folder-ghost",
    onEnd: (event) => {
      if (event.oldIndex === undefined || event.newIndex === undefined) return;
      const next = [...props.folders];
      const [moved] = next.splice(event.oldIndex, 1);
      if (!moved) return;
      next.splice(event.newIndex, 0, moved);
      emit("reorder", next);
    },
  });
}

watch(
  () => [props.open, props.mode, query.value, props.folders.join("\u0000")] as const,
  () => void initializeSortable(),
  { immediate: true },
);

watch(() => props.open, (open) => {
  if (!open) query.value = "";
});

onUnmounted(destroySortable);

function activateFolder(folder: string): void {
  if (props.mode === "move") emit("move", folder);
  else emit("select", folder);
}
</script>

<template>
  <Transition name="diary-sheet">
    <div v-if="open" class="fixed inset-0 z-sheet no-swipe" role="presentation">
      <button
        type="button"
        class="absolute inset-0 w-full h-full border-0 bg-black/45"
        aria-label="关闭文件夹面板"
        @click="emit('close')"
      />
      <section
        class="diary-folder-sheet absolute inset-x-0 bottom-0 max-h-[76vh] flex flex-col text-[var(--primary-text)]"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
      >
        <span class="diary-sheet-grabber" aria-hidden="true" />
        <header class="diary-sheet-header">
          <div class="min-w-0 flex-1">
            <span class="diary-sheet-eyebrow">VCP MEMO · FOLDERS</span>
            <h2>{{ title }}</h2>
          </div>
          <button type="button" class="diary-sheet-icon" aria-label="关闭" @click="emit('close')">
            <X :size="19" />
          </button>
        </header>

        <label class="diary-folder-search">
          <Search :size="15" class="text-[var(--diary-muted-text)]" />
          <input
            v-model="query"
            class="min-w-0 flex-1 border-0 bg-transparent text-sm text-[var(--primary-text)] outline-none"
            placeholder="筛选文件夹"
            aria-label="筛选文件夹"
          />
        </label>

        <div class="flex-1 min-h-0 overflow-y-auto vcp-scrollable no-rubber-band no-swipe pb-[var(--vcp-safe-bottom,48px)]">
          <div v-if="filteredFolders.length === 0" class="px-4 py-10 text-center text-xs text-[var(--diary-muted-text)]">
            没有匹配的文件夹
          </div>

          <div v-else-if="mode === 'manage'" ref="sortableRoot">
            <div
              v-for="folder in filteredFolders"
              :key="folder"
              :data-folder="folder"
              class="diary-folder-manage-row"
            >
              <button
                type="button"
                class="diary-folder-drag-handle w-12 h-12 inline-flex items-center justify-center border-0 bg-transparent text-[var(--diary-muted-text)] touch-none"
                :disabled="Boolean(query.trim())"
                aria-label="拖动排序"
              >
                <GripVertical :size="17" />
              </button>
              <span class="diary-folder-icon"><Folder :size="15" /></span>
              <span class="min-w-0 flex-1 truncate text-sm">{{ folder }}</span>
              <span v-if="hiddenFolders.includes(folder)" class="text-[10px] text-[var(--diary-muted-text)]">已隐藏</span>
              <button
                type="button"
                class="diary-sheet-icon"
                :aria-label="hiddenFolders.includes(folder) ? `恢复 ${folder}` : `隐藏 ${folder}`"
                @click="hiddenFolders.includes(folder) ? emit('restore', folder) : emit('hide', folder)"
              >
                <Eye v-if="hiddenFolders.includes(folder)" :size="16" />
                <EyeOff v-else :size="16" />
              </button>
              <button
                type="button"
                class="diary-sheet-icon text-[var(--danger-color)]"
                :aria-label="`删除空文件夹 ${folder}`"
                @click="emit('deleteFolder', folder)"
              >
                <Trash2 :size="16" />
              </button>
            </div>
          </div>

          <template v-else>
            <section v-for="category in (['diary', 'cluster'] as DiaryFolderCategory[])" :key="category">
              <button
                type="button"
                class="diary-folder-category"
                @click="emit('toggleCategory', category)"
              >
                <ChevronRight v-if="collapsedCategories.includes(category)" :size="14" />
                <ChevronDown v-else :size="14" />
                {{ category === "diary" ? "日记 / 知识库" : "思维簇" }}
                <span class="font-mono opacity-60">{{ groupedFolders[category].length }}</span>
              </button>
              <template v-if="!collapsedCategories.includes(category)">
                <button
                  v-for="folder in groupedFolders[category]"
                  :key="folder"
                  type="button"
                  class="diary-folder-row"
                  :class="selectedFolder === folder ? 'diary-folder-current' : ''"
                  @click="activateFolder(folder)"
                >
                  <span class="diary-folder-icon"><Folder :size="15" /></span>
                  <span class="truncate">{{ folder }}</span>
                  <span v-if="mode === 'move' && hiddenFolders.includes(folder)" class="ml-auto text-[10px] text-[var(--diary-muted-text)]">本机已隐藏</span>
                </button>
              </template>
            </section>
          </template>
        </div>
      </section>
    </div>
  </Transition>
</template>

<style scoped>
.diary-folder-sheet {
  overflow: hidden;
  border: 1px solid var(--diary-line);
  border-bottom: 0;
  border-radius: 18px 18px 0 0;
  background: var(--primary-bg);
}

.diary-sheet-grabber {
  width: 34px;
  height: 4px;
  flex: 0 0 auto;
  align-self: center;
  margin-top: 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--diary-muted-text) 34%, transparent);
}

.diary-sheet-header {
  min-height: 60px;
  box-sizing: border-box;
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px 6px 16px;
  border-bottom: 1px solid var(--diary-line);
}

.diary-sheet-header h2 {
  margin: 2px 0 0;
  font-size: 15px;
  font-weight: 700;
  line-height: 20px;
}

.diary-sheet-eyebrow {
  display: block;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 700;
  line-height: 12px;
  letter-spacing: 0.14em;
}

.diary-sheet-icon {
  width: 48px;
  height: 48px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: 12px;
  background: transparent;
  color: inherit;
}

.diary-sheet-icon:active {
  background: var(--diary-surface);
}

.diary-folder-search {
  height: 46px;
  box-sizing: border-box;
  flex: 0 0 auto;
  margin: 10px 12px;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 14px;
  border: 1px solid var(--diary-line);
  border-radius: 999px;
  background: var(--diary-surface);
}

.diary-folder-search:focus-within {
  border-color: color-mix(in srgb, var(--highlight-text) 58%, var(--diary-line));
}

.diary-folder-manage-row {
  min-height: 60px;
  margin: 0 10px;
  display: flex;
  align-items: center;
  gap: 6px;
  border-bottom: 1px solid var(--diary-line);
}

.diary-folder-category {
  width: 100%;
  height: 40px;
  padding: 0 16px;
  display: flex;
  align-items: center;
  gap: 7px;
  border: 0;
  background: transparent;
  color: var(--diary-muted-text);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-align: left;
}

.diary-folder-row {
  position: relative;
  width: calc(100% - 20px);
  min-height: 50px;
  margin: 2px 10px;
  padding: 0 12px;
  display: flex;
  align-items: center;
  gap: 10px;
  border: 1px solid transparent;
  border-radius: 999px;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
  text-align: left;
}

.diary-folder-row:active {
  background: var(--diary-surface-soft);
}

.diary-folder-icon {
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--diary-line);
  border-radius: 10px;
  background: var(--diary-surface-soft);
  color: var(--diary-muted-text);
}

.diary-folder-current {
  border-color: color-mix(in srgb, var(--highlight-text) 26%, transparent);
  background: color-mix(in srgb, var(--highlight-text) 8%, var(--diary-surface-soft));
}

.diary-folder-current::before {
  content: "";
  position: absolute;
  inset: 10px auto 10px 0;
  width: 2px;
  border-radius: 999px;
  background: var(--highlight-text);
}

.diary-folder-ghost {
  opacity: 0.45;
}

.diary-sheet-enter-active,
.diary-sheet-leave-active {
  transition: opacity 200ms ease;
}

.diary-sheet-enter-from,
.diary-sheet-leave-to {
  opacity: 0;
}

@media (prefers-reduced-motion: reduce) {
  .diary-sheet-enter-active,
  .diary-sheet-leave-active {
    transition: none;
  }
}
</style>
