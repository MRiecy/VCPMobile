<script setup lang="ts">
import { ref, computed } from 'vue';
import type { SyncTreeNode } from '../core/utils/syncService';

defineOptions({ name: 'SyncFileTree' });

const props = defineProps<{
  node: SyncTreeNode;
  depth: number;
  selectedPaths: Set<string>;
  getFileStatus?: (path: string) => { label: string, color: string, icon: string };
}>();

const emit = defineEmits<{
  (e: 'toggleFiles', paths: string[], checked: boolean): void;
}>();

const isExpanded = ref(props.depth === 0); // Expand root by default

const isFile = computed(() => props.node.type === 'file');

// For directories, we check if ALL its leaf files are in selectedPaths
const childFiles = computed(() => {
  const files: string[] = [];
  const gather = (n: SyncTreeNode) => {
    if (n.type === 'file') {
      files.push(n.path);
    } else if (n.children) {
      Object.values(n.children).forEach(gather);
    }
  };
  gather(props.node);
  return files;
});

const isChecked = computed(() => {
  if (isFile.value) {
    return props.selectedPaths.has(props.node.path);
  } else {
    if (childFiles.value.length === 0) return false;
    return childFiles.value.every(f => props.selectedPaths.has(f));
  }
});

const isIndeterminate = computed(() => {
  if (isFile.value) return false;
  if (childFiles.value.length === 0) return false;
  const selectedCount = childFiles.value.filter(f => props.selectedPaths.has(f)).length;
  return selectedCount > 0 && selectedCount < childFiles.value.length;
});

const toggleSelect = () => {
  const targetState = !isChecked.value;
  if (isFile.value) {
    emit('toggleFiles', [props.node.path], targetState);
  } else {
    emit('toggleFiles', childFiles.value, targetState);
  }
};

const onChildToggle = (paths: string[], checked: boolean) => {
  emit('toggleFiles', paths, checked);
};

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

const toggleExpand = () => {
  if (!isFile.value) {
    isExpanded.value = !isExpanded.value;
  }
};
</script>

<template>
  <div class="sync-tree-node">
    <div class="flex items-center py-2 pr-2 hover:bg-white/5 rounded-lg transition-colors cursor-pointer select-none"
      :style="{ paddingLeft: `${depth * 1.2 + 0.5}rem` }" @click="toggleExpand">
      <!-- Expand/Collapse Icon -->
      <div class="w-6 h-6 flex-center shrink-0 opacity-50 mr-1">
        <svg v-if="!isFile" class="transition-transform duration-200" :class="{ 'rotate-90': isExpanded }" width="16"
          height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"
          stroke-linejoin="round">
          <path d="m9 18 6-6-6-6" />
        </svg>
        <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <polyline points="14 2 14 8 20 8" />
          <line x1="16" y1="13" x2="8" y2="13" />
          <line x1="16" y1="17" x2="8" y2="17" />
          <polyline points="10 9 9 9 8 9" />
        </svg>
      </div>

      <!-- Icon & Name -->
      <div class="flex flex-col min-w-0 flex-1">
        <div class="flex items-center gap-1.5 min-w-0">
          <span class="text-sm font-medium truncate" :class="{ 'opacity-80': isFile }">{{ node.name }}</span>
          <!-- Status Tag for Files -->
          <template v-if="isFile && getFileStatus">
            <div
              :class="['flex items-center gap-0.5 shrink-0 px-1 rounded bg-white/5 border border-white/5', getFileStatus(node.path).color]">
              <svg width="8" height="8" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"
                stroke-linecap="round" stroke-linejoin="round">
                <path :d="getFileStatus(node.path).icon" />
              </svg>
              <span class="text-[8px] font-black uppercase tracking-tighter">{{ getFileStatus(node.path).label }}</span>
            </div>
          </template>
        </div>
        <span v-if="!isFile" class="text-[10px] opacity-40">{{ childFiles.length }} 项 · {{ formatBytes(node.sizeBytes)
          }}</span>
        <span v-else class="text-[10px] opacity-40">{{ formatBytes(node.sizeBytes) }}</span>
      </div>

      <!-- Checkbox -->
      <div class="p-2 shrink-0 flex items-center" @click.stop>
        <input type="checkbox" :checked="isChecked" :indeterminate="isIndeterminate" @change="toggleSelect"
          class="w-5 h-5 accent-blue-500 rounded cursor-pointer" />
      </div>
    </div>

    <!-- Children -->
    <div v-if="!isFile && isExpanded && node.children" class="flex flex-col">
      <SyncFileTree v-for="(child, key) in node.children" :key="key" :node="child" :depth="depth + 1"
        :selectedPaths="selectedPaths" :getFileStatus="getFileStatus" @toggleFiles="onChildToggle" />
    </div>
  </div>
</template>