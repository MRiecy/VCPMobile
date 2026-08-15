<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { ChevronLeft, Info, SquareTerminal } from "lucide-vue-next";
import SlidePage from "../../../components/ui/SlidePage.vue";
import { useKeyboardInsets } from "../../../core/composables/useKeyboardInsets";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import { useVcpCliStore } from "../vcpCliStore";
import VcpCliManifestPanel from "./VcpCliManifestPanel.vue";
import VcpCliRunPanel from "./VcpCliRunPanel.vue";
import VcpCliSkillsPanel from "./VcpCliSkillsPanel.vue";
import VcpCliTerminalPanel from "./VcpCliTerminalPanel.vue";

const props = defineProps<{
  isOpen: boolean;
  zIndex: number;
}>();

const emit = defineEmits<{
  close: [];
}>();

type CliTab = "terminal" | "jobs" | "skills";

const store = useVcpCliStore();
const {
  hasInternalDetail,
  selectedJobId,
  selectedSkillId,
  runtimeLoading,
  runtimeError,
} = storeToRefs(store);
const { keyboardHeight } = useKeyboardInsets();
const { registerModal, unregisterModal } = useModalHistory();
const activeTab = ref<CliTab>("terminal");
const showInfo = ref(false);
const internalRegistered = ref(false);
const hasPageDetail = computed(
  () => hasInternalDetail.value || showInfo.value,
);

const pageTitle = computed(() => {
  if (selectedJobId.value) return "Job 输出";
  if (selectedSkillId.value) return "Skill 阅读";
  if (showInfo.value) return "工具说明";
  return "VCP CLI";
});

const pageEyebrow = computed(() => {
  if (selectedJobId.value) return "LOCAL JOB · BOUNDED TAIL";
  if (selectedSkillId.value) return "CONTROLLED CATALOG · READ ONLY";
  if (showInfo.value) return "MANIFEST · AGENT CONTRACT";
  if (activeTab.value === "terminal") return "LOCAL PTY · FOREGROUND SESSION";
  return "LOCAL WORKBENCH · JOBS FOREGROUND ONLY";
});

function registerInternalDetail(): void {
  if (!props.isOpen || !hasPageDetail.value || internalRegistered.value)
    return;
  internalRegistered.value = true;
  registerModal("VcpCli:Internal", () => {
    internalRegistered.value = false;
    if (hasInternalDetail.value) store.closeInternalDetail();
    else showInfo.value = false;
    return true;
  });
}

function releaseInternalDetail(): void {
  if (!internalRegistered.value) return;
  internalRegistered.value = false;
  unregisterModal("VcpCli:Internal");
}

function closeCurrentDetail(): void {
  if (hasInternalDetail.value) store.closeInternalDetail();
  else showInfo.value = false;
  releaseInternalDetail();
}

function handleNavigation(): void {
  if (hasPageDetail.value) {
    closeCurrentDetail();
    return;
  }
  emit("close");
}

function selectTab(tab: CliTab): void {
  showInfo.value = false;
  activeTab.value = tab;
  if (tab === "skills") void store.loadSkills();
}

watch(
  () => props.isOpen,
  (isOpen) => {
    if (isOpen) {
      activeTab.value = "terminal";
      showInfo.value = false;
      void store.openView();
      return;
    }
    releaseInternalDetail();
    store.closeView();
  },
  { immediate: true },
);

watch(hasPageDetail, (hasDetail) => {
  if (hasDetail) registerInternalDetail();
  else releaseInternalDetail();
});

onBeforeUnmount(() => {
  releaseInternalDetail();
  store.closeView();
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <main
      class="flex h-full min-h-0 w-full flex-col bg-[var(--primary-bg)] text-[var(--primary-text)]"
      aria-labelledby="vcp-cli-title"
    >
      <header
        class="shrink-0 border-b border-black/10 bg-[var(--primary-bg)] pt-[calc(var(--vcp-safe-top,24px)+6px)] dark:border-white/10"
      >
        <div class="flex min-h-13 min-w-0 items-center gap-2 px-2">
          <button
            type="button"
            class="flex h-11 w-11 shrink-0 items-center justify-center rounded-xl opacity-60 active:bg-black/5 active:opacity-100 dark:active:bg-white/5"
            :aria-label="hasPageDetail ? '返回 VCP CLI' : '关闭 VCP CLI'"
            data-vcp-cli-action="navigate-back"
            @click="handleNavigation"
          >
            <ChevronLeft :size="21" />
          </button>
          <SquareTerminal
            :size="18"
            class="shrink-0 text-[var(--highlight-text)]"
            aria-hidden="true"
          />
          <div class="min-w-0 flex-1">
            <p
              class="truncate text-[8px] font-black uppercase tracking-[0.14em] opacity-45"
            >
              {{ pageEyebrow }}
            </p>
            <h1 id="vcp-cli-title" class="truncate text-[16px] font-bold">
              {{ pageTitle }}
            </h1>
          </div>
          <button
            v-if="!hasInternalDetail && !showInfo"
            type="button"
            class="flex h-11 w-11 shrink-0 items-center justify-center opacity-55 active:opacity-100"
            aria-label="打开 VCP CLI 工具说明"
            data-vcp-cli-action="open-info"
            @click="showInfo = !showInfo"
          >
            <Info :size="18" />
          </button>
        </div>
        <nav
          v-if="!hasInternalDetail && !showInfo"
          class="grid grid-cols-3 border-t border-black/5 px-2 dark:border-white/5"
          aria-label="VCP CLI 页面"
        >
          <button
            v-for="tab in [
              { id: 'terminal', label: '终端' },
              { id: 'jobs', label: 'Jobs' },
              { id: 'skills', label: 'Skills' },
            ] as const"
            :key="tab.id"
            type="button"
            class="min-h-10 border-b-2 text-[10px] font-bold"
            :class="
              activeTab === tab.id
                ? 'border-[var(--highlight-text)] text-[var(--highlight-text)]'
                : 'border-transparent opacity-45'
            "
            :aria-current="activeTab === tab.id ? 'page' : undefined"
            :data-vcp-cli-tab="tab.id"
            @click="selectTab(tab.id)"
          >
            {{ tab.label }}
          </button>
        </nav>
      </header>

      <div
        v-if="activeTab !== 'jobs' && runtimeLoading"
        class="shrink-0 border-b border-blue-500/15 px-3 py-1.5 font-mono text-[9px] text-blue-600 dark:text-blue-400"
        role="status"
      >
        正在重新同步 Rust Runtime 代际…
      </div>
      <div
        v-else-if="activeTab !== 'jobs' && runtimeError"
        class="flex shrink-0 items-center gap-2 border-b border-red-500/20 px-3 py-1.5 text-[9px] text-red-500"
        role="alert"
      >
        <span class="min-w-0 flex-1 truncate font-mono">
          {{ runtimeError.code }} · {{ runtimeError.message }}
        </span>
        <button
          type="button"
          class="min-h-8 shrink-0 rounded-lg border border-red-500/25 px-2 font-bold"
          @click="store.refreshView()"
        >
          重试状态
        </button>
      </div>

      <VcpCliRunPanel
        v-if="selectedJobId || (!selectedSkillId && !showInfo && activeTab === 'jobs')"
        :keyboard-height="keyboardHeight"
      />
      <VcpCliSkillsPanel
        v-else-if="selectedSkillId || (!showInfo && activeTab === 'skills')"
      />
      <VcpCliManifestPanel
        v-else-if="showInfo"
        :is-open="props.isOpen && showInfo"
      />
      <VcpCliTerminalPanel
        v-else
        :keyboard-height="keyboardHeight"
      />
    </main>
  </SlidePage>
</template>
