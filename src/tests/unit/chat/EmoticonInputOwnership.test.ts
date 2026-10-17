import { afterEach, describe, expect, it } from 'vitest';
import { useEmoticonFixer } from '@/core/composables/useEmoticonFixer';
import { applyFrame, cleanupRegistry, rebuildSnapshot } from '@/core/utils/astExecutor';
import type { InlineNode } from '@/core/types/chat';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

type EmoticonWindow = Window & {
  __vcpFixEmoticon?: (img: HTMLImageElement) => Promise<void> | void;
  __vcpShowEmoticon?: (img: HTMLImageElement) => void;
};
const globalFixer = window as EmoticonWindow;
const original = 'https://example.com/表情包/first.png';

function deferred() {
  let resolve!: (value: string) => void;
  const promise = new Promise<string>((done) => { resolve = done; });
  return { promise, resolve };
}

function mountedImage(src = original, alt = '') {
  const container = document.createElement('div');
  const img = document.createElement('img');
  img.src = src;
  img.alt = alt;
  container.append(img);
  document.body.append(container);
  return { container, img };
}

afterEach(() => {
  delete globalFixer.__vcpFixEmoticon;
  delete globalFixer.__vcpShowEmoticon;
});

describe('emoticon input ownership', () => {
  it('does not repeat IPC for repeated scans, even when the repaired URL still identifies an emoticon', async () => {
    const pending = deferred();
    mockInvoke('fix_emoticon_url', () => pending.promise);
    const { container, img } = mountedImage();
    const { processEmoticonsInContainer: process } = useEmoticonFixer();

    const first = process(container);
    await process(container);
    await useEmoticonFixer().processEmoticonsInContainer(container);
    expect(invokeMock).toHaveBeenCalledTimes(1);

    const fixed = 'https://example.com/表情包/fixed.png';
    pending.resolve(fixed);
    await first;
    await process(container);
    expect(img.getAttribute('src')).toBe(fixed);
    expect(img.classList.contains('vcp-emoticon')).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it.each(['old-first', 'new-first'])('keeps the new source when repairs complete %s', async (order) => {
    const first = deferred();
    const second = deferred();
    const replacement = 'https://example.com/表情包/second.png';
    mockInvoke('fix_emoticon_url', (args) => args?.originalSrc === original ? first.promise : second.promise);
    const { container, img } = mountedImage();
    const { processEmoticonsInContainer: process } = useEmoticonFixer();

    const scanA = process(container);
    img.src = replacement;
    const scanB = process(container);
    if (order === 'old-first') {
      first.resolve('https://example.com/fixed-first.png');
      await scanA;
      expect(img.getAttribute('src')).toBe(replacement);
    }
    second.resolve('https://example.com/fixed-second.png');
    await scanB;
    first.resolve('https://example.com/fixed-first.png');
    await scanA;
    expect(img.getAttribute('src')).toBe('https://example.com/fixed-second.png');
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it('rejects a stale result after alt changes, and reprocesses the changed input', async () => {
    const first = deferred();
    mockInvoke('fix_emoticon_url', () => first.promise);
    const { container, img } = mountedImage('https://example.com/image.png', '表情包 first');
    const { processEmoticonsInContainer: process } = useEmoticonFixer();

    const scan = process(container);
    img.alt = 'ordinary image';
    first.resolve('https://example.com/stale.png');
    await scan;
    expect(img.getAttribute('src')).toBe('https://example.com/image.png');
    await process(container);
    expect(img.classList.contains('vcp-emoticon')).toBe(false);

    img.alt = '表情包 second';
    mockInvoke('fix_emoticon_url', () => 'https://example.com/current.png');
    await process(container);
    expect(img.getAttribute('src')).toBe('https://example.com/current.png');
    expect(img.classList.contains('vcp-emoticon')).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it('recognizes a new emoticon input on a previously scanned ordinary image', async () => {
    mockInvoke('fix_emoticon_url', () => 'https://example.com/fixed.png');
    const { container, img } = mountedImage('https://example.com/photo.png');
    const { processEmoticonsInContainer: process } = useEmoticonFixer();
    await process(container);
    expect(invokeMock).not.toHaveBeenCalled();

    img.alt = encodeURIComponent('表情包');
    await process(container);
    expect(img.getAttribute('src')).toBe('https://example.com/fixed.png');
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it.each([original, 'https://example.com/fixed.png'])('restores emoticon routing after AST morphs the parent, without repeating repair for %s', async (fixedUrl) => {
    mockInvoke('fix_emoticon_url', () => fixedUrl);
    const messageId = 'emoticon-morph-ownership';
    const container = document.createElement('div');
    document.body.append(container);
    const strongImage = (src: string, text: string): InlineNode => ({
      type: 'strong', children: [
        { type: 'image', src, alt: '', title: null, needs_asset_conversion: false },
        { type: 'text', value: text },
      ],
    });
    const { processEmoticonsInContainer: process } = useEmoticonFixer();

    try {
      rebuildSnapshot([{ type: 'paragraph', children: [strongImage(original, 'before')] }], messageId, container);
      const img = container.querySelector('img')!;
      await process(container);
      expect(img.classList.contains('vcp-emoticon')).toBe(true);

      expect(applyFrame([
        { op: 'replace_inline', id: 't0.i0', node: strongImage(fixedUrl, 'after') },
      ], messageId, container).ok).toBe(true);
      expect(container.querySelector('img')).toBe(img);
      expect(img.classList.contains('vcp-emoticon')).toBe(false);
      await process(container);
      expect(img.classList.contains('vcp-emoticon')).toBe(true);
      expect(invokeMock).toHaveBeenCalledTimes(1);
    } finally {
      cleanupRegistry(messageId);
    }
  });

  it.each(['image removed', 'container removed', 'image reparented'])('does not write after %s', async (change) => {
    const pending = deferred();
    mockInvoke('fix_emoticon_url', () => pending.promise);
    const { container, img } = mountedImage();
    const { processEmoticonsInContainer: process } = useEmoticonFixer();
    const scan = process(container);

    if (change === 'image removed') img.remove();
    else if (change === 'container removed') container.remove();
    else document.body.append(img);
    pending.resolve('https://example.com/stale.png');
    await scan;
    expect(img.getAttribute('src')).toBe(original);
  });

  it('does not let an old request own a later return to the same source', async () => {
    const requests = [deferred(), deferred(), deferred()];
    let next = 0;
    mockInvoke('fix_emoticon_url', () => requests[next++].promise);
    const { container, img } = mountedImage();
    const { processEmoticonsInContainer: process } = useEmoticonFixer();
    const firstA = process(container);
    img.src = 'https://example.com/表情包/second.png';
    const scanB = process(container);
    img.src = original;
    const secondA = process(container);

    requests[0].resolve('https://example.com/stale-a.png');
    requests[1].resolve('https://example.com/stale-b.png');
    await Promise.all([firstA, scanB]);
    expect(img.getAttribute('src')).toBe(original);
    requests[2].resolve('https://example.com/current-a.png');
    await secondA;
    expect(img.getAttribute('src')).toBe('https://example.com/current-a.png');
  });

  it('shares processing with template error handlers and stops repeated repair after a failed reload', async () => {
    const pending = deferred();
    mockInvoke('fix_emoticon_url', () => pending.promise);
    const { container, img } = mountedImage();
    const fixer = useEmoticonFixer();
    fixer.initGlobalFixer();
    img.style.visibility = 'hidden';
    const failure = globalFixer.__vcpFixEmoticon!(img);
    await fixer.processEmoticonsInContainer(container);
    expect(invokeMock).toHaveBeenCalledTimes(1);

    const fixed = 'https://example.com/表情包/fixed.png';
    pending.resolve(fixed);
    await failure;
    expect(img.getAttribute('src')).toBe(fixed);
    await globalFixer.__vcpFixEmoticon!(img);
    expect(img.style.visibility).toBe('visible');
    expect(invokeMock).toHaveBeenCalledTimes(1);

    img.style.visibility = 'hidden';
    globalFixer.__vcpShowEmoticon!(img);
    expect(img.style.visibility).toBe('visible');
  });

  it.each(['source changed', 'image removed'])('does not write or reveal from a late template error handler after %s', async (change) => {
    const pending = deferred();
    mockInvoke('fix_emoticon_url', () => pending.promise);
    const { img } = mountedImage();
    useEmoticonFixer().initGlobalFixer();
    img.style.visibility = 'hidden';
    const failure = globalFixer.__vcpFixEmoticon!(img);
    if (change === 'source changed') img.src = 'https://example.com/replacement.png';
    else img.remove();
    const current = img.getAttribute('src');
    pending.resolve('');
    await failure;
    expect(img.getAttribute('src')).toBe(current);
    expect(img.style.visibility).toBe('hidden');
  });

  it('continues scanning past malformed URI escapes', async () => {
    mockInvoke('fix_emoticon_url', () => 'https://example.com/fixed.png');
    const { container } = mountedImage('https://example.com/%zz.png');
    const img = document.createElement('img');
    img.src = original;
    container.append(img);
    await useEmoticonFixer().processEmoticonsInContainer(container);
    expect(img.getAttribute('src')).toBe('https://example.com/fixed.png');
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });
});
