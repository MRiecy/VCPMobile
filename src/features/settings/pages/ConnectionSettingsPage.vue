<script setup lang="ts">
import { computed, ref } from 'vue';
import { useSettingsStore, type AppSettings } from '../../../core/stores/settings';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import VcpCoreSettingsSection from '../components/VcpCoreSettingsSection.vue';
import SyncSettingsSection from '../components/SyncSettingsSection.vue';
import ConfigBackupSection from '../components/ConfigBackupSection.vue';
import { CONNECTION_FIELDS, type ConnectionFields } from '../editing/fieldSets';
import { useSettingsPageSession } from '../editing/useSettingsPageSession';
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
    return await applyConfigBackup(backup, async (patch: Partial<AppSettings>) => {
      await settingsStore.updateSettings(patch);
      if (props.active) {
        session.acceptImport(CONNECTION_FIELDS.filter((key) => Object.prototype.hasOwnProperty.call(patch, key)));
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
  <div class="space-y-6">
    <p v-if="error" role="alert" class="text-sm text-red-500 break-words">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading" @click="reload">重试</button>
    </p>
    <template v-if="draft">
      <section>
        <h3 class="text-[11px] font-black tracking-[0.15em] opacity-50 mb-3 px-1">核心连接</h3>
        <SettingsCard>
          <VcpCoreSettingsSection :settings="draft" :active="active" :disabled="disabled" :commit="commitForAction" @edit="edit" />
        </SettingsCard>
      </section>
      <section>
        <h3 class="text-[11px] font-black tracking-[0.15em] opacity-50 mb-3 px-1">数据同步</h3>
        <SettingsCard>
          <SyncSettingsSection :settings="draft" :active="active" :disabled="disabled" :commit="commitForAction" @edit="edit" />
        </SettingsCard>
      </section>
      <section>
        <h3 class="text-[11px] font-black tracking-[0.15em] opacity-50 mb-3 px-1">配置备份</h3>
        <SettingsCard>
          <ConfigBackupSection :active="active" :disabled="disabled" :commit="commitForAction" :apply-import="applyImport" />
        </SettingsCard>
      </section>
    </template>
    <p v-else-if="loading" class="text-sm opacity-60">正在加载设置...</p>
    <p v-if="leaving" role="status" class="text-xs opacity-60">正在保存设置...</p>
  </div>
</template>
