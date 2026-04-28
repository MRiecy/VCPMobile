<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { invoke, Channel } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
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

interface DownloadProgress {
  downloaded: number;
  total: number | null;
}

type UpdateStatus =
  | { type: 'idle' }
  | { type: 'checking' }
  | { type: 'no-update' }
  | { type: 'update-available'; info: UpdateInfo }
  | { type: 'downloading'; progress: number; total: number | null }
  | { type: 'downloaded' }
  | { type: 'installing' }
  | { type: 'error'; message: string };

const currentVersion = ref('');
const status = ref<UpdateStatus>({ type: 'idle' });
const downloadUrlRef = ref<string | null>(null);

const progressPercent = computed(() => {
  if (status.value.type !== 'downloading') return 0;
  const { progress, total } = status.value;
  if (!total || total === 0) return 0;
  return Math.min(100, Math.round((progress / total) * 100));
});

onMounted(async () => {
  try {
    currentVersion.value = await getVersion();
  } catch (e) {
    console.error('[UpdateSection] Failed to get version:', e);
  }
});

const formatBytes = (bytes: number) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
};

const checkUpdate = async () => {
  status.value = { type: 'checking' };
  try {
    const info: UpdateInfo = await invoke('check_for_update');
    if (!info.hasUpdate) {
      status.value = { type: 'no-update' };
      setTimeout(() => {
        if (status.value.type === 'no-update') {
          status.value = { type: 'idle' };
        }
      }, 4000);
      return;
    }
    downloadUrlRef.value = info.downloadUrl;
    status.value = { type: 'update-available', info };
  } catch (e: any) {
    status.value = { type: 'error', message: String(e) };
  }
};

const downloadAndInstall = async () => {
  const info = status.value.type === 'update-available' ? status.value.info : null;
  if (!info?.downloadUrl) {
    status.value = { type: 'error', message: '缺少下载链接' };
    return;
  }

  status.value = { type: 'downloading', progress: 0, total: null };

  const channel = new Channel<DownloadProgress>();
  channel.onmessage = (msg) => {
    status.value = { type: 'downloading', progress: msg.downloaded, total: msg.total };
  };

  let apkPath: string;
  try {
    apkPath = await invoke('download_update', {
      url: info.downloadUrl,
      onProgress: channel,
    });
  } catch (e: any) {
    status.value = { type: 'error', message: `下载失败: ${e}` };
    return;
  }

  status.value = { type: 'installing' };

  try {
    await invoke('install_update', { apkPath });
    // 安装器已唤起，给用户提供提示
    status.value = { type: 'idle' };
  } catch (e: any) {
    // 本地安装失败，尝试用浏览器打开直链或 Release 页面
    try {
      if (info.downloadUrl) {
        await openUrl(info.downloadUrl);
        status.value = { type: 'idle' };
        return;
      } else if (info.releasePageUrl) {
        await openUrl(info.releasePageUrl);
        status.value = { type: 'idle' };
        return;
      }
    } catch {
      // ignore
    }
    status.value = { type: 'error', message: `安装失败: ${e}` };
  }
};
</script>

<template>
  <div class="space-y-2">
    <SettingsRow
      title="版本更新"
      :description="currentVersion ? `当前版本: ${currentVersion}` : '获取版本中...'"
    >
      <template #action>
        <SettingsActionButton
          variant="secondary"
          size="sm"
          :loading="status.type === 'checking'"
          :disabled="status.type === 'downloading' || status.type === 'installing'"
          @click="checkUpdate"
        >
          检查更新
        </SettingsActionButton>
      </template>
    </SettingsRow>

    <!-- 状态反馈 -->
    <div v-if="status.type === 'checking'" class="mt-2">
      <SettingsInlineStatus type="loading" message="正在检查最新版本..." />
    </div>
    <div v-else-if="status.type === 'no-update'" class="mt-2">
      <SettingsInlineStatus type="success" :message="`当前已是最新版本 (${currentVersion})`" />
    </div>
    <div v-else-if="status.type === 'error'" class="mt-2">
      <SettingsInlineStatus type="error" :message="status.message" multiline />
    </div>

    <!-- 发现更新 -->
    <div
      v-if="status.type === 'update-available'"
      class="mt-3 p-3 bg-white/5 rounded-lg space-y-2"
    >
      <div class="text-sm font-bold">
        发现新版本:
        <span class="font-mono text-blue-400">{{ status.info.latestVersion }}</span>
        <span v-if="status.info.apkSize" class="text-[10px] opacity-50 font-normal ml-2">
          ({{ formatBytes(status.info.apkSize) }})
        </span>
      </div>
      <div
        v-if="status.info.releaseNotes"
        class="text-xs opacity-60 max-h-32 overflow-y-auto whitespace-pre-wrap leading-relaxed"
      >
        {{ status.info.releaseNotes }}
      </div>
      <SettingsActionButton
        variant="primary"
        size="sm"
        full-width
        @click="downloadAndInstall"
      >
        下载并安装
      </SettingsActionButton>
    </div>

    <!-- 下载中 -->
    <div v-if="status.type === 'downloading'" class="mt-3 space-y-2">
      <div class="h-1.5 bg-white/10 rounded-full overflow-hidden">
        <div
          class="h-full bg-blue-500 transition-all duration-200"
          :style="{ width: progressPercent + '%' }"
        />
      </div>
      <div class="flex justify-between text-[10px] font-mono opacity-50">
        <span>{{ progressPercent }}%</span>
        <span v-if="status.total">
          {{ formatBytes(status.progress) }} / {{ formatBytes(status.total) }}
        </span>
        <span v-else>{{ formatBytes(status.progress) }}</span>
      </div>
    </div>

    <!-- 安装中 -->
    <div v-if="status.type === 'installing'" class="mt-2">
      <SettingsInlineStatus type="loading" message="正在启动安装器..." />
    </div>
  </div>
</template>
