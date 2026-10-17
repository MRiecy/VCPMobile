import { invoke } from '@tauri-apps/api/core';

export interface EmoticonItem {
  id: number | null;
  url: string;
  category: string;
  filename: string;
  searchKey: string;
}

interface EmoticonInput {
  src: string;
  alt: string;
  isEmoticon: boolean;
}

// 节点只记住处理过的输入及分类；私有属性不会被 AST/morphdom 的属性同步清除。
// 所有 composable 实例及全局 onerror 共用这个标记，避免同一输入重复 IPC。
const processedInput = Symbol('processedEmoticonInput');
type EmoticonImage = HTMLImageElement & { [processedInput]?: EmoticonInput };

function mentionsEmoticon(value: string): boolean {
  if (value.includes('表情包')) return true;
  if (!value.includes('%')) return false;
  try {
    return decodeURIComponent(value).includes('表情包');
  } catch {
    return false;
  }
}

export function useEmoticonFixer() {
  const fixUrl = async (url: string): Promise<string> => {
    try {
      return await invoke<string>('fix_emoticon_url', { originalSrc: url });
    } catch (e) {
      console.error('[EmoticonFixer] Failed to fix URL:', e);
      return url;
    }
  };

  const getLibrary = async (): Promise<EmoticonItem[]> => {
    try {
      return await invoke<EmoticonItem[]>('get_emoticon_library');
    } catch (e) {
      console.error('[EmoticonFixer] Failed to get library:', e);
      return [];
    }
  };

  const regenerateLibrary = async (): Promise<number> => {
    try {
      return await invoke<number>('regenerate_emoticon_library');
    } catch (e) {
      console.error('[EmoticonFixer] Failed to regenerate library:', e);
      return 0;
    }
  };

  const processImage = (img: EmoticonImage, owner: Node, showFailure = false): Promise<void> | void => {
    if (!img.isConnected) return;

    const src = img.getAttribute('src') || '';
    const alt = img.alt;
    const previous = img[processedInput];
    if (previous?.src === src && previous.alt === alt) {
      if (previous.isEmoticon && !img.classList.contains('vcp-emoticon')) {
        img.classList.add('vcp-emoticon');
      }
      if (showFailure) img.style.visibility = 'visible';
      return;
    }

    const isEmoticon = mentionsEmoticon(src) || mentionsEmoticon(alt);
    const input = { src, alt, isEmoticon };
    img[processedInput] = input;
    if (!isEmoticon) {
      if (previous) img.classList.remove('vcp-emoticon');
      if (showFailure) img.style.visibility = 'visible';
      return;
    }
    img.classList.add('vcp-emoticon');

    return fixUrl(src).then((fixedUrl) => {
      // AST 可以原地改写图片，终态也可以替换整棵 DOM；迟到结果只属于发起时的输入和容器。
      if (
        img[processedInput] !== input
        || (img.getAttribute('src') || '') !== src
        || img.alt !== alt
        || !img.isConnected
        || !owner.contains(img)
      ) return;

      if (fixedUrl && fixedUrl !== src) {
        img.src = fixedUrl;
        // 修复后的 URL 也是同一次已处理输入，加载失败时不再递归修复。
        img[processedInput] = { src: img.getAttribute('src') || '', alt, isEmoticon };
      } else if (showFailure) {
        img.style.visibility = 'visible';
      }
    });
  };

  const initGlobalFixer = () => {
    (window as any).__vcpFixEmoticon = (img: HTMLImageElement) =>
      processImage(img, img.parentNode ?? img.ownerDocument, true);

    // 辅助函数：加载成功后显示
    (window as any).__vcpShowEmoticon = (img: HTMLImageElement) => {
      if (img.isConnected) img.style.visibility = 'visible';
    };
  };

  const processEmoticonsInContainer = async (container: HTMLElement) => {
    if (!container) return;

    const imgs = container.querySelectorAll('img');
    const promises: Promise<void>[] = [];

    imgs.forEach((img) => {
      const pending = processImage(img, container);
      if (pending) promises.push(pending);
    });

    await Promise.allSettled(promises);
  };

  return {
    fixUrl,
    getLibrary,
    regenerateLibrary,
    initGlobalFixer,
    processEmoticonsInContainer
  };
}
