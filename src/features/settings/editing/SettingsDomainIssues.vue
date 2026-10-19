<script setup lang="ts">
import { computed } from 'vue';
import type { ConfigDomain, DomainStatus, FieldIssue } from '../../../core/config/generated/config';
import type { SettingsEditableKey } from './fieldSets';
import { CONFIG_DOMAIN_LABELS, configIssueLabel } from './configLabels';

const props = defineProps<{
  domains: (DomainStatus & { domain: ConfigDomain })[];
  fields?: readonly SettingsEditableKey[];
  errors: FieldIssue[];
  disabled?: boolean;
  reset?: (domain: ConfigDomain, expectedRevision: string) => void;
}>();
const affectsPage = (issue: FieldIssue) => !props.fields || props.fields.some(path => path === fieldPath(issue)
  || path.startsWith(`${issue.domain}.${issue.field ? `${issue.field}.` : ''}`));
const relevant = computed(() => props.domains.filter(status =>
  status.health === 'unavailable' || status.issues.some(affectsPage)));
const fieldPath = (issue: FieldIssue) => `${issue.domain}.${issue.field}` as SettingsEditableKey;
</script>

<template>
  <div v-if="relevant.length || errors.length" class="flex flex-col gap-2 text-sm" role="status">
    <template v-for="status in relevant" :key="status.domain">
      <p class="m-0 text-[var(--secondary-text)]">{{ CONFIG_DOMAIN_LABELS[status.domain] }}配置需要检查，可在设置首页将整组配置恢复默认值。</p>
      <template v-for="(issue, index) in status.issues" :key="`${issue.field}:${issue.code}:${index}`">
        <div v-if="status.health === 'unavailable' || affectsPage(issue)" class="flex items-center justify-between gap-3">
          <span>{{ configIssueLabel(issue) }}：{{ issue.message }}</span>
        </div>
      </template>
      <button v-if="reset" type="button" class="min-h-11 self-start underline"
        :disabled="disabled" @click="!disabled && reset(status.domain, status.revision)">将{{ CONFIG_DOMAIN_LABELS[status.domain] }}整组恢复默认值</button>
    </template>
    <p v-for="issue in errors" :key="`${issue.domain}:${issue.field}:${issue.code}`" class="m-0 text-red-500" role="alert">
      {{ configIssueLabel(issue) }}：{{ issue.message }}
    </p>
  </div>
</template>
