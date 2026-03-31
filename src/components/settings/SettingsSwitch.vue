<script setup lang="ts">
const props = defineProps<{
  modelValue: boolean;
  disabled?: boolean;
  activeColor?: string;
}>();

const emit = defineEmits<{
  (e: 'update:modelValue', value: boolean): void;
}>();

const toggle = () => {
  if (!props.disabled) {
    emit('update:modelValue', !props.modelValue);
  }
};
</script>

<template>
  <div 
    class="settings-switch relative inline-flex items-center cursor-pointer transition-all duration-300"
    :class="[
      disabled ? 'opacity-30 cursor-not-allowed' : 'active:scale-90'
    ]"
    @click="toggle"
  >
    <div 
      class="w-10 h-5 rounded-full transition-colors duration-300 relative"
      :class="[
        modelValue ? (activeColor || 'bg-blue-500') : 'bg-black/10 dark:bg-white/10'
      ]"
    >
      <div 
        class="absolute top-[2px] left-[2px] bg-white rounded-full h-4 w-4 transition-transform duration-300 shadow-sm"
        :class="modelValue ? 'translate-x-5' : 'translate-x-0'"
      ></div>
    </div>
  </div>
</template>
