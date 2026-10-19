<script setup lang="ts">
import { computed, ref } from 'vue';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
import VoiceTextArea from './VoiceTextArea.vue';
import { DEFAULT_VOICE_SETTINGS } from './settings';
import { VOICE_PRESET_FIELDS } from '../../settings/editing/fieldSets';
import { useSettingsPageSession } from '../../settings/editing/useSettingsPageSession';
import type { SettingsPageProps } from '../../settings/contracts';

const props = defineProps<SettingsPageProps>();
const { session, loading, error, reload } = useSettingsPageSession(VOICE_PRESET_FIELDS, () => props.active);
const { draft, saving, leaving } = session;
const disabled = computed(() => !props.active || leaving.value);
const validationError = ref('');
const beforeLeave = () => {
  validationError.value = '';
  if (draft.value && (!draft.value.voiceAudioTagRules.trim()
    || (draft.value.voiceAutoGuidance && !draft.value.voiceGuidanceRules.trim()))) {
    validationError.value = '请填写音频标签规则，以及启用时的自然语言指导规则。';
    return Promise.resolve(false);
  }
  return session.prepareLeave();
};
defineExpose({ beforeLeave });
</script>

<template>
  <SettingsCanvas viewport class="voice-presets-page" :inert="!active" aria-label="语音预设">
    <p v-if="error" role="alert" class="m-0 text-sm text-red-500">
      {{ draft ? '语音预设保存失败，修改已保留，请再次返回重试。' : '语音预设加载失败，请重试。' }}
      <button v-if="!draft" type="button" :disabled="loading || !active" class="ml-2 underline" @click="reload">重试</button>
    </p>
    <p v-else-if="!draft && loading" role="status" class="m-0 text-sm text-[var(--secondary-text)]">正在加载语音预设…</p>
    <p v-if="saving || leaving" role="status" class="m-0 text-xs text-[var(--secondary-text)]">正在保存语音预设…</p>
    <p v-if="validationError" role="alert" class="m-0 text-sm text-red-500">{{ validationError }}</p>
    <template v-if="draft">
      <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">
        仅用于文字设计模式的通话回复。两项规则换行拼接为同一个注入块，固定放在上下文末尾。
      </p>
      <SettingsSection title="LLM 音频标签指导" description="通话时自动注入，让聊天模型在正文中生成音频标签。">
        <VoiceTextArea label="音频标签生成规则" :model-value="draft.voiceAudioTagRules" :disabled="disabled" :rows="7"
          @update:model-value="session.edit('voiceAudioTagRules', $event)" />
        <button type="button" class="voice-rule-reset" :disabled="disabled"
          @click="session.edit('voiceAudioTagRules', DEFAULT_VOICE_SETTINGS.voiceAudioTagRules)">恢复默认标签规则</button>
      </SettingsSection>
      <SettingsSection title="LLM 自然语言指导">
        <SettingsRow title="模型自动生成指导" description="每条助手回复生成一次，作用于该回复的全部朗读分段。">
          <template #action>
            <SettingsSwitch :model-value="draft.voiceAutoGuidance" :disabled="disabled" aria-label="模型自动生成指导"
              @update:model-value="session.edit('voiceAutoGuidance', $event)" />
          </template>
        </SettingsRow>
        <template v-if="draft.voiceAutoGuidance">
          <VoiceTextArea label="自然语言指导生成规则" :model-value="draft.voiceGuidanceRules" :disabled="disabled" :rows="7"
            description="只填写生成要求，指导块的格式由程序补充。"
            @update:model-value="session.edit('voiceGuidanceRules', $event)" />
          <button type="button" class="voice-rule-reset" :disabled="disabled"
            @click="session.edit('voiceGuidanceRules', DEFAULT_VOICE_SETTINGS.voiceGuidanceRules)">恢复默认指导规则</button>
        </template>
        <template #footer>自动指导关闭或缺失时，使用语音设置中的音色描述。</template>
      </SettingsSection>
      <p class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">返回时自动保存，下一次回复生效。</p>
    </template>
  </SettingsCanvas>
</template>

<style scoped>
.voice-rule-reset { align-self: flex-start; min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.voice-rule-reset:disabled { opacity: 0.5; }
</style>
