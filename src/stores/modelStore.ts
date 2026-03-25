import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';

export interface ModelInfo {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

export const useModelStore = defineStore('model', {
  state: () => ({
    models: [] as ModelInfo[],
    hotModels: [] as string[],
    favorites: [] as string[],
    isLoading: false,
    lastRefreshed: 0,
  }),

  getters: {
    sortedModels: (state) => {
      // 排序优先级：收藏 > 热门 > 其他
      return [...state.models].sort((a, b) => {
        const aFav = state.favorites.includes(a.id) ? 1 : 0;
        const bFav = state.favorites.includes(b.id) ? 1 : 0;
        if (aFav !== bFav) return bFav - aFav;

        const aHot = state.hotModels.indexOf(a.id);
        const bHot = state.hotModels.indexOf(b.id);
        if (aHot !== -1 && bHot !== -1) return aHot - bHot;
        if (aHot !== -1) return -1;
        if (bHot !== -1) return 1;

        return a.id.localeCompare(b.id);
      });
    },
    isFavorite: (state) => (modelId: string) => state.favorites.includes(modelId),
  },

  actions: {
    async fetchModels(force = false) {
      if (!force && this.models.length > 0 && Date.now() - this.lastRefreshed < 1000 * 60 * 10) {
        return;
      }

      this.isLoading = true;
      try {
        // 先尝试获取缓存
        if (this.models.length === 0) {
          this.models = await invoke<ModelInfo[]>('get_cached_models');
        }
        
        // 只有在明确要求或没缓存时才刷新
        if (force || this.models.length === 0) {
          this.models = await invoke<ModelInfo[]>('refresh_models');
          this.lastRefreshed = Date.now();
        }

        await Promise.all([
          this.fetchHotModels(),
          this.fetchFavorites(),
        ]);
      } catch (error) {
        console.error('Failed to fetch models:', error);
      } finally {
        this.isLoading = false;
      }
    },

    async fetchHotModels() {
      try {
        this.hotModels = await invoke<string[]>('get_hot_models', { limit: 10 });
      } catch (error) {
        console.error('Failed to fetch hot models:', error);
      }
    },

    async fetchFavorites() {
      try {
        this.favorites = await invoke<string[]>('get_favorite_models');
      } catch (error) {
        console.error('Failed to fetch favorite models:', error);
      }
    },

    async toggleFavorite(modelId: string) {
      try {
        const isFav = await invoke<boolean>('toggle_favorite_model', { modelId });
        if (isFav) {
          if (!this.favorites.includes(modelId)) this.favorites.push(modelId);
        } else {
          this.favorites = this.favorites.filter(id => id !== modelId);
        }
      } catch (error) {
        console.error('Failed to toggle favorite:', error);
      }
    },

    async recordUsage(modelId: string) {
      try {
        await invoke('record_model_usage', { modelId });
        // 更新本地热门列表（可选，或者等待下次 fetch）
        this.fetchHotModels();
      } catch (error) {
        console.error('Failed to record usage:', error);
      }
    }
  }
});
