<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { syncService } from '../../../core/utils/syncService';
import type { AppSettings } from '../../../core/stores/settings';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsActionButton from '../../../components/settings/SettingsActionButton.vue';
import SettingsInlineStatus from '../../../components/settings/SettingsInlineStatus.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: 'save-request'): void;
  (e: 'open-sync'): void;
}>();

const pingStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });
const emoticonStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const testSyncConnection = async () => {
  emit('save-request');

  pingStatus.value = { type: 'loading', message: '正在连接桌面端...' };
  try {
    const res = await syncService.pingServer(
      props.settings.syncServerIp,
      props.settings.syncServerPort,
      props.settings.syncToken
    );
    pingStatus.value = { type: 'success', message: `连接成功！设备: ${res.deviceName}` };
  } catch (e: any) {
    pingStatus.value = { type: 'error', message: `连接失败: ${e.message}` };
  }
};

const openSyncCenter = () => {
  emit('open-sync');
};

const rebuildEmoticonLibrary = async () => {
  emoticonStatus.value = { type: 'loading', message: '正在扫描表情包...' };
  try {
    const count = await invoke<number>('regenerate_emoticon_library');
    emoticonStatus.value = { type: 'success', message: `成功重载 ${count} 个表情包` };
    setTimeout(() => { emoticonStatus.value = { type: null, message: '' }; }, 3000);
  } catch (e: any) {
    emoticonStatus.value = { type: 'error', message: `重载失败: ${e}` };
  }
};
</script>

<template>
  <SettingsSection title="桌面端数据同步" accent-color="bg-green-500">
    <SettingsCard class="space-y-5">
      <div class="flex gap-4">
        <div class="flex-[2]">
          <SettingsTextField 
            v-model="settings.syncServerIp" 
            label="同步服务器 IP" 
            placeholder="192.168.x.x" 
            mono
          />
        </div>
        <div class="flex-1">
          <SettingsTextField 
            v-model.number="settings.syncServerPort" 
            type="number" 
            label="端口" 
            placeholder="5974" 
            mono
            center
          />
        </div>
      </div>
      <SettingsTextField 
        v-model="settings.syncToken" 
        label="Mobile Sync Token" 
        placeholder="输入桌面端 config.env 中的 Token" 
        mono
      />

      <div class="pt-2 flex items-center justify-between gap-4">
        <SettingsInlineStatus 
          v-if="pingStatus.type"
          :type="pingStatus.type" 
          :message="pingStatus.message" 
          class="flex-1"
        />
        <div class="flex gap-2 shrink-0">
          <SettingsActionButton 
            variant="secondary" 
            size="sm"
            :loading="pingStatus.type === 'loading'"
            @click="testSyncConnection"
          >
            测试连接
          </SettingsActionButton>
          <SettingsActionButton 
            variant="primary" 
            size="sm"
            @click="openSyncCenter"
          >
            进入同步面板
          </SettingsActionButton>
        </div>
      </div>

      <div class="border-t border-black/5 dark:border-white/5 pt-2">
        <SettingsRow 
          title="本地表情包修复库" 
          :description="emoticonStatus.message || 'IDLE'"
        >
          <template #action>
            <SettingsActionButton 
              variant="secondary" 
              size="sm"
              :loading="emoticonStatus.type === 'loading'"
              @click="rebuildEmoticonLibrary"
            >
              RESCAN
            </SettingsActionButton>
          </template>
        </SettingsRow>
      </div>
    </SettingsCard>
  </SettingsSection>
</template>
