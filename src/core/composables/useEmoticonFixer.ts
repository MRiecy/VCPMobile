import { invoke } from '@tauri-apps/api/core';

export interface EmoticonItem {
  url: string;
  category: string;
  filename: string;
  searchKey: string;
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

  const initGlobalFixer = () => {
    (window as any).__vcpFixEmoticon = async (img: HTMLImageElement) => {
      // 1. 如果已经尝试修复过且依然报错，则显示破碎图片并停止
      if (img.dataset.vcpFixed === 'true') {
        img.style.visibility = 'visible';
        return;
      }
      
      const originalSrc = img.src;
      
      // 2. 只有包含“表情包”关键词的才尝试修复
      const isEmoticon = decodeURIComponent(originalSrc).includes('表情包');
      if (!isEmoticon) {
        img.style.visibility = 'visible';
        return;
      }
      
      img.dataset.vcpFixed = 'true';
      
      try {
        const fixedUrl = await fixUrl(originalSrc);
        if (fixedUrl && fixedUrl !== originalSrc) {
          // console.log(`[EmoticonFixer] Fixed: ${originalSrc} -> ${fixedUrl}`);
          img.src = fixedUrl; // 这会重新触发加载，如果再次失败会进入上面的 dataset 拦截
        } else {
          // 无法修复，直接显示破碎图标
          img.style.visibility = 'visible';
        }
      } catch (e) {
        console.error('[EmoticonFixer] Error in global fixer:', e);
        img.style.visibility = 'visible';
      }
    };

    // 辅助函数：加载成功后显示
    (window as any).__vcpShowEmoticon = (img: HTMLImageElement) => {
      img.style.visibility = 'visible';
    };
  };

  return {
    fixUrl,
    getLibrary,
    regenerateLibrary,
    initGlobalFixer
  };
}
