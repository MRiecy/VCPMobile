import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { reactive } from "vue";

interface AvatarCache {
  blobUrl: string;
  version: number;
}

interface AvatarResult {
  mime_type: string;
  image_data: number[];
  dominant_color: string | null;
  updated_at: number;
}

/**
 * 采用 Canvas 提取图片的主色调 (在前端 WebView 高效执行，100% 避免后台体积和 ffmpeg 权限问题)
 */
const extractDominantColorFromBlob = (blobUrl: string): Promise<string> => {
  return new Promise((resolve) => {
    const img = new Image();
    img.crossOrigin = "Anonymous";
    img.onload = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = 16;
        canvas.height = 16;
        const ctx = canvas.getContext("2d");
        if (!ctx) {
          resolve("#808080");
          return;
        }
        
        ctx.drawImage(img, 0, 0, 16, 16);
        const imgData = ctx.getImageData(0, 0, 16, 16).data;
        
        const colorBuckets = new Map<string, { r: number, g: number, b: number, count: number }>();
        let rSum = 0, gSum = 0, bSum = 0, count = 0;
        
        for (let i = 0; i < imgData.length; i += 4) {
          const r = imgData[i];
          const g = imgData[i + 1];
          const b = imgData[i + 2];
          const a = imgData[i + 3];
          
          if (a < 128) continue; // 忽略透明像素
          
          // 计算亮度与色度以进行过滤
          const max = Math.max(r, g, b);
          const min = Math.min(r, g, b);
          const chroma = max - min;
          
          // 排除纯黑、纯白以及低饱和度的灰色
          if (max < 30 || min > 225 || chroma < 25) {
            continue;
          }
          
          // 512-bin 相似色归纳量化
          const rBin = Math.floor(r / 32);
          const gBin = Math.floor(g / 32);
          const bBin = Math.floor(b / 32);
          const binKey = `${rBin},${gBin},${bBin}`;
          
          const bucket = colorBuckets.get(binKey) || { r: 0, g: 0, b: 0, count: 0 };
          bucket.r += r;
          bucket.g += g;
          bucket.b += b;
          bucket.count++;
          colorBuckets.set(binKey, bucket);
          
          rSum += r;
          gSum += g;
          bSum += b;
          count++;
        }
        
        let bestBucket = null;
        let maxCount = 0;
        for (const bucket of colorBuckets.values()) {
          if (bucket.count > maxCount) {
            maxCount = bucket.count;
            bestBucket = bucket;
          }
        }
        
        if (bestBucket) {
          const r = Math.round(bestBucket.r / bestBucket.count);
          const g = Math.round(bestBucket.g / bestBucket.count);
          const b = Math.round(bestBucket.b / bestBucket.count);
          resolve(`#${((1 << 24) + (r << 16) + (g << 8) + b).toString(16).slice(1)}`);
        } else if (count > 0) {
          const r = Math.round(rSum / count);
          const g = Math.round(gSum / count);
          const b = Math.round(bSum / count);
          resolve(`#${((1 << 24) + (r << 16) + (g << 8) + b).toString(16).slice(1)}`);
        } else {
          resolve("#808080");
        }
      } catch (e) {
        console.error("[AvatarStore] Canvas dominant color computation error:", e);
        resolve("#808080");
      }
    };
    img.onerror = () => {
      resolve("#808080");
    };
    img.src = blobUrl;
  });
};

export const useAvatarStore = defineStore("avatar", () => {
  // 使用 reactive 包装 Map，配合同步访问
  const cache = reactive(new Map<string, AvatarCache>());
  
  // 用于追踪正在进行的请求，防止并发重复请求同一个 ID
  const pending = new Map<string, Promise<string>>();
  // 用于追踪正在进行的 dominant_color 计算，防止重复触发
  const inFlightCompute = new Set<string>();

  // dominant_color 同步缓存，供 computeShell 等同步场景使用
  const dominantColors = reactive(new Map<string, string>());

  /**
   * 获取头像 URL (带自动缓存和版本检查)
   */
  const getAvatarUrl = async (
    ownerType: string, 
    ownerId: string, 
    version: number = 0
  ): Promise<string> => {
    const key = `${ownerType}:${ownerId}`;
    const existing = cache.get(key);

    // 核心修复：如果缓存存在，且满足以下任一条件，则直接返回：
    // 1. 请求的版本为 0 (不强制刷新，只要有就行)
    // 2. 缓存的版本已经大于或等于请求的版本
    if (existing && (version === 0 || existing.version >= version)) {
      return existing.blobUrl;
    }

    // 防止并发重复请求：如果该 ID 已经在加载中，直接返回那个 Promise
    if (pending.has(key)) {
      return pending.get(key)!;
    }

    const fetchTask = (async () => {
      try {
        const result = await invoke<AvatarResult | null>("get_avatar", {
          ownerType,
          ownerId,
        });

        if (result && result.image_data) {
          // Cache dominant_color for synchronous access (e.g. computeShell)
          if (result.dominant_color) {
            dominantColors.set(key, result.dominant_color);
          }
          // 如果 dominant_color 缺失，在前端通过 Canvas 计算并回写到后端数据库
          if (result.dominant_color === null) {
            if (!inFlightCompute.has(key)) {
              inFlightCompute.add(key);
              
              const bytes = new Uint8Array(result.image_data);
              const blob = new Blob([bytes], { type: result.mime_type });
              const tempBlobUrl = URL.createObjectURL(blob);

              extractDominantColorFromBlob(tempBlobUrl)
                .then((color) => {
                  dominantColors.set(key, color);
                  return invoke("store_dominant_color", { ownerType, ownerId, color });
                })
                .then(() => {
                  console.log(`[AvatarStore] Computed and stored dominant_color for ${key}`);
                })
                .catch((err) => {
                  console.error(`[AvatarStore] Failed to handle dominant_color for ${key}:`, err);
                })
                .finally(() => {
                  inFlightCompute.delete(key);
                  URL.revokeObjectURL(tempBlobUrl);
                });
            }
          }

          // 清理旧缓存的物理内存
          if (existing) {
            URL.revokeObjectURL(existing.blobUrl);
          }

          const bytes = new Uint8Array(result.image_data);
          const blob = new Blob([bytes], { type: result.mime_type });
          const blobUrl = URL.createObjectURL(blob);

          // 核心修复：缓存版本号取 (后端实际时间戳 和 请求时间戳) 的最大值
          // 这样确保下次进入时 existing.version >= version 成立，切断死循环
          const MAX_AVATAR_CACHE = 50;
          if (cache.size >= MAX_AVATAR_CACHE) {
            const firstKey = cache.keys().next().value;
            if (firstKey) {
              const old = cache.get(firstKey);
              if (old) URL.revokeObjectURL(old.blobUrl);
              cache.delete(firstKey);
            }
          }
          cache.set(key, { 
            blobUrl, 
            version: Math.max(result.updated_at, version) 
          });
          return blobUrl;
        }
      } catch (err) {
        console.error(`[AvatarStore] Failed to fetch avatar for ${key}:`, err);
      } finally {
        pending.delete(key);
      }
      return "";
    })();

    pending.set(key, fetchTask);
    return fetchTask;
  };

  /**
   * 手动清除特定头像缓存 (强制刷新)
   */
  const clearCache = (ownerType: string, ownerId: string) => {
    const key = `${ownerType}:${ownerId}`;
    const existing = cache.get(key);
    if (existing) {
      URL.revokeObjectURL(existing.blobUrl);
      cache.delete(key);
    }
    dominantColors.delete(key);
  };

  /**
   * 同步获取已缓存的 dominant_color，未缓存时返回 undefined
   */
  const getDominantColor = (ownerType: string, ownerId: string): string | undefined => {
    return dominantColors.get(`${ownerType}:${ownerId}`);
  };

  return {
    cache, // 暴露 cache 以供同步检查
    getAvatarUrl,
    clearCache,
    getDominantColor,
  };
});
