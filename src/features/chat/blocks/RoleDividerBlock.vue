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
    <span class="divider-text">角色分界: {{ roleDisplay }} {{ actionText }}</span>
  </div>
</template>

<style>
/* VCP Role Divide Styles (Ported from VChat / styles/messageRenderer.css) */
.vcp-role-divider {
  display: flex;
  align-items: center;
  justify-content: center;
  margin: 15px 0;
  font-size: 0.85em;
  color: var(--primary-text);
  opacity: 0.7;
  user-select: none;
  clear: both;
}

.vcp-role-divider::before,
.vcp-role-divider::after {
  content: "";
  flex: 1;
  border-bottom: 1px dashed var(--border-color, #ccc);
  margin: 0 15px;
}

.vcp-role-divider.role-system {
  color: #e67e22;
}

.vcp-role-divider.role-assistant {
  color: #3498db;
}

.vcp-role-divider.role-user {
  color: #2ecc71;
}

.vcp-role-divider.type-end {
  opacity: 0.5;
}
</style>
