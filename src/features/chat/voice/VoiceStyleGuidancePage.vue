<script setup lang="ts">
import { computed } from 'vue';
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
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
        适用于预置音色与样本复刻的通话回复。两项规则换行拼接为同一个注入块，固定放在上下文末尾。
      </SettingsNote>
      <SettingsSection title="LLM 音频标签指导" description="通话时自动注入，让聊天模型在正文中生成音频标签。">
        <VoiceTextArea label="音频标签生成规则" :model-value="draft['voice.guidance.audioTagRules']" :max-length="settingDescriptor('voice.guidance.audioTagRules').maxChars" :disabled="disabled" :rows="7"
          @update:model-value="session.edit('voice.guidance.audioTagRules', $event)" />
        <button type="button" class="voice-rule-reset" :disabled="disabled"
          @click="session.edit('voice.guidance.audioTagRules', SETTINGS_DEFAULTS.voice.guidance.audioTagRules)">恢复默认标签规则</button>
      </SettingsSection>
      <SettingsSection title="LLM 自然语言指导">
        <SettingsRow title="模型自动生成指导" description="每条助手回复生成一次，作用于该回复的全部朗读分段。">
          <template #action>
            <SettingsSwitch :model-value="draft['voice.guidance.autoGuidance']" :disabled="disabled" aria-label="模型自动生成指导"
              @update:model-value="session.edit('voice.guidance.autoGuidance', $event)" />
          </template>
        </SettingsRow>
        <template v-if="draft['voice.guidance.autoGuidance']">
          <VoiceTextArea label="自然语言指导生成规则" :model-value="draft['voice.guidance.guidanceRules']" :max-length="settingDescriptor('voice.guidance.guidanceRules').maxChars" :disabled="disabled" :rows="7"
            description="只填写生成要求，指导块的格式由程序补充。"
            @update:model-value="session.edit('voice.guidance.guidanceRules', $event)" />
          <button type="button" class="voice-rule-reset" :disabled="disabled"
            @click="session.edit('voice.guidance.guidanceRules', SETTINGS_DEFAULTS.voice.guidance.guidanceRules)">恢复默认指导规则</button>
        </template>
        <template #footer>自动指导关闭或缺失时，使用语音设置中的默认演绎指导；留空则按音色自然朗读。</template>
      </SettingsSection>
      <SettingsNote>返回时自动保存，下一次回复生效。</SettingsNote>
    </template>
  </SettingsCanvas>
</template>

<style scoped>
.voice-rule-reset { align-self: flex-start; min-height: 44px; padding: 8px 0; border: 0; background: transparent; color: var(--secondary-text); font-size: 12px; text-decoration: underline; touch-action: manipulation; }
.voice-rule-reset:disabled { opacity: 0.5; }
</style>
