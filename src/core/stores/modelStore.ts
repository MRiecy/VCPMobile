import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';

export interface ModelInfo {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

export const useModelStore = defineStore('model', () => {
  // --- State ---
  const models = ref<ModelInfo[]>([]);
  const hotModels = ref<string[]>([]);
  const favorites = ref<string[]>([]);
  const isLoading = ref(false);
  const lastRefreshed = ref(0);

  // --- Getters ---
  const sortedModels = computed(() => {
    // 排序优先级：收藏 > 热门 > 其他
    return [...models.value].sort((a, b) => {
      const aFav = favorites.value.includes(a.id) ? 1 : 0;
      const bFav = favorites.value.includes(b.id) ? 1 : 0;
      if (aFav !== bFav) return bFav - aFav;

      const aHot = hotModels.value.indexOf(a.id);
      const bHot = hotModels.value.indexOf(b.id);
      if (aHot !== -1 && bHot !== -1) return aHot - bHot;
      if (aHot !== -1) return -1;
      if (bHot !== -1) return 1;

      return a.id.localeCompare(b.id);
    });
  });

  const isFavorite = computed(() => (modelId: string) => favorites.value.includes(modelId));

  // --- Actions ---
  const fetchModels = async (force = false) => {
    if (!force && models.value.length > 0 && Date.now() - lastRefreshed.value < 1000 * 60 * 10) {
      return;
    }

    isLoading.value = true;
    try {
      // 先尝试获取缓存
      if (models.value.length === 0) {
        models.value = await invoke<ModelInfo[]>('get_cached_models');
      }

      // 只有在明确要求或没缓存时才刷新
      if (force || models.value.length === 0) {
        models.value = await invoke<ModelInfo[]>('refresh_models');
        lastRefreshed.value = Date.now();
      }

      await Promise.all([
        fetchHotModels(),
        fetchFavorites(),
      ]);
    } catch (error) {
      console.error('Failed to fetch models:', error);
    } finally {
      isLoading.value = false;
    }
  };

  const fetchHotModels = async () => {
    try {
      hotModels.value = await invoke<string[]>('get_hot_models', { limit: 10 });
    } catch (error) {
      console.error('Failed to fetch hot models:', error);
    }
  };

  const fetchFavorites = async () => {
    try {
      favorites.value = await invoke<string[]>('get_favorite_models');
    } catch (error) {
      console.error('Failed to fetch favorite models:', error);
    }
  };

  const toggleFavorite = async (modelId: string) => {
    try {
      const isFav = await invoke<boolean>('toggle_favorite_model', { modelId });
      if (isFav) {
        if (!favorites.value.includes(modelId)) favorites.value.push(modelId);
      } else {
        favorites.value = favorites.value.filter(id => id !== modelId);
      }
    } catch (error) {
      console.error('Failed to toggle favorite:', error);
    }
  };

  const recordUsage = async (modelId: string) => {
    try {
      await invoke('record_model_usage', { modelId });
      // 更新本地热门列表（可选，或者等待下次 fetch）
      fetchHotModels();
    } catch (error) {
      console.error('Failed to record usage:', error);
    }
  };

  return {
    models,
    hotModels,
    favorites,
    isLoading,
    lastRefreshed,
    sortedModels,
    isFavorite,
    fetchModels,
    fetchHotModels,
    fetchFavorites,
    toggleFavorite,
    recordUsage,
  };
});
