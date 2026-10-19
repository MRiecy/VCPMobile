<script setup lang="ts">
import { useId } from 'vue';

withDefaults(defineProps<{
  modelValue: string;
  label: string;
  description?: string;
  placeholder?: string;
  disabled?: boolean;
  rows?: number;
}>(), { rows: 4 });
const emit = defineEmits<{ 'update:modelValue': [value: string] }>();
const id = useId();
</script>

<template>
  <div class="group-text-field">
    <label :for="id" class="text-xs text-[var(--secondary-text)]">{{ label }}</label>
    <textarea :id="id" :value="modelValue" :disabled="disabled" :rows="rows" :placeholder="placeholder"
      :aria-describedby="description ? `${id}-help` : undefined" class="group-text-input vcp-scrollable"
      @input="emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)" />
    <p v-if="description" :id="`${id}-help`" class="m-0 text-xs leading-relaxed text-[var(--secondary-text)]">{{ description }}</p>
  </div>
</template>

<style scoped>
.group-text-field { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.group-text-input {
  width: 100%; min-height: 88px; padding: 8px 12px; resize: vertical;
  border: 1px solid var(--border-color); border-radius: 8px;
  background: var(--input-bg, transparent); color: var(--primary-text);
  font: inherit; font-size: var(--settings-input-font-size, 16px); line-height: 1.6;
}
.group-text-input:focus-visible { outline: 2px solid var(--highlight-text, var(--primary-text)); outline-offset: 2px; }
.group-text-input:disabled { opacity: 0.5; }
</style>
