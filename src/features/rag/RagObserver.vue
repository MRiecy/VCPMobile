<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue';
import { X, Trash2, ChevronDown, ChevronUp, Copy, Loader2, Sparkles, Check } from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import { useRagObserverStore } from '../../core/stores/ragObserver';
import { marked } from 'marked';

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
const payloadCache = ref<Record<string, any>>({});
const payloadLoading = ref<Record<string, boolean>>({});
const copiedCardId = ref<string | null>(null);

// 频谱 Canvas 绘图相关
const spectrumCanvas = ref<HTMLCanvasElement | null>(null);
let animationFrameId: number | null = null;
const numBars = 24;
let barsHeights = Array(numBars).fill(4);
let targetHeights = Array(numBars).fill(4);

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
  if (expandedCardIds.value.has(id)) {
    expandedCardIds.value.delete(id);
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
  if (type === 'AGENT_PRIVATE_CHAT_PREVIEW') return 'border-l-green-500';
  if (type === 'AI_MEMO_RETRIEVAL' || type === 'DailyNote') return 'border-l-orange-500';
  if (type.startsWith('AGENT_DREAM_')) return 'border-l-yellow-500';
  return 'border-l-blue-500'; // RAG 默认蓝色
};

// 安全解析 HTML
const renderMarkdown = (text: string) => {
  try {
    return marked.parse(text) as string;
  } catch (e) {
    return text;
  }
};

// 24柱全宽跳动音频频谱微动画
const drawSpectrum = () => {
  const canvas = spectrumCanvas.value;
  if (!canvas) return;
  const ctx = canvas.getContext('2d');
  if (!ctx) return;

  const render = () => {
    if (!canvas) return;
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

    for (let i = 0; i < numBars; i++) {
      if (isAnimating) {
        if (Math.abs(barsHeights[i] - targetHeights[i]) < 1) {
          targetHeights[i] = Math.floor(Math.random() * (height - 2 * dpr)) + 2 * dpr;
        }
        barsHeights[i] += (targetHeights[i] - barsHeights[i]) * 0.15;
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
    <div class="fixed inset-0 flex flex-col bg-[#0a0f14] text-white overflow-hidden"
         :class="{ 'pointer-events-none': !props.isOpen }">

      <!-- 头部状态栏 -->
      <div class="flex items-center justify-between px-4 pt-[calc(env(safe-area-inset-top)+8px)] pb-3 bg-[#0e161f]/80 backdrop-blur-md">
        <div class="flex items-center gap-3">
          <div class="flex items-center gap-2">
            <div class="w-2 h-2 rounded-full" :class="statusDotClass"></div>
            <span class="text-[10px] font-bold uppercase tracking-widest text-white/70">{{ statusLabel }}</span>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <span class="text-xs font-bold tracking-wider text-white/50 flex items-center gap-1">
            <Sparkles :size="12" class="text-blue-400" />
            灵视中心
          </span>
        </div>

        <div class="flex items-center gap-2">
          <!-- 清空 -->
          <button @click="store.clearAll()" class="p-2 text-gray-400 hover:text-white transition-colors" title="清空全部">
            <Trash2 :size="16" />
          </button>
          <!-- 关闭 -->
          <button @click="emit('close')" class="p-2 -mr-2 text-gray-400 hover:text-white transition-colors">
            <X :size="20" class="opacity-80" />
          </button>
        </div>
      </div>

      <!-- 全宽频谱 Canvas 跳动动画，作为顶栏与内容区的霓虹分割线 -->
      <div class="relative w-full h-[6px] bg-[#0a0f14]">
        <canvas ref="spectrumCanvas" class="w-full h-full block opacity-80 pointer-events-none"></canvas>
      </div>

      <!-- 横向滑动选项卡 Tab -->
      <div class="flex gap-2 px-3 py-2.5 overflow-x-auto no-scrollbar border-b border-white/5 bg-[#0a0f14]">
        <button
          v-for="tab in filterTabs"
          :key="tab.value"
          @click="activeFilter = tab.value"
          class="shrink-0 px-3 py-1 rounded-full text-[11px] font-bold tracking-wider transition-all"
          :class="activeFilter === tab.value
            ? 'bg-blue-500/25 text-blue-400 border border-blue-500/40 shadow-[0_0_8px_rgba(52,152,219,0.25)]'
            : 'bg-white/5 text-white/40 border border-transparent hover:text-white/70'"
        >
          {{ tab.label }}
        </button>
      </div>

      <!-- 消息列表区 -->
      <div class="flex-1 overflow-y-auto no-rubber-band px-3 py-2 bg-[#090d12]">
        <!-- 空白占位 -->
        <div v-if="filteredMetadataList.length === 0" class="flex flex-col items-center justify-center py-20 text-white/20">
          <Sparkles :size="32" class="mb-4 stroke-[1.5] text-white/10" />
          <span class="text-xs tracking-wider">暂无灵视认知广播数据</span>
          <span class="text-[9px] mt-1 opacity-50">等待 AI 进行思考或检索...</span>
        </div>

        <template v-else>
          <div
            v-for="item in filteredMetadataList"
            :key="item.id"
            class="mb-3 border-l-2 bg-[#101720]/80 rounded-r border border-white/5 transition-all overflow-hidden"
            :class="[getAccentClass(item.type), { 'ring-1 ring-white/10': expandedCardIds.has(item.id) }]"
          >
            <!-- 折叠栏：Metadata 显示 -->
            <div
              @click="toggleCard(item.id)"
              class="flex items-start justify-between p-3 active:bg-white/5 transition-colors cursor-pointer select-none"
            >
              <div class="flex-1 min-w-0 pr-2">
                <div class="flex items-center justify-between mb-1.5">
                  <span class="text-[12px] font-bold tracking-wide text-white/95 truncate">
                    {{ item.title }}
                  </span>
                  <span class="text-[9px] font-mono text-white/30 shrink-0 ml-2">
                    {{ new Date(item.timestamp).toLocaleTimeString() }}
                  </span>
                </div>
                <div v-if="item.subtitle" class="text-[9px] font-mono font-bold tracking-wider text-blue-400/80 mb-1">
                  {{ item.subtitle }}
                </div>
                <!-- 折叠态的 summary 预览 -->
                <div v-if="!expandedCardIds.has(item.id)" class="text-[11px] text-white/45 truncate leading-relaxed">
                  {{ item.summary }}
                </div>
              </div>

              <!-- 展开/折叠指示图标 -->
              <div v-if="item.hasDetails" class="text-white/30 shrink-0 pt-0.5">
                <ChevronUp v-if="expandedCardIds.has(item.id)" :size="16" />
                <ChevronDown v-else :size="16" />
              </div>
            </div>

            <!-- 展开栏：Payload Lazy 加载与结构化渲染 -->
            <div v-if="expandedCardIds.has(item.id)" class="border-t border-white/5 bg-black/30 p-3 text-[11px]">
              
              <!-- 1. 加载中 -->
              <div v-if="payloadLoading[item.id]" class="flex items-center justify-center py-4 text-white/30 gap-2">
                <Loader2 :size="14" class="animate-spin" />
                <span>正在提取饱水 Payload 详情...</span>
              </div>

              <!-- 2. 加载失败 -->
              <div v-else-if="!payloadCache[item.id]" class="flex flex-col items-center justify-center py-4 gap-2 text-red-400">
                <span>⚠️ 获取详情失败或文件已被清理</span>
                <button
                  @click.stop="toggleCard(item.id); toggleCard(item.id)"
                  class="px-2 py-0.5 rounded border border-red-400/40 text-[9px] hover:bg-red-400/10 transition-colors"
                >
                  重试
                </button>
              </div>

              <!-- 3. 加载成功 - 渲染详情 -->
              <template v-else>
                <div class="flex justify-between items-center mb-3 pb-2 border-b border-white/5">
                  <span class="text-[9px] font-mono text-white/30">ID: {{ item.id }}</span>
                  <!-- 复制按钮 -->
                  <button
                    @click="copyPayload(item.id, $event)"
                    class="flex items-center gap-1 px-2 py-0.5 rounded border border-white/10 hover:bg-white/10 text-[9px] text-white/50 transition-all active:scale-95"
                  >
                    <Check v-if="copiedCardId === item.id" :size="10" class="text-green-400" />
                    <Copy v-else :size="10" />
                    <span>{{ copiedCardId === item.id ? '已复制' : '复制完整JSON' }}</span>
                  </button>
                </div>

                <!-- 各种数据类型针对性排版 -->
                <!-- RAG 知识库检索结果详情 -->
                <div v-if="item.type === '' || item.type === 'RAG_RETRIEVAL_DETAILS'" class="space-y-3">
                  <div v-if="payloadCache[item.id].query" class="bg-blue-500/5 p-2 rounded border border-blue-500/10 mb-2">
                    <div class="text-[9px] font-bold text-blue-400/80 mb-1">RAG 提问词:</div>
                    <div class="text-white/80 leading-relaxed break-words font-mono">{{ payloadCache[item.id].query }}</div>
                  </div>
                  <div class="text-[9px] font-bold uppercase tracking-widest text-white/20 mb-2">召回结果列表 ({{ payloadCache[item.id].results?.length || 0 }})</div>
                  <div
                    v-for="(res, idx) in payloadCache[item.id].results"
                    :key="idx"
                    class="p-2.5 rounded bg-white/5 border border-white/5"
                  >
                    <div class="flex justify-between items-center mb-1 text-[9px] text-white/45">
                      <span class="px-1 py-0.5 rounded bg-blue-500/20 text-blue-400 font-bold font-mono">Score: {{ res.score?.toFixed(3) || 'Time' }}</span>
                      <span class="font-mono">来源: {{ res.source || 'Unknown' }}</span>
                    </div>
                    <!-- 召回原文 -->
                    <div class="text-white/80 leading-relaxed whitespace-pre-wrap break-words font-mono select-text mt-1.5">{{ res.text }}</div>
                  </div>
                </div>

                <!-- 元思考链详情 -->
                <div v-else-if="item.type === 'META_THINKING_CHAIN'" class="space-y-3">
                  <div class="text-[9px] font-bold uppercase tracking-widest text-white/20 mb-2">阶段执行追踪</div>
                  <div
                    v-for="stage in payloadCache[item.id].stages"
                    :key="stage.stage"
                    class="border-l border-purple-500/50 pl-2.5 space-y-2 mb-3"
                  >
                    <div class="text-[10px] font-bold text-purple-300">
                      阶段 {{ stage.stage }}: {{ stage.clusterName }} (命中 {{ stage.resultCount }} 个结果)
                    </div>
                    <div
                      v-for="(res, rIdx) in stage.results"
                      :key="rIdx"
                      class="p-2 rounded bg-white/5 border border-white/5 font-mono text-[9px]"
                    >
                      <div class="text-white/40 mb-1 flex justify-between">
                        <span>匹配度: {{ res.score?.toFixed(3) || 'N/A' }}</span>
                        <span>来源: {{ res.source }}</span>
                      </div>
                      <div class="text-white/80 leading-relaxed whitespace-pre-wrap break-words select-text">{{ res.text }}</div>
                    </div>
                  </div>
                </div>

                <!-- Agent 私聊 -->
                <div v-else-if="item.type === 'AGENT_PRIVATE_CHAT_PREVIEW'" class="space-y-2">
                  <div class="border-l border-white/10 pl-2">
                    <div class="text-[9px] text-white/35 font-bold mb-1">USER</div>
                    <div class="text-white/80 leading-relaxed whitespace-pre-wrap break-words select-text font-mono">{{ payloadCache[item.id].query }}</div>
                  </div>
                  <div class="border-l border-green-500/50 pl-2 mt-2">
                    <div class="text-[9px] text-green-400 font-bold mb-1">AI RESPONSE</div>
                    <div class="text-white/85 leading-relaxed whitespace-pre-wrap break-words select-text font-mono">{{ payloadCache[item.id].response }}</div>
                  </div>
                </div>

                <!-- 记忆检索 (AIMemo) -->
                <div v-else-if="item.type === 'AI_MEMO_RETRIEVAL'" class="space-y-2">
                  <div v-if="payloadCache[item.id].query" class="bg-orange-500/5 p-2 rounded border border-orange-500/10">
                    <div class="text-[9px] font-bold text-orange-400/80 mb-1">检索提问:</div>
                    <div class="text-white/80 leading-relaxed font-mono">{{ payloadCache[item.id].query }}</div>
                  </div>
                  <div class="bg-black/30 p-2.5 rounded border border-white/5 mt-2">
                    <div class="text-[9px] text-orange-400 font-bold mb-1">提炼出的联合记忆:</div>
                    <!-- 使用 marked 渲染提炼记忆 -->
                    <div
                      class="prose prose-invert prose-xs text-white/80 leading-relaxed select-text font-mono"
                      v-html="renderMarkdown(payloadCache[item.id].extractedMemories || '')"
                    ></div>
                  </div>
                </div>

                <!-- Agent 梦境叙事 -->
                <div v-else-if="item.type === 'AGENT_DREAM_NARRATIVE'" class="space-y-2 select-text font-mono">
                  <div class="text-[9px] text-yellow-400 font-bold mb-1">梦境叙事:</div>
                  <div
                    class="prose prose-invert prose-xs text-white/85 leading-relaxed whitespace-pre-wrap break-words"
                    v-html="renderMarkdown(payloadCache[item.id].narrative || '')"
                  ></div>
                </div>

                <!-- 梦境联想 -->
                <div v-else-if="item.type === 'AGENT_DREAM_ASSOCIATIONS'" class="space-y-3">
                  <div class="text-[9px] font-bold uppercase tracking-widest text-white/20">入梦触发种子 ({{ payloadCache[item.id].seedCount }})</div>
                  <div class="space-y-1">
                    <div
                      v-for="(seed, idx) in payloadCache[item.id].seeds"
                      :key="idx"
                      class="p-1.5 rounded bg-white/5 text-[9px] font-mono text-white/60"
                    >
                      <div class="font-bold text-white/80 truncate">{{ seed.file }}</div>
                      <div class="text-white/40 truncate">{{ seed.snippet }}</div>
                    </div>
                  </div>
                  <div class="text-[9px] font-bold uppercase tracking-widest text-white/20 mt-2">梦境联想共Resonance ({{ payloadCache[item.id].associationCount }})</div>
                  <div class="space-y-1">
                    <div
                      v-for="(assoc, idx) in payloadCache[item.id].associations"
                      :key="idx"
                      class="flex justify-between items-center p-1.5 rounded bg-white/5 text-[9px] font-mono text-white/50"
                    >
                      <span class="truncate text-white/70">{{ assoc.file }}</span>
                      <span class="shrink-0 text-yellow-400/80">相似度: {{ assoc.score }}</span>
                    </div>
                  </div>
                </div>

                <!-- 兜底 JSON 显示 -->
                <div v-else class="mt-2">
                  <pre class="bg-black/40 p-2.5 rounded border border-white/5 overflow-x-auto text-[10px] font-mono leading-relaxed select-text text-white/70">{{ JSON.stringify(payloadCache[item.id], null, 2) }}</pre>
                </div>
              </template>

            </div>
          </div>
        </template>
      </div>

      <!-- 底部栏 -->
      <div class="flex items-center justify-between px-4 py-2 border-t border-white/5 bg-[#0e161f]/80 backdrop-blur-md">
        <div class="text-[9px] opacity-30 font-bold tracking-[0.2em] uppercase">
          VCPinfo 灵视引擎状态监听
        </div>
        <div class="text-[9px] text-white/40 font-mono">
          缓存数: {{ store.metadataList.length }}/500
        </div>
      </div>

    </div>
  </SlidePage>
</template>

<style scoped>
/* 针对 marked 渲染的 HTML 标签小排版 */
:deep(.prose) {
  font-size: 11px;
}
:deep(.prose p) {
  margin-top: 0.25rem;
  margin-bottom: 0.5rem;
}
:deep(.prose code) {
  font-family: monospace;
  background-color: rgba(255, 255, 255, 0.1);
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
</style>
