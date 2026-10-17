<script setup lang="ts">
import { ref, onMounted, computed } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import { useUpdateStore } from '../../../core/stores/update';
import { useNotificationStore } from '../../../core/stores/notification';
import { isAndroid, isIOS, nativePlatform } from '../../../core/utils/platform';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsActionButton from '../../../components/settings/SettingsActionButton.vue';
import SettingsInlineStatus from '../../../components/settings/SettingsInlineStatus.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';

const currentVersion = ref('');
const checking = ref(false);
const justCheckedNoUpdate = ref(false);

const updateStore = useUpdateStore();
const notificationStore = useNotificationStore();

onMounted(async () => {
  if (isAndroid) {
    void updateStore.init().catch((error) => console.warn('[UpdateSection] Init failed:', error));
  }
  if (nativePlatform !== null) {
    try {
      currentVersion.value = await getVersion();
    } catch (e) {
      console.error('[UpdateSection] Failed to get version:', e);
    }
  }
});

const onAutoCheckChange = (value: boolean) => {
  const result = updateStore.setAutoCheckEnabled(value);
  if (!result.ok) {
    notificationStore.addNotification({
      type: 'error',
      title: '保存失败',
      message: '自动检查更新偏好未能保存，请重试。',
      toastOnly: true,
    });
  }
};

const rowDescription = computed(() => {
  if (!isAndroid) {
    const version = currentVersion.value ? `当前版本: v${currentVersion.value} · ` : '';
    return version + (isIOS ? '在应用外自行签名安装更新' : '应用内更新仅适用于 Android');
  }
  const { state, info } = updateStore.status;
  switch (state) {
    case 'checking':
      return '正在检查最新版本...';
    case 'downloading':
      return `正在下载: ${updateStore.progressPercent}% (点击查看进度)`;
    case 'verifying':
      return '正在校验安装包完整性...';
    case 'installing':
      return '正在启动安装器...';
    case 'readyToInstall':
      return info ? `v${info.latestVersion} 已就绪，点击安装` : '更新包已就绪';
    case 'failed':
      return '更新出错 (点击查看详情)';
    default:
      if (info?.hasUpdate) return `发现新版本: v${info.latestVersion} (点击查看)`;
      return currentVersion.value ? `当前版本: v${currentVersion.value}` : '获取版本中...';
  }
});

const rowClickable = computed(() => {
  return (
    !isAndroid ||
    !!updateStore.info?.hasUpdate ||
    ['downloading', 'verifying', 'installing', 'readyToInstall', 'failed'].includes(
      updateStore.state,
    )
  );
});

const checkUpdate = async () => {
  if (!isAndroid) {
    openPrompt();
    return;
  }
  checking.value = true;
  justCheckedNoUpdate.value = false;
  try {
    const status = await updateStore.check();
    if (status.info?.hasUpdate) {
      updateStore.openPrompt();
    } else {
      justCheckedNoUpdate.value = true;
      setTimeout(() => {
        justCheckedNoUpdate.value = false;
      }, 4000);
    }
  } catch (e) {
    console.error('[UpdateSection] check failed:', e);
  } finally {
    checking.value = false;
  }
};

const openPrompt = () => {
  if (!isAndroid) {
    notificationStore.addNotification({
      type: 'info',
      title: '版本更新',
      message: isIOS ? 'iOS 版本需在应用外自行签名安装更新' : '应用内 APK 更新仅适用于 Android',
      toastOnly: true,
    });
    return;
  }
  updateStore.openPrompt();
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
          v-if="rowClickable"
          variant="secondary"
          size="sm"
          @click.stop="openPrompt"
        >
          {{ updateStore.state === 'readyToInstall' ? '安装' : '查看' }}
        </SettingsActionButton>
        <SettingsActionButton
          v-else
          variant="secondary"
          size="sm"
          :loading="checking || updateStore.state === 'checking'"
          @click.stop="checkUpdate"
        >
          检查更新
        </SettingsActionButton>
      </template>
    </SettingsRow>

    <SettingsRow v-if="isAndroid" title="自动检查更新" description="启动时检查新版本（24 小时内最多一次）">
      <template #action>
        <SettingsSwitch
          :model-value="updateStore.autoCheckEnabled"
          aria-label="自动检查更新"
          @update:model-value="onAutoCheckChange"
        />
      </template>
    </SettingsRow>

    <!-- 状态反馈 -->
    <div v-if="justCheckedNoUpdate && updateStore.state === 'idle'" class="mt-2">
      <SettingsInlineStatus type="success" :message="`当前已是最新版本 (v${currentVersion})`" />
    </div>
    <div v-else-if="updateStore.error" class="mt-2">
      <SettingsInlineStatus type="error" :message="updateStore.error.message" />
    </div>
  </div>
</template>
