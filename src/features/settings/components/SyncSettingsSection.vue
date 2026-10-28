<script setup lang="ts">
import { onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ConnectionFields } from "../editing/fieldSets";
import type { SettingsSaveResult } from "../editing/useSettingsEditSession";
import { useOverlayStore } from "../../../core/stores/overlay";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsActionButton from "../../../components/settings/SettingsActionButton.vue";
import SettingsActionWithStatus from "../../../components/settings/SettingsActionWithStatus.vue";
import SettingsRow from "../../../components/settings/SettingsRow.vue";

const props = defineProps<{
  settings: Readonly<ConnectionFields>;
  active: boolean;
  disabled?: boolean;
  commit: () => Promise<SettingsSaveResult>;
}>();

const overlayStore = useOverlayStore();

const emit = defineEmits<{
  (e: "edit", patch: Partial<ConnectionFields>): void;
  (e: "blur"): void;
}>();

const emoticonStatus = ref<{
  type: "success" | "error" | "loading" | null;
  message: string;
}>({ type: null, message: "" });

const openingSync = ref(false);
const startManualSync = async () => {
  if (disposed || !props.active || props.disabled || openingSync.value) return;
  const request = generation;
  openingSync.value = true;
  try {
    // A blur save may still be pending when this click arrives.
    const saved = await props.commit();
    if (disposed || !props.active || request !== generation || !saved.ok) return;
    overlayStore.openSyncSession();
  } finally {
    openingSync.value = false;
  }
};

let disposed = false;
let generation = 0;
let clearTimer: ReturnType<typeof setTimeout> | null = null;
const deactivate = () => {
  generation += 1;
  if (clearTimer) clearTimeout(clearTimer);
  clearTimer = null;
};
watch(() => props.active, (active) => { if (!active) deactivate(); }, { flush: 'sync' });
onUnmounted(() => { disposed = true; deactivate(); });

const rebuildEmoticonLibrary = async () => {
  if (disposed || !props.active || props.disabled || emoticonStatus.value.type === 'loading') return;
  const request = ++generation;
  if (clearTimer) clearTimeout(clearTimer);
  emoticonStatus.value = { type: "loading", message: "正在从远程服务器获取..." };
  try {
    const saved = await props.commit();
    if (disposed || !props.active || request !== generation) return;
    if (!saved.ok) throw new Error(saved.error);
    const count = await invoke<number>("regenerate_emoticon_library");
    if (disposed || !props.active || request !== generation) return;
    emoticonStatus.value = {
      type: "success",
      message: `同步成功：共计 ${count} 个表情`,
    };
    clearTimer = setTimeout(() => {
      if (!disposed && props.active && request === generation) emoticonStatus.value = { type: null, message: "" };
      clearTimer = null;
    }, 3000);
  } catch (e: any) {
    if (disposed || !props.active || request !== generation) return;
    emoticonStatus.value = { type: "error", message: `同步失败: ${e}` };
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <SettingsTextField
      :model-value="settings['sync.httpUrl']"
      :disabled="disabled"
      @update:model-value="emit('edit', { 'sync.httpUrl': $event })"
      @blur="emit('blur')"
      label="HTTP 服务 URL"
      placeholder="http://192.168.x.x:5974"
      mono
    />
    <SettingsTextField
      :model-value="settings['sync.serverUrl']"
      :disabled="disabled"
      @update:model-value="emit('edit', { 'sync.serverUrl': $event })"
      @blur="emit('blur')"
      label="WebSocket 服务 URL"
      placeholder="ws://192.168.x.x:5975"
      mono
    />
    <SettingsTextField
      :model-value="settings['sync.token']"
      :disabled="disabled"
      @update:model-value="emit('edit', { 'sync.token': $event })"
      @blur="emit('blur')"
      is-secure
      label="Mobile Sync Token"
      placeholder="输入桌面端 config.env 中的 Token"
      mono
    />

    <div class="border-t border-black/5 dark:border-white/5 pt-2 space-y-4">
      <SettingsTextField
        :model-value="settings['server.fileKey']"
        :disabled="disabled"
        @update:model-value="emit('edit', { 'server.fileKey': $event })"
        @blur="emit('blur')"
        is-secure
        label="表情包图床密钥 (fileKey)"
        placeholder="用于构造表情包 URL 的密码"
        mono
      />

      <SettingsActionWithStatus
        title="表情包修复库"
        description="从 VCP 服务器同步表情包元数据"
        button-variant="secondary"
        button-size="sm"
        button-label="REFRESH"
        :button-loading="emoticonStatus.type === 'loading'"
        :button-disabled="disabled"
        :status-type="emoticonStatus.type"
        :status-message="emoticonStatus.message"
        @action-click="rebuildEmoticonLibrary"
      />

      <SettingsRow
        title="全量神经同步"
        description="打开全量神经同步面板，查看历史日志或开始同步"
      >
        <template #action>
          <SettingsActionButton
            variant="secondary"
            size="sm"
            :disabled="disabled"
            :loading="openingSync"
            @click="startManualSync"
          >
            OPEN PANEL
          </SettingsActionButton>
        </template>
      </SettingsRow>
    </div>
  </div>
</template>
