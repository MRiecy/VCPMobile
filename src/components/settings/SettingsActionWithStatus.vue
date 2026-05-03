<script setup lang="ts">
import SettingsRow from './SettingsRow.vue';
import SettingsActionButton from './SettingsActionButton.vue';
import SettingsInlineStatus from './SettingsInlineStatus.vue';

const emit = defineEmits<{
  (e: 'action-click'): void;
}>();

defineProps<{
  title?: string;
  description?: string;
  buttonVariant: 'primary' | 'secondary' | 'danger' | 'ghost';
  buttonSize?: 'sm' | 'md' | 'lg';
  buttonLabel: string;
  buttonLoading?: boolean;
  buttonDisabled?: boolean;
  buttonIcon?: any;
  statusType?: 'success' | 'error' | 'loading' | 'info' | null;
  statusMessage?: string;
  statusMono?: boolean;
  statusMultiline?: boolean;
}>();
</script>

<template>
  <div class="settings-action-with-status">
    <!-- 有标题时使用 SettingsRow，按钮嵌入右侧 -->
    <SettingsRow v-if="title" :title="title" :description="description">
      <template #action>
        <slot name="action">
          <SettingsActionButton
            :variant="buttonVariant"
            :size="buttonSize"
            :loading="buttonLoading"
            :disabled="buttonDisabled"
            :icon="buttonIcon"
            @click="emit('action-click')"
          >
            {{ buttonLabel }}
          </SettingsActionButton>
        </slot>
      </template>
    </SettingsRow>

    <!-- 无标题时按钮右对齐 -->
    <div v-else class="flex justify-end">
      <slot name="action">
        <SettingsActionButton
          :variant="buttonVariant"
          :size="buttonSize"
          :loading="buttonLoading"
          :disabled="buttonDisabled"
          :icon="buttonIcon"
          @click="emit('action-click')"
        >
          {{ buttonLabel }}
        </SettingsActionButton>
      </slot>
    </div>

    <!-- 状态反馈始终显示在按钮下方 -->
    <div v-if="statusType || statusMessage" class="mt-2">
      <slot name="status">
        <SettingsInlineStatus
          :type="statusType || undefined"
          :message="statusMessage || ''"
          :mono="statusMono"
          :multiline="statusMultiline"
        />
      </slot>
    </div>
  </div>
</template>
