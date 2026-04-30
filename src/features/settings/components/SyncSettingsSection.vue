<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings } from "../../../core/stores/settings";
import { useSyncSessionStore } from "../../../core/stores/syncSession";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsActionButton from "../../../components/settings/SettingsActionButton.vue";
import SettingsRow from "../../../components/settings/SettingsRow.vue";

defineProps<{
  settings: AppSettings;
}>();

const syncStore = useSyncSessionStore();

const emoticonStatus = ref<{
  type: "success" | "error" | "loading" | null;
  message: string;
}>({ type: null, message: "" });

const startManualSync = () => {
  syncStore.open();
};

const rebuildEmoticonLibrary = async () => {
  emoticonStatus.value = { type: "loading", message: "正在从远程服务器获取..." };
  try {
    const count = await invoke<number>("regenerate_emoticon_library");
    emoticonStatus.value = {
      type: "success",
      message: `同步成功：共计 ${count} 个表情`,
    };
    setTimeout(() => {
      emoticonStatus.value = { type: null, message: "" };
    }, 3000);
  } catch (e: any) {
    emoticonStatus.value = { type: "error", message: `同步失败: ${e}` };
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <SettingsTextField
      v-model="settings.syncHttpUrl"
      label="HTTP 服务 URL"
      placeholder="http://192.168.x.x:5974"
      mono
    />
    <SettingsTextField
      v-model="settings.syncServerUrl"
      label="WebSocket 服务 URL"
      placeholder="ws://192.168.x.x:5975"
      mono
    />
    <SettingsTextField
      v-model="settings.syncToken"
      is-secure
      label="Mobile Sync Token"
      placeholder="输入桌面端 config.env 中的 Token"
      mono
    />

    <div class="border-t border-black/5 dark:border-white/5 pt-2 space-y-4">
      <SettingsTextField
        v-model="settings.fileKey"
        is-secure
        label="表情包图床密钥 (fileKey)"
        placeholder="用于构造表情包 URL 的密码"
        mono
      />

      <SettingsRow
        title="表情包修复库"
        description="从 VCP 服务器同步表情包元数据"
      >
        <template #action>
          <SettingsActionButton
            variant="secondary"
            size="sm"
            :loading="emoticonStatus.type === 'loading'"
            @click="rebuildEmoticonLibrary"
          >
            REFRESH
          </SettingsActionButton>
        </template>
      </SettingsRow>

      <div
        v-if="emoticonStatus.message"
        class="mt-1 text-xs px-1"
        :class="{
          'text-green-500': emoticonStatus.type === 'success',
          'text-red-400': emoticonStatus.type === 'error',
          'opacity-50': emoticonStatus.type === 'loading',
        }"
      >
        {{ emoticonStatus.message }}
      </div>

      <SettingsRow
        title="全量神经同步"
        description="手动触发与桌面端的数据全量比对与同步"
      >
        <template #action>
          <SettingsActionButton
            variant="secondary"
            size="sm"
            @click="startManualSync"
          >
            START SYNC
          </SettingsActionButton>
        </template>
      </SettingsRow>
    </div>
  </div>
</template>
