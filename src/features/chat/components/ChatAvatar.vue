<script setup lang="ts">
import VcpAvatar from "../../../components/ui/VcpAvatar.vue";
import { computed } from "vue";

const props = defineProps<{
  imageUrl?: string | null;
  fallbackText: string;
  isUser: boolean;
  borderColor?: string;
  fallbackColor?: string;
}>();

const ownerInfo = computed(() => {
  if (!props.imageUrl) return null;
  if (props.imageUrl.startsWith('vcp-avatar://')) {
    const parts = props.imageUrl.replace('vcp-avatar://', '').split('/');
    if (parts.length >= 2) {
      // 提取 ownerType (user/agent/group) 和 ownerId
      return { 
        type: parts[0] as "user" | "agent" | "group", 
        id: parts[1].split('?')[0] 
      };
    }
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
