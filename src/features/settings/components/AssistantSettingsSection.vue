<script setup lang="ts">
// [SUSPENDED BETA] 划词悬浮助手设置面板当前已暂停使用，SettingsView.vue 中的入口已注释关闭。
// 保留组件代码供后续重启该功能时恢复。
import { ref, onMounted, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore, type AppSettings } from "../../../core/stores/settings";
import { useAssistantStore } from "../../../core/stores/assistant";
import { useAppLifecycleStore } from "../../../core/stores/appLifecycle";
import SettingsSwitch from "../../../components/settings/SettingsSwitch.vue";
import SettingsRow from "../../../components/settings/SettingsRow.vue";

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save-request"): void;
}>();

const settingsStore = useSettingsStore();
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
    // 开启时懒加载 Agent 列表
    try {
      await assistantStore.fetchAgentsAndGroups();
    } catch (_) {}
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

const lifecycleStore = useAppLifecycleStore();

watch(() => lifecycleStore.isBackground, async (newVal) => {
  if (!newVal) {
    await checkPermission();
    if (props.settings.enableAssistant && hasOverlayPermission.value) {
      try {
        await assistantStore.fetchAgentsAndGroups();
      } catch (_) {}
      try {
        await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: true });
        await invoke("reconcile_local_server_cmd", { enable: true });
      } catch (_) {}
    }
  }
});

onMounted(async () => {
  await checkPermission();
  
  // 若用户手动设置了开启且有权限，则在 mounted 时确保拉起悬浮球并懒加载 Agent 列表
  if (props.settings.enableAssistant && hasOverlayPermission.value) {
    try {
      await assistantStore.fetchAgentsAndGroups();
    } catch (_) {}
    try {
      await invoke("plugin:vcp-mobile|toggle_floating_ball", { show: true });
      await invoke("reconcile_local_server_cmd", { enable: true });
    } catch (_) {}
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
          :disabled="settingsStore.loading"
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
