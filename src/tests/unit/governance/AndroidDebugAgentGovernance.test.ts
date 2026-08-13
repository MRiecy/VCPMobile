import { describe, expect, it } from 'vitest';

import packageManifestSource from '../../../../package.json?raw';
import agentSource from '../../../../tests/e2e-android/scripts/android-debug-agent.cjs?raw';
import adbSource from '../../../../tests/e2e-android/scripts/adb-env.cjs?raw';
import agentGuide from '../../../../docs/ANDROID_AGENT_DEBUGGING.md?raw';
import performanceStartup from '../../../../tests/perf/scripts/measure_startup_adb.cjs?raw';
import performanceCapture from '../../../../tests/perf/scripts/collect_android_dumpsys.cjs?raw';

const packageManifest = JSON.parse(packageManifestSource) as {
  scripts: Record<string, string>;
};
const trackedAndroidScripts = Object.keys(
  import.meta.glob('../../../../tests/e2e-android/scripts/*.cjs', {
    query: '?raw',
    import: 'default',
    eager: true,
  }),
);

describe('Android Debug Agent governance', () => {
  it('exposes one tracked command family and retires legacy entrypoints', () => {
    const commands = Object.entries(packageManifest.scripts)
      .filter(([name]) => name === 'android:debug' || name.startsWith('android:debug:'));

    expect(commands.length).toBeGreaterThanOrEqual(10);
    for (const [, command] of commands) {
      expect(command).toContain('tests/e2e-android/scripts/android-debug-agent.cjs');
      expect(command).not.toContain('scripts/tauri_android_dev.cjs');
    }

    for (const legacy of ['adb-smoke.cjs', 'install-apk.cjs', 'grant-permissions.cjs']) {
      expect(trackedAndroidScripts.some((path) => path.endsWith(`/${legacy}`))).toBe(false);
    }
  });

  it('keeps device writes Debug-only and output bounded', () => {
    expect(agentSource).toContain("const MAX_LOG_LINES = 200");
    expect(agentSource).toMatch(/\bDEBUG_PACKAGE\b/);
    expect(agentSource).toContain('`--pid=${status.app.pid}`');
    expect(agentSource).not.toContain("'logcat', '-c'");
    expect(agentSource).not.toContain('reverse --remove-all');
    expect(agentSource).not.toContain('E2E_PACKAGE');
    expect(adbSource).not.toContain('RELEASE_PACKAGE');
    expect(adbSource).not.toContain('E2E_PACKAGE');
  });

  it('keeps performance device scripts on the isolated Debug package', () => {
    expect(packageManifest.scripts).not.toHaveProperty('perf:collect');
    expect(packageManifest.scripts['perf:collect:full']).toContain(
      'collect_android_dumpsys.cjs',
    );

    for (const source of [performanceStartup, performanceCapture]) {
      expect(source).toContain('DEBUG_PACKAGE');
      expect(source).not.toContain('--mode');
      expect(source).not.toContain('getPackageName');
    }
  });

  it('documents the Agent data-volume and Release boundaries', () => {
    expect(agentGuide).toContain('com.vcp.avatar.debug');
    expect(agentGuide).toContain('com.vcp.avatar');
    expect(agentGuide).toContain('200');
    expect(agentGuide).toContain('logcat -c');
    expect(agentGuide).toContain('adb reverse --remove-all');
    expect(agentGuide).toContain('pnpm android:debug:snapshot');
  });
});
