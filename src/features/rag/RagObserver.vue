<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue';
import { 
  X, Trash2, ChevronDown, ChevronUp, Copy, Loader2, Sparkles, Check,
  Brain, Clock, BookOpen, Moon, ShieldAlert, Calendar,
  Share2, MessageSquare, History, Search, Bookmark
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import { useRagObserverStore } from '../../core/stores/ragObserver';
import { useThemeStore } from '../../core/stores/theme';
import { marked } from 'marked';

const themeStore = useThemeStore();

// 配置 marked：支持 GFM 和换行
marked.setOptions({
  gfm: true,
  breaks: true,
});

interface Props {
  isOpen: boolean;
  zIndex?: number;
}

const props = defineProps<Props>();
const emit = defineEmits(['close']);

const store = useRagObserverStore();

const activeFilter = ref<'all' | 'rag' | 'chain' | 'chat' | 'memo' | 'dream'>('all');
const expandedCardIds = ref<Set<string>>(new Set());
const expandedSubCardIds = ref<Set<string>>(new Set());
const payloadCache = ref<Record<string, any>>({});
const payloadLoading = ref<Record<string, boolean>>({});
const copiedCardId = ref<string | null>(null);
const cardRefs = ref<Record<string, any>>({});
const subCardRefs = ref<Record<string, any>>({});

const toggleSubCard = async (subId: string) => {
  const isCollapsing = expandedSubCardIds.value.has(subId);
  if (isCollapsing) {
    expandedSubCardIds.value.delete(subId);
    await nextTick();
    const subCardEl = subCardRefs.value[subId];
    if (subCardEl) {
      subCardEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  } else {
    expandedSubCardIds.value.add(subId);
  }
};

// 左右滑动切换 Tab 手势逻辑 (限制偏角在 +-25 度以内)
let touchStartX = 0;
let touchStartY = 0;
let isSwipeProcessed = false;

const handleTouchStart = (e: TouchEvent) => {
  if (e.touches.length > 0) {
    touchStartX = e.touches[0].clientX;
    touchStartY = e.touches[0].clientY;
    isSwipeProcessed = false;
  }
};

const handleTouchMove = (e: TouchEvent) => {
  if (isSwipeProcessed || e.touches.length === 0) return;

  const currentX = e.touches[0].clientX;
  const currentY = e.touches[0].clientY;

  const deltaX = currentX - touchStartX;
  const deltaY = currentY - touchStartY;

  const absX = Math.abs(deltaX);
  const absY = Math.abs(deltaY);

  // 距离阈值：水平方向滑动超过 50 像素，且限制偏角在 +-25 度以内 (tan(25) = 0.4663)
  if (absX > 50 && absY <= absX * 0.4663) {
    isSwipeProcessed = true; // 确保单次滑动只触发一次翻页
    const currentIdx = filterTabs.findIndex(tab => tab.value === activeFilter.value);
    if (currentIdx !== -1) {
      if (deltaX > 0) {
        // 向右滑，切换至上一个选项卡
        if (currentIdx > 0) {
          activeFilter.value = filterTabs[currentIdx - 1].value;
        }
      } else {
        // 向左滑，切换至下一个选项卡
        if (currentIdx < filterTabs.length - 1) {
          activeFilter.value = filterTabs[currentIdx + 1].value;
        }
      }
    }
  }
};

const handleTouchEnd = (e: TouchEvent) => {
  if (isSwipeProcessed) return;
  if (e.changedTouches.length > 0) {
    const deltaX = e.changedTouches[0].clientX - touchStartX;
    const deltaY = e.changedTouches[0].clientY - touchStartY;
    
    const absX = Math.abs(deltaX);
    const absY = Math.abs(deltaY);
    
    if (absX > 50 && absY <= absX * 0.4663) {
      isSwipeProcessed = true;
      const currentIdx = filterTabs.findIndex(tab => tab.value === activeFilter.value);
      if (currentIdx !== -1) {
        if (deltaX > 0) {
          if (currentIdx > 0) {
            activeFilter.value = filterTabs[currentIdx - 1].value;
          }
        } else {
          if (currentIdx < filterTabs.length - 1) {
            activeFilter.value = filterTabs[currentIdx + 1].value;
          }
        }
      }
    }
  }
};

// 频谱 Canvas 绘图相关
const spectrumCanvas = ref<HTMLCanvasElement | null>(null);
let animationFrameId: number | null = null;
const numBars = 24;
let barsHeights = Array(numBars).fill(4);

// 选项卡列表
const filterTabs = [
  { value: 'all', label: '全部' },
  { value: 'rag', label: 'RAG知识库' },
  { value: 'chain', label: '元思考链' },
  { value: 'chat', label: 'Agent会话' },
  { value: 'memo', label: '记忆检索' },
  { value: 'dream', label: 'Agent梦境' }
] as const;

// 监听是否打开，挂载和注销 Tauri WebSocket 监听
watch(() => props.isOpen, (isOpen) => {
  if (isOpen) {
    store.initListener();
    drawSpectrum();
  } else {
    store.destroyListener();
    if (animationFrameId) {
      cancelAnimationFrame(animationFrameId);
      animationFrameId = null;
    }
  }
});

onMounted(() => {
  if (props.isOpen) {
    store.initListener();
    drawSpectrum();
  }
});

onUnmounted(() => {
  store.destroyListener();
  if (animationFrameId) {
    cancelAnimationFrame(animationFrameId);
  }
});

// 根据 Filter 过滤列表
const filteredMetadataList = computed(() => {
  if (activeFilter.value === 'all') {
    return store.metadataList;
  }
  return store.metadataList.filter((m) => {
    const type = m.type;
    switch (activeFilter.value) {
      case 'rag':
        // RAG 检索没有固定 type 名字，但 hasDetails && type !== 其他类型，或者是空
        return type === '' || type === 'RAG_RETRIEVAL_DETAILS';
      case 'chain':
        return type === 'META_THINKING_CHAIN';
      case 'chat':
        return type === 'AGENT_PRIVATE_CHAT_PREVIEW';
      case 'memo':
        return type === 'AI_MEMO_RETRIEVAL' || type === 'DailyNote';
      case 'dream':
        return type.startsWith('AGENT_DREAM_');
      default:
        return false;
    }
  });
});

// 计算状态小点的样式
const statusDotClass = computed(() => {
  switch (store.connectionStatus) {
    case 'connecting': return 'bg-yellow-400 animate-pulse';
    case 'connected': return 'bg-green-400';
    case 'error': return 'bg-red-400';
    default: return 'bg-gray-500';
  }
});

const statusLabel = computed(() => {
  switch (store.connectionStatus) {
    case 'connecting': return '连接中';
    case 'connected': return '已连接';
    case 'error': return '连接异常';
    default: return '未连接';
  }
});

// 折叠/展开卡片
const toggleCard = async (id: string) => {
  const isCollapsing = expandedCardIds.value.has(id);
  
  if (isCollapsing) {
    expandedCardIds.value.delete(id);
    delete payloadCache.value[id]; // 折叠时自动清理 JS 内存中该项的饱水 Payload
    await nextTick();
    const cardEl = cardRefs.value[id];
    if (cardEl) {
      cardEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  } else {
    expandedCardIds.value.add(id);
    // 按需拉取详情
    if (!payloadCache.value[id] && !payloadLoading.value[id]) {
      payloadLoading.value[id] = true;
      try {
        const payload = await store.fetchPayload(id);
        payloadCache.value[id] = payload;
      } catch (err) {
        console.error('Failed to load payload for item:', id);
      } finally {
        payloadLoading.value[id] = false;
      }
    }
  }
};

// 复制单个卡片的 Payload JSON
const copyPayload = async (id: string, event: Event) => {
  event.stopPropagation();
  const payload = payloadCache.value[id];
  if (!payload) return;
  try {
    const text = JSON.stringify(payload, null, 2);
    await navigator.clipboard.writeText(text);
    copiedCardId.value = id;
    setTimeout(() => {
      copiedCardId.value = null;
    }, 1500);
  } catch (err) {
    console.error('Failed to copy payload:', err);
  }
};

// 获取不同消息类型的颜色和高亮标识类
const getAccentClass = (type: string) => {
  if (type === 'META_THINKING_CHAIN') return 'border-l-purple-500';
  if (type === 'AGENT_PRIVATE_CHAT_PREVIEW') return 'border-l-orange-500';
  if (type === 'AI_MEMO_RETRIEVAL' || type === 'DailyNote') return 'border-l-green-500';
  if (type.startsWith('AGENT_DREAM_')) return 'border-l-pink-500';
  return 'border-l-blue-500'; // RAG 默认蓝色
};

// 获取不同消息类型的标题样式和标志性图标
const getTitleStyle = (type: string) => {
  if (type === 'META_THINKING_CHAIN') {
    return {
      icon: Share2,
      colorClass: 'text-purple-400',
    };
  }
  if (type === 'AGENT_PRIVATE_CHAT_PREVIEW') {
    return {
      icon: MessageSquare,
      colorClass: 'text-orange-400',
    };
  }
  if (type === 'AI_MEMO_RETRIEVAL') {
    return {
      icon: History,
      colorClass: 'text-green-400',
    };
  }
  if (type === 'DailyNote') {
    return {
      icon: Bookmark,
      colorClass: 'text-green-400',
    };
  }
  if (type.startsWith('AGENT_DREAM_')) {
    return {
      icon: Moon,
      colorClass: 'text-pink-400',
    };
  }
  return {
    icon: Search,
    colorClass: 'text-blue-400',
  };
};

// 安全解析 HTML
const renderMarkdown = (text: string) => {
  try {
    if (!text) return '';
    // 修复 Markdown 引擎将 "[AI]:" 或 "[USER]:" 识别为隐藏链接定义（Link Reference Definition）从而吞字的 Bug
    const safeText = text.replace(/^(\s*)\[([^\]]+)\]:/gm, '$1\\[$2\\]:');
    return marked.parse(safeText) as string;
  } catch (e) {
    return text;
  }
};

// 转义特殊 HTML 字符，防止在 v-html 渲染 query 文本时因浏览器误判 <Tauri> 等标签而吞字
const renderSafeQuery = (text: string) => {
  if (!text) return '';
  const escaped = text.replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return renderMarkdown(escaped);
};



// 24柱全宽跳动音频频谱微动画
const drawSpectrum = () => {
  let phase = 0;

  const render = () => {
    // 动态在循环内部解包，一旦组件销毁且 canvas 被设为 null，循环自动安全终止
    const canvas = spectrumCanvas.value;
    if (!canvas) {
      animationFrameId = null;
      return;
    }
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      animationFrameId = null;
      return;
    }

    const dpr = window.devicePixelRatio || 1;
    const expectedWidth = canvas.clientWidth * dpr;
    const expectedHeight = canvas.clientHeight * dpr;

    if (canvas.width !== expectedWidth || canvas.height !== expectedHeight) {
      canvas.width = expectedWidth;
      canvas.height = expectedHeight;
    }

    const width = canvas.width;
    const height = canvas.height;

    if (width === 0 || height === 0) {
      animationFrameId = requestAnimationFrame(render);
      return;
    }

    ctx.clearRect(0, 0, width, height);

    const isAnimating = store.triggerSpectrumAnimation;
    const spacing = 1.5 * dpr;
    const barWidth = (width - (numBars - 1) * spacing) / numBars;

    // 创建横向霓虹渐变
    const grad = ctx.createLinearGradient(0, 0, width, 0);
    grad.addColorStop(0, '#9b59b6');
    grad.addColorStop(0.5, '#3498db');
    grad.addColorStop(1, '#9b59b6');
    ctx.fillStyle = grad;

    if (isAnimating) {
      phase += 0.06; // 控制正弦波流动的速度
    }

    for (let i = 0; i < numBars; i++) {
      if (isAnimating) {
        // 计算正弦波的当前角度（使得 24 柱正好呈现大约 1.5 个周期的完整波形）
        const angle = (i / numBars) * Math.PI * 2 * 1.5 - phase;
        // 振幅限制在可用 Canvas 高度的一半，避免触顶溢出
        const amplitude = (height - 3 * dpr) / 2;
        const offset = height / 2;
        const targetHeight = Math.sin(angle) * amplitude + offset;

        barsHeights[i] += (targetHeight - barsHeights[i]) * 0.2;
      } else {
        // 静默时平滑收缩到 1px 物理高度的精致底部线
        barsHeights[i] += (1 * dpr - barsHeights[i]) * 0.15;
      }

      const x = i * (barWidth + spacing);
      const y = height - barsHeights[i];

      ctx.fillRect(x, y, barWidth, barsHeights[i]);
    }

    animationFrameId = requestAnimationFrame(render);
  };

  if (animationFrameId) {
    cancelAnimationFrame(animationFrameId);
  }
  render();
};
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="relative w-full h-full flex flex-col overflow-hidden z-10"
         :class="[
           themeStore.isDarkResolved ? 'bg-[#131922] text-slate-100' : 'bg-[#e8ecef] text-slate-800',
           { 'pointer-events-none': !props.isOpen }
         ]">

      <!-- 头部状态栏 -->
      <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top)+8px)] pb-3 border-b relative z-10"
           :class="themeStore.isDarkResolved ? 'bg-[#1a202c] border-white/5 text-slate-300' : 'bg-[#edf0f2] border-gray-300/60 shadow-sm text-slate-700'">
        <div class="flex items-center gap-3">
          <div class="flex items-center gap-2">
            <div class="w-2 h-2 rounded-full" :class="statusDotClass"></div>
            <span class="text-[10px] font-bold uppercase tracking-widest" :class="themeStore.isDarkResolved ? 'text-white/70' : 'text-gray-500'">{{ statusLabel }}</span>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <span class="text-xs font-bold tracking-wider flex items-center gap-1" :class="themeStore.isDarkResolved ? 'text-white/50' : 'text-gray-600'">
            <Sparkles :size="12" class="text-blue-500" />
            灵视中心
          </span>
        </div>

        <div class="flex items-center gap-2">
          <!-- 清空 -->
          <button @click="store.clearAll()" class="p-2 transition-colors" :class="themeStore.isDarkResolved ? 'text-gray-400 hover:text-white' : 'text-gray-500 hover:text-gray-900'" title="清空全部">
            <Trash2 :size="16" />
          </button>
          <!-- 关闭 -->
          <button @click="emit('close')" class="p-2 -mr-2 transition-colors" :class="themeStore.isDarkResolved ? 'text-gray-400 hover:text-white' : 'text-gray-500 hover:text-gray-900'">
            <X :size="20" class="opacity-80" />
          </button>
        </div>

        <!-- 全宽频谱 Canvas 跳动动画，作为顶栏底部的霓虹分割线 -->
        <canvas ref="spectrumCanvas" class="absolute left-0 bottom-0 w-full h-[6px] block opacity-80 pointer-events-none"></canvas>
      </div>

      <!-- 横向滑动选项卡 Tab -->
      <div class="flex gap-2 px-3 py-2.5 overflow-x-auto no-scrollbar border-b bg-transparent relative z-10"
           :class="themeStore.isDarkResolved ? 'border-white/5' : 'border-gray-300/40'">
        <button
          v-for="tab in filterTabs"
          :key="tab.value"
          @click="activeFilter = tab.value"
          class="shrink-0 px-3 py-1 rounded-full text-[11px] font-bold tracking-wider transition-all"
          :class="activeFilter === tab.value
            ? (themeStore.isDarkResolved
                ? 'bg-blue-500/25 text-blue-400 border border-blue-500/40 shadow-[0_0_8px_rgba(52,152,219,0.25)]'
                : 'bg-blue-600 text-white border border-blue-600 shadow-sm')
            : (themeStore.isDarkResolved
                ? 'bg-white/8 text-white/50 border border-transparent hover:text-white/70'
                : 'bg-gray-300/40 text-slate-600 border border-transparent hover:bg-gray-300/70')"
        >
          {{ tab.label }}
        </button>
      </div>

      <!-- 消息列表区 -->
      <div ref="listContainer" 
           class="flex-1 overflow-y-auto no-rubber-band px-3 py-2 bg-transparent relative z-10"
           @touchstart="handleTouchStart"
           @touchmove="handleTouchMove"
           @touchend="handleTouchEnd">
        <!-- 空白占位 -->
        <div v-if="filteredMetadataList.length === 0" class="flex flex-col items-center justify-center py-20"
             :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-400/50'">
          <Sparkles :size="32" class="mb-4 stroke-[1.5]" :class="themeStore.isDarkResolved ? 'text-white/10' : 'text-gray-300'" />
          <span class="text-xs tracking-wider">暂无灵视认知广播数据</span>
          <span class="text-[9px] mt-1 opacity-50">等待 AI 进行思考或检索...</span>
        </div>

        <template v-else>
          <div
            v-for="item in filteredMetadataList"
            :key="item.id"
            :ref="el => { if (el) cardRefs[item.id] = el }"
            class="mb-3 border-l-2 rounded-r border transition-all overflow-hidden vcp-info-card-item"
            :class="[
              getAccentClass(item.type),
              themeStore.isDarkResolved 
                ? 'bg-[#1b222f] border-white/5 text-slate-100' 
                : 'bg-[#f5f7f9] border-gray-300/60 shadow-sm text-slate-800',
              expandedCardIds.has(item.id) 
                ? (themeStore.isDarkResolved ? 'ring-1 ring-white/10' : 'ring-1 ring-black/5') 
                : ''
            ]"
          >
            <!-- 折叠栏：Metadata 显示 -->
            <div
              @click="toggleCard(item.id)"
              class="flex items-start justify-between p-3 transition-colors cursor-pointer select-none"
              :class="themeStore.isDarkResolved ? 'active:bg-white/5' : 'active:bg-gray-200/40'"
            >
              <div class="flex-1 min-w-0 pr-2">
                <div class="flex items-center justify-between mb-1.5">
                  <div class="flex items-center gap-1.5 min-w-0">
                    <component 
                      :is="getTitleStyle(item.type).icon" 
                      :size="12" 
                      :class="themeStore.isDarkResolved ? getTitleStyle(item.type).colorClass : getTitleStyle(item.type).colorClass.replace('-400', '-600')" 
                      class="shrink-0" 
                    />
                    <span 
                      class="text-[12px] font-bold tracking-wide truncate"
                      :class="themeStore.isDarkResolved ? getTitleStyle(item.type).colorClass : getTitleStyle(item.type).colorClass.replace('-400', '-600')"
                    >
                      {{ item.title }}
                    </span>
                  </div>
                  <span class="text-[9px] font-mono shrink-0 ml-2" :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-400'">
                    {{ new Date(item.timestamp).toLocaleTimeString() }}
                  </span>
                </div>
                <div v-if="item.subtitle" class="text-[9px] font-mono font-bold tracking-wider mb-1" :class="themeStore.isDarkResolved ? 'text-blue-400/80' : 'text-blue-600'">
                  {{ item.subtitle }}
                </div>
                <!-- summary 预览：折叠时单行截断，展开时完整换行展示 -->
                <div 
                  class="text-[11px] leading-relaxed break-words"
                  :class="[
                    expandedCardIds.has(item.id) ? '' : 'truncate',
                    themeStore.isDarkResolved ? 'text-white/45' : 'text-gray-500'
                  ]"
                >
                  {{ item.summary }}
                </div>
              </div>

              <!-- 展开/折叠指示图标 -->
              <div v-if="item.hasDetails" class="shrink-0 pt-0.5" :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-400'">
                <ChevronUp v-if="expandedCardIds.has(item.id)" :size="16" />
                <ChevronDown v-else :size="16" />
              </div>
            </div>

            <!-- 展开栏：Payload Lazy 加载与结构化渲染 -->
            <div v-if="expandedCardIds.has(item.id)" class="border-t p-3 text-[11px]"
                 :class="themeStore.isDarkResolved ? 'border-white/5 bg-[#111620]' : 'border-gray-300/40 bg-[#e9ecef]'">
              
              <!-- 1. 加载中 -->
              <div v-if="payloadLoading[item.id]" class="flex items-center justify-center py-4 gap-2" :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-400'">
                <Loader2 :size="14" class="animate-spin" />
                <span>正在提取饱水 Payload 详情...</span>
              </div>

              <!-- 2. 加载失败 -->
              <div v-else-if="!payloadCache[item.id]" class="flex flex-col items-center justify-center py-4 gap-2 text-red-500">
                <span>⚠️ 获取详情失败或文件已被清理</span>
                <button
                  @click.stop="toggleCard(item.id); toggleCard(item.id)"
                  class="px-2 py-0.5 rounded border border-red-500/40 text-[9px] hover:bg-red-500/10 transition-colors"
                >
                  重试
                </button>
              </div>

              <!-- 3. 加载成功 - 渲染详情 -->
              <template v-else>
                <div class="flex justify-between items-center mb-3 pb-2 border-b" :class="themeStore.isDarkResolved ? 'border-white/5' : 'border-gray-300/50'">
                  <span class="text-[9px] font-mono" :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-500'">ID: {{ item.id }}</span>
                  <!-- 复制按钮 -->
                  <button
                    @click="copyPayload(item.id, $event)"
                    class="flex items-center gap-1 px-2 py-0.5 rounded transition-all active:scale-95 text-[9px]"
                    :class="themeStore.isDarkResolved 
                      ? 'border border-white/10 hover:bg-white/10 text-white/50' 
                      : 'border border-gray-300 hover:bg-gray-200/60 text-gray-600'"
                  >
                    <Check v-if="copiedCardId === item.id" :size="10" class="text-green-500" />
                    <Copy v-else :size="10" />
                    <span>{{ copiedCardId === item.id ? '已复制' : '复制完整JSON' }}</span>
                  </button>
                </div>

                <!-- RAG 知识库检索结果详情 -->
                <div v-if="item.type === '' || item.type === 'RAG_RETRIEVAL_DETAILS'" class="space-y-2.5">
                  <!-- RAG Query Section -->
                  <div 
                    v-if="payloadCache[item.id].query" 
                    @click.stop="toggleSubCard(`${item.id}_query`)"
                    :ref="el => { if (el) subCardRefs[`${item.id}_query`] = el }"
                    class="p-2 rounded border mb-1.5 cursor-pointer transition-colors"
                    :class="themeStore.isDarkResolved 
                      ? 'bg-blue-500/5 border-blue-500/10 active:bg-blue-500/10 text-slate-100' 
                      : 'bg-blue-50/30 border-blue-200/80 active:bg-blue-100/30 text-slate-700'"
                  >
                    <div class="flex justify-between items-center mb-1 text-[9px] font-bold"
                         :class="themeStore.isDarkResolved ? 'text-blue-400/80' : 'text-blue-600'">
                      <div class="flex items-center gap-1">
                        <Brain :size="10" /> RAG 检索提问
                      </div>
                      <div v-if="payloadCache[item.id].query.length > 30" class="flex items-center gap-0.5 scale-90 select-none"
                           :class="themeStore.isDarkResolved ? 'text-blue-400/60' : 'text-blue-500'">
                        <span>{{ expandedSubCardIds.has(`${item.id}_query`) ? '收起' : '展开' }}</span>
                        <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_query`)" :size="10" />
                        <ChevronDown v-else :size="10" />
                      </div>
                    </div>
                    <div class="leading-relaxed font-mono text-[10px] select-text">
                      <template v-if="payloadCache[item.id].query.length > 30 && !expandedSubCardIds.has(`${item.id}_query`)">
                        {{ payloadCache[item.id].query.slice(0, 30) }}...
                      </template>
                      <template v-else>
                        <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].query)"></div>
                      </template>
                    </div>
                  </div>

                  <!-- Core Tags -->
                  <div v-if="payloadCache[item.id].coreTags && payloadCache[item.id].coreTags.length > 0" class="flex flex-wrap gap-1.5 mb-2">
                    <span 
                      v-for="tag in payloadCache[item.id].coreTags" 
                      :key="tag" 
                      class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-white font-bold text-[9px]"
                      :class="themeStore.isDarkResolved ? 'bg-[#b38c2b]/90 shadow-[0_0_6px_rgba(179,140,43,0.2)]' : 'bg-[#b38c2b] shadow-sm'"
                    >
                      <Sparkles :size="9" class="text-yellow-100 fill-yellow-100 shrink-0" />
                      {{ tag }}
                    </span>
                  </div>

                  <!-- Tag Stats & Time Ranges -->
                  <div class="flex flex-wrap gap-1.5 mb-2 text-[9px] font-mono"
                       :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-500'">
                    <span v-if="payloadCache[item.id].tagStats" class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-yellow-500/10 border-yellow-500/20 text-yellow-500/80' : 'bg-yellow-50 border-yellow-200 text-yellow-700'">
                      🏷️ 匹配标签: {{ payloadCache[item.id].tagStats.uniqueMatchedTags?.length || 0 }}个 | Boost均值: {{ payloadCache[item.id].tagStats.avgBoostFactor }}
                    </span>
                    <span v-if="payloadCache[item.id].timeRanges && payloadCache[item.id].timeRanges.length > 0" class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-blue-500/10 border-blue-500/20 text-blue-400' : 'bg-blue-50 border-blue-200 text-blue-700'">
                      📅 时间窗口: {{ payloadCache[item.id].timeRanges[0].start.slice(0,10) }} ➜ {{ payloadCache[item.id].timeRanges[0].end.slice(0,10) }}
                    </span>
                  </div>

                  <div class="text-[9px] font-bold uppercase tracking-widest mb-1.5 flex items-center gap-1"
                       :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-400'">
                    召回结果列表 ({{ payloadCache[item.id].results?.length || 0 }})
                  </div>

                  <div class="space-y-2">
                    <div
                      v-for="(res, idx) in payloadCache[item.id].results"
                      :key="idx"
                      class="p-2 rounded border space-y-1.5 transition-all"
                      :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-[#f1f3f5] border-gray-350/50 shadow-sm text-slate-800'"
                    >
                      <!-- Sub-card Header, clickable to toggle detail collapse -->
                      <div 
                        @click.stop="toggleSubCard(`${item.id}_res_${idx}`)"
                        class="flex flex-col text-[9px] cursor-pointer select-none py-0.5"
                        :class="themeStore.isDarkResolved ? 'text-white/45' : 'text-gray-500'"
                      >
                        <div class="flex justify-between items-center">
                          <div class="flex items-center gap-1.5 min-w-0 pr-2">
                            <span class="font-bold shrink-0" :class="themeStore.isDarkResolved ? 'text-white/25' : 'text-gray-400'">#{{ idx + 1 }}</span>
                            <span 
                              class="px-1 py-0.5 rounded font-bold font-mono text-[9px] shrink-0"
                              :class="res.originalScore && res.originalScore !== res.score 
                                ? (themeStore.isDarkResolved ? 'bg-yellow-500/20 text-yellow-400 border border-yellow-500/30' : 'bg-yellow-50 border-yellow-200 text-yellow-700')
                                : (themeStore.isDarkResolved ? 'bg-blue-500/20 text-blue-400' : 'bg-blue-50 text-blue-700')"
                            >
                              Score: {{ res.score?.toFixed(3) || 'Time' }}
                              <template v-if="res.originalScore && res.originalScore !== res.score">⚡</template>
                            </span>
                            <span class="font-mono text-[8px] px-1 py-0.5 rounded truncate"
                                  :class="themeStore.isDarkResolved ? 'bg-white/5' : 'bg-gray-250 text-gray-600'">来源: {{ res.source || 'Unknown' }}</span>
                          </div>
                          <div :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-400'" class="shrink-0">
                            <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_res_${idx}`)" :size="12" />
                            <ChevronDown v-else :size="12" />
                          </div>
                        </div>
                        <!-- 30-char preview when collapsed -->
                        <div v-if="!expandedSubCardIds.has(`${item.id}_res_${idx}`)" class="text-[9px] truncate mt-1 pl-1 font-mono leading-normal"
                             :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-400'">
                          {{ res.text.slice(0, 30) }}{{ res.text.length > 30 ? '...' : '' }}
                        </div>
                      </div>

                      <!-- Sub-card Expandable Content -->
                      <div v-if="expandedSubCardIds.has(`${item.id}_res_${idx}`)" class="space-y-1.5 pt-1.5 border-t"
                           :class="themeStore.isDarkResolved ? 'border-white/5' : 'border-gray-200'">
                        <div 
                          class="leading-relaxed font-mono select-text text-[10px] break-words"
                          :class="[themeStore.isDarkResolved ? 'prose-invert text-white/85' : 'text-gray-800', 'prose prose-xs text-[10px]']"
                          v-html="renderMarkdown(res.text)"
                        ></div>

                        <!-- Matched Tags (matchedTags) -->
                        <div v-if="res.matchedTags && res.matchedTags.length > 0" class="pt-1.5 border-t font-mono font-bold text-[9px] leading-relaxed"
                             :class="themeStore.isDarkResolved ? 'border-white/5 text-slate-400' : 'border-gray-200 text-gray-500'">
                          🏷️ {{ res.matchedTags.join(', ') }}
                        </div>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 元思考链详情 -->
                <div v-else-if="item.type === 'META_THINKING_CHAIN'" class="space-y-3">
                  <!-- Flow path -->
                  <div class="p-2 rounded border font-mono text-[9px]"
                       :class="themeStore.isDarkResolved ? 'bg-purple-900/10 border-purple-500/10 text-purple-300' : 'bg-purple-50/50 border-purple-200 text-purple-700'">
                    <div class="font-bold mb-1 flex items-center gap-1">
                      <Sparkles :size="10" /> 思考流脉络:
                    </div>
                    <div class="flex flex-wrap items-center gap-1 leading-normal">
                      <template v-for="(stage, idx) in payloadCache[item.id].stages" :key="stage.stage">
                        <span class="px-1 py-0.5 rounded" :class="themeStore.isDarkResolved ? 'bg-purple-500/20' : 'bg-purple-100/60'">{{ stage.clusterName }}</span>
                        <span v-if="idx < payloadCache[item.id].stages.length - 1" :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-300'">➜</span>
                      </template>
                    </div>
                  </div>

                  <!-- Thinking Chain Query Section -->
                  <div 
                    v-if="payloadCache[item.id].query" 
                    @click.stop="toggleSubCard(`${item.id}_chain_query`)"
                    :ref="el => { if (el) subCardRefs[`${item.id}_chain_query`] = el }"
                    class="p-2 rounded border mb-1 cursor-pointer transition-colors"
                    :class="themeStore.isDarkResolved 
                      ? 'bg-purple-900/10 border-purple-500/10 active:bg-purple-500/10 text-slate-100' 
                      : 'bg-purple-50/50 border-purple-200 active:bg-purple-100/50 text-slate-700'"
                  >
                    <div class="flex justify-between items-center mb-1 text-[9px] font-bold"
                         :class="themeStore.isDarkResolved ? 'text-purple-400/80' : 'text-purple-700'">
                      <div class="flex items-center gap-1">
                        <Brain :size="10" /> 思考查询
                      </div>
                      <div v-if="payloadCache[item.id].query.length > 30" class="flex items-center gap-0.5 scale-90 select-none"
                           :class="themeStore.isDarkResolved ? 'text-purple-400/60' : 'text-purple-600'">
                        <span>{{ expandedSubCardIds.has(`${item.id}_chain_query`) ? '收起' : '展开' }}</span>
                        <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_chain_query`)" :size="10" />
                        <ChevronDown v-else :size="10" />
                      </div>
                    </div>
                    <div class="leading-relaxed font-mono text-[10px] select-text">
                      <template v-if="payloadCache[item.id].query.length > 30 && !expandedSubCardIds.has(`${item.id}_chain_query`)">
                        {{ payloadCache[item.id].query.slice(0, 30) }}...
                      </template>
                      <template v-else>
                        <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].query)"></div>
                      </template>
                    </div>
                  </div>

                  <!-- activated groups -->
                  <div v-if="payloadCache[item.id].activatedGroups && payloadCache[item.id].activatedGroups.length > 0" class="flex flex-wrap gap-1 mb-1.5">
                    <span v-for="grp in payloadCache[item.id].activatedGroups" :key="grp" class="px-1.5 py-0.5 rounded border text-[8px] font-mono"
                          :class="themeStore.isDarkResolved ? 'bg-purple-900/10 border-purple-500/20 text-purple-300' : 'bg-purple-50 border-purple-200 text-purple-700'">
                      组: {{ grp }}
                    </span>
                  </div>

                  <div class="text-[9px] font-bold uppercase tracking-widest mb-2"
                       :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-400'">阶段执行详情</div>
                  
                  <div class="space-y-2.5">
                    <div
                      v-for="stage in payloadCache[item.id].stages"
                      :key="stage.stage"
                      class="border-l-2 border-purple-500/40 pl-2.5 space-y-1.5"
                    >
                      <div class="text-[10px] font-bold flex justify-between items-center"
                           :class="themeStore.isDarkResolved ? 'text-purple-200' : 'text-purple-850'">
                        <span>阶段 {{ stage.stage }}: {{ stage.clusterName }}</span>
                        <span class="text-[8px] font-mono"
                              :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-slate-500'">召回: {{ stage.resultCount }}项</span>
                      </div>
                      <div class="space-y-1.5">
                        <div
                          v-for="(res, rIdx) in stage.results"
                          :key="rIdx"
                          class="p-2 rounded border font-mono text-[9px] space-y-1 transition-all"
                          :class="themeStore.isDarkResolved 
                            ? 'bg-white/5 border-white/5 text-slate-100' 
                            : 'bg-gray-300/25 border-gray-300/40 shadow-sm text-slate-800'"
                        >
                          <!-- Sub-card Header -->
                          <div 
                            @click.stop="toggleSubCard(`${item.id}_s${stage.stage}_res_${rIdx}`)"
                            class="flex flex-col text-[8px] cursor-pointer select-none py-0.5"
                            :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-slate-500'"
                          >
                            <div class="flex justify-between items-center">
                              <div class="flex items-center gap-1.5 min-w-0">
                                <span class="font-bold shrink-0"
                                      :class="themeStore.isDarkResolved ? 'text-white/25' : 'text-slate-400'">#{{ rIdx + 1 }}</span>
                                <span 
                                  class="px-1 py-0.5 rounded font-bold font-mono text-[8px] shrink-0"
                                  :class="themeStore.isDarkResolved 
                                    ? 'bg-purple-500/20 text-purple-400' 
                                    : 'bg-purple-600/10 text-purple-700'"
                                >
                                  Score: {{ res.score?.toFixed(3) || 'N/A' }}
                                </span>
                              </div>
                              <div class="shrink-0"
                                   :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-slate-500'">
                                <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_s${stage.stage}_res_${rIdx}`)" :size="12" />
                                <ChevronDown v-else :size="12" />
                              </div>
                            </div>
                            <!-- 30-char preview when collapsed -->
                            <div v-if="!expandedSubCardIds.has(`${item.id}_s${stage.stage}_res_${rIdx}`)" 
                                 class="text-[8px] truncate mt-0.5 pl-1 leading-normal"
                                 :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-slate-500'">
                              {{ res.text.slice(0, 30) }}{{ res.text.length > 30 ? '...' : '' }}
                            </div>
                          </div>

                          <!-- Sub-card Expandable Content -->
                          <div v-if="expandedSubCardIds.has(`${item.id}_s${stage.stage}_res_${rIdx}`)" 
                               class="space-y-1.5 pt-1.5 border-t"
                               :class="themeStore.isDarkResolved ? 'border-white/5' : 'border-gray-300/40'">
                            <div 
                              :class="[themeStore.isDarkResolved ? 'prose-invert text-white/85' : 'text-gray-800', 'prose prose-xs leading-relaxed break-words font-mono text-[10px] select-text']"
                              v-html="renderMarkdown(res.text)"
                            ></div>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- Agent 私聊 -->
                <div v-else-if="item.type === 'AGENT_PRIVATE_CHAT_PREVIEW'" class="space-y-2.5">
                  <div class="p-2.5 rounded border space-y-2"
                       :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-[#f1f3f5] border-gray-300/60 shadow-sm'">
                    <!-- USER QUERY Box -->
                    <div 
                      @click.stop="toggleSubCard(`${item.id}_chat_q`)"
                      :ref="el => { if (el) subCardRefs[`${item.id}_chat_q`] = el }"
                      class="border-l pl-2 cursor-pointer transition-colors"
                      :class="[
                        themeStore.isDarkResolved 
                          ? 'border-white/20 active:bg-white/5' 
                          : 'border-gray-300 active:bg-gray-200/40'
                      ]"
                    >
                      <div class="flex justify-between items-center mb-1 text-[8px] font-bold font-mono tracking-wider uppercase"
                           :class="themeStore.isDarkResolved ? 'text-white/35' : 'text-gray-400'">
                        <span>USER QUERY</span>
                        <div v-if="payloadCache[item.id].query.length > 30" class="flex items-center gap-0.5 scale-90 select-none font-normal normal-case"
                             :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-500'">
                          <span>{{ expandedSubCardIds.has(`${item.id}_chat_q`) ? '收起' : '展开' }}</span>
                          <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_chat_q`)" :size="10" />
                          <ChevronDown v-else :size="10" />
                        </div>
                      </div>
                      <div class="leading-relaxed font-mono text-[10px] select-text"
                           :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-800'">
                        <template v-if="payloadCache[item.id].query.length > 30 && !expandedSubCardIds.has(`${item.id}_chat_q`)">
                          {{ payloadCache[item.id].query.slice(0, 30) }}...
                        </template>
                        <template v-else>
                          <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].query)"></div>
                        </template>
                      </div>
                    </div>
                    
                    <!-- AI RESPONSE Box -->
                    <div 
                      @click.stop="toggleSubCard(`${item.id}_chat_r`)"
                      :ref="el => { if (el) subCardRefs[`${item.id}_chat_r`] = el }"
                      class="border-l pl-2 mt-2 pt-1 border-t cursor-pointer transition-colors"
                      :class="[
                        themeStore.isDarkResolved 
                          ? 'border-orange-500/50 border-t-white/5 active:bg-orange-500/5' 
                          : 'border-orange-500 border-t-gray-300 active:bg-orange-50/50'
                      ]"
                    >
                      <div class="flex justify-between items-center mb-1 text-[8px] font-bold text-orange-400 font-mono tracking-wider uppercase">
                        <span>AI INNER VOICE</span>
                        <div v-if="payloadCache[item.id].response.length > 30" class="flex items-center gap-0.5 scale-90 select-none font-normal normal-case"
                             :class="themeStore.isDarkResolved ? 'text-orange-400/60' : 'text-orange-600'">
                          <span>{{ expandedSubCardIds.has(`${item.id}_chat_r`) ? '收起' : '展开' }}</span>
                          <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_chat_r`)" :size="10" />
                          <ChevronDown v-else :size="10" />
                        </div>
                      </div>
                      <div class="leading-relaxed font-mono text-[10px] select-text"
                           :class="themeStore.isDarkResolved ? 'text-white/85' : 'text-gray-800'">
                        <template v-if="payloadCache[item.id].response.length > 30 && !expandedSubCardIds.has(`${item.id}_chat_r`)">
                          {{ payloadCache[item.id].response.slice(0, 30) }}...
                        </template>
                        <template v-else>
                          <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/85' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].response)"></div>
                        </template>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 记忆检索 (AIMemo) -->
                <div v-else-if="item.type === 'AI_MEMO_RETRIEVAL'" class="space-y-2.5">
                  <!-- Meta pills -->
                  <div class="flex flex-wrap gap-1.5 mb-1.5 text-[9px] font-mono">
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-green-500/10 border-green-500/20 text-green-400' : 'bg-green-50 border-green-200 text-green-700'">
                      模式: {{ payloadCache[item.id].mode || 'aggregated_single' }}
                    </span>
                    <span v-if="payloadCache[item.id].fileCount" class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-green-500/10 border-green-500/20 text-green-400' : 'bg-green-50 border-green-200 text-green-700'">
                      扫描: {{ payloadCache[item.id].fileCount }}个文件
                    </span>
                    <span v-if="payloadCache[item.id].dbNames && payloadCache[item.id].dbNames.length > 0" class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-green-500/10 border-green-500/20 text-green-400' : 'bg-green-50 border-green-200 text-green-700'">
                      联合库: {{ payloadCache[item.id].dbNames.join(', ') }}
                    </span>
                  </div>

                  <!-- Memo Query Box -->
                  <div 
                    v-if="payloadCache[item.id].query" 
                    @click.stop="toggleSubCard(`${item.id}_memo_query`)"
                    :ref="el => { if (el) subCardRefs[`${item.id}_memo_query`] = el }"
                    class="p-2 rounded border cursor-pointer transition-colors"
                    :class="themeStore.isDarkResolved 
                      ? 'bg-green-500/5 border-green-500/10 active:bg-green-500/10 text-slate-100' 
                      : 'bg-green-50/30 border-green-200/80 active:bg-green-100/30 text-slate-750'"
                  >
                    <div class="flex justify-between items-center mb-1 text-[9px] font-bold"
                         :class="themeStore.isDarkResolved ? 'text-green-400/80' : 'text-green-600'">
                      <div class="flex items-center gap-1">
                        <Brain :size="10" /> 联合检索提问
                      </div>
                      <div v-if="payloadCache[item.id].query.length > 30" class="flex items-center gap-0.5 scale-90 select-none"
                           :class="themeStore.isDarkResolved ? 'text-green-400/60' : 'text-green-500'">
                        <span>{{ expandedSubCardIds.has(`${item.id}_memo_query`) ? '收起' : '展开' }}</span>
                        <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_memo_query`)" :size="10" />
                        <ChevronDown v-else :size="10" />
                      </div>
                    </div>
                    <div class="leading-relaxed font-mono text-[10px] select-text">
                      <template v-if="payloadCache[item.id].query.length > 30 && !expandedSubCardIds.has(`${item.id}_memo_query`)">
                        {{ payloadCache[item.id].query.slice(0, 30) }}...
                      </template>
                      <template v-else>
                        <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].query)"></div>
                      </template>
                    </div>
                  </div>

                  <div class="p-2.5 rounded border mt-2"
                       :class="themeStore.isDarkResolved ? 'bg-black/30 border-white/5' : 'bg-[#f1f3f5] border-gray-300/60 shadow-sm'">
                    <div class="text-[9px] font-bold mb-1 flex items-center gap-1"
                         :class="themeStore.isDarkResolved ? 'text-green-400' : 'text-green-700'">
                      <BookOpen :size="10" /> 提炼出的联合记忆报告:
                    </div>
                    <!-- 使用 marked 渲染提炼记忆 -->
                    <div
                      class="leading-relaxed select-text font-mono"
                      :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']"
                      v-html="renderMarkdown(payloadCache[item.id].extractedMemories || '')"
                    ></div>
                  </div>
                </div>

                <!-- DailyNote 日记动作追踪 -->
                <div v-else-if="item.type === 'DailyNote'" class="space-y-2.5">
                  <div 
                    @click.stop="toggleSubCard(`${item.id}_note`)"
                    :ref="el => { if (el) subCardRefs[`${item.id}_note`] = el }"
                    class="p-2.5 rounded border-l-2 cursor-pointer transition-colors"
                    :class="themeStore.isDarkResolved 
                      ? 'bg-green-500/5 border border-green-500/15 border-l-green-500 active:bg-green-500/10' 
                      : 'bg-green-50/40 border border-green-200/80 border-l-green-600 active:bg-green-100/30'"
                  >
                    <div class="flex justify-between items-center mb-1 text-[10px] font-bold"
                         :class="themeStore.isDarkResolved ? 'text-green-400' : 'text-green-700'">
                      <div class="flex items-center gap-1.5">
                        <Calendar :size="11" />
                        <span>日记动作追踪</span>
                      </div>
                      <div v-if="payloadCache[item.id].message.length > 30" class="flex items-center gap-0.5 scale-90 select-none font-normal"
                           :class="themeStore.isDarkResolved ? 'text-green-400/60' : 'text-green-600/80'">
                        <span>{{ expandedSubCardIds.has(`${item.id}_note`) ? '收起' : '展开' }}</span>
                        <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_note`)" :size="10" />
                        <ChevronDown v-else :size="10" />
                      </div>
                    </div>
                    <div class="leading-relaxed font-mono text-[10px] break-words select-text"
                         :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-800'">
                      <template v-if="payloadCache[item.id].message.length > 30 && !expandedSubCardIds.has(`${item.id}_note`)">
                        {{ payloadCache[item.id].message.slice(0, 30) }}...
                      </template>
                      <template v-else>
                        <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/80' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderSafeQuery(payloadCache[item.id].message)"></div>
                      </template>
                    </div>
                  </div>
                </div>

                <!-- Agent 做梦联想 -->
                <div v-else-if="item.type === 'AGENT_DREAM_ASSOCIATIONS'" class="space-y-3">
                  <!-- dreamId 标签 -->
                  <div v-if="payloadCache[item.id].dreamId" class="text-[9px] font-mono mb-1.5"
                       :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-550'">
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-pink-500/10 border-pink-500/20 text-pink-400' : 'bg-pink-50 border-pink-200 text-pink-700'">
                      梦境ID: {{ payloadCache[item.id].dreamId }}
                    </span>
                  </div>
                  <!-- seeds statistics -->
                  <div class="flex flex-wrap gap-1.5 mb-1 text-[9px] font-mono">
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-yellow-500/10 border-yellow-500/20 text-yellow-400' : 'bg-yellow-50 border-yellow-250 text-yellow-700'">
                      种子日记: {{ payloadCache[item.id].seedCount || 0 }} 篇
                    </span>
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-yellow-500/10 border-yellow-500/20 text-yellow-400' : 'bg-yellow-50 border-yellow-250 text-yellow-700'">
                      联想唤醒: {{ payloadCache[item.id].associationCount || 0 }} 篇
                    </span>
                  </div>

                  <!-- seeds details -->
                  <div v-if="payloadCache[item.id].seeds && payloadCache[item.id].seeds.length > 0">
                    <div class="text-[9px] font-bold uppercase tracking-widest mb-1.5 flex items-center gap-1"
                         :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-550'">入梦采样种子列表</div>
                    <div class="space-y-1">
                      <div
                        v-for="(seed, idx) in payloadCache[item.id].seeds"
                        :key="idx"
                        class="p-1.5 rounded border text-[9px] font-mono"
                        :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5 text-white/70' : 'bg-[#f1f3f5] border-gray-300/50 text-gray-700'"
                      >
                        <div class="font-bold truncate" :class="themeStore.isDarkResolved ? 'text-white/90' : 'text-gray-900'">{{ seed.file }}</div>
                        <div class="truncate text-[8px] mt-0.5" :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-500'">{{ seed.snippet }}</div>
                      </div>
                    </div>
                  </div>

                  <!-- associations details -->
                  <div v-if="payloadCache[item.id].associations && payloadCache[item.id].associations.length > 0" class="mt-2.5">
                    <div class="text-[9px] font-bold uppercase tracking-widest mb-1.5 flex items-center gap-1"
                         :class="themeStore.isDarkResolved ? 'text-white/20' : 'text-gray-550'">梦境关联唤醒 (Resonance)</div>
                    <div class="space-y-1">
                      <div
                        v-for="(assoc, idx) in payloadCache[item.id].associations"
                        :key="idx"
                        class="flex justify-between items-center p-1.5 rounded border text-[9px] font-mono"
                        :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5 text-white/60' : 'bg-[#f1f3f5] border-gray-300/50 text-gray-600'"
                      >
                        <span class="truncate pr-2" :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-800'">{{ assoc.file }}</span>
                        <span class="shrink-0 font-bold text-[8px]" :class="themeStore.isDarkResolved ? 'text-yellow-400' : 'text-yellow-600'">得分: {{ assoc.score }}</span>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- Agent 做梦整理操作 -->
                <div v-else-if="item.type === 'AGENT_DREAM_OPERATIONS'" class="space-y-3">
                  <!-- dreamId 标签 -->
                  <div v-if="payloadCache[item.id].dreamId" class="text-[9px] font-mono mb-1.5"
                       :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-550'">
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-pink-500/10 border-pink-500/20 text-pink-400' : 'bg-pink-50 border-pink-200 text-pink-700'">
                      梦境ID: {{ payloadCache[item.id].dreamId }}
                    </span>
                  </div>
                  <!-- meta info -->
                  <div class="flex justify-between items-center text-[9px] font-mono pb-1.5 border-b"
                       :class="themeStore.isDarkResolved ? 'text-white/40 border-white/5' : 'text-gray-550 border-gray-300/60'">
                    <span>整理日志: {{ payloadCache[item.id].logFile || 'None' }}</span>
                    <span>操作数量: {{ payloadCache[item.id].operations?.length || 0 }} 项</span>
                  </div>

                  <!-- Operations list -->
                  <div class="space-y-1.5">
                    <div
                      v-for="(op, idx) in payloadCache[item.id].operations"
                      :key="idx"
                      class="flex items-center justify-between p-2 rounded border font-mono text-[9px]"
                      :class="themeStore.isDarkResolved ? 'bg-white/5 border-white/5' : 'bg-[#f1f3f5] border-gray-300/50'"
                    >
                      <div class="flex items-center gap-1.5 min-w-0 pr-2">
                        <!-- Type Badge -->
                        <span 
                          class="px-1.5 py-0.5 rounded-full text-[8px] font-bold uppercase tracking-wider"
                          :class="{
                            'bg-orange-500/25 text-orange-400 border border-orange-500/35': op.type === 'merge',
                            'bg-red-500/25 text-red-400 border border-red-500/35': op.type === 'delete',
                            'bg-blue-500/25 text-blue-400 border border-blue-500/35': op.type === 'insight',
                            'bg-gray-500/25 text-gray-400 border border-gray-500/35': op.type !== 'merge' && op.type !== 'delete' && op.type !== 'insight'
                          }"
                        >
                          {{ op.type }}
                        </span>
                        <span class="truncate text-[8px]" :class="themeStore.isDarkResolved ? 'text-white/30' : 'text-gray-500'">{{ op.operationId || 'ID: None' }}</span>
                      </div>

                      <!-- Status Badge -->
                      <span 
                        class="px-1 py-0.5 rounded text-[8px] font-bold"
                        :class="{
                          'bg-yellow-400/20 text-yellow-400 border border-yellow-400/30': op.status === 'pending_review',
                          'bg-green-400/20 text-green-400 border border-green-400/30': op.status === 'approved',
                          'bg-red-400/20 text-red-400 border border-red-400/30': op.status === 'rejected',
                          'bg-gray-400/20 text-gray-400': op.status !== 'pending_review' && op.status !== 'approved' && op.status !== 'rejected'
                        }"
                      >
                        {{ op.status || 'unknown' }}
                      </span>
                    </div>
                  </div>
                </div>

                <!-- Agent 梦叙事 -->
                <div v-else-if="item.type === 'AGENT_DREAM_NARRATIVE'" class="space-y-2.5 select-text font-mono">
                  <!-- dreamId 标签 -->
                  <div v-if="payloadCache[item.id].dreamId" class="text-[9px] font-mono mb-1.5"
                       :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-550'">
                    <span class="px-1.5 py-0.5 rounded border"
                          :class="themeStore.isDarkResolved ? 'bg-pink-500/10 border-pink-500/20 text-pink-400' : 'bg-pink-50 border-pink-200 text-pink-700'">
                      梦境ID: {{ payloadCache[item.id].dreamId }}
                    </span>
                  </div>
                  <div 
                    @click.stop="toggleSubCard(`${item.id}_narrative`)"
                    :ref="el => { if (el) subCardRefs[`${item.id}_narrative`] = el }"
                    class="p-2.5 rounded border cursor-pointer transition-colors"
                    :class="themeStore.isDarkResolved 
                      ? 'bg-white/5 border-white/5 active:bg-white/8' 
                      : 'bg-[#f1f3f5] border-gray-300/60 active:bg-gray-200/50 shadow-sm'"
                  >
                    <div class="flex justify-between items-center mb-1.5 text-[9px] font-bold"
                         :class="themeStore.isDarkResolved ? 'text-pink-400' : 'text-pink-600'">
                      <div class="flex items-center gap-1">
                        <Moon :size="10" /> 梦境长叙事正文
                      </div>
                      <div v-if="payloadCache[item.id].narrative && payloadCache[item.id].narrative.length > 50" class="flex items-center gap-0.5 scale-90 select-none"
                           :class="themeStore.isDarkResolved ? 'text-pink-400/60' : 'text-pink-500'">
                        <span>{{ expandedSubCardIds.has(`${item.id}_narrative`) ? '收起' : '展开' }}</span>
                        <ChevronUp v-if="expandedSubCardIds.has(`${item.id}_narrative`)" :size="10" />
                        <ChevronDown v-else :size="10" />
                      </div>
                    </div>
                    <div class="leading-relaxed break-words text-[10px]">
                      <template v-if="payloadCache[item.id].narrative && payloadCache[item.id].narrative.length > 50 && !expandedSubCardIds.has(`${item.id}_narrative`)">
                        <span :class="themeStore.isDarkResolved ? 'text-white/50' : 'text-gray-550'">
                          {{ payloadCache[item.id].narrative.slice(0, 50) }}...
                        </span>
                      </template>
                      <template v-else>
                        <div :class="[themeStore.isDarkResolved ? 'prose-invert text-white/85' : 'text-gray-800', 'prose prose-xs text-[10px]']" v-html="renderMarkdown(payloadCache[item.id].narrative || '')"></div>
                      </template>
                    </div>
                  </div>
                </div>

                <!-- Agent 梦境开始与结束 -->
                <div v-else-if="item.type === 'AGENT_DREAM_START'" class="space-y-2.5">
                  <div class="p-2.5 rounded border-l-2 flex items-center gap-2"
                       :class="themeStore.isDarkResolved 
                         ? 'bg-purple-500/10 border-purple-500/20 border-l-purple-500 text-white/95' 
                         : 'bg-purple-100/30 border-purple-200/80 border-l-purple-600 text-purple-950 shadow-sm'">
                    <Moon :size="12" class="text-purple-500 animate-pulse" />
                    <div class="font-mono text-[10px]">{{ payloadCache[item.id].message }}</div>
                  </div>
                </div>

                <div v-else-if="item.type === 'AGENT_DREAM_END'" class="space-y-2.5">
                  <div 
                    class="p-2.5 rounded border-l-2 flex flex-col gap-1.5"
                    :class="payloadCache[item.id].status === 'error' 
                      ? (themeStore.isDarkResolved ? 'bg-red-500/10 border-red-500/20 border-l-red-500 text-red-400' : 'bg-red-100/30 border-red-200/80 border-l-red-500 text-red-700 shadow-sm')
                      : (themeStore.isDarkResolved ? 'bg-green-500/10 border-green-500/20 border-l-green-500 text-green-400' : 'bg-green-100/30 border-green-200/80 border-l-green-600 text-green-700 shadow-sm')"
                  >
                    <div class="flex items-center gap-1.5 font-bold text-[10px]">
                      <ShieldAlert v-if="payloadCache[item.id].status === 'error'" :size="12" />
                      <Check v-else :size="12" />
                      <span>梦境运行结束: {{ payloadCache[item.id].status }}</span>
                    </div>
                    <div v-if="payloadCache[item.id].error" class="leading-relaxed font-mono text-[9px] select-text whitespace-pre-wrap"
                         :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-650'">{{ payloadCache[item.id].error }}</div>
                    <div class="leading-relaxed font-mono text-[10px]"
                         :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-750'">{{ payloadCache[item.id].message || '正常离梦，数据收拢。' }}</div>
                  </div>
                </div>

                <!-- Agent 梦调度通知 -->
                <div v-else-if="item.type === 'AGENT_DREAM_SCHEDULE'" class="space-y-2.5">
                  <div class="p-2.5 rounded border-l-2 space-y-1.5"
                       :class="themeStore.isDarkResolved 
                         ? 'bg-blue-500/10 border-blue-500/20 border-l-blue-500' 
                         : 'bg-blue-100/30 border-blue-200/80 border-l-blue-600 shadow-sm'">
                    <div class="flex items-center gap-1.5 font-bold text-[10px]" :class="themeStore.isDarkResolved ? 'text-blue-400' : 'text-blue-700'">
                      <Clock :size="11" />
                      <span>入梦调度广播</span>
                    </div>
                    <div class="leading-relaxed font-mono text-[10px]" :class="themeStore.isDarkResolved ? 'text-white/80' : 'text-gray-700'">{{ payloadCache[item.id].message }}</div>
                    <div class="text-[9px] font-mono pt-1 border-t"
                         :class="themeStore.isDarkResolved ? 'border-white/5 text-white/45' : 'border-gray-300/40 text-gray-550'">
                      计划调度成员: {{ payloadCache[item.id].agents?.join(', ') || 'None' }}
                    </div>
                  </div>
                </div>

                <!-- 兜底 JSON 显示 -->
                <div v-else class="mt-2">
                  <pre class="p-2.5 rounded border overflow-x-auto text-[10px] font-mono leading-relaxed select-text"
                       :class="themeStore.isDarkResolved 
                         ? 'bg-[#111620] border-white/5 text-slate-300' 
                         : 'bg-[#f1f3f5] border-gray-300/60 text-slate-700'">{{ JSON.stringify(payloadCache[item.id], null, 2) }}</pre>
                </div>
              </template>

            </div>
          </div>
        </template>
      </div>

      <!-- 底部栏 -->
      <div class="flex items-center justify-between px-4 py-2 border-t relative z-10"
           :class="themeStore.isDarkResolved ? 'bg-[#1a202c] border-white/5 text-white/40' : 'bg-[#edf0f2] border-gray-350/50 text-gray-500'">
        <div class="text-[9px] opacity-30 font-bold tracking-[0.2em] uppercase">
          VCPinfo 灵视引擎状态监听
        </div>
        <div class="text-[9px] font-mono" :class="themeStore.isDarkResolved ? 'text-white/40' : 'text-gray-500'">
          缓存数: {{ store.metadataList.length }}/500
        </div>
      </div>

    </div>
  </SlidePage>
</template>

<style scoped>
/* 针对 marked 渲染 of HTML 标签小排版 */
:deep(.prose) {
  font-size: 11px;
}
:deep(.prose p) {
  margin-top: 0.25rem;
  margin-bottom: 0.5rem;
}
:deep(.prose code) {
  font-family: monospace;
  background-color: rgba(120, 120, 120, 0.1);
  padding: 1px 3px;
  border-radius: 3px;
}
:deep(.prose ul) {
  list-style-type: disc;
  margin-left: 1rem;
  margin-bottom: 0.5rem;
}

/* 隐藏滚动条 */
.no-scrollbar::-webkit-scrollbar {
  display: none;
}
.no-scrollbar {
  -ms-overflow-style: none;
  scrollbar-width: none;
}

/* 去除滑动反弹 */
.no-rubber-band {
  overscroll-behavior: contain;
}

/* 优化长列表的渲染性能，跳过屏外卡片的布局计算 */
.vcp-info-card-item {
  content-visibility: auto;
  contain-intrinsic-size: 80px;
}
</style>
