<script setup lang="ts">
import { computed } from 'vue';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsNote from '../../../components/settings/SettingsNote.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { SETTINGS_DEFAULTS, settingDescriptor } from './settings';
import { VOICE_STYLE_GUIDANCE_FIELDS } from '../../settings/editing/fieldSets';
import { useSettingsPageSession } from '../../settings/editing/useSettingsPageSession';
import type { SettingsPageProps } from '../../settings/contracts';

const props = defineProps<SettingsPageProps>();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_STYLE_GUIDANCE_FIELDS, () => props.active);
const { draft, saving, leaving } = session;
const disabled = computed(() => !props.active || leaving.value);
const beforeLeave = () => session.prepareLeave();
defineExpose({ beforeLeave });
</script>

<template>
  <SettingsCanvas viewport class="voice-style-guidance-page" :inert="!active" aria-label="风格指导">
    <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
      {{ error }}
      <button v-if="!draft" type="button" :disabled="loading || !active" class="ml-2 underline" @click="reload">重试</button>
    </p>
    <p v-else-if="!draft && loading" role="status" class="m-0 text-sm text-[var(--secondary-text)]">正在加载风格指导…</p>
    <p v-if="saving || leaving" role="status" class="m-0 text-xs text-[var(--secondary-text)]">正在保存风格指导…</p>

    <template v-if="draft">
      <SettingsNote>
        通话时将本项规则原样发送给聊天模型。
      </SettingsNote>
      <SettingsSection>
        <VoiceTextArea label="风格指导规则" :model-value="draft['voice.guidance.rules']" :max-length="settingDescriptor('voice.guidance.rules').maxChars" :disabled="disabled" :rows="12"
          @update:model-value="session.edit('voice.guidance.rules', $event)" />
        <button type="button" class="voice-rule-reset" :disabled="disabled"
          @click="session.edit('voice.guidance.rules', SETTINGS_DEFAULTS.voice.guidance.rules)">恢复默认规则</button>
      </SettingsSection>
      <SettingsNote>返回时自动保存，下一次回复生效。</SettingsNote>
    </template>
  </SettingsCanvas>
</template>

<style scoped>
.voice-rule-reset { align-self: flex-start; min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.voice-rule-reset:disabled { opacity: 0.5; }
</style>
