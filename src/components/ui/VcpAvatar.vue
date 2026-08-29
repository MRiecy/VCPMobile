<script setup lang="ts">
import { ref, watchEffect, computed } from "vue";
import { useAvatarStore } from "../../core/stores/avatar";

export interface AvatarTarget {
  id: string;
  type: "user" | "agent" | "group";
  name?: string;
  avatarCalculatedColor?: string | null;
}

const props = defineProps<{
  target?: AvatarTarget | null;
  ownerType?: "user" | "agent" | "group";
  ownerId?: string;
  version?: number;
  fallbackName?: string;
  size?: string; // 如 'w-10 h-10'
  rounded?: string; // 如 'rounded-xl'
  dominantColor?: string | null;
}>();

const avatarStore = useAvatarStore();
const avatarUrl = ref("");
const imgExists = ref(false);
const isNearViewport = ref(false);

const handleIntersect = () => {
  isNearViewport.value = true;
};

const handleUnintersect = () => {
  isNearViewport.value = false;
  // 离开预加载区后卸载图片元素，释放 WebView 解码位图；压缩 Blob 仍由 Store LRU 管理。
  avatarUrl.value = "";
  imgExists.value = false;
};

// 解析属性值，优先从 target 中提取
const resolvedType = computed(() => props.target?.type || props.ownerType || "agent");
const resolvedId = computed(() => props.target?.id || props.ownerId || "");
const resolvedFallbackName = computed(() => props.target?.name || props.fallbackName || "");
const resolvedColor = computed(() => props.target?.avatarCalculatedColor || props.dominantColor || null);

// 处理主色调边框
const borderStyle = computed(() => {
  const color = resolvedColor.value;
  if (!color) return {};
  return {
    // 动态 inline style 无法像样式表一样提供 color-mix 声明级 fallback。
    // 直接使用已计算的主题色，确保旧 WebView 仍保留头像边界。
    borderColor: color,
    boxShadow: `0 0 8px ${color}33` // 减弱发光
  };
});

// 提取首字母用于 Fallback
const initial = computed(() => {
  const name = resolvedFallbackName.value || resolvedId.value || "?";
  return name.trim().charAt(0).toUpperCase();
});

// 根据 ID 生成一个确定的背景色，防止所有 Fallback 都一个颜色
const fallbackBg = computed(() => {
  if (resolvedColor.value) return resolvedColor.value;
  const colors = [
    "rgb(226, 54, 56)", // VCP Red
    "rgb(59, 130, 246)", // Blue
    "rgb(16, 185, 129)", // Green
    "rgb(245, 158, 11)", // Amber
    "rgb(139, 92, 246)", // Violet
  ];
  let hash = 0;
  const id = resolvedId.value;
  for (let i = 0; i < id.length; i++) {
    hash = id.charCodeAt(i) + ((hash << 5) - hash);
  }
  return colors[Math.abs(hash) % colors.length];
});

watchEffect(async (onCleanup) => {
  let cancelled = false;
  onCleanup(() => {
    cancelled = true;
  });
  const ownerIdVal = resolvedId.value;
  const ownerTypeVal = resolvedType.value;
  if (!ownerIdVal) {
    avatarUrl.value = "";
    imgExists.value = false;
    return;
  }
  if (!isNearViewport.value) return;
  
  const reqVersion = props.version || 0;

  const existing = avatarStore.getCachedAvatar(ownerTypeVal, ownerIdVal, reqVersion);
  if (existing) {
    avatarUrl.value = existing.blobUrl;
    imgExists.value = true;
    return;
  }

  imgExists.value = false;
  const url = await avatarStore.getAvatarUrl(ownerTypeVal, ownerIdVal, reqVersion);
  if (
    cancelled ||
    !isNearViewport.value ||
    resolvedId.value !== ownerIdVal ||
    resolvedType.value !== ownerTypeVal
  ) return;
  if (url) {
    avatarUrl.value = url;
    imgExists.value = true;
  } else {
    imgExists.value = false;
  }
});

const handleImgError = () => {
  imgExists.value = false;
};
</script>

<template>
  <div :class="[
    size || 'w-10 h-10', 
    rounded || 'rounded-xl',
    'relative overflow-hidden flex-shrink-0 flex items-center justify-center shadow-inner transition-all duration-500',
    resolvedColor ? 'border' : 'border border-black/15 dark:border-white/20'
  ]" :style="borderStyle"
    v-intersection-observer="{ rootMargin: '300px 0px', threshold: 0.01 }"
    @intersect="handleIntersect"
    @unintersect="handleUnintersect">
    <!-- Fallback 占位 (底层) -->
    <div 
      class="absolute inset-0 flex items-center justify-center text-white font-bold select-none"
      :style="{ backgroundColor: fallbackBg }"
      :class="size?.includes('w-16') ? 'text-xl' : 'text-sm'"
    >
      {{ initial }}
    </div>

    <!-- 头像图片 (顶层，靠 DOM 顺序自然覆盖) -->
    <img 
      v-if="imgExists && avatarUrl" 
      :src="avatarUrl" 
      draggable="false"
      @error="handleImgError" 
      class="relative w-full h-full object-cover transition-opacity duration-300"
    />
  </div>
</template>
