<script setup lang="ts">
import { computed, ref } from 'vue';
import { useSettingsStore } from '../../../core/stores/settings';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useNotificationStore } from '../../../core/stores/notification';
import { buildSettingsUpdate } from '../../../core/config/fields';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import VcpCoreSettingsSection from '../components/VcpCoreSettingsSection.vue';
import SyncSettingsSection from '../components/SyncSettingsSection.vue';
import ConfigBackupSection from '../components/ConfigBackupSection.vue';
import { CONNECTION_FIELDS, type ConnectionFields } from '../editing/fieldSets';
import { useSettingsPageSession } from '../editing/useSettingsPageSession';
import SettingsDomainIssues from '../editing/SettingsDomainIssues.vue';
import type { SettingsSaveResult } from '../editing/useSettingsEditSession';
import { applyConfigBackup, type BackupImportResult, type ParsedConfigBackup } from '../useConfigBackup';

const props = defineProps<{ active: boolean }>();
const settingsStore = useSettingsStore();
const { session, loading, error, reload } = useSettingsPageSession(CONNECTION_FIELDS, () => props.active);
const { draft, leaving } = session;
const actionBusy = ref(false);
const disabled = computed(() => !props.active || leaving.value || actionBusy.value);

const edit = (patch: Partial<ConnectionFields>) => {
  if (disabled.value) return;
  for (const key of CONNECTION_FIELDS) {
    const value = patch[key];
    if (value !== undefined) session.edit(key, value);
  }
};

// Freeze only until the confirmed snapshot has been captured, not throughout file/network IO.
const commitForAction = async (): Promise<SettingsSaveResult> => {
  if (disabled.value) return { ok: false, error: '设置页面当前不可操作' };
  actionBusy.value = true;
  try {
    const result = await session.save();
    if (!props.active) return { ok: false, error: '设置页面已关闭' };
    return result;
  } finally {
    actionBusy.value = false;
  }
};

const applyImport = async (backup: ParsedConfigBackup): Promise<BackupImportResult> => {
  if (disabled.value) throw new Error('设置页面当前不可操作');
  actionBusy.value = true;
  try {
    const settled = await session.settlePending();
    if (!settled.ok) throw new Error(settled.error);
    if (!props.active) throw new Error('设置页面已关闭');
    return await applyConfigBackup(backup, async (changes, groupPreferences) => {
      const snapshot = settingsStore.snapshot;
      if (!snapshot) throw new Error('配置尚未加载');
      const result = await settingsStore.importBackup(buildSettingsUpdate(snapshot, changes), groupPreferences);
      if (result.status !== 'saved') {
        const issues = result.status === 'rejected' ? result.issues : result.conflicts;
        throw new Error(issues.map(issue => `${issue.domain}.${issue.field}: ${issue.message}`).join('\n'));
      }
      if (props.active) {
        session.acceptImport(CONNECTION_FIELDS.filter((key) => Object.prototype.hasOwnProperty.call(changes, key)));
      }
      if (Object.keys(groupPreferences).length) {
        try {
          await useAssistantStore().fetchAgentsAndGroups();
        } catch {
          useNotificationStore().addNotification({
            type: 'warning', title: '配置已导入',
            message: '群聊本机设置已保存，列表暂未刷新；请重新打开群聊。', toastOnly: true,
          });
        }
      }
    });
  } finally {
    actionBusy.value = false;
  }
};

defineExpose({
  beforeLeave: () => actionBusy.value ? Promise.resolve(false) : session.prepareLeave(),
});
</script>

<template>
  <div class="flex flex-col gap-6">
    <p v-if="error" role="alert" class="text-sm text-red-500 break-words">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading" @click="reload">重试</button>
    </p>
    <template v-if="draft">
      <SettingsDomainIssues :domains="session.domainIssues.value" :errors="session.fieldErrors.value" :fields="CONNECTION_FIELDS" />
      <SettingsSection title="核心连接">
        <VcpCoreSettingsSection :settings="draft" :active="active" :disabled="disabled" :commit="commitForAction" @edit="edit" />
      </SettingsSection>
      <SettingsSection title="数据同步">
        <SyncSettingsSection :settings="draft" :active="active" :disabled="disabled" :commit="commitForAction" @edit="edit" />
      </SettingsSection>
      <SettingsSection title="配置备份">
        <ConfigBackupSection :active="active" :disabled="disabled" :commit="commitForAction" :apply-import="applyImport" />
      </SettingsSection>
    </template>
    <p v-else-if="loading" class="text-sm opacity-60">正在加载设置...</p>
    <p v-if="leaving" role="status" class="text-xs opacity-60">正在保存设置...</p>
  </div>
</template>
