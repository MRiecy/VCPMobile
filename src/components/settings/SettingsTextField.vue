<script setup lang="ts">
import { ref, computed } from 'vue';

const props = defineProps<{
  modelValue: string | number | undefined | null;
  label?: string;
  placeholder?: string;
  type?: string;
  mono?: boolean;
  disabled?: boolean;
  error?: boolean;
  center?: boolean;
  isSecure?: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: any): void;
  (e: 'blur'): void;
  (e: 'focus'): void;
}>();

const isMasked = ref(true);
const isFocused = ref(false);

const inputType = computed(() => {
  if (props.isSecure) {
    return 'text';
  }
  return props.type || 'text';
});

const isActuallyMasked = computed(() => {
  // 如果是安全模式，且处于遮罩状态，且没有获取焦点，则应用遮罩
  return props.isSecure && isMasked.value && !isFocused.value;
});

const onInput = (e: Event) => {
  const target = e.target as HTMLInputElement;
  emit('update:modelValue', target.value);
};

const onFocus = () => {
  isFocused.value = true;
  emit('focus');
};

const onBlur = () => {
  isFocused.value = false;
  emit('blur');
};

const toggleMask = () => {
  isMasked.value = !isMasked.value;
};
</script>

<template>
  <div class="settings-field flex flex-col gap-1.5 w-full">
    <label v-if="label" class="text-[10px] uppercase font-bold opacity-40 tracking-wider px-1">
      {{ label }}
    </label>
    <div class="relative group">
      <input :type="inputType" :value="modelValue ?? ''" :placeholder="placeholder" :disabled="disabled"
        autocomplete="off" autocapitalize="off" spellcheck="false"
        class="w-full bg-black/5 dark:bg-white/5 p-3.5 rounded-2xl outline-none border transition-all duration-200" :class="[
          mono || isSecure ? 'font-mono text-sm' : 'text-[14px]',
          center ? 'text-center' : '',
          error ? 'border-red-500/50 bg-red-500/5' : 'border-black/5 dark:border-white/5 focus:border-blue-500/50 focus:bg-black/10 dark:focus:bg-white/10',
          disabled ? 'opacity-40 cursor-not-allowed' : '',
          isSecure ? 'pr-12' : '',
          isActuallyMasked ? 'masked-input' : ''
        ]" @input="onInput" @blur="onBlur" @focus="onFocus" />

      <button v-if="isSecure" type="button" @click="toggleMask"
        class="absolute right-3 top-1/2 -translate-y-1/2 p-1.5 opacity-30 hover:opacity-100 transition-opacity active:scale-95">
        <svg v-if="isMasked" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
          stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
          <circle cx="12" cy="12" r="3"></circle>
        </svg>
        <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"
          stroke-linecap="round" stroke-linejoin="round">
          <path
            d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24">
          </path>
          <line x1="1" y1="1" x2="23" y2="23"></line>
        </svg>
      </button>

      <div v-if="error && !isSecure" class="absolute right-3 top-1/2 -translate-y-1/2 text-red-500 opacity-50 pointer-events-none">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
          stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10" />
          <line x1="12" y1="8" x2="12" y2="12" />
          <line x1="12" y1="16" x2="12.01" y2="16" />
        </svg>
      </div>
    </div>
  </div>
</template>

<style scoped>
.masked-input {
  -webkit-text-security: disc;
}
</style>
