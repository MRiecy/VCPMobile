<script setup lang="ts">
import { computed, defineAsyncComponent, ref, watch } from 'vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
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
  session.edit('topicSummaryModel', value);
  void session.save();
};
const save = () => { if (!disabled.value) void session.save(); };
watch(() => props.active, (active) => { if (!active) showSelector.value = false; }, { flush: 'sync' });
defineExpose({ beforeLeave: () => session.prepareLeave() });
</script>

<template>
  <div class="space-y-6">
    <p v-if="error" role="alert" class="text-sm text-red-500 break-words">
      {{ error }}
      <button v-if="!draft" type="button" class="ml-2 underline" :disabled="loading" @click="reload">重试</button>
    </p>
    <section v-if="draft">
      <h3 class="text-[11px] font-black tracking-[0.15em] opacity-50 mb-3 px-1">话题总结</h3>
      <SettingsCard>
        <TopicSummarySection :model-value="draft.topicSummaryModel" :disabled="disabled"
          @update:model-value="setModel" @save="save" @open-model-selector="openSelector" />
      </SettingsCard>
    </section>
    <p v-else-if="loading" class="text-sm opacity-60">正在加载设置...</p>
    <section>
      <h3 class="text-[11px] font-black tracking-[0.15em] opacity-50 mb-3 px-1">数据维护</h3>
      <SettingsCard><MaintenanceSection :active="active" :disabled="disabled" /></SettingsCard>
    </section>
    <p v-if="leaving" role="status" class="text-xs opacity-60">正在保存设置...</p>
    <ModelSelector v-if="selectorMounted && active" v-model="showSelector"
      :current-model="draft?.topicSummaryModel" title="选择总结专用模型" @select="setModel" />
  </div>
</template>
