<script setup lang="ts">
import { computed, onBeforeUnmount, shallowRef, watch } from 'vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import type { ContextMenuConfig, OverlayActionItem, PromptConfig } from '../../../core/types/overlay';
import type { JevProvider } from '../../../core/config/generated/config';
import { SETTINGS_FIELDS } from '../../../core/config/generated/metadata';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsDisclosure from '../../../components/settings/SettingsDisclosure.vue';
import SettingsTextField from '../../../components/settings/SettingsTextField.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
import type { SettingsPageProps } from '../contracts';
import { JEV_FIELDS } from '../editing/fieldSets';
import { useSettingsPageSession } from '../editing/useSettingsPageSession';

const props = defineProps<SettingsPageProps>();
const overlay = useOverlayStore();
const { session, loading, error, reload } = useSettingsPageSession(JEV_FIELDS, () => props.active);
const { draft, saving, leaving } = session;
const disabled = computed(() => !props.active || leaving.value);
const providers: { value: JevProvider; label: string }[] = [
  { value: 'typesafe', label: 'TypeSafe 官方' }, { value: 'openrouter', label: 'OpenRouter' },
];
const provider = computed(() => draft.value?.['jev.provider'] ?? 'typesafe');
const providerLabel = computed(() => providers.find(item => item.value === provider.value)?.label ?? '');
const connectionFields = computed(() => ({
  url: `jev.${provider.value}.apiUrl` as const,
  key: `jev.${provider.value}.apiKey` as const,
  model: `jev.${provider.value}.model` as const,
}));
const preset = (path: string) => SETTINGS_FIELDS.find(field => `jev.${field.path}` === path && field.domain === 'jev')?.preset;
const numbers = [
  { field: 'jev.timeoutMs', title: '响应超时', unit: 'ms', description: '每次决策最多等待多久。' },
  { field: 'jev.maxRetries', title: '最大重试次数', unit: '次', description: '服务繁忙或网络异常时重试。' },
  { field: 'jev.retryBaseDelayMs', title: '重试等待时间', unit: 'ms', description: '指数退避首次重试前的等待时间。' },
] as const;
type NumberOption = typeof numbers[number];
const menu = shallowRef<ContextMenuConfig | null>(null);
let prompt: PromptConfig | null = null;
const closeOverlays = () => {
  if (menu.value && overlay.contextMenuConfig === menu.value) overlay.closeContextMenu();
  if (prompt && overlay.promptConfig === prompt) overlay.closePrompt();
  menu.value = null;
  prompt = null;
};
const selectProvider = () => {
  if (disabled.value || !draft.value) return;
  const actions: OverlayActionItem[] = providers.map(item => ({
    label: item.label, selected: item.value === provider.value,
    handler: () => {
      if (!disabled.value && overlay.contextMenuConfig?.actions === actions) session.edit('jev.provider', item.value);
    },
  }));
  overlay.openContextMenu(actions, '服务提供商');
  menu.value = overlay.contextMenuConfig;
};
const editNumber = (item: NumberOption) => {
  if (disabled.value || !draft.value) return;
  const descriptor = SETTINGS_FIELDS.find(field => `jev.${field.path}` === item.field && field.domain === 'jev')!;
  const min = descriptor.min!;
  const max = descriptor.max!;
  const validate = (raw: string) => /^\d+$/.test(raw.trim()) && Number.isSafeInteger(Number(raw))
    && Number(raw) >= min && Number(raw) <= max ? null : `请输入 ${min}–${max} 之间的整数`;
  const config: PromptConfig = {
    title: `${item.title}（${item.unit}）`, initialValue: String(draft.value[item.field]),
    placeholder: `${min}–${max}`, inputMode: 'numeric', validate,
    onConfirm: value => {
      if (disabled.value || overlay.promptConfig !== prompt || prompt?.onConfirm !== config.onConfirm || validate(value)) return;
      session.edit(item.field, Number(value.trim()));
    },
  };
  overlay.openPrompt(config);
  prompt = overlay.promptConfig;
};
watch(disabled, value => { if (value) closeOverlays(); }, { flush: 'sync' });
onBeforeUnmount(closeOverlays);
defineExpose({ beforeLeave: () => session.prepareLeave() });
</script>

<template>
  <SettingsCanvas viewport :inert="!active">
    <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading || !active" @click="reload">重试</button>
    </p>
    <p v-else-if="!draft && loading" role="status" class="m-0 text-sm opacity-60">正在加载设置…</p>
    <p v-if="saving || leaving" role="status" class="m-0 text-xs text-[var(--secondary-text)]">正在保存 JEV 设置…</p>

    <div v-if="draft" class="flex flex-col gap-5">
      <SettingsSection title="服务连接">
        <div>
          <SettingsRow title="启用 JEV" description="全局 JEV 开关">
            <template #action>
              <SettingsSwitch :model-value="draft?.['jev.enabled'] ?? false" :disabled="disabled" aria-label="启用 JEV"
                @update:model-value="session.edit('jev.enabled', $event)" />
            </template>
          </SettingsRow>
          <SettingsRow title="服务提供商" :value="providerLabel" clickable :disabled="disabled" aria-haspopup="dialog"
            aria-label="服务提供商" :aria-expanded="overlay.contextMenuConfig !== null && overlay.contextMenuConfig === menu" @click="selectProvider" />
        </div>
        <div class="jev-fields">
          <SettingsTextField :model-value="draft?.[connectionFields.url]" label="API 地址" mono :disabled="disabled"
            :placeholder="preset(connectionFields.url)" @update:model-value="session.edit(connectionFields.url, $event)" />
          <SettingsTextField :model-value="draft?.[connectionFields.key]" label="API Key（支持多个）" is-secure :disabled="disabled"
            placeholder="多个 Key 用逗号或 | 分隔" @update:model-value="session.edit(connectionFields.key, $event)" />
          <SettingsTextField :model-value="draft?.[connectionFields.model]" label="决策模型" mono :disabled="disabled"
            :placeholder="preset(connectionFields.model)" @update:model-value="session.edit(connectionFields.model, $event)" />
        </div>
        <template #footer>地址和模型留空使用当前服务商预设。</template>
      </SettingsSection>

      <SettingsDisclosure title="高级参数" description="超时、重试与代理" :disabled="disabled">
        <div>
          <SettingsRow v-for="item in numbers" :key="item.field" :title="item.title" :description="item.description"
            :value="draft ? `${draft[item.field]} ${item.unit}` : ''" clickable :disabled="disabled" aria-haspopup="dialog" @click="editNumber(item)" />
        </div>
        <SettingsTextField :model-value="draft?.['jev.proxyUrl']" label="代理地址（可选）" mono :disabled="disabled"
          placeholder="http://127.0.0.1:7890" @update:model-value="session.edit('jev.proxyUrl', $event)" />
      </SettingsDisclosure>

      <SettingsDisclosure v-if="provider === 'openrouter'" title="应用信息" description="可选，用于 OpenRouter 识别应用" :disabled="disabled">
        <div class="jev-fields">
          <SettingsTextField :model-value="draft?.['jev.httpReferer']" label="应用网址（HTTP Referer）" mono :disabled="disabled"
            placeholder="https://your-site.example" @update:model-value="session.edit('jev.httpReferer', $event)" />
          <SettingsTextField :model-value="draft?.['jev.appTitle']" label="应用名称" :disabled="disabled"
            :placeholder="preset('jev.appTitle')" @update:model-value="session.edit('jev.appTitle', $event)" />
        </div>
      </SettingsDisclosure>
    </div>
  </SettingsCanvas>
</template>

<style scoped>
.jev-fields { display: grid; grid-template-columns: minmax(0, 1fr); gap: 14px; }
@media (min-width: 768px) { .jev-fields { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
