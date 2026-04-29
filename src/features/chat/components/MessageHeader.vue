<script setup lang="ts">
import ChatAvatar from "./ChatAvatar.vue";

defineProps<{
  isUser: boolean;
  ownerType?: "user" | "agent" | "group";
  ownerId?: string;
  displayName?: string | null;
  nameStyle?: Record<string, string>;
  avatarUrl?: string | null;
  avatarFallbackText: string;
  avatarFallbackColor?: string;
}>();
</script>

<template>
  <div class="flex items-center gap-2 mb-1.5 px-1 w-full" :class="isUser ? 'flex-row-reverse' : 'flex-row'">
    <ChatAvatar 
      :owner-type="ownerType"
      :owner-id="ownerId"
      :image-url="avatarUrl" 
      :fallback-text="avatarFallbackText" 
      :is-user="isUser"
      :fallback-color="avatarFallbackColor" 
    />

    <div v-if="displayName" class="text-[11px] font-bold tracking-widest opacity-80" :style="nameStyle">
      {{ displayName }}
    </div>
  </div>
</template>
