<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { useUpdateStore } from '../../../core/stores/update';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsActionButton from '../../../components/settings/SettingsActionButton.vue';
import SettingsInlineStatus from '../../../components/settings/SettingsInlineStatus.vue';

interface UpdateInfo {
  hasUpdate: boolean;
  currentVersion: string;
  latestVersion: string;
  downloadUrl: string | null;
  releasePageUrl: string | null;
  releaseNotes: string | null;
  apkSize: number | null;
}

type LocalUpdateStatus =
  | { type: 'idle' }
  | { type: 'checking' }
  | { type: 'no-update' }
  | { type: 'update-available'; info: UpdateInfo };

const currentVersion = ref('');
const localStatus = ref<LocalUpdateStatus>({ type: 'idle' });

const updateStore = useUpdateStore();

const progressPercent = computed(() => {
  if (updateStore.status !== 'downloading') return 0;
  const progress = updateStore.downloadProgress;
  const total = updateStore.downloadTotal;
  if (!total || total === 0) return 0;
  return Math.min(100, Math.round((progress / total) * 100));
});

const rowDescription = computed(() => {
  if (updateStore.status === 'downloading') {
    return `正在下载: ${progressPercent.value}% (点击查看进度)`;
  }
  if (updateStore.status === 'installing') {
    return '正在启动安装器...';
  }
  if (updateStore.status === 'error') {
    return '更新出错 (点击查看详情)';
  }
  if (updateStore.latestVersion) {
    return `发现新版本: v${updateStore.latestVersion} (点击查看)`;
  }
  return currentVersion.value ? `当前版本: v${currentVersion.value}` : '获取版本中...';
});

const rowClickable = computed(() => {
  return !!updateStore.latestVersion ||
         updateStore.status === 'downloading' ||
         updateStore.status === 'installing' ||
         updateStore.status === 'error';
});

onMounted(async () => {
  try {
    currentVersion.value = await getVersion();
  } catch (e) {
    console.error('[UpdateSection] Failed to get version:', e);
  }
});

const checkUpdate = async () => {
  localStatus.value = { type: 'checking' };
  try {
    const info: UpdateInfo = await invoke('check_for_update');
    if (!info.hasUpdate) {
      localStatus.value = { type: 'no-update' };
      setTimeout(() => {
        if (localStatus.value.type === 'no-update') {
          localStatus.value = { type: 'idle' };
        }
      }, 4000);
      return;
    }
    updateStore.openPrompt(info);
    localStatus.value = { type: 'idle' };
  } catch (e: any) {
    localStatus.value = { type: 'idle' };
    updateStore.setError(String(e));
  }
};

const openPrompt = () => {
  if (updateStore.updateInfo) {
    updateStore.openPrompt(updateStore.updateInfo);
  }
};
</script>

<template>
  <div class="space-y-2">
    <SettingsRow
      title="版本更新"
      :description="rowDescription"
      :clickable="rowClickable"
      @click="openPrompt"
    >
      <template #action>
        <SettingsActionButton
          v-if="updateStore.status === 'downloading' || updateStore.status === 'installing' || updateStore.status === 'error'"
          variant="secondary"
          size="sm"
          @click.stop="openPrompt"
        >
          查看进度
        </SettingsActionButton>
        <SettingsActionButton
          v-else
          variant="secondary"
          size="sm"
          :loading="localStatus.type === 'checking'"
          @click.stop="checkUpdate"
        >
          检查更新
        </SettingsActionButton>
      </template>
    </SettingsRow>

    <!-- 状态反馈 -->
    <div v-if="localStatus.type === 'checking'" class="mt-2">
      <SettingsInlineStatus type="loading" message="正在检查最新版本..." />
    </div>
    <div v-else-if="localStatus.type === 'no-update' && updateStore.status === 'idle'" class="mt-2">
      <SettingsInlineStatus type="success" :message="`当前已是最新版本 (v${currentVersion})`" />
    </div>
  </div>
</template>
