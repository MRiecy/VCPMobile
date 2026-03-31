<script setup lang="ts">
defineProps<{
  modelValue: string | number | undefined | null;
  label?: string;
  placeholder?: string;
  type?: string;
  mono?: boolean;
  disabled?: boolean;
  error?: boolean;
  center?: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: any): void;
  (e: 'blur'): void;
  (e: 'focus'): void;
}>();

const onInput = (e: Event) => {
  const target = e.target as HTMLInputElement;
  emit('update:modelValue', target.value);
};
</script>

<template>
  <div class="settings-field flex flex-col gap-1.5 w-full">
    <label v-if="label" class="text-[10px] uppercase font-bold opacity-40 tracking-wider px-1">
      {{ label }}
    </label>
    <div class="relative group">
      <input 
        :type="type || 'text'"
        :value="modelValue ?? ''"
        :placeholder="placeholder"
        :disabled="disabled"
        class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border transition-all duration-200"
        :class="[
          mono ? 'font-mono text-sm' : 'text-[14px]',
          center ? 'text-center' : '',
          error ? 'border-red-500/50 bg-red-500/5' : 'border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10',
          disabled ? 'opacity-40 cursor-not-allowed' : ''
        ]"
        @input="onInput"
        @blur="emit('blur')"
        @focus="emit('focus')"
      />
      <div v-if="error" class="absolute right-3 top-1/2 -translate-y-1/2 text-red-500 opacity-50">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
      </div>
    </div>
  </div>
</template>
