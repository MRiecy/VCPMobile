import { describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { resolve } from 'node:path';

import ciWorkflow from '../../../../.github/workflows/ci.yml?raw';
import androidManifest from '../../../../src-tauri/gen/android/app/src/main/AndroidManifest.xml?raw';
import viteConfig from '../../../../vite.config.ts?raw';
import unoConfig from '../../../../uno.config.ts?raw';
import appSource from '../../../App.vue?raw';
import avatarCropperSource from '../../../components/ui/AvatarCropper.vue?raw';
import vcpAvatarSource from '../../../components/ui/VcpAvatar.vue?raw';
import leftSidebarSource from '../../../components/layout/AgentSidebar.vue?raw';
import rightSidebarSource from '../../../components/layout/RightSidebar.vue?raw';
import assistantStoreSource from '../../../core/stores/assistant.ts?raw';
import avatarStoreSource from '../../../core/stores/avatar.ts?raw';
import { useAvatarStore } from '../../../core/stores/avatar';
import layersSource from '../../../core/constants/layers.ts?raw';
import sidebarSwipeSource from '../../../core/composables/useSidebarSwipe.ts?raw';
import agentSettingsSource from '../../../features/agent/AgentSettingsView.vue?raw';
import groupSettingsSource from '../../../features/agent/GroupSettingsView.vue?raw';
import diaryCenterSource from '../../../features/diary/DiaryCenterView.vue?raw';
import userProfileSource from '../../../features/settings/components/UserProfileSection.vue?raw';
import { mockInvoke } from '../../mocks/tauri';
import { nativeInsetsToCss, normalizeNativeInsets } from '../../../core/composables/useKeyboardInsets';

const themesSource = readFileSync(resolve('src/assets/themes.css'), 'utf8');
const messageBlocksSource = readFileSync(resolve('src/assets/message-blocks.css'), 'utf8');

const uiSources = {
  ...import.meta.glob('../../../../src/**/*.vue', {
    eager: true,
    query: '?raw',
    import: 'default',
  }),
  ...import.meta.glob('../../../../src/**/*.ts', {
    eager: true,
    query: '?raw',
    import: 'default',
  }),
} as Record<string, string>;

const productionUiSources = Object.entries(uiSources).filter(
  ([path]) => !path.includes('/tests/'),
).concat([
  ['src/assets/themes.css', themesSource],
  ['src/assets/message-blocks.css', messageBlocksSource],
]);

// A fixed/sticky singleton may enter this set only with the performance evidence
// required by the UI constitution. Content surfaces never belong here.
const approvedBackdropBlurSources = new Set<string>();

function withoutSupportsBlocks(source: string): string {
  let cursor = 0;
  let result = '';

  while (cursor < source.length) {
    const supportsStart = source.indexOf('@supports', cursor);
    if (supportsStart === -1) return result + source.slice(cursor);
    result += source.slice(cursor, supportsStart);

    const blockStart = source.indexOf('{', supportsStart);
    if (blockStart === -1) return result;
    let depth = 1;
    let index = blockStart + 1;
    while (index < source.length && depth > 0) {
      if (source[index] === '{') depth += 1;
      if (source[index] === '}') depth -= 1;
      index += 1;
    }
    cursor = index;
  }

  return result;
}

describe('Android mobile UI compatibility contracts', () => {
  it('requires touch hardware and rejects the TV launcher', () => {
    expect(androidManifest).not.toContain('android.software.leanback');
    expect(androidManifest).not.toContain('android.intent.category.LEANBACK_LAUNCHER');
    expect(androidManifest).toContain(
      '<uses-feature android:name="android.hardware.touchscreen" android:required="true" />',
    );
  });

  it('freezes a production syntax baseline and builds that artifact in CI', () => {
    expect(viteConfig).toContain('target: "chrome87"');
    expect(viteConfig).toContain('cssTarget: "chrome87"');
    expect(ciWorkflow).toContain('- name: Frontend Production Build');
    expect(ciWorkflow).toContain('run: pnpm build');
  });

  it('keeps the workspace horizontal at tablet widths with one authoritative breakpoint pair', () => {
    expect(appSource).toContain('vcp-workspace-row');
    expect(appSource).toContain('flex-direction: row');
    expect(leftSidebarSource).toContain('@media (min-width: 1024px)');
    expect(leftSidebarSource).toContain('flex: 0 0 280px');
    expect(rightSidebarSource).toContain('@media (min-width: 1280px)');
    expect(rightSidebarSource).toContain('flex: 0 0 300px');
    expect(appSource).toContain(".vcp-drawer-overlay:not(.is-right-open)");
    expect(sidebarSwipeSource).toContain("matchMedia('(min-width: 1024px)')");
    expect(sidebarSwipeSource).toContain("matchMedia('(min-width: 1280px)')");
  });

  it('uses persistent opacity-only edge shadows without extending drawer travel', () => {
    expect(themesSource).not.toContain('box-shadow: 0 0 40px');
    expect(themesSource).toContain('--vcp-drawer-shadow-width: 32px');
    expect(themesSource).toContain('.vcp-drawer::after');
    expect(themesSource).toContain('transition: opacity 0.28s ease-out');
    expect(themesSource).toContain('linear-gradient(to right');
    expect(themesSource).toContain('linear-gradient(to left');
    expect(leftSidebarSource).toContain('vcp-drawer-surface');
    expect(rightSidebarSource).toContain('vcp-drawer-surface');
    expect(leftSidebarSource).toContain('transform: translateX(-100%)');
    expect(rightSidebarSource).toContain('transform: translateX(100%)');
  });

  it('keeps color-mix enhancements behind @supports with stable base tokens', () => {
    expect(themesSource).toContain('--vcp-panel-bg-97: var(--secondary-bg)');
    expect(diaryCenterSource).toContain('--diary-surface: var(--secondary-bg)');

    for (const [path, source] of productionUiSources) {
      expect(withoutSupportsBlocks(source), `${path} has an unguarded color-mix()`).not.toContain(
        'color-mix(',
      );
    }
  });

  it('forbids brace-literal interpolations that break the Vue template parser', () => {
    // {{ '...'}}...' }} 这类内联字符串字面量会让模板解析器报
    // "Unterminated string constant"（插值内的 }} 被当作提前闭合），
    // 且只在运行/构建时暴露——用 v-text 绑定代替。
    const forbiddenPattern = /\{\{\s*['"`][^'"`]*\{\{|\{\{\s*['"`][^'"`]*\}\}/;
    for (const [path, source] of productionUiSources) {
      if (!path.endsWith('.vue')) continue;
      expect(
        forbiddenPattern.test(source),
        `${path} contains a brace-literal interpolation (use v-text instead)`,
      ).toBe(false);
    }
  });

  it('requires explicit blur approval and forbids direct safe-area env consumers', () => {
    for (const [path, source] of productionUiSources) {
      const hasBackdropBlur = /\bbackdrop-blur(?:-|\b)|(?:-webkit-)?backdrop-filter\s*:/.test(
        source,
      );
      if (hasBackdropBlur) {
        expect(
          approvedBackdropBlurSources.has(path),
          `${path} reintroduced an unapproved backdrop blur`,
        ).toBe(true);
      }
      if (!path.endsWith('/assets/themes.css')) {
        expect(source, `${path} bypasses the shared Insets variables`).not.toContain(
          'env(safe-area-inset-',
        );
      }
    }
  });

  it('keeps the TS, CSS and Uno semantic layer values aligned', () => {
    const expected = {
      editor: 70,
      viewer: 80,
      toast: 90,
      guide: 95,
      boot: 100,
      gate: 110,
    };

    for (const [name, value] of Object.entries(expected)) {
      expect(layersSource).toContain(`LAYER_${name.toUpperCase()} = ${value}`);
      expect(themesSource).toContain(`--layer-${name}: ${value}`);
      expect(unoConfig).toContain(`${name}: '${value}'`);
    }
  });

  it('keeps avatar cropping circular, DPR-bounded and first in the Android back stack', () => {
    expect(avatarCropperSource).toContain('fixed: true');
    expect(avatarCropperSource).toContain('fixedNumber: [1, 1]');
    expect(avatarCropperSource).toContain('high: false');
    expect(avatarCropperSource).toContain('context.arc(');
    expect(avatarCropperSource).toContain("registerModal(MODAL_ID, handleCancel)");
  });

  it('publishes saved avatars through the global cache owner instead of local view versions', () => {
    expect(assistantStoreSource).toContain('await avatarStore.refreshAvatar(ownerType, ownerId, hash)');
    expect(avatarStoreSource).toContain('const refreshAvatar =');
    expect(avatarStoreSource).toContain('requestGeneration');
    expect(avatarStoreSource).toContain('preloadMetadata');
    expect(avatarStoreSource).not.toContain('item.imageData');
    expect(vcpAvatarSource).toContain('v-intersection-observer');
    expect(viteConfig).toContain('exclude: ["tauri-plugin-vcp-mobile"]');
    for (const source of [agentSettingsSource, groupSettingsSource, userProfileSource]) {
      expect(source).not.toContain('avatarVersion');
    }
  });

  it('does not let an older avatar read overwrite a refresh that finished first', async () => {
    setActivePinia(createPinia());
    let resolveOldRead!: (value: unknown) => void;
    mockInvoke('get_avatar', () => new Promise((resolve) => {
      resolveOldRead = resolve;
    }));

    const createObjectUrl = vi.mocked(URL.createObjectURL);
    let objectUrlSequence = 0;
    createObjectUrl.mockImplementation(() => `blob:avatar-${++objectUrlSequence}`);

    try {
      const store = useAvatarStore();
      const oldRead = store.getAvatarUrl('agent', 'agent-1');
      await vi.waitFor(() => expect(resolveOldRead).toBeTypeOf('function'));

      mockInvoke('get_avatar', () => ({
        avatar_hash: 'new-hash',
        mime_type: 'image/png',
        image_data: [2],
        dominant_color: '#123456',
        updated_at: 2,
      }));
      const refreshed = await store.refreshAvatar('agent', 'agent-1', 'new-hash');

      resolveOldRead({
        avatar_hash: 'old-hash',
        mime_type: 'image/png',
        image_data: [1],
        dominant_color: '#654321',
        updated_at: 1,
      });

      expect(refreshed).toBe('blob:avatar-1');
      await expect(oldRead).resolves.toBe('blob:avatar-1');
      expect(store.cache.get('agent:agent-1')?.blobUrl).toBe('blob:avatar-1');
    } finally {
      createObjectUrl.mockImplementation(() => 'blob:mock');
    }
  });

  it('invalidates an unknown in-flight avatar read when metadata becomes authoritative', async () => {
    setActivePinia(createPinia());
    let resolveOldRead!: (value: unknown) => void;
    mockInvoke('get_avatar', () => new Promise((resolve) => {
      resolveOldRead = resolve;
    }));

    const createObjectUrl = vi.mocked(URL.createObjectURL);
    createObjectUrl.mockImplementation(() => 'blob:metadata-current');

    try {
      const store = useAvatarStore();
      const oldRead = store.getAvatarUrl('agent', 'agent-2');

      mockInvoke('batch_get_avatars', () => [{
        ownerType: 'agent',
        ownerId: 'agent-2',
        avatarHash: 'current-hash',
        dominantColor: '#123456',
        updatedAt: 2,
      }]);
      await store.refreshMetadata();

      resolveOldRead({
        avatar_hash: 'old-hash',
        mime_type: 'image/png',
        image_data: [1],
        dominant_color: '#654321',
        updated_at: 1,
      });
      await expect(oldRead).resolves.toBe('');
      expect(store.metadata.get('agent:agent-2')?.avatarHash).toBe('current-hash');

      mockInvoke('get_avatar', () => ({
        avatar_hash: 'current-hash',
        mime_type: 'image/png',
        image_data: [2],
        dominant_color: '#123456',
        updated_at: 2,
      }));
      await expect(store.getAvatarUrl('agent', 'agent-2')).resolves.toBe('blob:metadata-current');
      expect(store.cache.get('agent:agent-2')?.avatarHash).toBe('current-hash');
    } finally {
      createObjectUrl.mockImplementation(() => 'blob:mock');
    }
  });

  it('normalizes physical Insets once and subtracts navigation bars from the IME', () => {
    const snapshot = normalizeNativeInsets({
      safeTopPx: 72,
      safeRightPx: 12,
      safeBottomPx: 96,
      safeLeftPx: 6,
      imeBottomPx: 816,
      imeVisible: true,
    });

    expect(nativeInsetsToCss(snapshot, 3)).toEqual({
      safeTop: 24,
      safeRight: 4,
      safeBottom: 32,
      safeLeft: 2,
      imeExtraBottom: 240,
      imeVisible: true,
    });
    expect(nativeInsetsToCss({ ...snapshot, imeBottomPx: 48 }, 3).imeExtraBottom).toBe(0);
    expect(normalizeNativeInsets({ height: 600, visible: true, safeAreaBottom: 90 })).toEqual({
      safeTopPx: 0,
      safeRightPx: 0,
      safeBottomPx: 90,
      safeLeftPx: 0,
      imeBottomPx: 600,
      imeVisible: true,
    });
  });
});
