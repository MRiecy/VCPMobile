<script setup lang="ts">
import { computed } from 'vue';
import UserProfileSection from '../components/UserProfileSection.vue';
import SettingsDomainIssues from '../editing/SettingsDomainIssues.vue';
import { IDENTITY_FIELDS, type IdentityFields } from '../editing/fieldSets';
import { useSettingsPageSession } from '../editing/useSettingsPageSession';

const props = defineProps<{ active: boolean }>();
const { session, loading, error, reload } = useSettingsPageSession(IDENTITY_FIELDS, () => props.active);
const { draft, leaving } = session;
const disabled = computed(() => !props.active || leaving.value);
const edit = (patch: Partial<IdentityFields>) => {
  if (disabled.value) return;
  for (const key of IDENTITY_FIELDS) {
    const value = patch[key];
    if (value !== undefined) session.edit(key, value);
  }
};
defineExpose({ beforeLeave: () => session.prepareLeave() });
</script>

<template>
  <div class="flex flex-col gap-4">
    <SettingsDomainIssues :domains="session.domainIssues.value" :errors="session.fieldErrors.value" :fields="IDENTITY_FIELDS" />
    <p v-if="error" role="alert" class="text-sm text-red-500 break-words">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading" @click="reload">重试</button>
    </p>
    <UserProfileSection v-if="draft" :settings="draft" :active="active" :disabled="disabled" @edit="edit" />
    <p v-else-if="loading" class="text-sm opacity-60">正在加载设置...</p>
    <p v-if="leaving" role="status" class="text-xs opacity-60">正在保存设置...</p>
  </div>
</template>
