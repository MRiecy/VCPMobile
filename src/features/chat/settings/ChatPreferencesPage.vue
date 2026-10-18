<script setup lang="ts">
import { reactive } from 'vue';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSwitch from '../../../components/settings/SettingsSwitch.vue';
import { useNotificationStore } from '../../../core/stores/notification';
import { CHAT_PRESENTATION_OPTIONS, type ChatPresentationMode } from '../preferences/model';
import { useChatPreferencesStore } from '../preferences/store';

const props = withDefaults(defineProps<{ active?: boolean }>(), { active: true });
const preferences = useChatPreferencesStore();
const notifications = useNotificationStore();
const errors = reactive({ presentation: '', smoothStreaming: '', haptics: '', collapseTools: '' });

function reportResult(key: keyof typeof errors, title: string, result: { ok: boolean; error?: string }) {
  errors[key] = result.ok ? '' : result.error || '设置保存失败，请重试';
  if (!result.ok) {
    notifications.addNotification({
      id: `chat-preference-${key}-save-failed`, type: 'error', title,
      message: errors[key], toastOnly: true,
    });
  }
}

function selectPresentationMode(mode: ChatPresentationMode) {
  if (!props.active) return;
  reportResult('presentation', '消息呈现切换失败', preferences.setPresentationMode(mode));
}
function updateSmoothStreaming(enabled: boolean) {
  if (!props.active) return;
  reportResult('smoothStreaming', '流式平滑动效设置失败', preferences.setSmoothStreamingEnabled(enabled));
}
function updateHaptics(enabled: boolean) {
  if (!props.active) return;
  reportResult('haptics', '消息震动设置失败', preferences.setMessageHapticsEnabled(enabled));
}
function updateCollapseTools(enabled: boolean) {
  if (!props.active) return;
  reportResult('collapseTools', '流式工具折叠设置失败', preferences.setCollapseToolsWhileStreaming(enabled));
}
</script>

<template>
  <div class="space-y-6" data-testid="chat-preferences-page">
    <section aria-labelledby="presentation-title" data-testid="presentation-settings">
      <div class="preference-heading">
        <h3 id="presentation-title">消息呈现与内容宽度</h3>
        <p>点选后立即生效；聊天页长按深浅按钮也可快速切换。</p>
      </div>
      <SettingsCard>
        <div role="radiogroup" aria-labelledby="presentation-title">
          <button v-for="option in CHAT_PRESENTATION_OPTIONS" :key="option.value" type="button"
            class="presentation-option" :class="{ 'is-selected': preferences.presentationMode === option.value }"
            role="radio" :aria-checked="preferences.presentationMode === option.value" :disabled="!active"
            :data-presentation-value="option.value" @click="selectPresentationMode(option.value)">
            <span class="presentation-accent" aria-hidden="true"></span>
            <span class="presentation-copy"><strong>{{ option.title }}</strong><small>{{ option.description }}</small></span>
            <span class="shrink-0 font-mono text-xs opacity-60">{{ option.label }}</span>
            <svg v-if="preferences.presentationMode === option.value" class="shrink-0 text-[var(--highlight-text)]" width="16" height="16"
              viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="20 6 9 17 4 12" /></svg>
          </button>
        </div>
      </SettingsCard>
      <p v-if="errors.presentation" class="preference-error" role="alert">{{ errors.presentation }}</p>
    </section>

    <section aria-labelledby="streaming-title">
      <div class="preference-heading"><h3 id="streaming-title">生成与流式</h3></div>
      <SettingsCard>
        <div class="divide-y divide-[var(--vcp-border-subtle)]">
          <SettingsRow title="流式平滑动效" description="让实时生成的文字柔和淡入，减少跳字感；系统减弱动态效果时自动停用。">
            <template #action>
              <SettingsSwitch data-testid="smooth-streaming-switch" aria-label="流式平滑动效" :disabled="!active"
                :model-value="preferences.smoothStreamingEnabled" @update:model-value="updateSmoothStreaming" />
            </template>
          </SettingsRow>
          <SettingsRow title="流式中折叠工具" description="生成过程中自动收起工具详情；用户手动展开的内容保持展开。">
            <template #action>
              <SettingsSwitch data-testid="collapse-tools-switch" aria-label="流式中折叠工具" :disabled="!active"
                :model-value="preferences.collapseToolsWhileStreaming" @update:model-value="updateCollapseTools" />
            </template>
          </SettingsRow>
        </div>
      </SettingsCard>
      <p v-if="errors.smoothStreaming" class="preference-error" role="alert">{{ errors.smoothStreaming }}</p>
      <p v-if="errors.collapseTools" class="preference-error" role="alert">{{ errors.collapseTools }}</p>
    </section>

    <section aria-labelledby="feedback-title">
      <div class="preference-heading"><h3 id="feedback-title">触感与反馈</h3></div>
      <SettingsCard>
        <SettingsRow title="消息震动" description="发送消息与回复结束时，使用设备支持的轻微震动反馈。">
          <template #action>
            <SettingsSwitch data-testid="message-haptics-switch" aria-label="消息震动" :disabled="!active"
              :model-value="preferences.messageHapticsEnabled" @update:model-value="updateHaptics" />
          </template>
        </SettingsRow>
      </SettingsCard>
      <p v-if="errors.haptics" class="preference-error" role="alert">{{ errors.haptics }}</p>
    </section>
  </div>
</template>

<style scoped>
.preference-heading { margin: 0 0 0.75rem; padding: 0 0.25rem; }
.preference-heading h3 { margin: 0; font-size: 0.8125rem; font-weight: 800; }
.preference-heading p { margin: 0.25rem 0 0; color: var(--secondary-text); font-size: 0.6875rem; line-height: 1.45; }
.presentation-option { position: relative; display: flex; align-items: center; gap: 0.75rem; width: 100%; min-height: 4rem; padding: 0.7rem 0.75rem; color: var(--primary-text); background: transparent; border: 0; text-align: left; }
.presentation-option + .presentation-option { border-top: 1px solid var(--vcp-border-subtle); }
.presentation-option.is-selected { background-color: var(--vcp-highlight-bg-10); }
.presentation-accent { position: absolute; top: 0.5rem; bottom: 0.5rem; left: 0; width: 2px; }
.is-selected .presentation-accent { background-color: var(--highlight-text); }
.presentation-copy { display: flex; flex: 1 1 auto; flex-direction: column; min-width: 0; }
.presentation-copy strong { font-size: 0.8125rem; }
.presentation-copy small { margin-top: 0.18rem; color: var(--secondary-text); font-size: 0.6875rem; line-height: 1.4; }
.preference-error { margin: 0.625rem 0 0; color: var(--vcp-error-text, #ef4444); font-size: 0.75rem; }
</style>
