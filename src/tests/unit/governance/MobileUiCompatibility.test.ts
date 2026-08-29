import { describe, expect, it } from 'vitest';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Vitest runs in Node; the application tsconfig intentionally omits Node types.
import { resolve } from 'node:path';

import ciWorkflow from '../../../../.github/workflows/ci.yml?raw';
import androidManifest from '../../../../src-tauri/gen/android/app/src/main/AndroidManifest.xml?raw';
import viteConfig from '../../../../vite.config.ts?raw';
import unoConfig from '../../../../uno.config.ts?raw';
import appSource from '../../../App.vue?raw';
import leftSidebarSource from '../../../components/layout/AgentSidebar.vue?raw';
import rightSidebarSource from '../../../components/layout/RightSidebar.vue?raw';
import layersSource from '../../../core/constants/layers.ts?raw';
import sidebarSwipeSource from '../../../core/composables/useSidebarSwipe.ts?raw';
import diaryCenterSource from '../../../features/diary/DiaryCenterView.vue?raw';
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
