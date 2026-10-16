import { invoke } from '@tauri-apps/api/core';
import { saveImageFromPath, type GallerySaveResult } from 'tauri-plugin-vcp-mobile';

/** Maximum encoded WebView product; external files use native paths instead. */
export const MAX_TEMPORARY_EXPORT_BYTES = 8 * 1024 * 1024;

export async function saveBlobToGallery(
  blob: Blob,
  fileName: string,
  signal?: AbortSignal,
): Promise<GallerySaveResult> {
  const checkCancelled = () => {
    if (signal?.aborted) throw new DOMException('已取消保存', 'AbortError');
  };
  checkCancelled();
  if (!blob.size || blob.size > MAX_TEMPORARY_EXPORT_BYTES) {
    throw new Error('导出图片需在 8 MiB 以内，请缩小尺寸后重试');
  }
  const bytes = new Uint8Array(await blob.arrayBuffer());
  checkCancelled();
  const operationId = `export_${crypto.randomUUID()}`;
  await invoke('begin_file_operation', { operationId });
  let temporary: { path: string; size: number } | undefined;
  const cancelPreparation = () => {
    void invoke('cancel_attachment_import', { operationId }).catch(console.warn);
  };
  signal?.addEventListener('abort', cancelPreparation, { once: true });
  try {
    checkCancelled();
    temporary = await invoke<{ path: string; size: number }>('store_temporary_export', {
      fileBytes: Array.from(bytes), originalName: fileName, operationId,
    });
    checkCancelled();
    // Once PhotoKit receives the file, only its completion releases the source.
    signal?.removeEventListener('abort', cancelPreparation);
    return await saveImageFromPath(temporary.path, fileName);
  } finally {
    signal?.removeEventListener('abort', cancelPreparation);
    if (temporary) await invoke('release_temporary_export', { path: temporary.path }).catch(console.warn);
    await invoke('finish_file_operation', { operationId }).catch(console.warn);
  }
}
