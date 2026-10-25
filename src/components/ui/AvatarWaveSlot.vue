<script setup lang="ts">
withDefaults(defineProps<{
  active?: boolean;
  disabled?: boolean;
  dataGenerating?: string | boolean;
}>(), {
  active: false,
  disabled: false,
  dataGenerating: undefined,
});
</script>

<template>
  <div
    class="vcp-avatar-wave-slot relative rounded-full"
    :class="{
      'is-active': active,
      'is-generating': active,
    }"
    :data-wave-active="active ? 'true' : 'false'"
    :data-generating="dataGenerating !== undefined ? String(dataGenerating) : (active ? 'true' : 'false')"
  >
    <!-- 头像表面容器（活跃时自动微缩与调暗滤镜） -->
    <div v-if="$slots.default" class="vcp-avatar-wave-surface w-full h-full rounded-full overflow-hidden">
      <slot />
    </div>

    <!-- 精致音波指示器动效覆盖层 -->
    <div
      v-if="!disabled"
      class="vcp-avatar-wave-indicator vcp-message-avatar-indicator vcp-sidebar-avatar-indicator absolute inset-0 rounded-full overflow-hidden flex items-center justify-center pointer-events-none"
      aria-hidden="true"
    >
      <div class="vignette-mask absolute inset-0"></div>
      <svg class="relative z-10 w-[60%] h-[60%]" viewBox="0 0 24 24" fill="none">
        <line class="axis-line" x1="2.5" y1="12" x2="21.5" y2="12" stroke="rgba(255,255,255,0.34)" stroke-width="0.9" stroke-linecap="round" />
        <g transform="rotate(7 12 12)">
          <rect class="interlace-up st-2" x="4.8" y="9.2" width="1.65" height="5.6" rx="0.82" fill="rgba(255,255,255,0.68)" />
          <rect class="interlace-down st-1" x="8.1" y="6.0" width="1.85" height="12.0" rx="0.92" fill="rgba(255,255,255,0.9)" />
          <rect class="interlace-up st-0" x="11.6" y="3.4" width="2.0" height="17.2" rx="1.0" fill="#ffffff" />
          <rect class="interlace-down st-1" x="15.3" y="7.4" width="1.8" height="9.2" rx="0.9" fill="rgba(255,255,255,0.85)" />
          <circle class="interlace-dot st-2" cx="19.3" cy="12" r="1.25" fill="rgba(255,255,255,0.72)" />
        </g>
      </svg>
    </div>
  </div>
</template>

<style scoped>
.vcp-avatar-wave-surface {
  transition:
    transform 0.44s cubic-bezier(0.22, 1, 0.36, 1),
    filter 0.44s cubic-bezier(0.22, 1, 0.36, 1);
}

.vcp-avatar-wave-slot.is-active .vcp-avatar-wave-surface,
.vcp-avatar-wave-slot.is-generating .vcp-avatar-wave-surface,
.vcp-avatar-wave-slot.is-narrating .vcp-avatar-wave-surface {
  filter: brightness(0.63) saturate(0.82) contrast(1.06);
  transform: scale(0.96);
}

.vignette-mask {
  opacity: 0;
  background: radial-gradient(
    circle at 50% 50%,
    rgba(0, 0, 0, 0.6) 0%,
    rgba(0, 0, 0, 0.24) 64%,
    transparent 100%
  );
  transition: opacity 0.38s ease;
}

.vcp-avatar-wave-slot.is-active .vignette-mask,
.vcp-avatar-wave-slot.is-generating .vignette-mask,
.vcp-avatar-wave-slot.is-narrating .vignette-mask {
  opacity: 1;
}

.axis-line {
  transform-origin: 55% 50%;
  transform: scaleX(0);
  opacity: 0;
  transition:
    transform 0.42s cubic-bezier(0.16, 1, 0.3, 1),
    opacity 0.3s ease;
}

.vcp-avatar-wave-slot.is-active .axis-line,
.vcp-avatar-wave-slot.is-generating .axis-line,
.vcp-avatar-wave-slot.is-narrating .axis-line {
  transform: scaleX(1);
  opacity: 1;
}

.interlace-up {
  transform-origin: center center;
  transform: translateY(6.5px) scaleY(0.18);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.22, 1, 0.36, 1),
    opacity 0.32s ease;
}

.interlace-down {
  transform-origin: center center;
  transform: translateY(-6.5px) scaleY(0.18);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.22, 1, 0.36, 1),
    opacity 0.32s ease;
}

.interlace-dot {
  transform-origin: center center;
  transform: translateY(4.5px) scale(0.2);
  opacity: 0;
  transition:
    transform 0.46s cubic-bezier(0.34, 1.56, 0.64, 1),
    opacity 0.32s ease;
}

.vcp-avatar-wave-slot.is-active .interlace-up,
.vcp-avatar-wave-slot.is-active .interlace-down,
.vcp-avatar-wave-slot.is-generating .interlace-up,
.vcp-avatar-wave-slot.is-generating .interlace-down,
.vcp-avatar-wave-slot.is-narrating .interlace-up,
.vcp-avatar-wave-slot.is-narrating .interlace-down {
  transform: translateY(0) scaleY(1);
  opacity: 1;
}

.vcp-avatar-wave-slot.is-active .interlace-dot,
.vcp-avatar-wave-slot.is-generating .interlace-dot,
.vcp-avatar-wave-slot.is-narrating .interlace-dot {
  transform: translateY(0) scale(1);
  opacity: 1;
}

.st-0 {
  transition-delay: 0s;
}

.st-1 {
  transition-delay: 0.04s;
}

.st-2 {
  transition-delay: 0.085s;
}

@media (prefers-reduced-motion: reduce) {
  .vcp-avatar-wave-surface,
  .vignette-mask,
  .axis-line,
  .interlace-up,
  .interlace-down,
  .interlace-dot {
    transition: none !important;
  }
}
</style>
