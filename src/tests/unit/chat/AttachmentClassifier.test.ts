import { describe, expect, it } from 'vitest';
import { classifyAttachment } from '@/features/chat/attachment/utils/AttachmentClassifier';
import { AttachmentType } from '@/features/chat/attachment/types/AttachmentType';

describe('AttachmentClassifier', () => {
  it('classifies by explicit mime type', () => {
    expect(classifyAttachment('image/png', 'image.bin')).toBe(AttachmentType.IMAGE);
    expect(classifyAttachment('video/mp4', 'video.bin')).toBe(AttachmentType.VIDEO);
    expect(classifyAttachment('audio/mpeg', 'audio.bin')).toBe(AttachmentType.AUDIO);
    expect(classifyAttachment('text/plain', 'note.bin')).toBe(AttachmentType.TEXT);
  });

  it('classifies generic octet-stream by extension', () => {
    expect(classifyAttachment('application/octet-stream', 'photo.PNG')).toBe(AttachmentType.IMAGE);
    expect(classifyAttachment('application/octet-stream', 'song.mp3')).toBe(AttachmentType.AUDIO);
    expect(classifyAttachment('application/octet-stream', 'report.pdf')).toBe(AttachmentType.DOCUMENT);
    expect(classifyAttachment('application/octet-stream', 'component.vue')).toBe(AttachmentType.CODE);
    expect(classifyAttachment('application/octet-stream', 'readme.md')).toBe(AttachmentType.TEXT);
  });

  it('falls back to document for specific application mime and other for unknown', () => {
    expect(classifyAttachment('application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'a.bin')).toBe(AttachmentType.DOCUMENT);
    expect(classifyAttachment('application/x-custom', 'a.bin')).toBe(AttachmentType.DOCUMENT);
    expect(classifyAttachment('', 'archive.unknown')).toBe(AttachmentType.OTHER);
  });
});
