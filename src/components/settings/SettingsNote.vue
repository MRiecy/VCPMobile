<script setup lang="ts">
defineProps<{
  text?: string;
}>();
</script>

<template>
  <blockquote class="settings-note">
    <slot>{{ text }}</slot>
  </blockquote>
</template>

<style scoped>
.settings-note {
  margin: 0;
  padding: 2px 0 2px 8px;
  border-left: 2px solid var(--border-color);
  font-size: 12px;
  line-height: 1.5;
  color: var(--secondary-text);
  opacity: 0.78;
}
</style>
