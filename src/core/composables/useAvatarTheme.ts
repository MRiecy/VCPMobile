// useAvatarTheme.ts
import { invoke } from '@tauri-apps/api/core';

export function useAvatarTheme() {
  const colorCache = new Map<string, string>();

  const getDominantColor = (imgEl: HTMLImageElement): string => {
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) return '#888888';

    canvas.width = imgEl.naturalWidth || imgEl.width;
    canvas.height = imgEl.naturalHeight || imgEl.height;
    ctx.drawImage(imgEl, 0, 0);

    try {
      const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      let r = 0, g = 0, b = 0;
      const count = imageData.length / 4;

      for (let i = 0; i < imageData.length; i += 4) {
        r += imageData[i];
        g += imageData[i + 1];
        b += imageData[i + 2];
      }

      r = Math.floor(r / count);
      g = Math.floor(g / count);
      b = Math.floor(b / count);

      return `rgb(${r}, ${g}, ${b})`;
    } catch (e) {
      console.warn('Failed to get image data (cross-origin?)', e);
      return '#888888';
    }
  };

  const extractAndSaveColor = async (ownerId: string, avatarUrl: string) => {
    if (colorCache.has(avatarUrl)) return colorCache.get(avatarUrl);

    return new Promise<string>((resolve) => {
      const img = new Image();
      img.crossOrigin = 'anonymous';
      img.src = avatarUrl;
      img.onload = async () => {
        const color = getDominantColor(img);
        colorCache.set(avatarUrl, color);
        
        // 解析 URL 获取 owner_type
        let ownerType = 'agent';
        if (avatarUrl.includes('group/')) ownerType = 'group';
        else if (avatarUrl.includes('user/')) ownerType = 'user';

        // Save to Rust
        try {
          await invoke('save_avatar_color', { ownerType, ownerId, color });
        } catch (e) {
          console.error('Failed to save avatar color to Rust:', e);
        }
        
        resolve(color);
      };
      img.onerror = () => resolve('#888888');
    });
  };

  return {
    extractAndSaveColor
  };
}
