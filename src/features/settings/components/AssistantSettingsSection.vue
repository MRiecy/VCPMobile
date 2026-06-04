<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings } from "../../../core/stores/settings";
import { useAssistantStore } from "../../../core/stores/assistant";
import SettingsSwitch from "../../../components/settings/SettingsSwitch.vue";
import SettingsRow from "../../../components/settings/SettingsRow.vue";

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save-request"): void;
}>();

const assistantStore = useAssistantStore();
const hasOverlayPermission = ref(false);

const checkPermission = async () => {
  try {
    const status = await invoke<{ overlay: boolean }>("plugin:vcp-mobile|check_all_permissions");
    hasOverlayPermission.value = status.overlay;
    
    // 如果没有系统权限，但设置里是开启的，则重置设置状态并隐藏悬浮球
    if (!status.overlay && props.settings.enableAssistant) {
      props.settings.enableAssistant = false;
      await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: false });
    }
  } catch (e) {
    console.error("[AssistantSettings] Failed to check overlay permission:", e);
  }
};

const handleToggle = async (val: boolean) => {
  if (val) {
    await checkPermission();
    if (!hasOverlayPermission.value) {
      // 引导用户去系统设置开启权限
      try {
        await invoke("plugin:vcp-mobile|request_overlay_permission");
      } catch (e) {
        console.error("[AssistantSettings] Failed to request overlay permission:", e);
      }
      props.settings.enableAssistant = false;
      return;
    }
  }

  props.settings.enableAssistant = val;

  try {
    // 启停悬浮球（Android 原生层）
    await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: val });
    // 启停本地 HTTP 服务器（即时生效）
    await invoke("reconcile_local_server_cmd", { enable: val });
  } catch (e) {
    console.error("[AssistantSettings] Failed to toggle assistant:", e);
  }

  emit("save-request");
};

watch(
  () => props.settings.assistantAgentId,
  () => {
    emit("save-request");
  }
);

onMounted(async () => {
  try {
    await assistantStore.fetchAgents();
  } catch (_) {}
  await checkPermission();

  // 若用户手动设置了开启且有权限，则在 mounted 时确保拉起悬浮球和本地服务器
  if (props.settings.enableAssistant && hasOverlayPermission.value) {
    try {
      await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: true });
      await invoke("reconcile_local_server_cmd", { enable: true });
    } catch (_) {}
  }
});

// 监听生命周期 resume 事件以刷新权限状态
window.addEventListener("vcp-lifecycle", async (e: any) => {
  if (e.detail?.state === "resume") {
    await checkPermission();
    if (props.settings.enableAssistant && hasOverlayPermission.value) {
      try {
        await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: true });
        await invoke("reconcile_local_server_cmd", { enable: true });
      } catch (_) {}
    }
  }
});
</script>

<template>
  <div class="divide-y divide-black/5 dark:divide-white/5">
    <SettingsRow
      title="启用全局悬浮球"
      description="在其他应用上方显示悬浮球，随时唤起划词助手"
    >
      <template #title-suffix>
        <span class="ml-2 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wider rounded-full bg-amber-500/15 text-amber-600 dark:text-amber-400 border border-amber-500/25 select-none">Beta</span>
      </template>
      <template #action>
        <SettingsSwitch
          :modelValue="props.settings.enableAssistant || false"
          @update:modelValue="handleToggle"
        />
      </template>
    </SettingsRow>

    <SettingsRow
      v-if="props.settings.enableAssistant"
      title="助手绑定 Agent"
      description="选择悬浮窗口默认使用的智能体"
    >
      <template #action>
        <select
          v-model="props.settings.assistantAgentId"
          class="bg-transparent dark:bg-zinc-900 text-sm font-semibold opacity-60 border-none outline-none text-right cursor-pointer text-primary-text pr-2"
        >
          <option value="">未绑定 (使用默认)</option>
          <option
            v-for="agent in assistantStore.agents"
            :key="agent.id"
            :value="agent.id"
          >
            {{ agent.name }}
          </option>
        </select>
      </template>
    </SettingsRow>
  </div>
</template>
