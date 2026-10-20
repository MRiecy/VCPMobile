<script setup lang="ts">
import { computed, defineAsyncComponent, ref, watch } from 'vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import TopicSummarySection from '../components/TopicSummarySection.vue';
import MaintenanceSection from '../components/MaintenanceSection.vue';
import { SUMMARY_FIELDS } from '../editing/fieldSets';
import { useSettingsPageSession } from '../editing/useSettingsPageSession';

const ModelSelector = defineAsyncComponent(() => import('../../../components/ModelSelector.vue'));
const props = defineProps<{ active: boolean }>();
const { session, loading, error, reload } = useSettingsPageSession(SUMMARY_FIELDS, () => props.active);
const { draft, leaving } = session;
const showSelector = ref(false);
const selectorMounted = ref(false);
const disabled = computed(() => !props.active || leaving.value);

const openSelector = () => {
  if (disabled.value) return;
  selectorMounted.value = true;
  showSelector.value = true;
};
const setModel = (value: string) => {
  if (disabled.value) return;
  session.edit('conversation.topicSummaryModel', value);
  void session.save();
};
const save = () => { if (!disabled.value) void session.save(); };
watch(() => props.active, (active) => { if (!active) showSelector.value = false; }, { flush: 'sync' });
defineExpose({ beforeLeave: () => session.prepareLeave() });
</script>

<template>
  <div class="flex flex-col gap-6">
    <p v-if="error" role="alert" class="text-sm text-red-500 break-words">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading" @click="reload">重试</button>
    </p>
    <SettingsSection v-if="draft" title="话题总结">
      <TopicSummarySection :model-value="draft['conversation.topicSummaryModel']" :disabled="disabled"
        @update:model-value="setModel" @save="save" @open-model-selector="openSelector" />
    </SettingsSection>
    <p v-else-if="loading" class="text-sm opacity-60">正在加载设置...</p>
    <SettingsSection title="数据维护">
      <MaintenanceSection :active="active" :disabled="disabled" />
    </SettingsSection>
    <p v-if="leaving" role="status" class="text-xs opacity-60">正在保存设置...</p>
    <ModelSelector v-if="selectorMounted && active" v-model="showSelector"
      :current-model="draft?.['conversation.topicSummaryModel']" title="选择总结专用模型" @select="setModel" />
  </div>
</template>
