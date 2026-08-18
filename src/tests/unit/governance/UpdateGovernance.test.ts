import { describe, expect, it } from 'vitest';

import updateManagerSource from '../../../../src-tauri/src/vcp_modules/updater/update_manager.rs?raw';
import downloadSource from '../../../../src-tauri/src/vcp_modules/updater/download.rs?raw';
import appLibSource from '../../../../src-tauri/src/lib.rs?raw';
import updateStoreSource from '../../../core/stores/update.ts?raw';
import updateTypesSource from '../../../core/types/update.ts?raw';
import updateDownloaderSource from '../../../core/composables/useUpdateDownloader.ts?raw';
import autoUpdateSource from '../../../core/composables/useAutoUpdate.ts?raw';
import updateSectionSource from '../../../features/settings/components/UpdateSection.vue?raw';
import updatePromptSource from '../../../components/ui/UpdatePrompt.vue?raw';

describe('OTA update governance contracts', () => {
  it('selects the APK asset by suffix so the sha256 sidecar never matches', () => {
    expect(updateManagerSource).toContain('a.name.ends_with(APK_ASSET_SUFFIX)');
    expect(updateManagerSource).not.toContain('a.name.contains(APK_ASSET_SUFFIX)');
    expect(updateManagerSource).toContain('parse_sha256_sidecar');
    expect(updateManagerSource).toContain('verify_file_sha256');
  });

  it('keeps the Rust state machine as the single owner of update state', () => {
    // 五个命令全部注册
    for (const command of [
      'check_for_update',
      'start_update_download',
      'cancel_update_download',
      'get_update_status',
      'install_update',
    ]) {
      expect(appLibSource).toContain(command);
    }
    expect(appLibSource).toContain('app.manage(UpdateSession::new())');

    // 旧契约已移除：不再存在 Channel 进度或前端传 URL/path 的命令
    expect(updateManagerSource).not.toContain('Channel<DownloadProgress>');
    expect(updateManagerSource).not.toContain('download_update');
    expect(updateManagerSource).toContain('UPDATE_STATUS_EVENT');
    expect(updateTypesSource).toContain('vcp-update://status');
    expect(updateStoreSource).toContain('UPDATE_STATUS_EVENT');
  });

  it('never lets the frontend hand URLs or file paths to update commands', () => {
    const frontendSources = [
      updateDownloaderSource,
      autoUpdateSource,
      updateSectionSource,
      updatePromptSource,
    ];
    for (const source of frontendSources) {
      // 不允许直接调用更新命令（必须经由 update store）
      expect(source).not.toContain("'check_for_update'");
      expect(source).not.toContain("'start_update_download'");
      expect(source).not.toContain("'cancel_update_download'");
      expect(source).not.toContain("'install_update'");
      // 旧契约残留：下载命令携带 url、安装命令携带 apkPath
      expect(source).not.toContain('onProgress: channel');
      expect(source).not.toContain('apkPath');
    }
    // store 内调用不得携带参数（URL 只来自 Rust 验证过的 GitHub API 响应）
    expect(updateStoreSource).toContain("invoke<UpdateStatus>('start_update_download')");
    expect(updateStoreSource).toContain("invoke<UpdateStatus>('install_update')");
  });

  it('keeps download robustness machinery in place', () => {
    expect(downloadSource).toContain('STALL_TIMEOUT');
    expect(downloadSource).toContain('reqwest::header::RANGE');
    expect(downloadSource).toContain('parse_content_range_start');
    expect(downloadSource).toContain('RestartFromScratch');
    expect(updateManagerSource).toContain('MAX_DOWNLOAD_ATTEMPTS');
    expect(updateManagerSource).toContain('acquire_ota_keepalive');
    expect(updateManagerSource).toContain('release_ota_keepalive');
    expect(updateManagerSource).toContain('can_install_packages');
    expect(updateManagerSource).toContain('verify_apk_signature');
  });

  it('keeps the update prompt compliant with the UI constitution', () => {
    expect(updatePromptSource).not.toContain('rounded-2xl');
    // 注意：本文件自身也会被 blur 扫描器命中，故用拼接规避字面量
    expect(updatePromptSource).not.toContain('backdrop-' + 'blur');
    expect(updatePromptSource).not.toContain('blur-' + '3xl');
    expect(updatePromptSource).not.toContain('active:scale');
    expect(updatePromptSource).not.toContain('cubic-bezier(0.34, 1.56');
    expect(updatePromptSource).toContain('z-dialog');
  });
});
