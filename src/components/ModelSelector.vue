<script setup lang="ts">
import { ref, computed, onMounted } from 'vue';
import { useModelStore } from '../stores/modelStore';
import { 
  Search, 
  Star, 
  Flame, 
  X, 
  RefreshCw, 
  Check,
  Cpu
} from 'lucide-vue-next';

defineProps<{
  modelValue: boolean;
  currentModel?: string;
  title?: string;
}>();

const emit = defineEmits(['update:modelValue', 'select']);

const modelStore = useModelStore();
const searchQuery = ref('');

const filteredModels = computed(() => {
  const query = searchQuery.value.toLowerCase().trim();
  if (!query) return modelStore.sortedModels;
  return modelStore.sortedModels.filter(m => 
    m.id.toLowerCase().includes(query) || 
    m.owned_by.toLowerCase().includes(query)
  );
});

const close = () => {
  emit('update:modelValue', false);
};

const selectModel = (modelId: string) => {
  emit('select', modelId);
  close();
};

const toggleFavorite = (e: Event, modelId: string) => {
  e.stopPropagation();
  modelStore.toggleFavorite(modelId);
};

const refresh = () => {
  modelStore.fetchModels(true);
};

onMounted(() => {
  modelStore.fetchModels();
});
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩层 -->
    <Transition name="fade">
      <div v-if="modelValue" 
           class="fixed inset-0 bg-black/50 z-[999] backdrop-blur-[2px]" 
           @click="close"
           @touchmove.prevent>
      </div>
    </Transition>
    
    <!-- 抽屉内容 -->
    <Transition name="slide-up">
      <div v-if="modelValue" 
           class="fixed bottom-0 left-0 right-0 z-[1000] bg-white/95 dark:bg-zinc-900/95 backdrop-blur-2xl rounded-t-3xl shadow-2xl flex flex-col border-t border-black/5 dark:border-white/10 max-h-[85vh] overflow-hidden"
           style="padding-bottom: env(safe-area-inset-bottom, 20px);">
        
        <!-- 顶部拉手条 -->
        <div class="w-12 h-1.5 bg-black/10 dark:bg-white/20 rounded-full mx-auto mt-4 mb-1"></div>

        <!-- 头部区域 -->
        <div class="px-5 pt-3 pb-3 flex items-center justify-between">
          <div class="flex flex-col">
            <h3 class="text-[17px] font-bold text-gray-900 dark:text-zinc-100 tracking-tight">
              {{ title || '选择模型' }}
            </h3>
            <span class="text-[11px] text-gray-500 dark:text-gray-400 mt-0.5">
              共 {{ modelStore.models.length }} 个可用模型
            </span>
          </div>
          <button @click="refresh" 
                  class="p-2 rounded-xl bg-black/5 dark:bg-white/5 active:scale-95 transition-all text-gray-600 dark:text-gray-300"
                  :class="{ 'animate-spin': modelStore.isLoading }">
            <RefreshCw :size="18" />
          </button>
        </div>

        <!-- 搜索框 -->
        <div class="px-5 pb-3">
          <div class="relative group">
            <Search class="absolute left-3.5 top-1/2 -translate-y-1/2 text-gray-400" :size="16" />
            <input 
              v-model="searchQuery"
              type="text" 
              placeholder="搜索模型名称..."
              class="w-full pl-10 pr-4 py-3 bg-black/5 dark:bg-black/20 rounded-2xl text-[15px] outline-none border border-transparent focus:border-blue-500/30 focus:bg-white dark:focus:bg-zinc-800 transition-all placeholder-gray-400"
            />
            <button v-if="searchQuery" @click="searchQuery = ''" class="absolute right-3.5 top-1/2 -translate-y-1/2 text-gray-400">
              <X :size="16" />
            </button>
          </div>
        </div>

        <!-- 模型列表 (桌面端结构的单行列表) -->
        <div class="flex-1 overflow-y-auto px-2 pb-4 space-y-1">
          <div v-if="filteredModels.length === 0" class="py-20 text-center opacity-50">
            <Cpu :size="28" class="mx-auto mb-3 text-gray-400" />
            <p class="text-sm font-medium text-gray-500">未找到匹配的模型</p>
          </div>

          <div v-for="model in filteredModels" :key="model.id" 
               @click="selectModel(model.id)"
               class="relative group px-4 py-3.5 flex items-center gap-3 rounded-2xl active:bg-black/5 dark:active:bg-white/5 transition-colors cursor-pointer"
               :class="{ 'bg-blue-50 dark:bg-blue-500/10': currentModel === model.id }">
            
            <!-- 选中指示器 (左侧细条) -->
            <div class="absolute left-0 top-1/4 bottom-1/4 w-1 bg-blue-500 rounded-r-md transition-all scale-y-0"
                 :class="{ 'scale-y-100': currentModel === model.id }"></div>

            <!-- 模型 ID (高对比) -->
            <div class="flex-1 min-w-0 flex flex-col justify-center">
              <div class="flex items-center gap-2">
                <span class="text-[15px] font-medium tracking-tight truncate text-gray-900 dark:text-zinc-100"
                      :class="{ 'text-blue-600 dark:text-blue-400 font-semibold': currentModel === model.id }">
                  {{ model.id }}
                </span>
                <Flame v-if="modelStore.hotModels.includes(model.id)" :size="14" class="text-orange-500 fill-orange-500/20 shrink-0" />
              </div>
              <div class="flex items-center gap-2 mt-0.5">
                <span class="text-[11px] text-gray-500 dark:text-gray-400">{{ model.owned_by }}</span>
              </div>
            </div>

            <!-- 右侧状态 (星星 + 勾选) -->
            <div class="flex items-center gap-3 shrink-0">
              <button @click="toggleFavorite($event, model.id)" 
                      class="p-2 -mr-2 transition-transform active:scale-75"
                      :class="modelStore.isFavorite(model.id) ? 'text-yellow-500' : 'text-gray-300 dark:text-zinc-600'">
                <Star :size="20" :fill="modelStore.isFavorite(model.id) ? 'currentColor' : 'none'" />
              </button>
              <Check v-if="currentModel === model.id" :size="18" class="text-blue-500" />
            </div>
          </div>
        </div>

        <!-- 底部取消 -->
        <div class="px-5 pt-2 pb-3">
          <button @click="close" 
                  class="w-full py-3.5 rounded-2xl text-[15px] font-medium bg-black/5 dark:bg-white/5 text-gray-600 dark:text-zinc-300 active:scale-[0.98] transition-all">
            取消
          </button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s cubic-bezier(0.16, 1, 0.3, 1);
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.slide-up-enter-active,
.slide-up-leave-active {
  transition: transform 0.35s cubic-bezier(0.16, 1, 0.3, 1);
}
.slide-up-enter-from,
.slide-up-leave-to {
  transform: translateY(100%);
}

.overflow-y-auto {
  scrollbar-width: none;
}
.overflow-y-auto::-webkit-scrollbar {
  display: none;
}
</style>