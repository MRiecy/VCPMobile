<script setup lang="ts">
import { resolveThemeWallpaperUrl, type ThemeInfo } from '../../core/stores/theme';

const props = defineProps<{
  theme: ThemeInfo | null;
  initializationError?: string;
}>();

const previewVariants = [
  { key: 'dark', label: '深色' },
  { key: 'light', label: '浅色' },
] as const;

const wallpaperFor = (variant: 'dark' | 'light') => {
  if (!props.theme) return null;
  const variableName = variant === 'dark' ? '--chat-wallpaper-dark' : '--chat-wallpaper-light';
  return resolveThemeWallpaperUrl(props.theme.variables[variant]?.[variableName]);
};

const previewStyle = (variant: 'dark' | 'light'): Record<string, string> => {
  const variables = props.theme?.variables[variant];
  if (!variables) return {};

  const wallpaper = wallpaperFor(variant);
  const style: Record<string, string> = {
    '--preview-bg': variables['--primary-bg'] || (variant === 'dark' ? '#1c1c1e' : '#f4f6f8'),
    '--preview-panel': variables['--secondary-bg'] || (variant === 'dark' ? '#28282c' : '#ffffff'),
    '--preview-primary-text': variables['--primary-text'] || (variant === 'dark' ? '#e0e0e0' : '#2c3e50'),
    '--preview-secondary-text': variables['--secondary-text'] || (variant === 'dark' ? '#a0a0a0' : '#5a6f80'),
    '--preview-agent-text': variables['--agent-text'] || variables['--primary-text'] || '#e0e0e0',
    '--preview-user-text': variables['--user-text'] || variables['--primary-text'] || '#ffffff',
    '--preview-agent-bubble': variables['--assistant-bubble-bg'] || variables['--secondary-bg'] || '#28282c',
    '--preview-user-bubble': variables['--user-bubble-bg'] || variables['--highlight-text'] || '#3b82f6',
    '--preview-accent': variables['--highlight-text'] || '#3b82f6',
    '--preview-border': variables['--border-color'] || '#7c7c80',
  };
  if (wallpaper) style.backgroundImage = `url("${wallpaper}")`;
  return style;
};
</script>

<template>
  <section class="theme-settings-section" aria-labelledby="theme-preview-title">
    <div class="theme-section-heading">
      <div>
        <h3 id="theme-preview-title">实时预览</h3>
        <p>候选主题只在这里试穿，确认前不会修改聊天界面。</p>
      </div>
      <span class="theme-candidate-name">{{ theme?.name || '载入中' }}</span>
    </div>

    <div v-if="theme" class="theme-live-preview" data-testid="theme-live-preview">
      <article v-for="variant in previewVariants" :key="variant.key"
        class="theme-preview-pane" :data-variant="variant.key" :style="previewStyle(variant.key)">
        <div class="theme-preview-content">
          <div class="theme-preview-header">
            <span>{{ variant.label }}</span>
            <span class="theme-preview-status"></span>
          </div>
          <div class="theme-preview-message is-agent">
            <span class="theme-preview-avatar">V</span>
            <span>主题预览消息</span>
          </div>
          <div class="theme-preview-message is-user">清晰、克制、易读</div>
          <div class="theme-preview-action">强调色</div>
        </div>
      </article>
    </div>
    <div v-else class="theme-preview-empty" role="status">
      {{ initializationError ? '主题载入失败' : '正在准备主题预览…' }}
    </div>
  </section>
</template>

<style scoped>
.theme-settings-section {
  padding: 0.875rem;
  color: var(--primary-text);
  background-color: var(--vcp-panel-bg-90);
  border: 1px solid var(--vcp-border-subtle);
  border-radius: 1rem;
}

.theme-section-heading {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 0.75rem;
  margin-bottom: 0.75rem;
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

.theme-candidate-name {
  flex: 0 0 auto;
  max-width: 45%;
  overflow: hidden;
  color: var(--secondary-text);
  font-size: 0.6875rem;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.theme-live-preview {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0.5rem;
  min-height: 9.5rem;
  height: clamp(9.5rem, 23vh, 13rem);
}

.theme-preview-pane {
  min-width: 0;
  overflow: hidden;
  padding: 0.625rem;
  color: var(--preview-primary-text);
  background-color: var(--preview-bg);
  background-position: center;
  background-size: cover;
  border: 1px solid var(--preview-border);
  border-radius: 0.75rem;
}

.theme-preview-content {
  display: flex;
  flex-direction: column;
  gap: 0.45rem;
  height: 100%;
  padding: 0.5rem;
  background-color: var(--preview-panel);
  border-radius: 0.5rem;
  box-shadow: 0 1px 5px rgba(0, 0, 0, 0.12);
}

.theme-preview-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--preview-secondary-text);
  font-size: 0.625rem;
  font-weight: 700;
}

.theme-preview-status {
  width: 0.4rem;
  height: 0.4rem;
  background-color: var(--preview-accent);
  border-radius: 50%;
}

.theme-preview-message {
  max-width: 88%;
  padding: 0.42rem 0.5rem;
  font-size: 0.625rem;
  line-height: 1.35;
  border: 1px solid var(--preview-border);
  border-radius: 0.5rem;
}

.theme-preview-message.is-agent {
  display: flex;
  align-items: center;
  gap: 0.35rem;
  color: var(--preview-agent-text);
  background-color: var(--preview-agent-bubble);
}

.theme-preview-message.is-user {
  align-self: flex-end;
  color: var(--preview-user-text);
  background-color: var(--preview-user-bubble);
}

.theme-preview-avatar {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  width: 1rem;
  height: 1rem;
  color: var(--preview-bg);
  background-color: var(--preview-accent);
  border-radius: 50%;
  font-family: monospace;
  font-size: 0.5rem;
  font-weight: 800;
}

.theme-preview-action {
  align-self: flex-start;
  margin-top: auto;
  padding: 0.25rem 0.45rem;
  color: var(--preview-bg);
  background-color: var(--preview-accent);
  border-radius: 0.35rem;
  font-size: 0.5625rem;
  font-weight: 800;
}

.theme-preview-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 9.5rem;
  color: var(--secondary-text);
  border: 1px dashed var(--vcp-border-subtle);
  border-radius: 0.75rem;
  font-size: 0.75rem;
}

@media (max-height: 700px) {
  .theme-live-preview,
  .theme-preview-empty {
    min-height: 8.25rem;
    height: 8.25rem;
  }
}
</style>
