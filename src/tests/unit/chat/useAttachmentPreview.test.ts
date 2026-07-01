import { describe, expect, it } from 'vitest';
import { useAttachmentPreview } from '@/features/chat/attachment/composables/useAttachmentPreview';
import { AttachmentType } from '@/features/chat/attachment/types/AttachmentType';
import type { Attachment } from '@/core/types/chat';

function attachment(partial: Partial<Attachment>): Attachment {
  return {
    type: 'application/octet-stream',
    src: '',
    name: 'file.bin',
    size: 0,
    ...partial,
  } as Attachment;
}

describe('useAttachmentPreview', () => {
  it('detects previewable attachments by type and metadata', () => {
    const preview = useAttachmentPreview();

    expect(preview.canPreview.value(attachment({ type: 'image/png', name: 'a.png' }))).toBe(true);
    expect(preview.canPreview.value(attachment({ type: 'video/mp4', name: 'a.mp4' }))).toBe(true);
    expect(preview.canPreview.value(attachment({ type: 'audio/mpeg', name: 'a.mp3', src: 'asset://a.mp3' }))).toBe(true);
    expect(preview.canPreview.value(attachment({ type: 'application/pdf', name: 'a.pdf' }))).toBe(false);
    expect(preview.canPreview.value(attachment({ type: 'application/pdf', name: 'a.pdf', extractedText: 'text' }))).toBe(true);
    expect(preview.canPreview.value(attachment({ type: 'application/octet-stream', name: 'a.rs', extractedText: 'fn main() {}' }))).toBe(true);
  });

  it('formats file size and extracts extension', () => {
    const preview = useAttachmentPreview();

    expect(preview.formatFileSize(0)).toBe('0 B');
    expect(preview.formatFileSize(1024)).toBe('1 KB');
    expect(preview.formatFileSize(1536)).toBe('1.5 KB');
    expect(preview.formatFileSize(1024 * 1024)).toBe('1 MB');
    expect(preview.getFileExtension(attachment({ name: 'Archive.TAR.GZ' }))).toBe('gz');
  });

  it('truncates preview text at word boundary when possible', () => {
    const preview = useAttachmentPreview();
    const item = attachment({ extractedText: 'hello world from attachment' });

    expect(preview.getPreviewText.value(item, 11)).toBe('hello...');
    expect(preview.getPreviewText.value(attachment({ extractedText: 'short' }), 20)).toBe('short');
    expect(preview.getPreviewText.value(attachment({}), 20)).toBe('');
  });

  it('computes attachment statistics', () => {
    const preview = useAttachmentPreview();
    const stats = preview.getStats.value([
      attachment({ type: 'image/png', name: 'a.png', thumbnailPath: 'thumb' }),
      attachment({ type: 'application/pdf', name: 'a.pdf', extractedText: 'doc' }),
      attachment({ type: 'application/octet-stream', name: 'a.rs', extractedText: 'code' }),
    ]);

    expect(stats.total).toBe(3);
    expect(stats.byType[AttachmentType.IMAGE]).toBe(1);
    expect(stats.byType[AttachmentType.DOCUMENT]).toBe(1);
    expect(stats.byType[AttachmentType.CODE]).toBe(1);
    expect(stats.canPreview).toBe(3);
    expect(stats.hasText).toBe(2);
    expect(stats.hasThumbnails).toBe(1);
  });
});
