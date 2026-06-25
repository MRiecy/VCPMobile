<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue';
import { useModelStore, type ModelInfo } from '../core/stores/modelStore';
import { useModalHistory } from '../core/composables/useModalHistory';
import {
  Search,
  Star,
  Flame,
  X,
  RefreshCw,
  Check,
  Cpu,
  Zap,
  Loader2,
  Square
} from 'lucide-vue-next';

const props = defineProps<{
  modelValue: boolean;
  currentModel?: string;
  title?: string;
}>();

const emit = defineEmits(['update:modelValue', 'select']);

const modelStore = useModelStore();
const searchQuery = ref('');
const activeTag = ref('全部');

// --- Modal History Shield ---
const { registerModal, unregisterModal } = useModalHistory();
const modalId = 'ModelSelector';

// --- Dynamic Manufacturer Filter ---
const manufacturerTags = computed(() => {
  const tags = ['全部', '收藏', '热门'];
  const counts: Record<string, number> = {};
  
  modelStore.models.forEach((m: ModelInfo) => {
    if (m.owned_by) {
      counts[m.owned_by] = (counts[m.owned_by] || 0) + 1;
    }
  });

  // 按拥有的模型数量降序排列厂商
  const sortedManufacturers = Object.keys(counts).sort((a, b) => counts[b] - counts[a]);
  return [...tags, ...sortedManufacturers];
});

// --- List Filters & Search ---
const filteredModels = computed(() => {
  let list = modelStore.sortedModels;

  // 1. Tag 快捷切片过滤
  if (activeTag.value === '收藏') {
    list = list.filter(m => modelStore.isFavorite(m.id));
  } else if (activeTag.value === '热门') {
    list = list.filter(m => modelStore.hotModels.includes(m.id));
  } else if (activeTag.value !== '全部') {
    list = list.filter(m => m.owned_by === activeTag.value);
  }

  // 2. 搜索关键词匹配
  const query = searchQuery.value.toLowerCase().trim();
  if (!query) return list;
  return list.filter((m: ModelInfo) =>
    m.id.toLowerCase().includes(query) ||
    m.owned_by.toLowerCase().includes(query)
  );
});

// --- Scroll To Top Reference ---
const scrollContainerRef = ref<HTMLElement | null>(null);

const selectTag = (tag: string) => {
  activeTag.value = tag;
  // 切换 Tag 时滚动重置回顶部
  if (scrollContainerRef.value) {
    scrollContainerRef.value.scrollTop = 0;
  }
};

// 搜索词改变时同样重置回顶部
watch(searchQuery, () => {
  if (scrollContainerRef.value) {
    scrollContainerRef.value.scrollTop = 0;
  }
});

// --- Drag Down Gesture Closure Mechanism ---
const sheetRef = ref<HTMLElement | null>(null);
const touchStartY = ref(0);
const currentTranslateY = ref(0);
const isDragging = ref(false);

const onTouchStart = (e: TouchEvent) => {
  touchStartY.value = e.touches[0].clientY;
  isDragging.value = true;
  if (sheetRef.value) {
    sheetRef.value.style.transition = 'none';
  }
};

const onTouchMove = (e: TouchEvent) => {
  if (!isDragging.value) return;
  const clientY = e.touches[0].clientY;
  const deltaY = clientY - touchStartY.value;

  if (deltaY > 0) {
    currentTranslateY.value = deltaY;
    if (sheetRef.value) {
      // 开启 GPU 硬件加速，防止滚动时内部子元素（如取消按钮）的重排闪烁
      sheetRef.value.style.transform = `translate3d(0, ${deltaY}px, 0)`;
      const maskEl = document.querySelector('.bg-black\\/50') as HTMLElement;
      if (maskEl) {
        maskEl.style.opacity = String(Math.max(0, 1 - deltaY / 400));
      }
    }
  }
};

const onTouchEnd = () => {
  if (!isDragging.value) return;
  isDragging.value = false;

  if (sheetRef.value) {
    sheetRef.value.style.transition = 'transform 0.3s cubic-bezier(0.16, 1, 0.3, 1)';
  }

  // 120px 动力学触发关闭阀值
  if (currentTranslateY.value > 120) {
    close();
  } else {
    // 弹回原点
    currentTranslateY.value = 0;
    if (sheetRef.value) {
      sheetRef.value.style.transform = '';
    }
    const maskEl = document.querySelector('.bg-black\\/50') as HTMLElement;
    if (maskEl) {
      maskEl.style.opacity = '';
    }
  }
};

// --- Actions ---
const close = () => {
  // 确保退出时重置位移
  currentTranslateY.value = 0;
  if (sheetRef.value) {
    sheetRef.value.style.transform = '';
  }
  const maskEl = document.querySelector('.bg-black\\/50') as HTMLElement;
  if (maskEl) {
    maskEl.style.opacity = '';
  }
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


// --- Modal History Stack Registration ---
watch(() => props.modelValue, (newVal) => {
  if (newVal) {
    registerModal(modalId, close);
    // 🌟 5分钟检测：如果为空，或者距离上次同步超过 5 分钟，直接触发强力同步（与点击刷新按钮完全等价，自动播放转圈并展示 Toast）
    const shouldRefresh = modelStore.models.length === 0 || (Date.now() - modelStore.lastRefreshed > 1000 * 60 * 5);
    if (shouldRefresh) {
      modelStore.fetchModels(true);
    }
  } else {
    modelStore.stopTestAll(); // 抽屉关闭时，自动停止后台未完成的所有测试，清理垃圾状态以防残留 0.0s
    unregisterModal(modalId);
  }
}, { immediate: true });

// --- Latency Color Classification ---
const getLatencyClass = (modelId: string) => {
  const res = modelStore.testResults[modelId];
  if (!res) return 'text-gray-400 dark:text-zinc-600';
  if (res.status === 'testing') return 'text-blue-500 dark:text-blue-400';
  if (res.status === 'failed') return 'text-red-500 dark:text-red-400';

  const latency = res.latency || 0;
  if (latency < 1500) return 'text-emerald-600 dark:text-emerald-400 font-medium';
  if (latency < 5000) return 'text-amber-600 dark:text-amber-400 font-medium';
  return 'text-orange-600 dark:text-orange-400 font-medium';
};

const getLatencyDotClass = (modelId: string) => {
  const res = modelStore.testResults[modelId];
  if (!res || res.status !== 'success') return 'bg-gray-400';

  const latency = res.latency || 0;
  if (latency < 1500) return 'bg-emerald-500';
  if (latency < 5000) return 'bg-amber-500';
  return 'bg-orange-500';
};

const getTestResultText = (modelId: string) => {
  const res = modelStore.testResults[modelId];
  if (!res) return '';
  if (res.status === 'testing') return '测试中...';
  if (res.status === 'failed') {
    const err = res.error || '';
    const errLower = err.toLowerCase();
    
    // 优先检测超时错误，避免被归类为“网络异常”
    if (
      errLower.includes("timeout") || 
      errLower.includes("timed out") || 
      errLower.includes("timedout") || 
      err.includes("超时")
    ) {
      return '连接超时 (60s)';
    }

    if (err.includes("401")) return '401 鉴权失败';
    if (err.includes("403")) return '403 拒绝访问';
    if (err.includes("404")) return '404 模型缺失';
    if (err.includes("429")) return '429 频控限流';
    if (err.includes("500") || err.includes("502") || err.includes("503") || err.includes("504")) return '5xx 服务端错误';
    if (errLower.includes("connect") || err.includes("连接") || errLower.includes("reach")) return '网络异常';
    return '连接失败';
  }
  const sec = (res.latency || 0) / 1000;
  return `${sec.toFixed(1)}s`;
};

// --- Test All Filtered Models ---
const testAllFiltered = () => {
  const ids = filteredModels.value.map(m => m.id);
  modelStore.testAllModels(ids);
};

onUnmounted(() => {
  modelStore.stopTestAll(); // 组件卸载时，自动清理后台未完成的测试，规避内存与网络泄漏
  unregisterModal(modalId);
});
</script>

<template>
  <Teleport to="body">
    <!-- 遮罩层 -->
    <Transition name="fade">
      <div v-if="modelValue" class="fixed inset-0 bg-black/50 z-sheet" @click="close"
        @touchmove.prevent>
      </div>
    </Transition>

    <!-- 抽屉内容 -->
    <Transition name="slide-up">
      <div v-if="modelValue"
        ref="sheetRef"
        class="fixed bottom-0 left-0 right-0 z-sheet bg-white/95 dark:bg-zinc-900/95 rounded-t-3xl shadow-2xl flex flex-col border-t border-black/5 dark:border-white/10 max-h-[85vh] overflow-hidden select-none no-rubber-band"
        style="padding-bottom: calc(var(--vcp-safe-bottom, 48px) + 12px);"
        :class="{ 'transition-transform duration-300': !isDragging }">

        <!-- 顶部拉手条及头部 (支持手势下拉) -->
        <div class="w-full shrink-0"
          @touchstart="onTouchStart"
          @touchmove.prevent="onTouchMove"
          @touchend="onTouchEnd"
          @touchcancel="onTouchEnd">
          <div class="w-12 h-1.5 bg-black/10 dark:bg-white/20 rounded-full mx-auto mt-4 mb-1 cursor-grab active:cursor-grabbing"></div>
          
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
            <div class="flex items-center gap-2">
              <!-- 停止测试按钮 (红色醒目) -->
              <button v-if="modelStore.isTestingAll" @click="modelStore.stopTestAll"
                class="px-3 py-1.5 rounded-xl bg-red-500 text-white text-[11px] font-bold active:scale-95 transition-all flex items-center gap-1 shadow-md shadow-red-500/20 shrink-0 animate-pulse"
                title="停止当前测试">
                <Square :size="10" class="fill-current shrink-0" />
                <span>停止测试</span>
              </button>
              <!-- 开启批量测试按钮 -->
              <button v-else @click="testAllFiltered"
                class="p-2 rounded-xl bg-black/5 dark:bg-white/5 active:scale-95 transition-all text-gray-600 dark:text-gray-300 shrink-0"
                title="测试当前列表中模型的延迟">
                <Zap :size="18" />
              </button>
              <button @click="refresh"
                class="p-2 rounded-xl bg-black/5 dark:bg-white/5 active:scale-95 transition-all text-gray-600 dark:text-gray-300"
                :class="{ 'animate-spin will-change-transform': modelStore.isLoading }"
                title="同步模型列表">
                <RefreshCw :size="18" />
              </button>
            </div>
          </div>
        </div>

        <!-- 搜索框 -->
        <div class="px-5 pb-2">
          <div class="relative group">
            <Search class="absolute left-3.5 top-1/2 -translate-y-1/2 text-gray-400" :size="16" />
            <input v-model="searchQuery" type="text" placeholder="搜索模型名称..."
              class="w-full pl-10 pr-4 py-3 bg-black/5 dark:bg-black/20 rounded-2xl text-[15px] outline-none border border-transparent focus:border-blue-500/30 focus:bg-white dark:focus:bg-zinc-800 transition-all placeholder-gray-400" />
            <button v-if="searchQuery" @click="searchQuery = ''"
              class="absolute right-3.5 top-1/2 -translate-y-1/2 text-gray-400">
              <X :size="16" />
            </button>
          </div>
        </div>

        <!-- 厂商快捷分类 Tags (横向惯性滚动栏) -->
        <div class="px-5 pb-3">
          <div class="flex items-center gap-1.5 overflow-x-auto no-scrollbar scroll-smooth py-1 -mx-5 px-5">
            <button v-for="tag in manufacturerTags" :key="tag" @click="selectTag(tag)"
              class="px-3.5 py-1.5 rounded-full text-xs font-semibold whitespace-nowrap transition-all duration-200"
              :class="activeTag === tag
                ? 'bg-blue-500 text-white shadow-md shadow-blue-500/20 scale-105'
                : 'bg-black/5 dark:bg-white/5 text-gray-600 dark:text-zinc-400 active:scale-95'">
              {{ tag }}
            </button>
          </div>
        </div>

        <!-- 模型列表 (GPU 物理滚动列表) -->
        <div 
          ref="scrollContainerRef"
          class="flex-1 overflow-y-auto px-2 pb-4 no-rubber-band vcp-scrollable"
        >
          <!-- 骨架屏 -->
          <div v-if="modelStore.isLoading && filteredModels.length === 0" class="px-2 py-1 space-y-2">
            <div v-for="i in 5" :key="i"
              class="relative overflow-hidden flex items-center gap-3 px-4 py-3.5 rounded-2xl border border-black/5 dark:border-white/5 bg-gray-50/30 dark:bg-zinc-800/10">
              <div class="w-1 h-6 bg-gray-200 dark:bg-zinc-800 rounded-r-md"></div>
              <div class="flex-1 space-y-2">
                <div class="h-4 w-1/2 bg-gray-200 dark:bg-zinc-800 rounded-md shimmer-bar"></div>
                <div class="h-3 w-1/4 bg-gray-150 dark:bg-zinc-850 rounded-md shimmer-bar"></div>
              </div>
              <div class="w-8 h-8 rounded-full bg-gray-200 dark:bg-zinc-800 shimmer-bar shrink-0"></div>
            </div>
          </div>

          <!-- 兜底屏 -->
          <div v-else-if="filteredModels.length === 0" class="py-20 text-center opacity-50">
            <Cpu :size="28" class="mx-auto mb-3 text-gray-400" />
            <p class="text-sm font-medium text-gray-500">未找到匹配的模型</p>
          </div>

          <!-- 实际渲染的模型列表 -->
          <div v-else class="space-y-1">
            <div v-for="item in filteredModels" :key="item.id" @click="selectModel(item.id)"
              class="relative group px-4 py-3.5 flex items-center gap-3 rounded-2xl active:bg-black/5 dark:active:bg-white/5 transition-colors cursor-pointer"
              :class="{ 'bg-blue-50 dark:bg-blue-500/10': currentModel === item.id }"
              style="height: 62px; box-sizing: border-box; content-visibility: auto; contain-intrinsic-size: 62px;"
            >
              <div class="absolute left-0 top-1/4 bottom-1/4 w-1 bg-blue-500 rounded-r-md transition-all scale-y-0"
                :class="{ 'scale-y-100': currentModel === item.id }"></div>

              <div class="flex-1 min-w-0 flex flex-col justify-center">
                <div class="flex items-center gap-2">
                  <span class="text-[15px] font-medium tracking-tight truncate text-gray-900 dark:text-zinc-100"
                    :class="{ 'text-blue-600 dark:text-blue-400 font-semibold': currentModel === item.id }">
                    {{ item.id }}
                  </span>
                  <Flame v-if="modelStore.hotModels.includes(item.id)" :size="14"
                    class="text-orange-500 fill-orange-500/20 shrink-0" />
                </div>
                <div class="flex items-center gap-2 mt-0.5">
                  <span class="text-[11px] text-gray-500 dark:text-gray-400 shrink-0">{{ item.owned_by }}</span>
                  <template v-if="modelStore.testResults[item.id]">
                    <span class="text-[10px] text-gray-300 dark:text-zinc-700 shrink-0">•</span>
                    <span 
                      class="text-[11px] flex items-center gap-1 min-w-0" 
                      :class="getLatencyClass(item.id)"
                    >
                      <span v-if="modelStore.testResults[item.id].status === 'testing'" class="w-1 h-1 rounded-full bg-blue-500 shrink-0"></span>
                      <span v-else-if="modelStore.testResults[item.id].status === 'success'" class="w-1 h-1 rounded-full shrink-0" :class="getLatencyDotClass(item.id)"></span>
                      <span v-else-if="modelStore.testResults[item.id].status === 'failed'" class="w-1 h-1 rounded-full bg-red-500 shrink-0"></span>
                      <span class="truncate">{{ getTestResultText(item.id) }}</span>
                    </span>
                  </template>
                </div>
              </div>

              <div class="flex items-center gap-2.5 shrink-0">
                <!-- Connectivity test action -->
                <button 
                  v-if="!modelStore.testResults[item.id]"
                  @click.stop="modelStore.testModel(item.id)"
                  class="p-2 -mr-1 text-gray-400 hover:text-blue-500 dark:text-zinc-600 dark:hover:text-blue-400 active:scale-75 transition-transform shrink-0"
                  title="测试延迟"
                >
                  <Zap :size="16" />
                </button>
                <button 
                  v-else-if="modelStore.testResults[item.id].status === 'testing'"
                  class="p-2 -mr-1 text-blue-500 animate-spin will-change-transform shrink-0"
                >
                  <Loader2 :size="16" />
                </button>
                <button 
                  v-else
                  @click.stop="modelStore.testModel(item.id)"
                  class="p-2 -mr-1 active:scale-75 transition-transform shrink-0"
                  :class="getLatencyClass(item.id)"
                  :title="modelStore.testResults[item.id].status === 'failed' ? modelStore.testResults[item.id].error : '重新测试'"
                >
                  <RefreshCw :size="14" />
                </button>

                <button @click="toggleFavorite($event, item.id)" class="p-2 -mr-2 transition-transform active:scale-75 shrink-0"
                  :class="modelStore.isFavorite(item.id) ? 'text-yellow-500' : 'text-gray-300 dark:text-zinc-600'">
                  <Star :size="20" :fill="modelStore.isFavorite(item.id) ? 'currentColor' : 'none'" />
                </button>
                <Check v-if="currentModel === item.id" :size="18" class="text-blue-500 shrink-0" />
              </div>
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
  transition: opacity 0.25s cubic-bezier(0.16, 1, 0.3, 1);
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

.no-scrollbar {
  scrollbar-width: none;
}
.no-scrollbar::-webkit-scrollbar {
  display: none;
}


/* 🌟 高质感 Shimmer 拂过扫光动画 🌟 */
.shimmer-bar {
  position: relative;
  overflow: hidden;
}

.shimmer-bar::after {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  left: 0;
  transform: translateX(-100%);
  background-image: linear-gradient(
    90deg,
    rgba(255, 255, 255, 0) 0%,
    rgba(255, 255, 255, 0.25) 30%,
    rgba(255, 255, 255, 0.5) 60%,
    rgba(255, 255, 255, 0) 100%
  );
  animation: vcp-shimmer 1.5s infinite ease-in-out;
  content: '';
}

@media (prefers-color-scheme: dark) {
  .shimmer-bar::after {
    background-image: linear-gradient(
      90deg,
      rgba(255, 255, 255, 0) 0%,
      rgba(255, 255, 255, 0.05) 30%,
      rgba(255, 255, 255, 0.12) 60%,
      rgba(255, 255, 255, 0) 100%
    );
  }
}

@keyframes vcp-shimmer {
  100% {
    transform: translateX(100%);
  }
}
</style>