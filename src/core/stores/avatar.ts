import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { reactive } from "vue";

interface AvatarCache {
  blobUrl: string;
  version: number;
  avatarHash: string;
}

interface AvatarResult {
  avatar_hash: string;
  mime_type: string;
  image_data: number[];
  dominant_color: string | null;
  updated_at: number;
}

interface AvatarMetadataDto {
  ownerType: string;
  ownerId: string;
  avatarHash: string;
  dominantColor: string | null;
  updatedAt: number;
}

interface AvatarMetadata {
  avatarHash: string;
  updatedAt: number;
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
  const MAX_AVATAR_CACHE = 50;
  const MAX_CONCURRENT_AVATAR_READS = 2;

  // 使用 reactive 包装 Map，配合同步访问
  const cache = reactive(new Map<string, AvatarCache>());
  // 启动/同步只预取轻量元数据，头像二进制由可视区组件按需读取。
  const metadata = reactive(new Map<string, AvatarMetadata>());
  const cacheRecency = new Map<string, number>();
  let cacheAccessSequence = 0;
  let metadataLoadId = 0;
  
  // 用于追踪正在进行的请求，防止并发重复请求同一个 ID
  const pending = new Map<string, Promise<string>>();
  // 每个头像的本地代际；保存/失效后，旧请求不得把旧 Blob 回灌缓存。
  const generations = new Map<string, number>();
  // 用于追踪正在进行的 dominant_color 计算，防止重复触发
  const inFlightCompute = new Set<string>();
  const avatarReadWaiters: Array<() => void> = [];
  let activeAvatarReads = 0;

  // dominant_color 同步缓存，供 computeShell 等同步场景使用
  const dominantColors = reactive(new Map<string, string>());

  const acquireAvatarReadSlot = (): Promise<void> => {
    if (activeAvatarReads < MAX_CONCURRENT_AVATAR_READS) {
      activeAvatarReads += 1;
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      avatarReadWaiters.push(resolve);
    });
  };

  const releaseAvatarReadSlot = () => {
    const next = avatarReadWaiters.shift();
    if (next) {
      next();
      return;
    }
    activeAvatarReads = Math.max(0, activeAvatarReads - 1);
  };

  const touchCache = (key: string) => {
    cacheRecency.set(key, ++cacheAccessSequence);
  };

  const revokeCachedAvatar = (key: string) => {
    const existing = cache.get(key);
    if (existing) {
      URL.revokeObjectURL(existing.blobUrl);
      cache.delete(key);
    }
    cacheRecency.delete(key);
  };

  const advanceGeneration = (key: string) => {
    generations.set(key, (generations.get(key) ?? 0) + 1);
    pending.delete(key);
  };

  const invalidateAvatarKey = (key: string, clearMetadata: boolean) => {
    advanceGeneration(key);
    revokeCachedAvatar(key);
    dominantColors.delete(key);
    if (clearMetadata) metadata.delete(key);
  };

  const evictCacheOverflow = () => {
    while (cache.size > MAX_AVATAR_CACHE) {
      let oldestKey: string | undefined;
      let oldestAccess = Number.POSITIVE_INFINITY;
      for (const key of cache.keys()) {
        const access = cacheRecency.get(key) ?? 0;
        if (access < oldestAccess) {
          oldestAccess = access;
          oldestKey = key;
        }
      }
      if (!oldestKey) break;
      revokeCachedAvatar(oldestKey);
    }
  };

  const setCachedAvatar = (
    key: string,
    blobUrl: string,
    version: number,
    avatarHash: string,
  ) => {
    revokeCachedAvatar(key);
    cache.set(key, { blobUrl, version, avatarHash });
    touchCache(key);
    evictCacheOverflow();
  };

  const getCachedAvatar = (
    ownerType: string,
    ownerId: string,
    version: number = 0,
  ): AvatarCache | undefined => {
    const key = `${ownerType}:${ownerId}`;
    const existing = cache.get(key);
    if (!existing) return undefined;

    const knownMetadata = metadata.get(key);
    if (knownMetadata && knownMetadata.avatarHash !== existing.avatarHash) {
      revokeCachedAvatar(key);
      return undefined;
    }
    if (version !== 0 && existing.version < version) return undefined;
    touchCache(key);
    return existing;
  };

  const scheduleDominantColor = (
    key: string,
    ownerType: string,
    ownerId: string,
    avatarHash: string,
    requestGeneration: number,
    blob: Blob,
  ) => {
    const computeKey = `${key}:${avatarHash}`;
    if (inFlightCompute.has(computeKey)) return;
    inFlightCompute.add(computeKey);

    const tempBlobUrl = URL.createObjectURL(blob);
    extractDominantColorFromBlob(tempBlobUrl)
      .then(async (color) => {
        if ((generations.get(key) ?? 0) !== requestGeneration) return false;
        const stored = await invoke<boolean>("store_dominant_color", {
          ownerType,
          ownerId,
          color,
          expectedAvatarHash: avatarHash,
        });
        if (stored && (generations.get(key) ?? 0) === requestGeneration) {
          dominantColors.set(key, color);
        }
        return stored;
      })
      .then((stored) => {
        if (stored) {
          console.log(`[AvatarStore] Computed and stored dominant_color for ${key}`);
        }
      })
      .catch((err) => {
        console.error(`[AvatarStore] Failed to handle dominant_color for ${key}:`, err);
      })
      .finally(() => {
        inFlightCompute.delete(computeKey);
        URL.revokeObjectURL(tempBlobUrl);
      });
  };

  /**
   * 获取头像 URL (带自动缓存和版本检查)
   */
  const getAvatarUrl = async (
    ownerType: string, 
    ownerId: string, 
    version: number = 0
  ): Promise<string> => {
    const key = `${ownerType}:${ownerId}`;
    const existing = getCachedAvatar(ownerType, ownerId, version);
    if (existing) {
      return existing.blobUrl;
    }

    // 防止并发重复请求：如果该 ID 已经在加载中，直接返回那个 Promise
    if (pending.has(key)) {
      return pending.get(key)!;
    }

    const requestGeneration = generations.get(key) ?? 0;
    const fetchTask = (async () => {
      await acquireAvatarReadSlot();
      try {
        if ((generations.get(key) ?? 0) !== requestGeneration) {
          return cache.get(key)?.blobUrl ?? "";
        }

        const result = await invoke<AvatarResult | null>("get_avatar", {
          ownerType,
          ownerId,
        });

        if ((generations.get(key) ?? 0) !== requestGeneration) {
          return cache.get(key)?.blobUrl ?? "";
        }

        if (result && result.image_data.length > 0) {
          const currentMetadata = metadata.get(key);
          let resolvedGeneration = requestGeneration;
          if (currentMetadata && currentMetadata.avatarHash !== result.avatar_hash) {
            invalidateAvatarKey(key, false);
            resolvedGeneration = generations.get(key) ?? requestGeneration;
          }
          metadata.set(key, {
            avatarHash: result.avatar_hash,
            updatedAt: result.updated_at,
          });

          // Cache dominant_color for synchronous access (e.g. computeShell)
          if (result.dominant_color) {
            dominantColors.set(key, result.dominant_color);
          }

          const bytes = new Uint8Array(result.image_data);
          const blob = new Blob([bytes], { type: result.mime_type });
          const blobUrl = URL.createObjectURL(blob);

          setCachedAvatar(
            key,
            blobUrl,
            Math.max(result.updated_at, version),
            result.avatar_hash,
          );
          if (result.dominant_color === null) {
            scheduleDominantColor(
              key,
              ownerType,
              ownerId,
              result.avatar_hash,
              resolvedGeneration,
              blob,
            );
          }
          return blobUrl;
        }

        invalidateAvatarKey(key, true);
      } catch (err) {
        console.error(`[AvatarStore] Failed to fetch avatar for ${key}:`, err);
      } finally {
        releaseAvatarReadSlot();
      }
      return "";
    })();

    pending.set(key, fetchTask);
    void fetchTask.finally(() => {
      if (pending.get(key) === fetchTask) pending.delete(key);
    });
    return fetchTask;
  };

  /**
   * 手动清除特定头像缓存 (强制刷新)
   */
  const clearCache = (ownerType: string, ownerId: string) => {
    const key = `${ownerType}:${ownerId}`;
    invalidateAvatarKey(key, true);
  };

  const refreshAvatar = (
    ownerType: string,
    ownerId: string,
    avatarHash: string,
  ): Promise<string> => {
    const key = `${ownerType}:${ownerId}`;
    clearCache(ownerType, ownerId);
    metadata.set(key, { avatarHash, updatedAt: Date.now() });
    return getAvatarUrl(ownerType, ownerId, Date.now());
  };

  const getDominantColor = (ownerType: string, ownerId: string): string | undefined => {
    return dominantColors.get(`${ownerType}:${ownerId}`);
  };

  const loadMetadata = async (): Promise<void> => {
    const loadId = ++metadataLoadId;
    const generationSnapshot = new Map(generations);
    const startTime = Date.now();
    try {
      const results = (await invoke<AvatarMetadataDto[]>("batch_get_avatars")) ?? [];
      if (loadId !== metadataLoadId) return;

      const incomingKeys = new Set<string>();
      for (const item of results) {
        const key = `${item.ownerType}:${item.ownerId}`;
        incomingKeys.add(key);
        if ((generations.get(key) ?? 0) !== (generationSnapshot.get(key) ?? 0)) {
          continue;
        }

        const existingMetadata = metadata.get(key);
        if (existingMetadata && existingMetadata.updatedAt > item.updatedAt) {
          continue;
        }
        const existingCache = cache.get(key);
        const pendingReadNeedsInvalidation = pending.has(key) && (
          !existingMetadata ||
          existingMetadata.avatarHash !== item.avatarHash ||
          existingMetadata.updatedAt < item.updatedAt
        );
        if (
          pendingReadNeedsInvalidation ||
          (existingMetadata && existingMetadata.avatarHash !== item.avatarHash) ||
          (existingCache && existingCache.avatarHash !== item.avatarHash)
        ) {
          invalidateAvatarKey(key, false);
        }

        metadata.set(key, {
          avatarHash: item.avatarHash,
          updatedAt: item.updatedAt,
        });
        if (item.dominantColor) {
          dominantColors.set(key, item.dominantColor);
        } else {
          dominantColors.delete(key);
        }
      }

      const knownKeys = new Set([
        ...metadata.keys(),
        ...cache.keys(),
        ...pending.keys(),
      ]);
      for (const key of knownKeys) {
        if (incomingKeys.has(key)) continue;
        if ((generations.get(key) ?? 0) !== (generationSnapshot.get(key) ?? 0)) {
          continue;
        }
        invalidateAvatarKey(key, true);
      }

      console.log(`[AvatarStore] Loaded ${results.length} avatar metadata rows in ${Date.now() - startTime}ms.`);
    } catch (err) {
      console.error("[AvatarStore] Failed to load avatar metadata:", err);
    }
  };

  const preloadMetadata = (): Promise<void> => loadMetadata();
  const refreshMetadata = (): Promise<void> => loadMetadata();

  return {
    cache, // 暴露 cache 以供同步检查
    metadata,
    getCachedAvatar,
    getAvatarUrl,
    clearCache,
    refreshAvatar,
    getDominantColor,
    preloadMetadata,
    refreshMetadata,
  };
});
