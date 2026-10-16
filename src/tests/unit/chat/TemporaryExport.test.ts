import { describe, expect, it, vi } from 'vitest';
import { saveImageFromPath } from 'tauri-plugin-vcp-mobile';
import { saveBlobToGallery } from '@/core/utils/temporaryExport';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';
import { flushPromises } from '@/tests/utils/flush';

describe('temporary gallery export ownership', () => {
  it('keeps the file until the system save finishes even if its viewer closes', async () => {
    let complete!: (value: { uri: string; displayName: string; mimeType: string; size: number }) => void;
    vi.mocked(saveImageFromPath).mockImplementationOnce(() => new Promise((resolve) => { complete = resolve; }));
    mockInvoke('store_temporary_export', () => ({ path: '/cache/export.png', size: 3 }));
    const controller = new AbortController();
    const saving = saveBlobToGallery(new Blob(['png'], { type: 'image/png' }), 'diagram.png', controller.signal);
    await flushPromises();
    expect(saveImageFromPath).toHaveBeenCalledWith('/cache/export.png', 'diagram.png');
    controller.abort();
    expect(invokeMock.mock.calls.some(([command]) => command === 'release_temporary_export')).toBe(false);
    complete({ uri: 'photos:asset', displayName: 'diagram.png', mimeType: 'image/png', size: 3 });
    await expect(saving).resolves.toMatchObject({ uri: 'photos:asset' });
    expect(invokeMock).toHaveBeenCalledWith('release_temporary_export', { path: '/cache/export.png' });
  });

  it('releases a late staging result when cancelled before system submission', async () => {
    let stage!: (value: { path: string; size: number }) => void;
    mockInvoke('store_temporary_export', () => new Promise((resolve) => { stage = resolve; }));
    const controller = new AbortController();
    const saving = saveBlobToGallery(new Blob(['png']), 'diagram.png', controller.signal);
    const rejected = expect(saving).rejects.toMatchObject({ name: 'AbortError' });
    await flushPromises();
    controller.abort();
    stage({ path: '/cache/late.png', size: 3 });
    await rejected;
    expect(invokeMock).toHaveBeenCalledWith('release_temporary_export', { path: '/cache/late.png' });
    expect(invokeMock.mock.calls.some(([command]) => command === 'cancel_attachment_import')).toBe(true);
  });
});
