<script setup lang="ts">
const props = defineProps<{
  modelValue: boolean;
  disabled?: boolean;
  loading?: boolean;
  activeColor?: string;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void;
}>();

const toggle = () => {
  if (!props.disabled && !props.loading) {
    emit('update:modelValue', !props.modelValue);
  }
};
</script>

<template>
  <button type="button" role="switch" :aria-checked="modelValue" :aria-busy="loading || undefined"
    :disabled="disabled || loading"
    class="settings-switch relative inline-flex min-h-11 min-w-11 items-center justify-center bg-transparent border-0 p-0 cursor-pointer transition-opacity duration-200"
    :class="disabled || loading ? 'opacity-30 cursor-not-allowed' : 'active:opacity-70'" @click="toggle">
    <span aria-hidden="true" class="w-10 h-5 rounded-full transition-colors duration-300 relative" :class="[
      modelValue ? (activeColor || 'bg-blue-500') : 'bg-black/10 dark:bg-white/10'
    ]">
      <span
        class="absolute top-[2px] left-[2px] bg-white rounded-full h-4 w-4 transition-transform duration-300 shadow-sm"
        :class="modelValue ? 'translate-x-5' : 'translate-x-0'"></span>
    </span>
  </button>
</template>
