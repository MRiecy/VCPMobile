import { describe, expect, it } from 'vitest';

import ciWorkflow from '../../../../.github/workflows/ci.yml?raw';
import releaseWorkflow from '../../../../.github/workflows/release.yml?raw';
import androidSettingsGenerator from '../../../../.github/generate-tauri-android-settings.mjs?raw';
import packageManifest from '../../../../package.json?raw';
import tauriConfig from '../../../../src-tauri/tauri.conf.json?raw';
import rootReadme from '../../../../README.md?raw';
import claudeGuide from '../../../../CLAUDE.md?raw';
import pluginIndex from '../../../../docs/plugins/00_总览与导航.md?raw';
import syncIndex from '../../../../docs/sync/00_总览与导航.md?raw';
import syncGuide from '../../../../docs/sync/15_开发指南与FAQ.md?raw';
import localServerGuide from '../../../../docs/modules/22_本地服务器与浮动助手.md?raw';
import rootGradle from '../../../../src-tauri/gen/android/build.gradle.kts?raw';
import appGradle from '../../../../src-tauri/gen/android/app/build.gradle.kts?raw';
import androidManifest from '../../../../src-tauri/gen/android/app/src/main/AndroidManifest.xml?raw';
import backupRules from '../../../../src-tauri/gen/android/app/src/main/res/xml/backup_rules.xml?raw';
import dataExtractionRules from '../../../../src-tauri/gen/android/app/src/main/res/xml/data_extraction_rules.xml?raw';
import wrapperProperties from '../../../../src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties?raw';
import appSource from '../../../App.vue?raw';
import lifecycleSource from '../../../core/stores/appLifecycle.ts?raw';
import permissionGateSource from '../../../components/layout/PermissionGate.vue?raw';
import updateDownloaderSource from '../../../core/composables/useUpdateDownloader.ts?raw';

const workflowActionReferences = (source: string) =>
  Array.from(source.matchAll(/^\s*uses:\s*([^\s#]+)(?:\s+#.*)?$/gm), (match) => match[1]);

describe('release and Android governance contracts', () => {
  it('pins every GitHub Action to an immutable commit SHA', () => {
    const references = [
      ...workflowActionReferences(ciWorkflow),
      ...workflowActionReferences(releaseWorkflow),
    ];

    expect(references.length).toBeGreaterThan(0);
    for (const reference of references) {
      expect(reference).toMatch(/^[^@]+@[0-9a-f]{40}$/);
    }
    expect(references).toContain('pnpm/action-setup@b906affcce14559ad1aafd4ab0e942779e9f58b1');
    expect(references).toContain('Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6');
  });

  it('gates a release on commit, version, certificate and checksum identity', () => {
    expect(releaseWorkflow).toContain('"$HEAD_COMMIT" != "$GITHUB_SHA"');
    expect(releaseWorkflow).toContain('github.event.repository.default_branch');
    expect(releaseWorkflow).toContain('git merge-base --is-ancestor "$HEAD_COMMIT" "origin/$DEFAULT_BRANCH"');
    expect(releaseWorkflow).not.toContain('gh run list');
    expect(releaseWorkflow).not.toContain('--event push');
    expect(releaseWorkflow).not.toContain('conclusion == "success"');
    expect(releaseWorkflow).toContain("require('./package.json').version");
    expect(releaseWorkflow).toContain("require('./src-tauri/tauri.conf.json').version");
    expect(releaseWorkflow).toContain('bundle.android.versionCode');
    expect(releaseWorkflow).not.toContain(
      "sed -n 's/^tauri\\.android\\.versionCode=//p' src-tauri/gen/android/app/tauri.properties",
    );
    expect(releaseWorkflow).not.toContain('ANDROID_RELEASE_CERT_SHA256');
    expect(releaseWorkflow).toContain('APK certificate SHA-256: ${APK_CERTS[0]}');
    expect(releaseWorkflow).toContain(
      "sed -nE 's/.*certificate SHA-256 digest: ([0-9a-fA-F]{64}).*/\\1/p'",
    );
    expect(releaseWorkflow).toContain('APK is signed with Android Debug key');
    expect(releaseWorkflow).toContain('sha256sum "$target_apk"');
    expect(releaseWorkflow).toContain('*.sha256');
    expect(releaseWorkflow).toContain('git diff --exit-code -- pnpm-lock.yaml src-tauri/Cargo.lock');

    const packageJson = JSON.parse(packageManifest);
    const tauriJson = JSON.parse(tauriConfig);
    expect(packageJson.version).toBe('1.1.4');
    expect(tauriJson.version).toBe('1.1.4');
    expect(tauriJson.bundle.android.versionCode).toBe(1001004);

    expect(releaseWorkflow).toContain('APK_ENTRIES=$(unzip -Z1 "$SOURCE_APK" 2>&1)');
    expect(releaseWorkflow).not.toMatch(/unzip -Z1[^\n]*\|[^\n]*grep -q/);
    expect(releaseWorkflow).toContain('lib/arm64-v8a/libvcp_pty\\.so');
    expect(releaseWorkflow).toContain('forbidden non-arm64 ABI');
    expect(releaseWorkflow).toContain('vcp-semantic-(tokenizer|model)');

    const cleanupIndex = releaseWorkflow.indexOf('- name: Remove Android keystore');
    const uploadIndex = releaseWorkflow.indexOf('- name: Update GitHub Release assets');
    expect(cleanupIndex).toBeGreaterThan(-1);
    expect(uploadIndex).toBeGreaterThan(cleanupIndex);
    const cleanupStep = releaseWorkflow.slice(cleanupIndex, uploadIndex);
    expect(cleanupStep).toContain('if: always()');
    expect(cleanupStep).toContain('rm -f -- "$ANDROID_KEYSTORE_PATH"');
  });

  it('uses the official checksummed Gradle wrapper', () => {
    expect(wrapperProperties).toContain(
      'distributionUrl=https\\://services.gradle.org/distributions/gradle-8.14.3-bin.zip',
    );
    expect(wrapperProperties).toContain(
      'distributionSha256Sum=bd71102213493060956ec229d946beee57158dbd89d0e62b91bca0fa2c5f3531',
    );
    expect(rootGradle).not.toContain('maven.aliyun.com');
    expect(releaseWorkflow).toContain(
      '7d3a4ac4de1c32b59bc6a4eb8ecb8e612ccd0cf1ae1e99f66902da64df296172',
    );
    expect(releaseWorkflow).toContain('sha256sum --check --strict');
    expect(releaseWorkflow).toContain(
      'pnpm tauri android build --apk --target aarch64',
    );
  });

  it('rejects incomplete release signing and makes trusted LAN cleartext explicit', () => {
    expect(appGradle).not.toContain('signingConfigs.getByName("debug")');
    expect(appGradle).toContain('Release signing is incomplete');
    expect(appGradle).toContain('VCP_TRUSTED_LAN_MODE');
    expect(appGradle).toContain('trustedLanMode == "enabled"');
    expect(androidManifest).toContain('android:usesCleartextTraffic="${usesCleartextTraffic}"');
    expect(releaseWorkflow).toContain('VCP_TRUSTED_LAN_MODE: enabled');
  });

  it('opts sensitive application data out of Android backup and device transfer', () => {
    expect(androidManifest).toContain('android:allowBackup="false"');
    expect(androidManifest).toContain('android:fullBackupContent="@xml/backup_rules"');
    expect(androidManifest).toContain('android:dataExtractionRules="@xml/data_extraction_rules"');
    expect(backupRules).toContain('<exclude domain="database" path="." />');
    expect(backupRules).toContain('<exclude domain="file" path="." />');
    expect(dataExtractionRules).toContain('<device-transfer>');
    expect(dataExtractionRules).toContain('<exclude domain="sharedpref" path="." />');
  });

  it('gates bootstrap on keep-alive critical permissions and requests the rest at feature use', () => {
    // 本产品定位为常驻中继节点：通知、电池优化豁免与通知监听是保活核心能力，
    // 必须在 bootstrap 阶段经 PermissionGate 硬门禁收齐，禁止再次整体移除门禁。
    expect(appSource).toContain('PermissionGate');
    expect(lifecycleSource).toContain("'PERMISSIONS'");
    expect(lifecycleSource).toContain('check_all_permissions');
    expect(lifecycleSource).toContain('check_notification_listener_permission');

    // 门禁硬门槛仅限保活核心三项；存储（全媒体读取）等非核心权限保持按需申请，
    // 不得回流进启动门禁。
    expect(permissionGateSource).toContain("pType: type");
    expect(permissionGateSource).not.toContain('储存空间权限');
    expect(permissionGateSource).not.toContain("| 'storage'");
    expect(lifecycleSource).not.toContain('pStatus.storage');

    // 次级链路（应用内更新下载）仍按需请求通知权限，且必须先请求再发通知。
    const requestIndex = updateDownloaderSource.indexOf("pType: 'notification'");
    const notificationIndex = updateDownloaderSource.indexOf('start_download_notification');
    expect(requestIndex).toBeGreaterThan(-1);
    expect(notificationIndex).toBeGreaterThan(requestIndex);
  });

  it('keeps CI commands locked, audited, real and documented', () => {
    const scripts = JSON.parse(packageManifest).scripts as Record<string, string>;
    expect(scripts.check).toContain('cargo check --locked');
    expect(scripts['test:integration']).toContain('--test file_extractor_integration');
    expect(scripts['test:integration']).toContain('--locked');
    expect(scripts['ci:prepare-android-settings']).toBe(
      'node .github/generate-tauri-android-settings.mjs',
    );
    expect(Object.keys(scripts).some((name) => name.startsWith('io:'))).toBe(false);
    expect(scripts).not.toHaveProperty('memory:refresh');
    expect(scripts).not.toHaveProperty('dev:android');
    expect(scripts).not.toHaveProperty('dev:usb');
    expect(scripts['android:debug']).toBe(
      'node tests/e2e-android/scripts/android-debug-agent.cjs',
    );
    expect(scripts['android:debug:dev']).toContain('android-debug-agent.cjs dev');
    expect(scripts['android:debug:logs']).toContain('android-debug-agent.cjs logs');
    expect(scripts['android:debug:snapshot']).toContain('android-debug-agent.cjs snapshot');
    expect(ciWorkflow).toContain('pnpm audit --audit-level=high');
    expect(ciWorkflow).not.toContain('pnpm audit:rust');
    expect(ciWorkflow).toContain('cargo test --locked --workspace --lib');
    expect(ciWorkflow).toContain('cargo clippy --locked --workspace --lib --tests -- -D warnings');
    expect(ciWorkflow).not.toContain('cargo bench');
    const generatorIndex = ciWorkflow.indexOf('pnpm ci:prepare-android-settings');
    const androidTestsIndex = ciWorkflow.indexOf(
      ':tauri-plugin-vcp-mobile:testDebugUnitTest',
    );
    expect(generatorIndex).toBeGreaterThan(-1);
    expect(generatorIndex).toBeLessThan(androidTestsIndex);
    expect(ciWorkflow).toContain('test -s src-tauri/gen/android/tauri.settings.gradle');
    expect(androidSettingsGenerator).toMatch(/["']metadata["'],\s*["']--locked["']/);
    expect(androidSettingsGenerator).toContain("metadata.resolve?.root");
    expect(androidSettingsGenerator).toContain('tauri.build.gradle.kts');
    expect(androidSettingsGenerator).not.toMatch(/\/home\/|[A-Za-z]:\\\\/);
    expect(ciWorkflow).toContain('git status --porcelain --untracked-files=all --');
    expect(ciWorkflow).toContain('src-tauri/gen/android');
    expect(ciWorkflow).toContain('src-tauri/plugins/vcp-mobile/permissions');
    expect(claudeGuide).not.toContain('build_android_release.ps1');

    for (const guide of [rootReadme, claudeGuide, pluginIndex, syncIndex, syncGuide, localServerGuide]) {
      expect(guide).not.toMatch(
        /build_android_release\.ps1|pnpm dev:usb|pnpm dev:android|scripts\/tauri_android_dev|pnpm memory:refresh/,
      );
    }
  });
});
