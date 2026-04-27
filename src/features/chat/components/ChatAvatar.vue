<script setup lang="ts">
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";
import { computed } from "vue";

const props = defineProps<{
  ownerType?: "user" | "agent" | "group";
  ownerId?: string;
  imageUrl?: string | null; // 保持兼容，但优先使用上面的显式参数
  fallbackText: string;
  isUser: boolean;
  borderColor?: string;
  fallbackColor?: string;
}>();

const ownerInfo = computed(() => {
  // 1. 优先使用显式参数
  if (props.ownerType && props.ownerId) {
    return { type: props.ownerType, id: props.ownerId };
  }

  // 2. 其次尝试解析 imageUrl (兼容旧代码)
  if (props.imageUrl && props.imageUrl.startsWith('vcp-avatar://')) {
    const parts = props.imageUrl.replace('vcp-avatar://', '').split('/');
    if (parts.length >= 2) {
      return { 
        type: parts[0] as "user" | "agent" | "group", 
        id: parts[1].split('?')[0] 
      };
    }
  }

  // 3. 如果是用户且没有 ID，使用默认 user 标识
  if (props.isUser) {
    return { type: 'user' as const, id: 'user_avatar' };
  }

  return null;
});
</script>

<template>
  <div class="flex-shrink-0 w-9 h-9">
    <VcpAvatar 
      v-if="ownerInfo"
      :owner-type="ownerInfo.type"
      :owner-id="ownerInfo.id"
      :fallback-name="fallbackText"
      size="w-9 h-9"
      rounded="rounded-full"
      :style="{ borderColor: borderColor || 'transparent' }"
      class="border"
    />
    <div v-else class="w-full h-full rounded-full overflow-hidden border transition-all duration-500 shadow-sm flex items-center justify-center text-xs font-bold text-white" 
      :class="isUser ? 'bg-primary border-primary' : 'bg-white dark:bg-gray-800'"
      :style="[
        !isUser ? { borderColor: borderColor || 'transparent' } : {},
        {
          backgroundColor: isUser
            ? fallbackColor || 'var(--primary)'
            : fallbackColor || '#374151',
        }
      ]">
      {{ fallbackText.charAt(0).toUpperCase() }}
    </div>
  </div>
</template>
