<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import {
  CHAT_PRESENTATION_OPTIONS,
  normalizeThemeModuleKey,
  resolveThemeWallpaperUrl,
  useThemeStore,
  type ChatPresentationMode,
  type ThemeInfo,
} from '../../core/stores/theme';
import { useNotificationStore } from '../../core/stores/notification';
import ThemeLivePreview from './ThemeLivePreview.vue';

const themeStore = useThemeStore();
const notificationStore = useNotificationStore();
const railRef = ref<HTMLElement | null>(null);
const draftThemeKey = ref('');
const isApplying = ref(false);
const initializationError = ref('');
const applyError = ref('');
const presentationError = ref('');
let scrollIdleTimer: number | null = null;
let scrollMeasureFrame: number | null = null;
let isActive = true;

const appliedThemeKey = computed(() => {
  return normalizeThemeModuleKey(themeStore.currentTheme) || themeStore.availableThemes[0]?.fileName || '';
});

const draftTheme = computed(() => {
  return themeStore.availableThemes.find((theme) => theme.fileName === draftThemeKey.value) || null;
});

const draftIndex = computed(() => {
  return themeStore.availableThemes.findIndex((theme) => theme.fileName === draftThemeKey.value);
});

const isDirty = computed(() => {
  return Boolean(draftThemeKey.value && draftThemeKey.value !== appliedThemeKey.value);
});

const applyButtonLabel = computed(() => {
  if (isApplying.value) return '正在应用主题…';
  if (applyError.value) return '重试应用主题';
  return isDirty.value ? '确认应用主题' : '主题已应用';
});

const wallpaperFor = (theme: ThemeInfo, variant: 'dark' | 'light') => {
  const variableName = variant === 'dark' ? '--chat-wallpaper-dark' : '--chat-wallpaper-light';
  return resolveThemeWallpaperUrl(theme.variables[variant]?.[variableName]);
};

const cardStyle = (theme: ThemeInfo, variant: 'dark' | 'light'): Record<string, string> => {
  const wallpaper = wallpaperFor(theme, variant);
  return {
    backgroundColor: theme.variables[variant]?.['--primary-bg'] || 'var(--secondary-bg)',
    backgroundImage: wallpaper ? `url("${wallpaper}")` : 'none',
  };
};

const scrollThemeIntoView = (themeKey: string, smooth = false) => {
  const rail = railRef.value;
  if (!rail) return;
  const card = Array.from(rail.querySelectorAll<HTMLElement>('[data-theme-key]')).find(
    (element) => element.dataset.themeKey === themeKey,
  );
  card?.scrollIntoView?.({
    behavior: smooth ? 'smooth' : 'auto',
    block: 'nearest',
    inline: 'center',
  });
};

const selectTheme = (theme: ThemeInfo, alignCard = true) => {
  draftThemeKey.value = theme.fileName;
  applyError.value = '';
  if (alignCard) scrollThemeIntoView(theme.fileName, true);
};

const selectNearestTheme = () => {
  const rail = railRef.value;
  if (!rail) return;
  const railRect = rail.getBoundingClientRect();
  const railCenter = railRect.left + railRect.width / 2;
  let nearest: { element: HTMLElement; distance: number } | null = null;

  for (const element of Array.from(rail.querySelectorAll<HTMLElement>('[data-theme-key]'))) {
    const rect = element.getBoundingClientRect();
    const distance = Math.abs(rect.left + rect.width / 2 - railCenter);
    if (!nearest || distance < nearest.distance) nearest = { element, distance };
  }

  const key = nearest?.element.dataset.themeKey;
  const theme = key
    ? themeStore.availableThemes.find((candidate) => candidate.fileName === key)
    : undefined;
  if (theme) selectTheme(theme, false);
};

const handleRailScroll = () => {
  if (scrollIdleTimer !== null) window.clearTimeout(scrollIdleTimer);
  scrollIdleTimer = window.setTimeout(() => {
    scrollIdleTimer = null;
    if (scrollMeasureFrame !== null) cancelAnimationFrame(scrollMeasureFrame);
    scrollMeasureFrame = requestAnimationFrame(() => {
      scrollMeasureFrame = null;
      selectNearestTheme();
    });
  }, 100);
};

const applyDraftTheme = async () => {
  if (!isDirty.value || !draftThemeKey.value || isApplying.value) return;
  isApplying.value = true;
  applyError.value = '';
  try {
    const result = await themeStore.applyThemeFile(draftThemeKey.value);
    if (!isActive) return;
    if (!result.ok) {
      applyError.value = result.error || '主题应用失败，请重试';
    }
  } catch (error) {
    if (isActive) applyError.value = error instanceof Error ? error.message : String(error);
  } finally {
    if (isActive) isApplying.value = false;
  }
};

const selectPresentationMode = (mode: ChatPresentationMode) => {
  presentationError.value = '';
  const result = themeStore.setPresentationMode(mode);
  if (result.ok) return;

  presentationError.value = result.error || '消息呈现设置保存失败';
  notificationStore.addNotification({
    id: 'chat-presentation-save-failed',
    type: 'error',
    title: '消息呈现切换失败',
    message: presentationError.value,
    toastOnly: true,
  });
};

onMounted(async () => {
  try {
    await themeStore.fetchThemes();
    if (!isActive) return;
    draftThemeKey.value = appliedThemeKey.value || themeStore.availableThemes[0]?.fileName || '';
    await nextTick();
    scrollThemeIntoView(draftThemeKey.value);
  } catch (e) {
    initializationError.value = e instanceof Error ? e.message : String(e);
    console.error('[ThemePicker] Initialization failed:', e);
  }
});

watch(
  () => themeStore.currentTheme,
  async (nextTheme, previousTheme) => {
    const previousAppliedKey = normalizeThemeModuleKey(previousTheme);
    const draftWasClean = !draftThemeKey.value || draftThemeKey.value === previousAppliedKey;
    if (!draftWasClean) return;
    draftThemeKey.value = normalizeThemeModuleKey(nextTheme) || themeStore.availableThemes[0]?.fileName || '';
    await nextTick();
    if (isActive) scrollThemeIntoView(draftThemeKey.value);
  },
);

onBeforeUnmount(() => {
  isActive = false;
  if (scrollIdleTimer !== null) window.clearTimeout(scrollIdleTimer);
  if (scrollMeasureFrame !== null) cancelAnimationFrame(scrollMeasureFrame);
});
</script>

<template>
  <div class="theme-picker-shell h-full min-h-0">
    <div class="theme-picker-scroll h-full min-h-0 overflow-y-auto no-rubber-band">
      <ThemeLivePreview :theme="draftTheme" :initialization-error="initializationError" />

      <section class="theme-settings-section" aria-labelledby="theme-carousel-title">
        <div class="theme-section-heading is-compact">
          <div>
            <h3 id="theme-carousel-title">主题选择</h3>
            <p>横向滑动浏览，深浅效果会同步更新。</p>
          </div>
          <span class="theme-position font-mono">
            {{ draftIndex >= 0 ? String(draftIndex + 1).padStart(2, '0') : '--' }} /
            {{ String(themeStore.availableThemes.length).padStart(2, '0') }}
          </span>
        </div>

        <div ref="railRef" class="theme-carousel" data-testid="theme-carousel" @scroll.passive="handleRailScroll">
          <button v-for="theme in themeStore.availableThemes" :key="theme.fileName" type="button"
            class="theme-card" :class="draftThemeKey === theme.fileName ? 'is-selected' : ''"
            :data-theme-key="theme.fileName" :aria-pressed="draftThemeKey === theme.fileName"
            :aria-label="`预览主题：${theme.name}`" @click="selectTheme(theme)">
            <span class="theme-card-visual is-dark" :style="cardStyle(theme, 'dark')"></span>
            <span class="theme-card-visual is-light" :style="cardStyle(theme, 'light')"></span>
            <span class="theme-card-shade"></span>
            <span class="theme-card-name">{{ theme.name }}</span>
            <svg v-if="draftThemeKey === theme.fileName" class="theme-card-check" width="16" height="16"
              viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
          </button>
        </div>

        <p v-if="initializationError" class="theme-error" role="alert">{{ initializationError }}</p>
        <p v-if="applyError" class="theme-error" role="alert">{{ applyError }}</p>
        <button type="button" class="theme-apply-button" data-testid="theme-apply-button"
          :disabled="!isDirty || isApplying || !draftTheme" @click="applyDraftTheme">
          {{ applyButtonLabel }}
        </button>
      </section>

      <section class="theme-settings-section presentation-settings" aria-labelledby="presentation-title"
        data-testid="presentation-settings">
        <div class="theme-section-heading">
          <div>
            <h3 id="presentation-title">消息呈现与内容宽度</h3>
            <p>点选后立即生效；聊天页长按深浅按钮也可快速切换。</p>
          </div>
        </div>

        <div class="presentation-options" role="radiogroup" aria-labelledby="presentation-title">
          <button v-for="option in CHAT_PRESENTATION_OPTIONS" :key="option.value" type="button"
            class="presentation-option" :class="themeStore.presentationMode === option.value ? 'is-selected' : ''"
            role="radio" :aria-checked="themeStore.presentationMode === option.value"
            :data-presentation-value="option.value" @click="selectPresentationMode(option.value)">
            <span class="presentation-accent" aria-hidden="true"></span>
            <span class="presentation-copy">
              <strong>{{ option.title }}</strong>
              <small>{{ option.description }}</small>
            </span>
            <span class="presentation-short-name font-mono">{{ option.label }}</span>
            <svg v-if="themeStore.presentationMode === option.value" class="presentation-check" width="16" height="16"
              viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
          </button>
        </div>
        <p v-if="presentationError" class="theme-error" role="alert">{{ presentationError }}</p>
      </section>
    </div>
  </div>
</template>

<style scoped>
.theme-picker-scroll {
  padding: 1rem 0.75rem calc(var(--vcp-safe-bottom, 48px) + 1rem);
  overscroll-behavior-y: contain;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.theme-picker-scroll::-webkit-scrollbar,
.theme-carousel::-webkit-scrollbar {
  display: none;
}

.theme-settings-section {
  padding: 0.875rem;
  color: var(--primary-text);
  background-color: var(--vcp-panel-bg-90);
  border: 1px solid var(--vcp-border-subtle);
  border-radius: 1rem;
}

.theme-settings-section + .theme-settings-section {
  margin-top: 0.875rem;
}

.theme-section-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 0.75rem;
  margin-bottom: 0.75rem;
}

.theme-section-heading.is-compact {
  align-items: center;
}

.theme-section-heading h3 {
  margin: 0;
  font-size: 0.8125rem;
  font-weight: 800;
  letter-spacing: 0.04em;
}

.theme-section-heading p {
  margin: 0.2rem 0 0;
  color: var(--secondary-text);
  font-size: 0.6875rem;
  line-height: 1.45;
}

.theme-position {
  flex: 0 0 auto;
  max-width: 45%;
  color: var(--secondary-text);
  font-size: 0.6875rem;
  text-align: right;
}

.theme-carousel {
  display: flex;
  gap: 0.625rem;
  overflow-x: auto;
  padding: 0.125rem max(0.75rem, 8vw) 0.5rem;
  scroll-padding-inline: max(0.75rem, 8vw);
  scroll-snap-type: x mandatory;
  overscroll-behavior-x: contain;
  touch-action: pan-x;
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.theme-card {
  position: relative;
  flex: 0 0 min(72vw, 15rem);
  height: 7.5rem;
  overflow: hidden;
  padding: 0;
  color: white;
  background: var(--secondary-bg);
  border: 2px solid transparent;
  border-radius: 0.875rem;
  scroll-snap-align: center;
  text-align: left;
}

.theme-card.is-selected {
  border-color: var(--highlight-text);
}

.theme-card-visual {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 50%;
  background-position: center;
  background-size: cover;
}

.theme-card-visual.is-dark {
  left: 0;
}

.theme-card-visual.is-light {
  right: 0;
}

.theme-card-shade {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, transparent 32%, rgba(0, 0, 0, 0.72));
}

.theme-card-name {
  position: absolute;
  right: 0.625rem;
  bottom: 0.5rem;
  left: 0.625rem;
  overflow: hidden;
  font-size: 0.6875rem;
  font-weight: 700;
  text-align: center;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.theme-card-check {
  position: absolute;
  top: 0.5rem;
  left: 0.5rem;
  padding: 0.18rem;
  color: white;
  background-color: var(--highlight-text);
  border-radius: 50%;
}

.theme-apply-button {
  width: 100%;
  min-height: 3rem;
  margin-top: 0.5rem;
  color: var(--primary-bg);
  background-color: var(--highlight-text);
  border: 1px solid var(--highlight-text);
  border-radius: 0.75rem;
  font-size: 0.8125rem;
  font-weight: 800;
  transition: opacity 0.18s ease;
}

.theme-apply-button:disabled {
  color: var(--secondary-text);
  background-color: transparent;
  border-color: var(--vcp-border-subtle);
  opacity: 0.68;
}

.presentation-options {
  border-top: 1px solid var(--vcp-border-subtle);
}

.presentation-option {
  position: relative;
  display: flex;
  align-items: center;
  gap: 0.75rem;
  width: 100%;
  min-height: 4rem;
  padding: 0.7rem 0.5rem 0.7rem 0.75rem;
  color: var(--primary-text);
  background-color: transparent;
  border: 0;
  border-bottom: 1px solid var(--vcp-border-subtle);
  text-align: left;
}

.presentation-option.is-selected {
  background-color: var(--vcp-highlight-bg-10);
}

.presentation-accent {
  position: absolute;
  top: 0.5rem;
  bottom: 0.5rem;
  left: 0;
  width: 2px;
  background-color: transparent;
  border-radius: 1px;
}

.presentation-option.is-selected .presentation-accent {
  background-color: var(--highlight-text);
}

.presentation-copy {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-width: 0;
}

.presentation-copy strong {
  font-size: 0.8125rem;
}

.presentation-copy small {
  margin-top: 0.18rem;
  color: var(--secondary-text);
  font-size: 0.6875rem;
  line-height: 1.4;
}

.presentation-short-name {
  flex: 0 0 auto;
  color: var(--secondary-text);
  font-size: 0.625rem;
  white-space: nowrap;
}

.presentation-check {
  flex: 0 0 auto;
  color: var(--highlight-text);
}

.theme-error {
  margin: 0.5rem 0 0;
  color: #ef4444;
  font-size: 0.6875rem;
  line-height: 1.4;
}

@media (min-width: 480px) {
  .theme-card {
    flex-basis: calc((100% - 1.25rem) / 2.2);
  }
}

@media (min-width: 840px) {
  .theme-picker-scroll {
    padding-right: 1rem;
    padding-left: 1rem;
  }

  .theme-card {
    flex-basis: calc((100% - 1.875rem) / 3.25);
  }
}

@media (max-height: 700px) {
  .theme-card {
    height: 6.75rem;
  }
}

@media (prefers-reduced-motion: reduce) {
  .theme-apply-button {
    transition: none;
  }
}
</style>
