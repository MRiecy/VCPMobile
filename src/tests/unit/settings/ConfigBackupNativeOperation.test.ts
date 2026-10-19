import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { exportConfigBackup } from '../../../features/settings/useConfigBackup';
import { cancelFileOperation } from '../../../core/utils/nativeFilePicker';
import { useThemeStore } from '../../../core/stores/theme';
import { useChatPreferencesStore } from '../../../features/chat/preferences/store';
import { invokeMock, mockInvoke } from '../../mocks/tauri';

vi.mock('../../../core/utils/platform', () => ({ isAndroid: false, isIOS: true }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((accept, decline) => { resolve = accept; reject = decline; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  localStorage.clear();
  setActivePinia(createPinia());
});

afterEach(() => {
  useThemeStore().$dispose();
  useChatPreferencesStore().$dispose();
});

describe('iOS 配置导出沿用文件操作接纳与取消', () => {
  it('页面在 prepare 返回前关闭，不启动文件保存并释放迟到的 reservation', async () => {
    const prepared = deferred<{ reservationToken: string }>();
    mockInvoke('plugin:vcp-mobile|prepare_file_operation', () => prepared.promise);
    const exporting = exportConfigBackup({}, 'export-cancelled-before-start');
    const rejected = expect(exporting).rejects.toThrow('cancelled');
    await cancelFileOperation('export-cancelled-before-start');
    prepared.resolve({ reservationToken: 'reservation-1' });
    await rejected;
    expect(invokeMock.mock.calls.some(([name]) => name === 'plugin:vcp-mobile|save_to_downloads')).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|cancel_file_operation', { operationId: 'export-cancelled-before-start' });
  });

  it('保存携带配置用途、操作身份和 reservation，开始后仍可由页面取消', async () => {
    const saving = deferred<{ displayName: string }>();
    const started = deferred<void>();
    mockInvoke('plugin:vcp-mobile|prepare_file_operation', () => ({ reservationToken: 'reservation-2' }));
    mockInvoke('plugin:vcp-mobile|save_to_downloads', () => { started.resolve(); return saving.promise; });
    const exporting = exportConfigBackup({}, 'export-started');
    const rejected = expect(exporting).rejects.toThrow('cancelled');
    await started.promise;
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|save_to_downloads', expect.objectContaining({
      purpose: 'config', operationId: 'export-started', reservationToken: 'reservation-2',
      mimeType: 'application/json', fileName: expect.stringMatching(/\.vcpcfg\.json$/),
    }));
    await cancelFileOperation('export-started');
    expect(invokeMock).toHaveBeenCalledWith('plugin:vcp-mobile|cancel_file_operation', { operationId: 'export-started' });
    saving.reject(new Error('cancelled: file operation'));
    await rejected;
  });
});
