<script setup lang="ts">
import type { ContentBlock } from "../../../core/composables/useContentProcessor";
import { computed } from "vue";

const props = defineProps<{
  block: ContentBlock;
}>();

const roleDisplay = computed(() => {
  const role = props.block.role || "unknown";
  return role.charAt(0).toUpperCase() + role.slice(1);
});

const actionText = computed(() => {
  return props.block.is_end ? "[结束]" : "[起始]";
});

const roleClass = computed(() => {
  return `role-${props.block.role?.toLowerCase() || "unknown"}`;
});

const typeClass = computed(() => {
  return props.block.is_end ? "type-end" : "type-start";
});
</script>

<template>
  <div class="vcp-role-divider" :class="[roleClass, typeClass]">
    <div class="divider-line"></div>
    <span class="divider-text">角色分界: {{ roleDisplay }} {{ actionText }}</span>
    <div class="divider-line"></div>
  </div>
</template>

<style scoped>
.vcp-role-divider {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 16px 0;
  opacity: 0.6;
  font-size: 11px;
  font-weight: bold;
  font-family: var(--font-mono, monospace);
  letter-spacing: 0.05em;
  user-select: none;
}

.divider-line {
  flex: 1;
  height: 1px;
  background: currentColor;
  opacity: 0.2;
}

.divider-text {
  white-space: nowrap;
  padding: 2px 8px;
  border: 1px solid currentColor;
  border-radius: 4px;
  background: rgba(var(--dynamic-color-rgb, 0, 0, 0), 0.05);
}

.role-system {
  color: #fbbf24; /* Amber */
}

.role-assistant {
  color: #3b82f6; /* Blue */
}

.role-user {
  color: #10b981; /* Emerald */
}

.type-end {
  opacity: 0.4;
  filter: grayscale(0.5);
}
</style>
