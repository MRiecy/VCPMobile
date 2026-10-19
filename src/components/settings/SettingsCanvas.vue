<script setup lang="ts">
import { provide } from 'vue';

/**
 * Opt-in settings layout. A list host owns scrolling when viewport is false.
 * A viewport host supplies bounded height; this shell owns scrolling, Insets,
 * and the optional actions slot. Drafts, saving and navigation stay in the page.
 */
defineProps<{ viewport?: boolean }>();
provide('vcp-settings-canvas', true);
</script>

<template>
  <div class="settings-canvas" :class="{ 'settings-canvas--viewport': viewport }">
    <div class="settings-canvas-scroll" :class="{ 'vcp-scrollable no-rubber-band': viewport, 'settings-canvas-scroll--standalone': !$slots.actions }">
      <div class="settings-canvas-content"><slot /></div>
    </div>
    <footer v-if="$slots.actions" class="settings-canvas-actions">
      <div class="settings-canvas-actions-content"><slot name="actions" /></div>
    </footer>
  </div>
</template>

<style scoped>
.settings-canvas { min-width: 0; color: var(--primary-text); }
.settings-canvas--viewport {
  display: flex;
  flex: 1;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  padding-bottom: var(--vcp-ime-offset, 0px);
}
.settings-canvas--viewport .settings-canvas-scroll { flex: 1; min-height: 0; overflow-y: auto; scroll-padding-block: 16px; }
.settings-canvas-content,
.settings-canvas-actions-content { width: 100%; max-width: 48rem; margin-inline: auto; }
.settings-canvas-content { display: flex; flex-direction: column; gap: 24px; }
.settings-canvas--viewport .settings-canvas-content { padding: 16px; }
.settings-canvas--viewport .settings-canvas-scroll--standalone .settings-canvas-content { padding-bottom: calc(16px + var(--vcp-safe-bottom, 0px)); }
.settings-canvas-actions { flex-shrink: 0; background: var(--primary-bg); border-top: 1px solid var(--vcp-border-subtle, var(--border-color)); }
.settings-canvas--viewport .settings-canvas-actions { padding-bottom: var(--vcp-safe-bottom, 0px); }
.settings-canvas-actions-content { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px 12px; padding: 8px 16px; }

.settings-canvas :deep(.settings-section) { gap: 8px; }
.settings-canvas :deep(.settings-section-header) { padding: 0; flex-wrap: wrap; }
.settings-canvas :deep(.settings-section-title) { color: var(--primary-text); font-size: 13px; line-height: 20px; font-weight: 600; }
.settings-canvas :deep(.settings-section-description),
.settings-canvas :deep(.settings-section-meta),
.settings-canvas :deep(.settings-section-footer) { color: var(--secondary-text); font-size: 12px; line-height: 18px; }
.settings-canvas :deep(.settings-section-meta) { overflow-wrap: anywhere; }
.settings-canvas :deep(.settings-section-footer) { padding: 0; }
.settings-canvas :deep(.settings-row) { width: 100%; min-height: 52px; padding: 4px 0; border: 0; border-radius: 0; background: transparent; text-align: left; transition: opacity 120ms ease; }
.settings-canvas :deep(.settings-row + .settings-row) { border-top: 1px solid var(--vcp-border-subtle, var(--border-color)); }
.settings-canvas :deep(.settings-row:active) { transform: none; }
.settings-canvas :deep(button.settings-row:active) { opacity: 0.65; }
.settings-canvas :deep(.settings-row-title) { font-size: 14px; line-height: 20px; font-weight: 500; white-space: normal; }
.settings-canvas :deep(.settings-row-description) { color: var(--secondary-text); font-size: 12px; line-height: 18px; opacity: 1; margin-top: 3px; }
.settings-canvas :deep(.settings-row-action) { min-width: 0; max-width: 55%; flex-shrink: 1; }
.settings-canvas :deep(.settings-row-value) { min-width: 0; color: var(--secondary-text); font-size: 14px; line-height: 20px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.settings-canvas :deep(.settings-field-label) { padding: 0; font-size: 12px; line-height: 18px; font-weight: 500; color: var(--secondary-text); opacity: 1; text-transform: none; letter-spacing: 0; }
.settings-canvas :deep(.settings-field-input) { min-height: 44px; padding: 8px 12px; font-size: 16px; line-height: 24px; border-radius: 8px; }
.settings-canvas :deep(.settings-field-input--secure) { padding-right: 48px; }
.settings-canvas :deep(.settings-disclosure) { border: 0; border-radius: 0; background: transparent; box-shadow: none; transition: none; }
.settings-canvas :deep(.settings-disclosure-toggle) { min-height: 52px; padding: 4px 0; background: transparent; transition: opacity 120ms ease; }
.settings-canvas :deep(.settings-disclosure-toggle:active) { opacity: 0.65; }
.settings-canvas :deep(.settings-disclosure-title) { font-size: 14px; line-height: 20px; font-weight: 500; white-space: normal; }
.settings-canvas :deep(.settings-disclosure-description) { font-size: 12px; line-height: 18px; color: var(--secondary-text); opacity: 1; }
.settings-canvas :deep(.settings-disclosure-content) { padding: 8px 0 0; }
.settings-canvas :deep(.settings-disclosure-body) { padding: 0; border: 0; }
.settings-canvas :deep(button:focus-visible) { outline: 2px solid var(--highlight-text, var(--primary-text)); outline-offset: 2px; }
@media (min-width: 640px) {
  .settings-canvas--viewport .settings-canvas-content { padding-inline: 24px; }
  .settings-canvas-actions-content { padding-inline: 24px; }
}
@media (prefers-reduced-motion: reduce) {
  .settings-canvas :deep(.settings-row),
  .settings-canvas :deep(.settings-disclosure-toggle) { transition: none; }
}
</style>
