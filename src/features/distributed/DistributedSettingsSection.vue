<script setup lang="ts">
import { computed } from "vue";
import type { AppSettings } from "../../core/stores/settings";
import { useDistributed } from "./composables/useDistributed";
import { useNotificationStore } from "../../core/stores/notification";

import SettingsTextField from "../../components/settings/SettingsTextField.vue";
import SettingsSwitch from "../../components/settings/SettingsSwitch.vue";
import SettingsInlineStatus from "../../components/settings/SettingsInlineStatus.vue";
import SettingsRow from "../../components/settings/SettingsRow.vue";

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save-request"): void;
}>();

const { status, loading, start, stop } = useDistributed();

// Local toggle state — bound to settings for persistence
const enabled = computed({
  get: () => props.settings.distributedEnabled ?? false,
  set: (val: boolean) => {
    props.settings.distributedEnabled = val;
  },
});

const deviceName = computed({
  get: () => props.settings.distributedDeviceName ?? "VCPMobile",
  set: (val: string) => {
    props.settings.distributedDeviceName = val;
  },
});

// Derive WS URL from vcpLogUrl (same main server, different path)
const derivedWsUrl = computed(() => {
  const logUrl = props.settings.vcpLogUrl || "";
  // vcpLogUrl is like "ws://host:port" — reuse it directly
  return logUrl.replace(/\/+$/, "");
});

const derivedVcpKey = computed(() => {
  return props.settings.vcpLogKey || "";
});

const statusDisplay = computed(() => {
  if (loading.value) return { type: "loading" as const, message: "连接中..." };
  if (status.value.connected) {
    return {
      type: "success" as const,
      message: `已连接 · ${status.value.server_id} · ${status.value.registered_tools} 个工具`,
    };
  }
  if (status.value.last_error) {
    return { type: "error" as const, message: status.value.last_error };
  }
  return { type: null, message: "未连接" };
});

const notificationStore = useNotificationStore();

const toggleConnection = async () => {
  if (enabled.value) {
    enabled.value = false;
    emit("save-request");
    await stop();
  } else {
    if (!derivedWsUrl.value || !derivedVcpKey.value) {
      notificationStore.addNotification({
        type: "warning",
        title: "配置缺失",
        message: "请先在服务器连接中配置 VCPLog/WebSocket 地址和密钥",
        toastOnly: true
      });
      return;
    }
    enabled.value = true;
    emit("save-request");
    try {
      await start(derivedWsUrl.value, derivedVcpKey.value, deviceName.value);
    } catch (e) {
      console.error("[Distributed] Start failed:", e);
    }
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <!-- 主开关 -->
    <SettingsRow title="分布式节点" :description="statusDisplay.message">
      <template #action>
        <SettingsSwitch
          :model-value="enabled"
          active-color="bg-purple-500"
          :disabled="loading || (!derivedWsUrl && !enabled)"
          @update:model-value="toggleConnection"
        />
      </template>
    </SettingsRow>

    <!-- 连接状态 -->
    <SettingsInlineStatus
      v-if="statusDisplay.type"
      :type="statusDisplay.type"
      :message="statusDisplay.message"
    />

    <!-- 节点名称 -->
    <SettingsTextField
      v-model="deviceName"
      label="节点名称"
      placeholder="VCPMobile"
      @blur="emit('save-request')"
    />

    <!-- 连接信息（只读，派生自 VCPLog 配置） -->
    <div class="text-xs opacity-40 space-y-1 pt-2">
      <div class="font-mono">
        WS: {{ derivedWsUrl || "未配置（请先设置核心连接 → VCPLog URL）" }}
      </div>
      <div class="font-mono">
        Key: {{ derivedVcpKey ? "●●●●●●●●" : "未配置" }}
      </div>
    </div>
  </div>
</template>
