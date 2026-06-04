<script setup lang="ts">
import { ref, onMounted, watch, computed } from "vue";
import { useSettingsStore } from "../../core/stores/settings";
import { useNotificationStore } from "../../core/stores/notification";
import { useDistributed } from "./composables/useDistributed";
import SlidePage from "../../components/ui/SlidePage.vue";

// Atomic settings UI components
import SettingsTextField from "../../components/settings/SettingsTextField.vue";
import SettingsCard from "../../components/settings/SettingsCard.vue";

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: 50,
  },
);

const emit = defineEmits<{
  close: [];
}>();

const settingsStore = useSettingsStore();
const notificationStore = useNotificationStore();
const { status, loading, start, stop } = useDistributed();

const activeTab = ref<"connection" | "plugins" | "placeholders">("connection");
const searchQuery = ref("");

// Tab 1 Inputs status
const deviceName = ref("VCPMobile");
const wsUrl = ref("");
const vcpKey = ref("");

// Interface types
interface PluginItem {
  id: string;
  name: string;
  englishName: string;
  description: string;
  type: "oneshot" | "streaming";
  placeholder?: string;
  icon: string;
  communication: {
    mode: "Ipc" | "Mock";
    payload?: {
      command: string;
      args?: any;
    };
  };
  enabled: boolean;
}

interface PlaceholderItem {
  macro: string;
  name: string;
  description: string;
  example: string;
}

// Reactively populated via get_registered_tools_metadata
const pluginsList = ref<PluginItem[]>([]);
const placeholdersList = ref<PlaceholderItem[]>([]);



const getInitialDisabledTools = () => {
  const stored = localStorage.getItem("disabled_tools");
  return stored ? JSON.parse(stored) : [];
};

const disabledTools = ref<string[]>(getInitialDisabledTools());

const syncDisabledToolsToBackend = async () => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("update_disabled_tools", { disabledNames: disabledTools.value });
  } catch (e) {
    console.error("[DistributedView] Failed to sync disabled tools to backend:", e);
  }
};

const toggleToolEnabled = async (plugin: PluginItem, event?: Event) => {
  if (event) {
    event.stopPropagation();
  }
  
  const targetState = !plugin.enabled;

  // 敏感设备插件的 JNI 动态权限申请防护
  if (targetState) {
    let requiredAlias: string | null = null;
    if (plugin.id === "MobileLocation") {
      requiredAlias = "location";
    } else if (plugin.id === "MobileNotification") {
      requiredAlias = "notification";
    }

    if (requiredAlias) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        // A. 检测权限状态
        const perms: any = await invoke("plugin:vcp-mobile|check_all_permissions");
        const hasPermission = perms[requiredAlias];

        if (!hasPermission) {
          notificationStore.addNotification({
            type: "info",
            title: "权限请求",
            message: `${plugin.name} 需要系统定位/通知权限，请在弹出的系统对话框中点击“允许”。`,
            toastOnly: true
          });
          // B. 调用 JNI 申请权限
          await invoke("plugin:vcp-mobile|request_android_permission", { pType: requiredAlias });

          // C. 再次检查，核实用户是否同意
          const permsRetry: any = await invoke("plugin:vcp-mobile|check_all_permissions");
          if (!permsRetry[requiredAlias]) {
            notificationStore.addNotification({
              type: "warning",
              title: "未获得权限",
              message: `由于未获得系统 ${requiredAlias} 权限，开启 ${plugin.name} 失败。`,
              toastOnly: true
            });
            return; // 中断开启，开关继续保持为 false
          }
        }
      } catch (err) {
        console.error("[DistributedView] Failed to verify system permissions:", err);
      }
    }
  }
  
  plugin.enabled = targetState;
  
  if (!targetState) {
    if (!disabledTools.value.includes(plugin.id)) {
      disabledTools.value.push(plugin.id);
    }
  } else {
    disabledTools.value = disabledTools.value.filter(id => id !== plugin.id);
  }
  
  localStorage.setItem("disabled_tools", JSON.stringify(disabledTools.value));
  await syncDisabledToolsToBackend();
  
  if (!targetState && expandedPluginId.value === plugin.id) {
    expandedPluginId.value = null;
  }
  
  await loadPluginsMetadata();
};

// Fetch dynamic plugin metadata from Rust backend
const loadPluginsMetadata = async () => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const rawTools = await invoke<any[]>("get_registered_tools_metadata");

    // 如果是首次初始化且 localStorage 中无已禁用记录，则默认禁用所有工具以省电
    const stored = localStorage.getItem("disabled_tools");
    if (!stored) {
      const allNames = rawTools.map(tool => tool.name);
      disabledTools.value = allNames;
      localStorage.setItem("disabled_tools", JSON.stringify(allNames));
      await syncDisabledToolsToBackend();
    }
    
    // Map backend registered tools to frontend list with visual decorator
    pluginsList.value = rawTools.map(tool => {
      return {
        id: tool.name,
        name: tool.display_name || tool.name,
        englishName: tool.name,
        description: tool.description || "",
        type: tool.category || "oneshot",
        placeholder: tool.placeholder || undefined,
        icon: tool.icon || "i-lucide-toy-brick",
        communication: tool.communication,
        enabled: tool.enabled !== undefined ? tool.enabled : true
      };
    });

    // Derive placeholders list dynamically from streaming plugins
    placeholdersList.value = rawTools
      .filter(tool => tool.placeholder)
      .map(tool => {
        const macro = tool.placeholder;
        return {
          macro,
          name: `${tool.display_name || tool.name}占位宏`,
          description: `解析并流式替换为 ${tool.display_name || tool.name} 的最新物理遥测采样。`,
          example: `(展开插件卡片以读取实时数据样本)`
        };
      });
  } catch (e) {
    console.error("[DistributedView] Failed to load tool metadata from backend:", e);
  }
};

// Expanded plugin details
const expandedPluginId = ref<string | null>(null);
const pluginLoading = ref<Record<string, boolean>>({});
const pluginData = ref<Record<string, string>>({});

// Loading settings from main settings store
const loadSettings = async () => {
  try {
    await settingsStore.fetchSettings();
    if (settingsStore.settings) {
      wsUrl.value = settingsStore.settings.vcpLogUrl || settingsStore.settings.vcpServerUrl || "";
      vcpKey.value = settingsStore.settings.vcpLogKey || settingsStore.settings.vcpApiKey || "";
      deviceName.value = settingsStore.settings.distributedDeviceName ?? "VCPMobile";
    }
    // Pull registered tools dynamically
    await loadPluginsMetadata();
  } catch (e) {
    console.error("[DistributedView] Failed to load settings:", e);
  }
};

const saveSettings = async () => {
  if (settingsStore.settings) {
    const updates = {
      vcpLogUrl: wsUrl.value,
      vcpLogKey: vcpKey.value,
      distributedDeviceName: deviceName.value,
    };
    await settingsStore.updateSettings(updates);
  }
};

const handleConnect = async () => {
  if (!wsUrl.value || !vcpKey.value) return;
  try {
    await saveSettings();
    await start(wsUrl.value, vcpKey.value, deviceName.value);
    
    // Once started, update the connection state inside settings too
    if (settingsStore.settings) {
      await settingsStore.updateSettings({ distributedEnabled: true });
    }
  } catch (e: any) {
    console.error("[DistributedView] Connection start failed:", e);
  }
};

const handleDisconnect = async () => {
  try {
    await stop();
    if (settingsStore.settings) {
      await settingsStore.updateSettings({ distributedEnabled: false });
    }
  } catch (e: any) {
    console.error("[DistributedView] Connection stop failed:", e);
  }
};

// Clipboard copy helper
const copyText = (text: string, title = "已复制到剪贴板") => {
  if (!text) return;
  navigator.clipboard.writeText(text)
    .then(() => {
      notificationStore.addNotification({
        type: "success",
        title,
        message: text,
        toastOnly: true
      });
    })
    .catch((err) => {
      console.error("[DistributedView] Copy failed:", err);
    });
};

const copyPlaceholder = (macro: string) => {
  copyText(macro, "占位符宏已复制");
};

// Toggle plugin fold and load JNI sensor/battery data on-demand (Fully dynamic binding)
const togglePlugin = async (plugin: PluginItem) => {
  if (!plugin.enabled) {
    notificationStore.addNotification({
      type: "warning",
      title: "插件已被禁用",
      message: `${plugin.name} 当前处于关闭状态，无法读取其实时物理遥测。`,
      toastOnly: true
    });
    return;
  }

  if (expandedPluginId.value === plugin.id) {
    expandedPluginId.value = null;
    return;
  }

  expandedPluginId.value = plugin.id;
  pluginLoading.value[plugin.id] = true;

  try {
    const comm = plugin.communication;
    if (comm && comm.mode === "Ipc" && comm.payload) {
      // 1. Dynamic JNI/IPC execution channel
      const { invoke } = await import("@tauri-apps/api/core");
      const res = await invoke<any>(comm.payload.command, comm.payload.args || {});
      
      if (res && typeof res === "object") {
        if (res.value !== undefined) {
          pluginData.value[plugin.id] = String(res.value);
        } else if (res.level !== undefined) {
          pluginData.value[plugin.id] = `剩余电量: ${res.level}%\n状态类型: ${res.charging ? "充电中 (Charging)" : "放电中 (Discharging)"}\n省电模式: ${res.isPowerSaveMode ? "已开启" : "未开启"}`;
        } else {
          pluginData.value[plugin.id] = JSON.stringify(res, null, 2);
        }
      } else {
        pluginData.value[plugin.id] = String(res);
      }
    } else {
      // 2. Mock mode: notify user there is no JNI channel
      pluginData.value[plugin.id] = "此分布式工具在当前物理节点上无需/无 JNI 物理遥测通道。";
    }
  } catch (e: any) {
    console.error(`[DistributedView] Failed to pull native sensor telemetry for ${plugin.id}:`, e);
    pluginData.value[plugin.id] = `遥测读取异常:\n${e.toString()}\n(请检查移动端传感器硬件模块是否正常运行或对应的 JNI 权限是否已授予)`;
  } finally {
    pluginLoading.value[plugin.id] = false;
  }
};

// Filters
const filteredPlugins = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return pluginsList.value;
  return pluginsList.value.filter(
    p => p.name.toLowerCase().includes(query) || 
         p.englishName.toLowerCase().includes(query) ||
         p.description.toLowerCase().includes(query)
  );
});

const filteredPlaceholders = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return placeholdersList.value;
  return placeholdersList.value.filter(
    item => item.macro.toLowerCase().includes(query) ||
            item.name.toLowerCase().includes(query) ||
            item.description.toLowerCase().includes(query)
  );
});

// Initialization
onMounted(async () => {
  if (props.isOpen) {
    await syncDisabledToolsToBackend();
    loadSettings();
  }
});

watch(
  () => props.isOpen,
  async (val: boolean) => {
    if (val) {
      await syncDisabledToolsToBackend();
      loadSettings();
      expandedPluginId.value = null;
      searchQuery.value = "";
    }
  }
);
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="distributed-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto">
      <!-- Header -->
      <header class="px-4 py-3 flex items-center justify-between border-b border-black/5 dark:border-white/5 pt-[calc(var(--vcp-safe-top,24px)+12px)] pb-3 shrink-0">
        <div class="flex flex-col">
          <h2 class="text-xl font-bold tracking-tight text-primary-text">分布式设备面板</h2>
          <span class="text-[8px] font-mono opacity-40 uppercase tracking-[0.2em]">Distributed Panel // Client V2</span>
        </div>
        <button
          @click="emit('close')"
          class="p-2 active:scale-90 transition-all flex items-center justify-center opacity-70 active:opacity-100 rounded-xl hover:bg-black/5 dark:hover:bg-white/5"
        >
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </header>

      <!-- Sub Tabs navigation -->
      <div class="px-4 py-2 shrink-0 border-b border-black/5 dark:border-white/5 flex gap-2">
        <button
          @click="activeTab = 'connection'"
          class="flex-1 py-1.5 text-xs font-bold rounded-lg transition-all tracking-wider text-center"
          :class="activeTab === 'connection' ? 'bg-black/5 dark:bg-white/5 text-[var(--highlight-text)] border border-[var(--highlight-text)]/20' : 'opacity-60 text-primary-text hover:opacity-80'"
        >
          基础连接
        </button>
        <button
          @click="activeTab = 'plugins'"
          class="flex-1 py-1.5 text-xs font-bold rounded-lg transition-all tracking-wider text-center"
          :class="activeTab === 'plugins' ? 'bg-black/5 dark:bg-white/5 text-[var(--highlight-text)] border border-[var(--highlight-text)]/20' : 'opacity-60 text-primary-text hover:opacity-80'"
        >
          插件列表
        </button>
        <button
          @click="activeTab = 'placeholders'"
          class="flex-1 py-1.5 text-xs font-bold rounded-lg transition-all tracking-wider text-center"
          :class="activeTab === 'placeholders' ? 'bg-black/5 dark:bg-white/5 text-[var(--highlight-text)] border border-[var(--highlight-text)]/20' : 'opacity-60 text-primary-text hover:opacity-80'"
        >
          占位符
        </button>
      </div>

      <!-- Tab Content Area -->
      <div class="flex-1 overflow-y-auto no-rubber-band relative">
        <!-- 1. Connection view -->
        <div v-if="activeTab === 'connection'" class="px-4 py-6 space-y-6">
          <!-- Connection Status Card -->
          <div class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl flex items-center justify-between">
            <div class="flex items-center gap-3">
              <span class="relative flex h-3 w-3">
                <span
                  class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75"
                  :class="status.connected ? 'bg-emerald-400' : loading ? 'bg-amber-400' : 'bg-rose-400'"
                ></span>
                <span
                  class="relative inline-flex rounded-full h-3 w-3"
                  :class="status.connected ? 'bg-emerald-500' : loading ? 'bg-amber-500' : 'bg-rose-500'"
                ></span>
              </span>
              <div class="flex flex-col">
                <span class="text-xs font-bold">连接状态</span>
                <span class="text-[8px] opacity-50 font-mono uppercase tracking-wider">{{ status.connected ? 'Connected' : loading ? 'Connecting...' : 'Disconnected' }}</span>
              </div>
            </div>
            <div class="flex gap-2">
              <button
                v-if="status.connected"
                @click="handleDisconnect"
                :disabled="loading"
                class="px-4 py-1.5 bg-red-500/20 text-red-500 text-xs font-bold rounded-xl active:scale-95 transition-all hover:bg-red-500/30 disabled:opacity-50"
              >
                断开连接
              </button>
              <button
                v-else
                @click="handleConnect"
                :disabled="loading || !wsUrl || !vcpKey"
                class="px-4 py-1.5 text-white text-xs font-bold rounded-xl active:scale-95 transition-all disabled:opacity-50"
                style="background-color: var(--highlight-text)"
              >
                {{ loading ? '连接中...' : '连接' }}
              </button>
            </div>
          </div>

          <!-- Configuration Fields -->
          <SettingsCard>
            <div class="space-y-4 py-1">
              <SettingsTextField
                v-model="deviceName"
                label="节点名称 / Device Name"
                placeholder="例如 VCPMobile"
                :disabled="loading || status.connected"
              />
              <SettingsTextField
                v-model="wsUrl"
                label="WS 服务地址 / WebSocket URL"
                placeholder="ws://192.168.x.x:port"
                mono
                :disabled="loading || status.connected"
              />
              <SettingsTextField
                v-model="vcpKey"
                label="VCP 鉴权密钥 / VCP API Key"
                placeholder="授权校验密钥"
                is-secure
                :disabled="loading || status.connected"
              />
            </div>
          </SettingsCard>

          <!-- Status Information (Only shown when connected) -->
          <div v-if="status.connected" class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl space-y-3">
            <div class="flex justify-between items-center text-xs border-b border-black/5 dark:border-white/5 pb-2">
              <span class="opacity-50">本地客户端 ID</span>
              <span 
                class="font-mono bg-black/10 dark:bg-white/10 px-2 py-0.5 rounded cursor-pointer select-all hover:bg-black/20 text-[10px]"
                @click="copyText(status.client_id || '', '客户端 ID 已复制')"
              >
                {{ status.client_id || 'N/A' }}
              </span>
            </div>
            <div class="flex justify-between items-center text-xs border-b border-black/5 dark:border-white/5 pb-2">
              <span class="opacity-50">服务端连接 ID</span>
              <span 
                class="font-mono bg-black/10 dark:bg-white/10 px-2 py-0.5 rounded cursor-pointer select-all hover:bg-black/20 text-[10px]"
                @click="copyText(status.server_id || '', '服务端 ID 已复制')"
              >
                {{ status.server_id || 'N/A' }}
              </span>
            </div>
            <div class="flex justify-between items-center text-xs">
              <span class="opacity-50">已注册分布式工具</span>
              <span class="font-mono font-bold">{{ status.registered_tools }} / {{ pluginsList.length }}</span>
            </div>
          </div>

          <!-- Connection Error Alert -->
          <div v-if="status.last_error" class="bg-red-500/10 border border-red-500/20 p-4 rounded-2xl space-y-2">
            <div class="flex items-center gap-2 text-red-500 text-xs font-bold">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <circle cx="12" cy="12" r="10"/>
                <line x1="12" y1="8" x2="12" y2="12"/>
                <line x1="12" y1="16" x2="12.01" y2="16"/>
              </svg>
              <span>错误日志 / Failure logs</span>
            </div>
            <p class="text-[10px] font-mono text-red-500 opacity-80 whitespace-pre-wrap leading-relaxed break-all">
              {{ status.last_error }}
            </p>
          </div>
        </div>

        <!-- 2. Plugins List view -->
        <div v-if="activeTab === 'plugins'" class="flex flex-col h-full">
          <!-- Search box -->
          <div class="px-4 py-3 shrink-0 border-b border-black/5 dark:border-white/5">
            <div class="relative">
              <input
                v-model="searchQuery"
                type="text"
                placeholder="搜索分布式插件 (例如 CPU, 定位...)"
                class="w-full bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-xl px-3.5 py-2.5 text-xs text-primary-text placeholder-opacity-40 focus:outline-none focus:border-[var(--highlight-text)]/50 font-sans"
              />
              <span 
                v-if="searchQuery" 
                @click="searchQuery = ''" 
                class="absolute right-3.5 top-1/2 -translate-y-1/2 opacity-40 text-[10px] uppercase font-bold tracking-wider active:scale-90 p-1 cursor-pointer select-none"
              >
                Clear
              </span>
            </div>
          </div>

          <!-- Plugins items list -->
          <div class="flex-1 overflow-y-auto px-4 py-4 space-y-3 no-rubber-band">
            <div
              v-for="plugin in filteredPlugins"
              :key="plugin.id"
              class="border border-black/5 dark:border-white/5 rounded-2xl overflow-hidden transition-all duration-300"
              :class="[
                expandedPluginId === plugin.id ? 'bg-black/5 dark:bg-white/5 border-black/10 dark:border-white/10 shadow-sm' : 'bg-black/2 dark:bg-white/2 hover:border-black/10 dark:hover:border-white/10',
                !plugin.enabled ? 'opacity-55' : ''
              ]"
            >
              <div
                @click="togglePlugin(plugin)"
                class="p-4 flex items-center justify-between cursor-pointer select-none active:bg-black/5 dark:active:bg-white/5"
              >
                <div class="flex items-center gap-3">
                  <div class="h-8 w-8 rounded-xl bg-black/5 dark:bg-white/5 flex items-center justify-center text-primary-text">
                    <span class="text-xs uppercase font-mono font-bold opacity-75">{{ plugin.englishName.substring(0, 2) }}</span>
                  </div>
                  <div class="flex flex-col">
                    <div class="flex items-center gap-1.5">
                      <span class="text-xs font-bold">{{ plugin.name }}</span>
                      <span class="text-[8px] font-mono opacity-40 uppercase tracking-wider">{{ plugin.englishName }}</span>
                    </div>
                    <span class="text-[10px] opacity-50 mt-0.5 line-clamp-1 pr-4">{{ plugin.description }}</span>
                  </div>
                </div>
                <div class="flex items-center gap-2">
                  <!-- Premium Toggle Switch -->
                  <div 
                    @click.stop="toggleToolEnabled(plugin, $event)"
                    class="w-8 h-4.5 rounded-full p-0.5 transition-colors duration-300 cursor-pointer flex items-center shrink-0"
                    :class="plugin.enabled ? 'bg-[var(--highlight-text)]/40 border border-[var(--highlight-text)]/30' : 'bg-black/15 dark:bg-white/15 border border-black/5 dark:border-white/5'"
                  >
                    <div 
                      class="w-3.5 h-3.5 rounded-full bg-white dark:bg-black shadow-sm transition-transform duration-300"
                      :class="plugin.enabled ? 'translate-x-3.5' : 'translate-x-0'"
                    ></div>
                  </div>

                  <span
                    class="text-[7px] font-mono uppercase px-1.5 py-0.5 rounded border border-black/10 dark:border-white/10"
                    :class="plugin.type === 'streaming' ? 'text-amber-500/80 border-amber-500/10' : 'text-blue-500/80 border-blue-500/10'"
                  >
                    {{ plugin.type }}
                  </span>
                  <svg
                    class="transition-transform duration-300 opacity-30"
                    :class="expandedPluginId === plugin.id ? 'rotate-180' : ''"
                    width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
                  >
                    <path d="m6 9 6 6 6-6"/>
                  </svg>
                </div>
              </div>

              <!-- Collapsible Content -->
              <div v-if="expandedPluginId === plugin.id" class="border-t border-black/5 dark:border-white/5 p-4 bg-black/10 dark:bg-white/10 space-y-3">
                <div class="flex justify-between items-center text-[9px] uppercase font-bold opacity-40 tracking-wider">
                  <span>实时遥测数据 / Realtime Telemetry</span>
                  <button
                    @click.stop="togglePlugin(plugin)"
                    class="px-2 py-0.5 bg-black/10 dark:bg-white/10 rounded-md hover:bg-black/20 flex items-center gap-1 active:scale-95 transition-all text-[8px]"
                  >
                    刷新读取
                  </button>
                </div>

                <div v-if="pluginLoading[plugin.id]" class="flex items-center justify-center py-6 opacity-40 text-xs gap-2">
                  <svg class="animate-spin h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3">
                    <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                    <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
                  </svg>
                  <span class="font-mono text-[10px]">JNI 物理通道拉取中...</span>
                </div>

                <div v-else class="rounded-xl bg-black/20 dark:bg-black/40 border border-black/10 dark:border-white/5 p-3.5">
                  <pre class="text-[10px] font-mono text-primary-text/90 whitespace-pre-wrap leading-relaxed break-all">{{ pluginData[plugin.id] || '无物理遥测数据' }}</pre>
                </div>

                <div v-if="plugin.placeholder" class="flex justify-between items-center text-[10px] text-primary-text pt-1">
                  <span class="opacity-50">内置占位符：</span>
                  <span
                    @click="copyPlaceholder(plugin.placeholder)"
                    class="font-mono bg-[var(--highlight-text)]/10 text-[var(--highlight-text)] px-1.5 py-0.5 rounded border border-[var(--highlight-text)]/20 active:scale-95 transition-all cursor-pointer font-semibold text-[9px]"
                  >
                    {{ plugin.placeholder }}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- 3. Placeholders list view -->
        <div v-if="activeTab === 'placeholders'" class="flex flex-col h-full">
          <!-- Search box -->
          <div class="px-4 py-3 shrink-0 border-b border-black/5 dark:border-white/5">
            <div class="relative">
              <input
                v-model="searchQuery"
                type="text"
                placeholder="搜索占位符宏 (例如 CPU, GPS...)"
                class="w-full bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-xl px-3.5 py-2.5 text-xs text-primary-text placeholder-opacity-40 focus:outline-none focus:border-[var(--highlight-text)]/50 font-sans"
              />
              <span 
                v-if="searchQuery" 
                @click="searchQuery = ''" 
                class="absolute right-3.5 top-1/2 -translate-y-1/2 opacity-40 text-[10px] uppercase font-bold tracking-wider active:scale-90 p-1 cursor-pointer select-none"
              >
                Clear
              </span>
            </div>
          </div>

          <!-- Placeholders layout -->
          <div class="flex-1 overflow-y-auto px-4 py-4 space-y-3 no-rubber-band">
            <div
              v-for="item in filteredPlaceholders"
              :key="item.macro"
              @click="copyPlaceholder(item.macro)"
              class="bg-black/2 dark:bg-white/2 border border-black/5 dark:border-white/5 rounded-2xl p-4 flex flex-col gap-2 hover:border-black/10 dark:hover:border-white/10 active:bg-black/5 dark:active:bg-white/5 transition-all cursor-pointer shadow-sm"
            >
              <div class="flex items-center justify-between">
                <div class="flex items-center gap-2">
                  <span class="text-xs font-bold text-primary-text">{{ item.name }}</span>
                </div>
                <span class="font-mono text-[9px] bg-black/10 dark:bg-white/10 border border-black/5 dark:border-white/5 px-2 py-0.5 rounded text-[var(--highlight-text)] font-semibold select-all">
                  {{ item.macro }}
                </span>
              </div>
              <p class="text-[10px] opacity-50">{{ item.description }}</p>
            </div>
          </div>
        </div>
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.distributed-view {
  background-color: color-mix(in srgb, var(--primary-bg) 100%, transparent);
}
</style>
