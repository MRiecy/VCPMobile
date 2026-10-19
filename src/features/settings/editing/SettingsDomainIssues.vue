<script setup lang="ts">
import { computed } from 'vue';
import type { ConfigDomain, DomainStatus, FieldIssue } from '../../../core/config/generated/config';
import { useOverlayStore } from '../../../core/stores/overlay';
import type { SettingsEditableKey } from './fieldSets';
import { CONFIG_DOMAIN_LABELS, configIssueLabel, configResetFields, configResetLabel, configResetDescription } from './configLabels';

const props = defineProps<{
  domains: (DomainStatus & { domain: ConfigDomain })[];
  fields: readonly SettingsEditableKey[];
  errors: FieldIssue[];
  disabled?: boolean;
  repair: (domain: ConfigDomain, confirmedRevision?: string) => Promise<boolean>;
}>();
const emit = defineEmits<{ reset: [field: SettingsEditableKey] }>();
const overlay = useOverlayStore();
const needsRepair = (issue: FieldIssue) => issue.code === 'invalidGroup' || issue.code === 'requiresRepair';
const affectsPage = (issue: FieldIssue) => props.fields.some(path => path === fieldPath(issue)
  || path.startsWith(`${issue.domain}.${issue.field ? `${issue.field}.` : ''}`));
const relevant = computed(() => {
  const statuses = props.domains.map(status => ({ ...status, issues: [...status.issues] }));
  for (const issue of props.errors.filter(needsRepair)) {
    const status = statuses.find(item => item.domain === issue.domain);
    if (status) status.issues.push(issue);
    else statuses.push({ domain: issue.domain, revision: '', schemaVersion: 0, health: 'recovered', issues: [issue] });
  }
  return statuses.filter(status => status.health === 'unavailable' || status.issues.some(affectsPage));
});
const fieldPath = (issue: FieldIssue) => `${issue.domain}.${issue.field}` as SettingsEditableKey;
const firstResetInGroup = (issues: FieldIssue[], index: number) => {
  const fields = configResetFields(fieldPath(issues[index]));
  return !issues.slice(0, index).some(issue => fields.includes(fieldPath(issue)));
};
const reset = async (issue: FieldIssue) => {
  if (props.disabled) return;
  const path = fieldPath(issue);
  if (configResetFields(path).length > 1) {
    const accepted = await overlay.showConfirm({
      title: configResetLabel(path),
      message: `将把${configResetDescription(path)}一并恢复默认值，包括相关凭据；其他设置不受影响。`,
    });
    if (!accepted || props.disabled) return;
  }
  emit('reset', path);
};
const repair = async (status: DomainStatus & { domain: ConfigDomain }) => {
  if (props.disabled) return;
  const domainName = CONFIG_DOMAIN_LABELS[status.domain];
  const scope = status.health === 'unavailable'
    ? `将以默认值重建${domainName}配置`
    : `将保留${domainName}中有效的配置，仅将损坏字段或关联配置组恢复默认值`;
  const accepted = await overlay.showConfirm({ title: `修复${domainName}配置`, message: `${scope}，并保留本页已修改的字段。原始数据会归档，其他领域不受影响。` });
  if (accepted && !props.disabled) await props.repair(status.domain, status.revision || undefined);
};
</script>

<template>
  <div v-if="relevant.length || errors.length" class="flex flex-col gap-2 text-sm" role="status">
    <template v-for="status in relevant" :key="status.domain">
      <p class="m-0 text-[var(--secondary-text)]">{{ CONFIG_DOMAIN_LABELS[status.domain] }}配置需要检查，原始值仍保留。</p>
      <template v-for="(issue, index) in status.issues" :key="`${issue.field}:${issue.code}:${index}`">
        <div v-if="status.health === 'unavailable' || affectsPage(issue)" class="flex items-center justify-between gap-3">
          <span>{{ configIssueLabel(issue) }}：{{ issue.message }}</span>
          <button v-if="status.health !== 'unavailable' && !needsRepair(issue) && fields.includes(fieldPath(issue)) && firstResetInGroup(status.issues, index)" type="button"
            class="min-h-11 shrink-0 underline" :disabled="disabled" @click="reset(issue)">{{ configResetLabel(fieldPath(issue)) }}</button>
        </div>
      </template>
      <button v-if="status.health === 'unavailable' || status.issues.some(needsRepair)" type="button" class="min-h-11 self-start underline"
        :disabled="disabled" @click="repair(status)">修复{{ CONFIG_DOMAIN_LABELS[status.domain] }}配置</button>
    </template>
    <p v-for="issue in errors" :key="`${issue.domain}:${issue.field}:${issue.code}`" class="m-0 text-red-500" role="alert">
      {{ configIssueLabel(issue) }}：{{ issue.message }}
    </p>
  </div>
</template>
