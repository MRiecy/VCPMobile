<script setup lang="ts">
import { computed, ref } from 'vue';
import { CircleHelp } from '@lucide/vue';
import { useOverlayStore } from '../../../core/stores/overlay';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsNote from '../../../components/settings/SettingsNote.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { SETTINGS_DEFAULTS, settingDescriptor } from './settings';
import { VOICE_STYLE_GUIDANCE_FIELDS } from '../../settings/editing/fieldSets';
import { useSettingsPageSession } from '../../settings/editing/useSettingsPageSession';
import type { SettingsPageProps } from '../../settings/contracts';

const props = defineProps<SettingsPageProps>();
const overlayStore = useOverlayStore();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_STYLE_GUIDANCE_FIELDS, () => props.active);
const { draft, leaving } = session;
const openingDetails = ref(false);
const disabled = computed(() => !props.active || leaving.value || openingDetails.value);
const openRuleDetails = async () => {
  if (disabled.value || !draft.value) return;
  openingDetails.value = true;
  try {
    // A covering page disposes this edit session. Save before pushing it.
    if ((await session.save()).ok && props.active) overlayStore.openVoiceRuleDetails();
  } finally {
    openingDetails.value = false;
  }
};
const beforeLeave = () => openingDetails.value ? false : session.prepareLeave();
defineExpose({ beforeLeave });
</script>

<template>
  <SettingsCanvas viewport class="voice-style-guidance-page" :inert="disabled" aria-label="风格指导">
    <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
      {{ error }}
      <button v-if="!draft" type="button" :disabled="loading || !active" class="ml-2 underline" @click="reload">重试</button>
    </p>
    <p v-else-if="!draft && loading" role="status" class="m-0 text-sm text-[var(--secondary-text)]">正在加载风格指导…</p>

    <template v-if="draft">
      <div class="flex items-center justify-between gap-3">
        <h3 class="m-0 text-sm font-medium">风格指导规则</h3>
        <button type="button" class="voice-rule-help" aria-label="规则细节"
          @click="openRuleDetails">
          <CircleHelp :size="16" :stroke-width="1.8" aria-hidden="true" />
          <span>规则细节</span>
        </button>
      </div>
      <SettingsNote>
        通话时将规则独立注入到最后一个user消息之后。
      </SettingsNote>
      <SettingsSection>
        <VoiceTextArea label="风格指导规则" hide-label :model-value="draft['voice.guidance.rules']" :max-length="settingDescriptor('voice.guidance.rules').maxChars" :rows="16"
          @update:model-value="session.edit('voice.guidance.rules', $event)" />
        <button type="button" class="voice-rule-reset"
          @click="session.edit('voice.guidance.rules', SETTINGS_DEFAULTS.voice.guidance.rules)">恢复默认规则</button>
      </SettingsSection>
    </template>
  </SettingsCanvas>
</template>

<style scoped>
.voice-rule-reset { align-self: flex-start; min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.voice-rule-help { display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0; gap: 6px; min-height: 44px; padding: 8px 4px; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; touch-action: manipulation; }
.voice-rule-help:active { opacity: 0.6; }
.voice-rule-help:focus-visible { outline: 2px solid var(--highlight-text, var(--primary-text)); outline-offset: 2px; }
</style>
