<script setup lang="ts">
import { ref, watchEffect, computed } from "vue";
import { useAvatarStore } from "../../core/stores/avatar";

const props = defineProps<{
  ownerType: "user" | "agent" | "group";
  ownerId: string;
  version?: number;
  fallbackName?: string;
  size?: string; // 如 'w-10 h-10'
  rounded?: string; // 如 'rounded-xl'
  outerBorder?: boolean;
  dominantColor?: string | null;
}>();

const avatarStore = useAvatarStore();
const avatarUrl = ref("");
const imgExists = ref(false);

// 处理主色调边框
const borderStyle = computed(() => {
  if (!props.outerBorder) return {};
  const color = props.dominantColor || 'var(--primary)';
  // 使用 color-mix 混合出 60% 不透明度的边框，使其更自然地融入背景
  const mixedColor = `color-mix(in srgb, ${color} 60%, transparent)`;
  return {
    borderColor: mixedColor,
    boxShadow: `0 0 8px ${color}33` // 减弱发光
  };
});

// 提取首字母用于 Fallback
const initial = computed(() => {
  const name = props.fallbackName || props.ownerId || "?";
  return name.trim().charAt(0).toUpperCase();
});

// 根据 ID 生成一个确定的背景色，防止所有 Fallback 都一个颜色
const fallbackBg = computed(() => {
  const colors = [
    "rgb(226, 54, 56)", // VCP Red
    "rgb(59, 130, 246)", // Blue
    "rgb(16, 185, 129)", // Green
    "rgb(245, 158, 11)", // Amber
    "rgb(139, 92, 246)", // Violet
  ];
  let hash = 0;
  for (let i = 0; i < props.ownerId.length; i++) {
    hash = props.ownerId.charCodeAt(i) + ((hash << 5) - hash);
  }
  return colors[Math.abs(hash) % colors.length];
});

watchEffect(async () => {
  if (!props.ownerId) {
    avatarUrl.value = "";
    imgExists.value = false;
    return;
  }
  
  const key = `${props.ownerType}:${props.ownerId}`;
  const reqVersion = props.version || 0;

  // 核心修复：同步检查缓存。如果命中且不需要强制刷新，立即显示，消除“顿一下”的感觉。
  const existing = avatarStore.cache.get(key);
  if (existing && (reqVersion === 0 || existing.version >= reqVersion)) {
    avatarUrl.value = existing.blobUrl;
    imgExists.value = true;
    return;
  }

  // 缓存未命中或版本过旧，再进入异步获取逻辑
  const url = await avatarStore.getAvatarUrl(props.ownerType, props.ownerId, reqVersion);
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
    'relative overflow-hidden flex-shrink-0 flex items-center justify-center bg-black/5 dark:bg-white/5 border shadow-inner transition-all duration-500',
    outerBorder ? 'border' : 'border-black/5 dark:border-white/10'
  ]" :style="borderStyle">
    <!-- Fallback 占位 (底层) -->
    <div 
      class="absolute inset-0 flex items-center justify-center text-white font-bold select-none"
      :style="{ backgroundColor: fallbackBg }"
      :class="[size?.includes('w-16') ? 'text-xl' : 'text-sm']"
    >
      {{ initial }}
    </div>

    <!-- 头像图片 (顶层，靠 DOM 顺序自然覆盖) -->
    <img 
      v-if="imgExists && avatarUrl" 
      :src="avatarUrl" 
      @error="handleImgError" 
      class="relative w-full h-full object-cover transition-opacity duration-300"
      :class="imgExists ? 'opacity-100' : 'opacity-0'"
    />
  </div>
</template>
