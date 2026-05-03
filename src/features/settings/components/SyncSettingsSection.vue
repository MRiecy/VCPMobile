<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings } from "../../../core/stores/settings";
import { useSyncSessionStore } from "../../../core/stores/syncSession";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsActionButton from "../../../components/settings/SettingsActionButton.vue";
import SettingsActionWithStatus from "../../../components/settings/SettingsActionWithStatus.vue";
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

      <SettingsActionWithStatus
        title="表情包修复库"
        description="从 VCP 服务器同步表情包元数据"
        button-variant="secondary"
        button-size="sm"
        button-label="REFRESH"
        :button-loading="emoticonStatus.type === 'loading'"
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
            @click="startManualSync"
          >
            OPEN PANEL
          </SettingsActionButton>
        </template>
      </SettingsRow>
    </div>
  </div>
</template>
