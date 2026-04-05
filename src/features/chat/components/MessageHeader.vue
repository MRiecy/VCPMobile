<script setup lang="ts">
import ChatAvatar from "./ChatAvatar.vue";

defineProps<{
  isUser: boolean;
  displayName?: string | null;
  nameStyle?: Record<string, string>;
  avatarUrl?: string | null;
  avatarBorderColor?: string;
  avatarFallbackText: string;
}>();
</script>

<template>
  <div
    class="flex items-center gap-2 mb-1.5 px-1 w-full"
    :class="isUser ? 'flex-row-reverse' : 'flex-row'"
  >
    <ChatAvatar
      :image-url="avatarUrl"
      :fallback-text="avatarFallbackText"
      :is-user="isUser"
      :border-color="avatarBorderColor"
    />

    <div
      v-if="displayName"
      class="text-[11px] font-bold tracking-widest opacity-80"
      :style="nameStyle"
    >
      {{ displayName }}
    </div>
  </div>
</template>
