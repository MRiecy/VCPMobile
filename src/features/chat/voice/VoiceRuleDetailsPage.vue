<script setup lang="ts">
import SettingsCanvas from '../../../components/settings/SettingsCanvas.vue';
import { renderSafeMarkdown } from '../../../core/utils/safeMarkdown';
import type { SettingsPageProps } from '../../settings/contracts';
import rulesMarkdown from './mimo-style-rules.md?raw';

defineProps<SettingsPageProps>();
const rulesHtml = renderSafeMarkdown(rulesMarkdown);
</script>

<template>
  <SettingsCanvas viewport :inert="!active">
    <!-- eslint-disable-next-line vue/no-v-html -- Bundled Markdown passes through the shared safe renderer. -->
    <article class="voice-rule-details" aria-label="MiMo 语音风格控制说明" v-html="rulesHtml" />
  </SettingsCanvas>
</template>

<style scoped>
.voice-rule-details { min-width: 0; font-size: 14px; line-height: 1.75; overflow-wrap: anywhere; user-select: text; }
.voice-rule-details :deep(h1) { margin: 0 0 8px; font-size: 20px; line-height: 1.4; font-weight: 600; }
.voice-rule-details :deep(h2) { margin: 28px 0 12px; padding-top: 16px; border-top: 1px solid var(--border-color); font-size: 16px; line-height: 1.5; font-weight: 600; }
.voice-rule-details :deep(h3) { margin: 20px 0 8px; font-size: 14px; font-weight: 600; }
.voice-rule-details :deep(p) { margin: 8px 0; }
.voice-rule-details :deep(ul), .voice-rule-details :deep(ol) { margin: 8px 0; padding-left: 22px; }
.voice-rule-details :deep(ul) { list-style-type: disc; }
.voice-rule-details :deep(ol) { list-style-type: decimal; }
.voice-rule-details :deep(li + li) { margin-top: 6px; }
.voice-rule-details :deep(blockquote) { margin: 12px 0; padding-left: 12px; border-left: 2px solid var(--border-color); color: var(--secondary-text); }
.voice-rule-details :deep(code) { font-family: var(--font-mono, ui-monospace, monospace); font-size: 0.9em; }
.voice-rule-details :deep(pre) { margin: 12px 0; padding: 12px; border: 1px solid var(--border-color); border-radius: 8px; background: var(--input-bg, transparent); white-space: pre-wrap; overflow-wrap: anywhere; }
.voice-rule-details :deep(table) { width: 100%; table-layout: fixed; border-collapse: collapse; margin: 12px 0; font-size: 12px; }
.voice-rule-details :deep(th), .voice-rule-details :deep(td) { padding: 8px; border-bottom: 1px solid var(--border-color); text-align: left; vertical-align: top; }
.voice-rule-details :deep(th:first-child), .voice-rule-details :deep(td:first-child) { width: 28%; padding-left: 0; }
</style>
