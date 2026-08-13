import { describe, expect, it } from 'vitest';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { resolve } from 'node:path';
import themeStoreSource from '@/core/stores/theme.ts?raw';
import chatViewSource from '@/features/chat/ChatView.vue?raw';
import messageRendererSource from '@/features/chat/MessageRenderer.vue?raw';
import themePickerSource from '@/features/settings/ThemePicker.vue?raw';
import settingsViewSource from '@/features/settings/SettingsView.vue?raw';

const themesCssSource = readFileSync(resolve('src/assets/themes.css'), 'utf8');

describe('theme presentation architecture contracts', () => {
  it('keeps one canonical three-mode owner for both entry points', () => {
    expect(themeStoreSource).toContain("['bubble', 'panel', 'immersive'] as const");
    expect(themeStoreSource).toContain("PRESENTATION_STORAGE_KEY = 'vcp-chat-presentation-mode'");
    expect(chatViewSource).toContain('v-longpress.suppress-click="openPresentationMenu"');
    expect(chatViewSource).toContain(':data-presentation-mode="themeStore.presentationMode"');
    expect(themePickerSource).toContain('消息呈现与内容宽度');
    expect(themePickerSource).toContain('CHAT_PRESENTATION_OPTIONS');
    expect(themePickerSource).toContain('themeStore.setPresentationMode(mode)');
  });

  it('places live theme preview before the settings-page presentation selector', () => {
    expect(themePickerSource.indexOf('<ThemeLivePreview')).toBeGreaterThan(-1);
    expect(themePickerSource.indexOf('data-testid="presentation-settings"')).toBeGreaterThan(
      themePickerSource.indexOf('<ThemeLivePreview'),
    );
    expect(settingsViewSource).toContain("currentSubPage === 'theme'");
    expect(settingsViewSource).toContain('overflow-hidden flex flex-col min-h-0');
  });

  it('keeps one MessageRenderer tree and limits mode CSS to semantic message shells', () => {
    expect(chatViewSource.match(/<MessageRenderer\b/g)).toHaveLength(1);
    for (const block of [
      'DiaryBlock',
      'ToolBlock',
      'ThoughtBlock',
      'HtmlPreviewBlock',
      'ToolSummaryBlock',
      'AttachmentPreview',
    ]) {
      expect(messageRendererSource).toContain(`<${block}`);
    }

    expect(themesCssSource).toContain('[data-presentation-mode="panel"]');
    expect(themesCssSource).toContain('[data-presentation-mode="immersive"]');
    expect(themesCssSource).not.toContain('.vcp-message-item *');

    const presentationStyles = themesCssSource.slice(
      themesCssSource.indexOf('/* --- 消息呈现模式'),
      themesCssSource.indexOf('/* 切换主题时的平滑过渡保护'),
    );
    expect(presentationStyles).not.toMatch(/backdrop-(?:filter|blur)/);
    const unsupportedColorFunction = ['color', 'mix('].join('-');
    expect(presentationStyles).not.toContain(unsupportedColorFunction);
  });
});
