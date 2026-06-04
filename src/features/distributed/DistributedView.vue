<script setup lang="ts">
import { ref, onMounted, watch, computed } from "vue";
import { useSettingsStore } from "../../core/stores/settings";
import { useNotificationStore } from "../../core/stores/notification";
import { useDistributed } from "./composables/useDistributed";
import SlidePage from "../../components/ui/SlidePage.vue";

// Atomic settings UI components
import SettingsTextField from "../../components/settings/SettingsTextField.vue";
import SettingsCard from "../../components/settings/SettingsCard.vue";
import { checkRootAccess, launchRootManager } from "../../../src-tauri/plugins/vcp-mobile/guest-js";

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
const { status } = useDistributed();

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
  requiresRoot: boolean;
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
        if (!perms || typeof perms !== "object") {
          throw new Error("返回的权限状态格式错误");
        }
        if (perms[requiredAlias] === undefined) {
          throw new Error(`权限对象中缺少 '${requiredAlias}' 字段。可用字段: ${Object.keys(perms).join(", ")}`);
        }
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
          if (!permsRetry || typeof permsRetry !== "object" || permsRetry[requiredAlias] === undefined) {
            throw new Error("重新校验权限时返回的数据错误");
          }
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
      } catch (err: any) {
        console.error("[DistributedView] Failed to verify system permissions:", err);
        notificationStore.addNotification({
          type: "error",
          title: "权限校验失败",
          message: `错误: ${err.toString()}`,
          toastOnly: true
        });
        return; // 出错时也中断开启，避免状态不一致
      }
    }
  }
  
  plugin.enabled = targetState;
  
  if (!targetState && expandedPluginId.value === plugin.id) {
    expandedPluginId.value = null;
  }

  // 实时提取当前所有被禁用插件的 ID (排除当前被启用的，或加入被禁用的)
  const currentDisabled = pluginsList.value
    .filter(p => p.id === plugin.id ? !targetState : !p.enabled)
    .map(p => p.id);

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("update_disabled_tools", { disabledNames: currentDisabled });
  } catch (e) {
    console.error("[DistributedView] Failed to sync disabled tools to backend:", e);
  }
  
  await loadPluginsMetadata();
};

// Fetch dynamic plugin metadata from Rust backend
const loadPluginsMetadata = async () => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const rawTools = await invoke<any[]>("get_registered_tools_metadata");


    
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
        enabled: tool.enabled !== undefined ? tool.enabled : true,
        requiresRoot: !!tool.requiresRoot
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

// Contextual folding block states (Scheme B)
interface FoldBlock {
  threshold: number;
  desc: string;
  content: string;
}

const pluginFoldBlocks = ref<Record<string, FoldBlock[]>>({});
const selectedFoldBlockIdx = ref<Record<string, number>>({});

const parseFoldBlocks = (raw: string): FoldBlock[] => {
  if (!raw.includes("[===vcp_fold:")) return [];

  const blockRegex = /\[===vcp_fold:\s*([\d.]+)(?:\s*::desc:\s*([^\]]+))?===\]\n?([\s\S]*?)(?=\n?\[===vcp_fold:|$)/g;
  const blocks: FoldBlock[] = [];
  let match;
  
  while ((match = blockRegex.exec(raw)) !== null) {
    const threshold = parseFloat(match[1]);
    const rawDesc = match[2] || "";
    const content = match[3].trim();
    const desc = rawDesc.trim() || `层级 ${threshold}`;
    blocks.push({ threshold, desc, content });
  }
  
  return blocks;
};

const selectTab = (pluginId: string, index: number) => {
  selectedFoldBlockIdx.value[pluginId] = index;
  const blocks = pluginFoldBlocks.value[pluginId];
  if (blocks && blocks[index]) {
    pluginData.value[pluginId] = blocks[index].content;
  }
};

// Loading settings from main settings store
const loadSettings = async () => {
  try {
    await settingsStore.fetchSettings();
    if (settingsStore.settings) {
      wsUrl.value = settingsStore.settings.distributedWsUrl || settingsStore.settings.vcpLogUrl || settingsStore.settings.vcpServerUrl || "";
      vcpKey.value = settingsStore.settings.distributedVcpKey || settingsStore.settings.vcpLogKey || settingsStore.settings.vcpApiKey || "";
      deviceName.value = settingsStore.settings.distributedDeviceName ?? "VCPMobile";
    }
    // Pull registered tools dynamically
    await loadPluginsMetadata();
  } catch (e) {
    console.error("[DistributedView] Failed to load settings:", e);
  }
};

const handleConnect = async () => {
  if (!wsUrl.value || !vcpKey.value) return;
  try {
    if (settingsStore.settings) {
      const updates = {
        distributedWsUrl: wsUrl.value,
        distributedVcpKey: vcpKey.value,
        distributedDeviceName: deviceName.value,
        distributedEnabled: true
      };
      await settingsStore.updateSettings(updates);
    }
  } catch (e: any) {
    console.error("[DistributedView] Connection start failed:", e);
  }
};

const handleDisconnect = async () => {
  try {
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
    const { invoke } = await import("@tauri-apps/api/core");
    const res = await invoke<string>("execute_distributed_tool", { name: plugin.id });
    
    // 解析是否含有折叠块协议
    const blocks = parseFoldBlocks(res);
    if (blocks.length > 0) {
      pluginFoldBlocks.value[plugin.id] = blocks;
      selectedFoldBlockIdx.value[plugin.id] = 0; // 默认选中首个 Tab (通常是 0.0 极简级)
      pluginData.value[plugin.id] = blocks[0].content;
    } else {
      pluginFoldBlocks.value[plugin.id] = [];
      // 如果返回的值可以解析为 JSON 对象，我们对其进行排版美化
      try {
        const parsed = JSON.parse(res);
        if (parsed && typeof parsed === "object") {
          pluginData.value[plugin.id] = JSON.stringify(parsed, null, 2);
        } else {
          pluginData.value[plugin.id] = res;
        }
      } catch {
        pluginData.value[plugin.id] = res;
      }
    }
  } catch (e: any) {
    console.error(`[DistributedView] Failed to pull native sensor telemetry for ${plugin.id}:`, e);
    pluginData.value[plugin.id] = `遥测读取异常:\n${e.toString()}\n(请检查移动端传感器硬件模块是否正常运行或对应的 JNI 权限是否已授予)`;
  } finally {
    pluginLoading.value[plugin.id] = false;
  }
};

// Root Access States & Functions
const isRootGranted = ref<boolean | null>(null);

const checkRootState = async () => {
  console.log("[DistributedView] checkRootState checking system root access...");
  try {
    const res = await checkRootAccess();
    console.log("[DistributedView] checkRootState result:", JSON.stringify(res));
    isRootGranted.value = res.isRoot;
  } catch (e) {
    console.error("[DistributedView] Failed to check root access:", e);
    isRootGranted.value = false;
  }
};

const handleLaunchRootManager = async () => {
  console.log("[DistributedView] handleLaunchRootManager triggered, attempting JNI redirect...");
  try {
    const res = await launchRootManager();
    console.log("[DistributedView] launchRootManager JNI result:", JSON.stringify(res));
    if (res.success) {
      notificationStore.addNotification({
        type: "success",
        title: "启动成功",
        message: `已成功启动 ${res.manager || 'Root管理器'}。请在其中授予 VCPMobile 的超级用户权限，然后返回应用重新检测。`,
        toastOnly: true
      });
      // 3秒后自动检测一次
      setTimeout(checkRootState, 3000);
    } else {
      notificationStore.addNotification({
        type: "warning",
        title: "未找到授权管理器",
        message: res.message || "未在设备上检测到 Magisk、KernelSU 或 APatch 管理器应用。",
        toastOnly: true
      });
    }
  } catch (e: any) {
    console.error("[DistributedView] Failed to launch root manager:", e);
    notificationStore.addNotification({
      type: "error",
      title: "启动管理器失败",
      message: e.toString(),
      toastOnly: true
    });
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
    loadSettings();
  }
});

watch(
  () => props.isOpen,
  async (val: boolean) => {
    if (val) {
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
        <div class="flex items-baseline gap-2 flex-wrap">
          <h2 class="text-xl font-bold tracking-tight text-primary-text shrink-0">分布式设备面板</h2>
          <span class="text-[8px] font-mono opacity-40 uppercase tracking-wider shrink-0">Distributed Panel // Client V2</span>
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
                  :class="status.connected ? 'bg-emerald-400' : (status.state === 'connecting' || status.state === 'disconnecting') ? 'bg-amber-400' : 'bg-rose-400'"
                ></span>
                <span
                  class="relative inline-flex rounded-full h-3 w-3"
                  :class="status.connected ? 'bg-emerald-500' : (status.state === 'connecting' || status.state === 'disconnecting') ? 'bg-amber-500' : 'bg-rose-500'"
                ></span>
              </span>
              <div class="flex items-baseline gap-1.5 flex-wrap">
                <span class="text-xs font-bold shrink-0">连接状态</span>
                <span class="text-[8px] opacity-50 font-mono uppercase tracking-wider shrink-0">{{ status.connected ? 'Connected' : (status.state === 'connecting' ? 'Connecting...' : status.state === 'disconnecting' ? 'Disconnecting...' : 'Disconnected') }}</span>
              </div>
            </div>
            <div class="flex gap-2">
              <button
                v-if="status.connected"
                @click="handleDisconnect"
                :disabled="status.state === 'disconnecting'"
                class="px-4 py-1.5 bg-red-500/20 text-red-500 text-xs font-bold rounded-xl active:scale-95 transition-all hover:bg-red-500/30 disabled:opacity-50"
              >
                断开连接
              </button>
              <button
                v-else
                @click="handleConnect"
                :disabled="status.state === 'connecting' || !wsUrl || !vcpKey"
                class="px-4 py-1.5 text-white text-xs font-bold rounded-xl active:scale-95 transition-all disabled:opacity-50"
                style="background-color: var(--highlight-text)"
              >
                {{ status.state === 'connecting' ? '连接中...' : '连接' }}
              </button>
            </div>
          </div>

          <!-- Root Status Card -->
          <div class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl space-y-3">
            <div class="flex items-center justify-between gap-3">
              <div class="flex items-center gap-3 min-w-0">
                <div class="h-8 w-8 rounded-xl bg-black/5 dark:bg-white/5 flex items-center justify-center shrink-0">
                  <svg v-if="isRootGranted === true" class="text-emerald-500" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
                    <path d="m9 11 2 2 4-4"/>
                  </svg>
                  <svg v-else-if="isRootGranted === false" class="text-rose-500" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
                    <line x1="12" y1="8" x2="12" y2="12"/>
                    <line x1="12" y1="16" x2="12.01" y2="16"/>
                  </svg>
                  <svg v-else class="text-primary-text/40" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
                    <circle cx="12" cy="12" r="1"/>
                  </svg>
                </div>
                <div class="flex flex-col min-w-0">
                  <span class="text-xs font-bold text-primary-text truncate">超级用户 Root 状态</span>
                  <span class="text-[7px] opacity-40 font-mono uppercase tracking-wider truncate">
                    {{ isRootGranted === true ? 'ROOT ACCESS GRANTED' : isRootGranted === false ? 'ROOT ACCESS DENIED' : 'ROOT STATUS UNCHECKED' }}
                  </span>
                </div>
              </div>
              <div class="flex gap-1.5 shrink-0">
                <button
                  v-if="isRootGranted === null"
                  @click="checkRootState"
                  class="px-2.5 py-1 text-white text-[9px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                  style="background-color: var(--highlight-text)"
                >
                  检测 Root
                </button>
                <template v-else>
                  <button
                    @click="checkRootState"
                    class="px-2.5 py-1 bg-black/10 dark:bg-white/10 text-primary-text text-[9px] font-bold rounded-lg active:scale-95 transition-all hover:bg-black/20 shrink-0"
                  >
                    重新检测
                  </button>
                  <button
                    v-if="isRootGranted === false"
                    @click="handleLaunchRootManager"
                    class="px-2.5 py-1 text-white text-[9px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                    style="background-color: var(--highlight-text)"
                  >
                    一键授权
                  </button>
                </template>
              </div>
            </div>
            
            <p v-if="isRootGranted === false" class="text-[10px] opacity-60 leading-relaxed">
              * 检测到未获得 Root 权限。由于 Android 系统的 SELinux 安全沙盒机制，没有 Root 权限时，诸如 CPU/GPU 实时负载及温度等核心物理遥测指标将不可用或信息降级。请在 Magisk/KernelSU 中通过授权。
            </p>
            <p v-else-if="isRootGranted === true" class="text-[10px] opacity-60 leading-relaxed">
              * 已成功取得系统级超级用户权限。所有底层的 sysfs / procfs 接口通道已解锁，分布式遥测数据将以最高精度输出。
            </p>
            <p v-else class="text-[10px] opacity-60 leading-relaxed">
              * 超级用户权限未检测。点击“检测 Root”将发起授权验证（可能触发系统的 Magisk/KernelSU 授权弹窗）。如果您不需要 CPU/GPU 核心占用等物理监控指标，可保持未检测状态。
            </p>
          </div>

          <!-- Configuration Fields -->
          <SettingsCard>
            <div class="space-y-4 py-1">
              <SettingsTextField
                v-model="deviceName"
                label="节点名称 / Device Name"
                placeholder="例如 VCPMobile"
                :disabled="status.state !== 'disconnected'"
              />
              <SettingsTextField
                v-model="wsUrl"
                label="WS 服务地址 / WebSocket URL"
                placeholder="ws://192.168.x.x:port"
                mono
                :disabled="status.state !== 'disconnected'"
              />
              <SettingsTextField
                v-model="vcpKey"
                label="VCP 鉴权密钥 / VCP API Key"
                placeholder="授权校验密钥"
                is-secure
                :disabled="status.state !== 'disconnected'"
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
                <div class="flex items-center gap-2.5">
                  <div class="w-5 h-5 shrink-0 flex items-center justify-center text-primary-text opacity-70">
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                      <template v-if="plugin.id.includes('CPU')">
                        <rect x="4" y="4" width="16" height="16" rx="2" />
                        <rect x="9" y="9" width="6" height="6" />
                        <path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 15h3M1 9h3M1 15h3" />
                      </template>
                      <template v-else-if="plugin.id.includes('GPU')">
                        <rect x="2" y="2" width="20" height="20" rx="2" />
                        <path d="M6 14h12M6 10h12M10 6v12M14 6v12" />
                      </template>
                      <template v-else-if="plugin.id.includes('Battery')">
                        <rect x="2" y="7" width="16" height="10" rx="2" ry="2" />
                        <line x1="22" y1="11" x2="22" y2="13" />
                      </template>
                      <template v-else-if="plugin.id.includes('Location')">
                        <path d="M20 10c0 6-8 12-8 12s-8-6-8-12a8 8 0 0 1 16 0Z" />
                        <circle cx="12" cy="10" r="3" />
                      </template>
                      <template v-else-if="plugin.id.includes('Network')">
                        <path d="M5 12.55a11 11 0 0 1 14.08 0" />
                        <path d="M1.42 9a16 16 0 0 1 21.16 0" />
                        <path d="M8.53 16.11a6 6 0 0 1 6.95 0" />
                        <line x1="12" y1="20" x2="12.01" y2="20" />
                      </template>
                      <template v-else-if="plugin.id.includes('Notification')">
                        <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
                        <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
                      </template>
                      <template v-else-if="plugin.id.includes('Clipboard')">
                        <rect x="8" y="2" width="8" height="4" rx="1" ry="1" />
                        <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
                      </template>
                      <template v-else-if="plugin.id.includes('Memory') || plugin.id.includes('Storage')">
                        <rect x="2" y="2" width="20" height="20" rx="2" />
                        <path d="M2 14h20M6 18h.01M10 18h.01" />
                      </template>
                      <template v-else-if="plugin.id.includes('Ambient')">
                        <circle cx="12" cy="12" r="4" />
                        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
                      </template>
                      <template v-else-if="plugin.id.includes('Motion')">
                        <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                      </template>
                      <template v-else-if="plugin.id.includes('DeviceInfo')">
                        <rect x="5" y="2" width="14" height="20" rx="2" ry="2" />
                        <line x1="12" y1="18" x2="12.01" y2="18" />
                      </template>
                      <template v-else-if="plugin.id.includes('StatusSummary')">
                        <line x1="18" y1="20" x2="18" y2="10" />
                        <line x1="12" y1="20" x2="12" y2="4" />
                        <line x1="6" y1="20" x2="6" y2="14" />
                      </template>
                      <template v-else>
                        <rect x="3" y="3" width="18" height="18" rx="2" />
                        <path d="M21 12H3M12 3v18" />
                      </template>
                    </svg>
                  </div>
                  <div class="flex flex-col">
                    <div class="flex items-baseline gap-1.5 flex-wrap">
                      <span class="text-xs font-bold shrink-0">{{ plugin.name }}</span>
                      <span class="text-[8px] font-mono opacity-40 uppercase tracking-wider shrink-0">{{ plugin.englishName }}</span>
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
                <!-- Plugin Description Area -->
                <div class="text-[10px] opacity-70 leading-relaxed text-primary-text border-b border-black/5 dark:border-white/5 pb-2.5">
                  <span class="font-bold opacity-50 text-[8px] uppercase tracking-wider block mb-1">功能描述 / Description</span>
                  {{ plugin.description }}
                </div>

                <div class="flex justify-between items-center text-[9px] uppercase font-bold opacity-40 tracking-wider">
                  <span>实时遥测数据 / Realtime Telemetry</span>
                  <button
                    @click.stop="togglePlugin(plugin)"
                    class="px-2 py-0.5 bg-black/10 dark:bg-white/10 rounded-md hover:bg-black/20 flex items-center gap-1 active:scale-95 transition-all text-[8px]"
                  >
                    刷新读取
                  </button>
                </div>

                <!-- Root status unchecked banner for system telemetries -->
                <div v-if="isRootGranted === null && plugin.requiresRoot" class="bg-amber-500/10 dark:bg-amber-400/5 border border-amber-500/20 dark:border-amber-400/10 p-2.5 rounded-xl flex items-center justify-between text-[10px]">
                  <div class="flex items-center gap-1.5 text-amber-800 dark:text-amber-400">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="shrink-0">
                      <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"/>
                      <line x1="12" y1="9" x2="12" y2="13"/>
                      <line x1="12" y1="17" x2="12.01" y2="17"/>
                    </svg>
                    <span>此工具需要 Root 权限以读取实时物理遥测</span>
                  </div>
                  <button 
                    @click.stop="checkRootState"
                    class="px-2 py-0.5 bg-amber-500/20 hover:bg-amber-500/30 dark:bg-amber-400/20 dark:hover:bg-amber-400/30 text-amber-800 dark:text-amber-400 rounded active:scale-95 transition-all text-[8px] font-bold shrink-0"
                  >
                    检测 Root
                  </button>
                </div>

                <!-- Root warning banner for system telemetries -->
                <div v-if="isRootGranted === false && plugin.requiresRoot" class="bg-amber-500/10 dark:bg-amber-400/5 border border-amber-500/20 dark:border-amber-400/10 p-2.5 rounded-xl flex items-center justify-between text-[10px]">
                  <div class="flex items-center gap-1.5 text-amber-800 dark:text-amber-400">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="shrink-0">
                      <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"/>
                      <line x1="12" y1="9" x2="12" y2="13"/>
                      <line x1="12" y1="17" x2="12.01" y2="17"/>
                    </svg>
                    <span>未获得 Root 权限，部分遥测参数已使用标准 API 降级</span>
                  </div>
                  <button 
                    @click.stop="handleLaunchRootManager"
                    class="px-2 py-0.5 bg-amber-500/20 hover:bg-amber-500/30 dark:bg-amber-400/20 dark:hover:bg-amber-400/30 text-amber-800 dark:text-amber-400 rounded active:scale-95 transition-all text-[8px] font-bold shrink-0"
                  >
                    跳转授权
                  </button>
                </div>

                <div v-if="pluginLoading[plugin.id]" class="flex items-center justify-center py-6 opacity-40 text-xs gap-2">
                  <svg class="animate-spin h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3">
                    <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                    <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
                  </svg>
                  <span class="font-mono text-[10px]">JNI 物理通道拉取中...</span>
                </div>

                <div v-else class="space-y-2">
                  <!-- 方案 B: 动态层级 Tabs 切换 -->
                  <div v-if="pluginFoldBlocks[plugin.id] && pluginFoldBlocks[plugin.id].length > 0" class="flex flex-wrap gap-1.5 pb-1 border-b border-black/5 dark:border-white/5">
                    <button
                      v-for="(block, index) in pluginFoldBlocks[plugin.id]"
                      :key="index"
                      @click.stop="selectTab(plugin.id, index)"
                      class="px-2.5 py-1 text-[9px] font-bold rounded-lg transition-all active:scale-95 border"
                      :class="selectedFoldBlockIdx[plugin.id] === index
                        ? 'text-[var(--highlight-text)]'
                        : 'bg-black/5 dark:bg-white/5 border-transparent opacity-60 text-primary-text hover:opacity-100'"
                      :style="selectedFoldBlockIdx[plugin.id] === index
                        ? {
                            backgroundColor: 'color-mix(in srgb, var(--highlight-text) 15%, transparent)',
                            borderColor: 'color-mix(in srgb, var(--highlight-text) 30%, transparent)'
                          }
                        : {}"
                    >
                      {{ block.desc }} ({{ block.threshold }})
                    </button>
                  </div>

                  <div class="rounded-xl bg-black/3 dark:bg-black/40 border border-black/5 dark:border-white/5 p-3.5">
                    <pre class="text-[10px] font-mono text-primary-text/90 whitespace-pre-wrap leading-relaxed break-all">{{ pluginData[plugin.id] || '无物理遥测数据' }}</pre>
                  </div>
                </div>

                <div v-if="plugin.placeholder" class="flex justify-between items-center text-[10px] text-primary-text pt-1">
                  <span class="opacity-50">内置占位符：</span>
                  <span
                    @click="copyPlaceholder(plugin.placeholder)"
                    class="font-mono px-1.5 py-0.5 rounded border border-[var(--highlight-text)]/20 active:scale-95 transition-all cursor-pointer font-semibold text-[9px]"
                    style="background-color: color-mix(in srgb, var(--highlight-text) 10%, transparent); color: var(--highlight-text);"
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

          <!-- Micro Tips for copy action -->
          <div class="px-4 py-1.5 shrink-0 bg-black/2 dark:bg-white/2 border-b border-black/5 dark:border-white/5 flex items-center gap-1.5">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="shrink-0 text-[var(--highlight-text)] opacity-70">
              <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
              <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
            </svg>
            <span class="text-[10px] text-primary-text opacity-60">点击任意占位符卡片即可快速复制到剪贴板</span>
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
