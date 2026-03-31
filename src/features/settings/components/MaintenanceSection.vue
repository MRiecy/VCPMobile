<script setup lang="ts">
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsActionButton from '../../../components/settings/SettingsActionButton.vue';
import SettingsInlineStatus from '../../../components/settings/SettingsInlineStatus.vue';

const cleanupStatus = ref<{ type: 'success' | 'error' | 'loading' | null; message: string }>({ type: null, message: '' });

const cleanupAttachments = async () => {
  cleanupStatus.value = { type: 'loading', message: '正在深度扫描孤儿附件...' };
  try {
    const result = await invoke<string>('cleanup_orphaned_attachments');
    cleanupStatus.value = { type: 'success', message: result };
    setTimeout(() => { cleanupStatus.value = { type: null, message: '' }; }, 5000);
  } catch (e: any) {
    cleanupStatus.value = { type: 'error', message: `清理失败: ${e}` };
  }
};
</script>

<template>
  <SettingsSection title="数据维护 (Maintenance)" accent-color="bg-red-500">
    <SettingsCard>
      <SettingsRow 
        title="附件库垃圾回收 (GC)" 
        description="深度扫描并删除未被引用的孤立附件与缩略图"
      >
        <template #action>
          <SettingsActionButton 
            variant="danger" 
            size="sm"
            :loading="cleanupStatus.type === 'loading'"
            @click="cleanupAttachments"
          >
            立即清理
          </SettingsActionButton>
        </template>
      </SettingsRow>
      
      <div v-if="cleanupStatus.type" class="mt-3">
        <SettingsInlineStatus 
          :type="cleanupStatus.type" 
          :message="cleanupStatus.message" 
          mono
        />
      </div>
    </SettingsCard>
  </SettingsSection>
</template>
