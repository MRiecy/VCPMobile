<script setup lang="ts">
/**
 * V工程 · ProjectForge 施工图（手机端，对齐桌面 VCPChat 的 V工程应用）
 *
 * 结构对齐桌面版的三页侧栏：
 *   [工程] 工程列表 / 工程详情（概览、开发脉络、改动文件、搜索历史、验收报告）
 *   [源码] 工作区文件树 / 查看 / 编辑 / 大纲与符号
 *   [Git]  工作区 Git 状态 / 差异 / 提交 / 推送
 * 数据统一走 src/core/bridge/bridgeConfig.ts 解析出的 MobilePluginBridge。
 */
import { computed, ref, watch } from 'vue';
import {
  ChevronLeft,
  FileCode2,
  GitBranch,
  LayoutGrid,
  RefreshCw,
  Settings,
  ShieldCheck,
  X,
} from "@lucide/vue";
import SlidePage from '../../components/ui/SlidePage.vue';
import { useNotificationStore } from '../../core/stores/notification';
import { useOverlayStore } from '../../core/stores/overlay';
import { BRIDGE_CONFIG_SOURCE_LABELS } from '../../core/bridge/bridgeConfig';
import { useProjectForgeStore } from './projectForgeStore';
import PfProjectList from './components/PfProjectList.vue';
import PfProjectDetail from './components/PfProjectDetail.vue';
import PfSourcePanel from './components/PfSourcePanel.vue';
import PfGitPanel from './components/PfGitPanel.vue';

const props = defineProps<{ isOpen: boolean; zIndex: number }>();
const emit = defineEmits<{ close: [] }>();

const store = useProjectForgeStore();
const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();

type Mode = 'project' | 'source' | 'git';
const mode = ref<Mode>('project');
const activeProjectId = ref('');
const refreshing = ref(false);
const settingsOpen = ref(false);
const sourcePanelRef = ref<{ hasUnsavedChanges?: () => boolean } | null>(null);
const sourceDirty = ref(false);

const settingsBase = ref('');
const settingsToken = ref('');
const settingsSignature = ref('');
const testing = ref(false);

const MODES: { key: Mode; label: string; icon: unknown }[] = [
  { key: 'project', label: '工程', icon: LayoutGrid },
  { key: 'source', label: '源码', icon: FileCode2 },
  { key: 'git', label: 'Git', icon: GitBranch },
];

const bridgeSourceLabel = computed(
  () => BRIDGE_CONFIG_SOURCE_LABELS[store.configSource] || '未知来源',
);
const connectionOk = computed(() => Boolean(store.connection?.ok));
const connectionText = computed(() => store.connection?.message || '未连接');
const activeProjectName = computed(
  () => store.detail?.project?.name || store.currentProject?.name || '工程详情',
);

const title = computed(() => {
  if (mode.value === 'source') return '源码';
  if (mode.value === 'git') return 'Git';
  return activeProjectId.value ? activeProjectName.value : '施工工程';
});

function syncSettingsDraft(): void {
  settingsBase.value = store.baseUrl;
  settingsToken.value = store.token;
  settingsSignature.value = store.signature || '';
}

function openSettings(): void {
  syncSettingsDraft();
  settingsOpen.value = true;
}

function notify(
  type: 'success' | 'error' | 'info' | 'warning',
  message: string,
  titleText = 'V工程',
): void {
  notificationStore.addNotification({
    id: `pf-view-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: titleText,
    message,
    type,
    duration: type === 'error' ? 4800 : 2400,
    toastOnly: true,
  });
}

async function confirmLeaveSource(): Promise<boolean> {
  if (mode.value !== 'source') return true;
  const dirty = sourceDirty.value || Boolean(sourcePanelRef.value?.hasUnsavedChanges?.());
  if (!dirty) return true;
  return overlayStore.showConfirm({
    title: '未保存的修改',
    message: '源码模式还有未保存的修改，离开将丢失。确定离开吗？',
    isDanger: true,
  });
}

async function handleBack(): Promise<void> {
  if (settingsOpen.value) {
    settingsOpen.value = false;
    return;
  }
  if (mode.value === 'project' && activeProjectId.value) {
    activeProjectId.value = '';
    return;
  }
  if (!(await confirmLeaveSource())) return;
  emit('close');
}

async function handleRefresh(): Promise<void> {
  if (refreshing.value) return;
  refreshing.value = true;
  try {
    await store.refreshAll();
    if (activeProjectId.value) {
      await store.openProject(activeProjectId.value, { force: true });
    }
    notify('success', '已刷新');
  } catch (error) {
    notify('error', String((error as Error)?.message || error), '刷新失败');
  } finally {
    refreshing.value = false;
  }
}

async function selectMode(next: Mode): Promise<void> {
  if (next === mode.value) return;
  if (!(await confirmLeaveSource())) return;
  mode.value = next;
  settingsOpen.value = false;
}

function onSelectProject(projectId: string): void {
  activeProjectId.value = projectId;
}

async function testBridge(): Promise<void> {
  if (testing.value) return;
  testing.value = true;
  try {
    const ok = await store.testConnection();
    notify(ok ? 'success' : 'error', ok ? '桥接连接成功' : store.connection?.message || '连接失败');
  } catch (error) {
    notify('error', String((error as Error)?.message || error), '连接失败');
  } finally {
    testing.value = false;
  }
}

async function saveBridgeSettings(): Promise<void> {
  try {
    await store.saveSettings({
      baseUrl: settingsBase.value.trim(),
      token: settingsToken.value.trim(),
    });
    if (settingsSignature.value !== (store.signature || '')) {
      store.setSignature(settingsSignature.value.trim());
    }
    settingsOpen.value = false;
    notify('success', '已保存桥接设置');
  } catch (error) {
    notify('error', String((error as Error)?.message || error), '保存失败');
  }
}

async function resetBridgeSettings(): Promise<void> {
  try {
    await store.resetSettings();
    syncSettingsDraft();
    notify('success', '已恢复默认桥接地址');
  } catch (error) {
    notify('error', String((error as Error)?.message || error), '恢复失败');
  }
}

watch(
  () => props.isOpen,
  (open) => {
    if (!open) return;
    settingsOpen.value = false;
    void store.bootstrap().catch(() => undefined);
  },
  { immediate: true },
);
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex" @close="emit('close')">
    <main class="h-full min-h-0 flex flex-col bg-[var(--primary-bg)] text-[var(--primary-text)] no-swipe">
      <header
        class="shrink-0 px-3 flex items-center gap-2 border-b border-black/5 dark:border-white/5"
        style="padding-top: var(--vcp-safe-top, 24px); min-height: calc(3.5rem + var(--vcp-safe-top, 24px));"
      >
        <button
          type="button"
          class="w-9 h-9 shrink-0 flex items-center justify-center rounded-full active:scale-95"
          aria-label="返回"
          @click="handleBack"
        >
          <ChevronLeft :size="20" />
        </button>
        <div class="flex-1 min-w-0">
          <div class="text-[9px] font-black tracking-[0.18em] opacity-45">PROJECTFORGE · V工程</div>
          <div class="text-sm font-bold truncate">{{ title }}</div>
        </div>
        <button
          type="button"
          class="w-8 h-8 flex items-center justify-center rounded-full"
          aria-label="刷新"
          :disabled="refreshing"
          @click="handleRefresh"
        >
          <RefreshCw :size="16" :class="refreshing ? 'animate-spin' : ''" />
        </button>
        <button
          type="button"
          class="w-8 h-8 flex items-center justify-center rounded-full"
          aria-label="V工程设置"
          @click="openSettings"
        >
          <Settings :size="16" />
        </button>
      </header>

      <div class="shrink-0 px-3 pt-2">
        <div class="pf-mode-bar">
          <button
            v-for="item in MODES"
            :key="item.key"
            type="button"
            class="pf-mode-btn"
            :class="{ 'pf-mode-btn--active': mode === item.key }"
            :aria-pressed="mode === item.key"
            @click="selectMode(item.key)"
          >
            <component :is="item.icon" :size="15" />
            <span>{{ item.label }}</span>
          </button>
        </div>
      </div>

      <div class="flex-1 min-h-0 overflow-hidden px-1 pt-1">
        <PfProjectList
          v-if="mode === 'project' && !activeProjectId"
          :active-project-id="activeProjectId"
          @select="onSelectProject"
        />
        <PfProjectDetail
          v-else-if="mode === 'project'"
          :key="activeProjectId"
          :project-id="activeProjectId"
          @back="activeProjectId = ''"
        />
        <PfSourcePanel
          v-else-if="mode === 'source'"
          ref="sourcePanelRef"
          :is-open="true"
          @dirty-change="sourceDirty = $event"
        />
        <PfGitPanel v-else />
      </div>
    </main>

    <!-- 桥接设置 -->
    <div
      v-if="settingsOpen"
      class="fixed inset-0 flex flex-col justify-end"
      :style="{ zIndex: props.zIndex + 5 }"
    >
      <div class="absolute inset-0 bg-black/50" @click="settingsOpen = false" />
      <section
        class="relative rounded-t-2xl px-4 pt-4 pb-5 bg-[var(--secondary-bg)] text-[var(--primary-text)] max-h-[78vh] overflow-y-auto"
        style="padding-bottom: calc(1.25rem + var(--vcp-safe-bottom, 16px));"
        aria-label="V工程设置"
      >
        <div class="flex items-center gap-2 mb-3">
          <ShieldCheck :size="16" class="opacity-70" />
          <div class="text-sm font-bold flex-1">桥接设置</div>
          <button
            type="button"
            class="w-8 h-8 flex items-center justify-center rounded-full opacity-70"
            aria-label="关闭设置"
            @click="settingsOpen = false"
          >
            <X :size="16" />
          </button>
        </div>

        <div class="pf-conn-line mb-3">
          <span class="pf-conn-dot" :class="connectionOk ? 'pf-conn-dot--ok' : 'pf-conn-dot--bad'" />
          <span class="truncate">{{ connectionText }}</span>
        </div>

        <label class="pf-field">
          <span class="pf-field-label">桥接地址 Base</span>
          <input v-model="settingsBase" class="pf-input" placeholder="http://桌面IP:端口/api/mobile-bridge" />
        </label>
        <label class="pf-field">
          <span class="pf-field-label">Bearer Token</span>
          <input v-model="settingsToken" class="pf-input" type="password" placeholder="MobilePluginBridge 的 Bearer Token" />
        </label>
        <label class="pf-field">
          <span class="pf-field-label">施工署名（回退/提交记录用）</span>
          <input v-model="settingsSignature" class="pf-input" placeholder="例如 @手机" />
        </label>
        <div class="text-[10px] opacity-55 mb-3">地址来源：{{ bridgeSourceLabel }}。留空恢复默认；默认会跟随「设置 → 同步」的桌面地址。</div>

        <div class="flex flex-wrap gap-2">
          <button type="button" class="pf-btn pf-btn--primary" :disabled="testing" @click="testBridge">
            {{ testing ? '测试中…' : '测试连接' }}
          </button>
          <button type="button" class="pf-btn" @click="saveBridgeSettings">保存</button>
          <button type="button" class="pf-btn" @click="resetBridgeSettings">恢复默认</button>
        </div>
      </section>
    </div>
  </SlidePage>
</template>

<style scoped>
.pf-mode-bar {
  display: flex;
  gap: 4px;
  padding: 3px;
  border-radius: 999px;
  background: var(--secondary-bg, rgba(127, 127, 127, 0.12));
  border: 1px solid var(--border-color, rgba(127, 127, 127, 0.18));
}
.pf-mode-btn {
  flex: 1;
  min-height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  opacity: 0.7;
  transition: all 0.16s ease;
}
.pf-mode-btn--active {
  opacity: 1;
  background: var(--primary-bg, #fff);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.18);
}
.pf-conn-line {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  opacity: 0.85;
}
.pf-conn-dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  flex-shrink: 0;
  background: #94a3b8;
}
.pf-conn-dot--ok { background: #22c55e; }
.pf-conn-dot--bad { background: #ef4444; }
.pf-field { display: flex; flex-direction: column; gap: 4px; margin-bottom: 10px; }
.pf-field-label { font-size: 11px; opacity: 0.6; }
.pf-input {
  width: 100%;
  min-height: 42px;
  padding: 0 12px;
  border-radius: 10px;
  font-size: 12px;
  background: var(--primary-bg);
  border: 1px solid var(--border-color, rgba(127, 127, 127, 0.24));
  color: var(--primary-text);
  outline: none;
}
.pf-btn {
  min-height: 40px;
  padding: 0 14px;
  border-radius: 10px;
  font-size: 12px;
  font-weight: 600;
  border: 1px solid var(--border-color, rgba(127, 127, 127, 0.24));
  background: transparent;
  color: var(--primary-text);
}
.pf-btn--primary {
  background: #6366f1;
  border-color: #6366f1;
  color: #fff;
}
.pf-btn:disabled { opacity: 0.55; }
</style>
