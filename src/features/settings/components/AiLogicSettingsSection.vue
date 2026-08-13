<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  MobileCliAgentRoute,
} from "../../../core/stores/settings";
import { useDistributed } from "../../distributed/composables/useDistributed";

const props = withDefaults(
  defineProps<{
    settings: AppSettings;
    saving?: boolean;
    saveError?: string | null;
  }>(),
  {
    saving: false,
    saveError: null,
  },
);

const emit = defineEmits<{
  routeChange: [route: MobileCliAgentRoute];
}>();

interface RegisteredToolMetadata {
  name?: string;
  enabled?: boolean;
}

type ToolAuthorizationState =
  | "idle"
  | "loading"
  | "authorized"
  | "unauthorized"
  | "unavailable";

const { status, activate, deactivate, refreshStatus } = useDistributed();
const toolAuthorization = ref<ToolAuthorizationState>("idle");
const toolAuthorizationDetail = ref("");
let preflightGeneration = 0;
let distributedConsumerActive = false;

const isVcpPluginRoute = computed(
  () => props.settings.mobileCliAgentRoute === "vcpPlugin",
);
const hasRemoteEndpoint = computed(() =>
  Boolean(props.settings.distributedWsUrl),
);
const hasRemoteKey = computed(() =>
  Boolean(props.settings.distributedVcpKey),
);

const selectRoute = (route: MobileCliAgentRoute) => {
  if (props.saving || props.settings.mobileCliAgentRoute === route) return;
  emit("routeChange", route);
};

const loadToolAuthorization = async (generation: number) => {
  try {
    const tools = await invoke<RegisteredToolMetadata[]>(
      "get_registered_tools_metadata",
    );
    if (generation !== preflightGeneration || !isVcpPluginRoute.value) return;

    const mobileCli = tools.find((tool) => tool.name === "VCPMobileCLI");
    if (!mobileCli) {
      toolAuthorization.value = "unavailable";
      toolAuthorizationDetail.value = "本机未扫描到 VCPMobileCLI";
    } else if (mobileCli.enabled === true) {
      toolAuthorization.value = "authorized";
      toolAuthorizationDetail.value = "VCPMobileCLI 已在显式 allowlist 中";
    } else {
      toolAuthorization.value = "unauthorized";
      toolAuthorizationDetail.value = "请在插件中心手动授权 VCPMobileCLI";
    }
  } catch (error) {
    if (generation !== preflightGeneration || !isVcpPluginRoute.value) return;
    toolAuthorization.value = "unavailable";
    toolAuthorizationDetail.value = `工具清单读取失败：${String(error)}`;
  }
};

const runRemotePreflight = async (refreshConnection = false) => {
  const generation = ++preflightGeneration;
  toolAuthorization.value = "loading";
  toolAuthorizationDetail.value = "正在读取只读状态…";

  try {
    if (!distributedConsumerActive) {
      distributedConsumerActive = true;
      await activate();
    } else if (refreshConnection) {
      await refreshStatus();
    }

    if (generation !== preflightGeneration || !isVcpPluginRoute.value) return;
    await loadToolAuthorization(generation);
  } catch (error) {
    if (generation !== preflightGeneration || !isVcpPluginRoute.value) return;
    toolAuthorization.value = "unavailable";
    toolAuthorizationDetail.value = `预检状态读取失败：${String(error)}`;
  } finally {
    if (
      generation === preflightGeneration &&
      isVcpPluginRoute.value &&
      toolAuthorization.value === "loading"
    ) {
      toolAuthorization.value = "unavailable";
      toolAuthorizationDetail.value = "预检未返回可用状态，请重试";
    }
  }
};

watch(
  isVcpPluginRoute,
  (enabled) => {
    if (enabled) {
      void runRemotePreflight();
      return;
    }

    preflightGeneration += 1;
    toolAuthorization.value = "idle";
    toolAuthorizationDetail.value = "";
    if (distributedConsumerActive) {
      distributedConsumerActive = false;
      deactivate();
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  preflightGeneration += 1;
  if (distributedConsumerActive) {
    distributedConsumerActive = false;
    deactivate();
  }
});
</script>

<template>
  <div class="space-y-4 px-1 py-1">
    <div class="space-y-2" role="radiogroup" aria-label="移动 CLI Agent 路由">
      <button
        type="button"
        role="radio"
        data-mobile-cli-route="localLoopback"
        :aria-checked="settings.mobileCliAgentRoute === 'localLoopback'"
        :disabled="saving"
        class="w-full border-l-2 rounded-lg px-3 py-2.5 text-left transition-opacity disabled:opacity-45"
        :class="settings.mobileCliAgentRoute === 'localLoopback'
          ? 'border-blue-500 bg-blue-500/8'
          : 'border-transparent bg-black/3 dark:bg-white/3 active:opacity-65'"
        @click="selectRoute('localLoopback')"
      >
        <span class="flex items-center justify-between gap-3">
          <span class="text-[13px] font-bold">本机闭环</span>
          <code class="text-[10px] opacity-55">localLoopback</code>
        </span>
        <span class="mt-1 block text-[10px] leading-relaxed opacity-55">
          默认。移动端本地执行工具续轮，不依赖分布式长连接。
        </span>
      </button>

      <button
        type="button"
        role="radio"
        data-mobile-cli-route="vcpPlugin"
        :aria-checked="settings.mobileCliAgentRoute === 'vcpPlugin'"
        :disabled="saving"
        class="w-full border-l-2 rounded-lg px-3 py-2.5 text-left transition-opacity disabled:opacity-45"
        :class="settings.mobileCliAgentRoute === 'vcpPlugin'
          ? 'border-blue-500 bg-blue-500/8'
          : 'border-transparent bg-black/3 dark:bg-white/3 active:opacity-65'"
        @click="selectRoute('vcpPlugin')"
      >
        <span class="flex items-center justify-between gap-3">
          <span class="text-[13px] font-bold">VCP 插件闭环</span>
          <code class="text-[10px] opacity-55">vcpPlugin</code>
        </span>
        <span class="mt-1 block text-[10px] leading-relaxed opacity-55">
          由 VCPToolBox 负责工具续轮；选择本项只保存路由，不会代你开启或授权任何能力。
        </span>
      </button>
    </div>

    <p
      v-if="saveError"
      role="alert"
      class="border-l-2 border-red-500 pl-2 text-[10px] leading-relaxed text-red-500"
    >
      {{ saveError }}
    </p>

    <div
      v-if="isVcpPluginRoute"
      data-mobile-cli-preflight
      class="border-t border-black/8 pt-3 dark:border-white/8"
    >
      <div class="mb-2 flex items-center justify-between gap-3">
        <div>
          <p class="text-[11px] font-black uppercase tracking-[0.12em] opacity-65">只读预检</p>
          <p class="mt-0.5 text-[9px] leading-relaxed opacity-45">缺项不会被自动修复</p>
        </div>
        <button
          type="button"
          class="rounded-lg border border-black/10 px-2.5 py-1.5 text-[10px] font-bold active:opacity-55 dark:border-white/10"
          :disabled="toolAuthorization === 'loading'"
          @click="runRemotePreflight(true)"
        >
          刷新
        </button>
      </div>

      <div class="divide-y divide-black/6 text-[10px] dark:divide-white/6">
        <div class="flex items-start justify-between gap-4 py-2">
          <span class="opacity-55">分布式节点</span>
          <span :class="settings.distributedEnabled ? 'text-emerald-500' : 'text-red-500'">
            {{ settings.distributedEnabled ? '已显式开启' : '未开启' }}
          </span>
        </div>
        <div class="flex items-start justify-between gap-4 py-2">
          <span class="opacity-55">连接配置</span>
          <span :class="hasRemoteEndpoint && hasRemoteKey ? 'text-emerald-500' : 'text-red-500'">
            {{ hasRemoteEndpoint && hasRemoteKey ? '地址与密钥已配置' : '缺少地址或密钥' }}
          </span>
        </div>
        <div class="flex items-start justify-between gap-4 py-2">
          <span class="opacity-55">实时连接</span>
          <span :class="status.connected ? 'text-emerald-500' : 'text-red-500'">
            {{ status.connected ? '已连接' : (status.last_error || '未连接') }}
          </span>
        </div>
        <div class="flex items-start justify-between gap-4 py-2">
          <span class="opacity-55">工具授权</span>
          <span
            class="max-w-[68%] text-right font-mono"
            :class="toolAuthorization === 'authorized' ? 'text-emerald-500' : 'text-red-500'"
          >
            {{ toolAuthorizationDetail }}
          </span>
        </div>
      </div>

      <p class="mt-3 border-l-2 border-amber-500 pl-2 text-[9px] leading-relaxed opacity-65">
        本页不会切换分布式开关、VCP 动态注入或工具 allowlist。提示词仍由用户与 VCPToolBox 所有。
      </p>
    </div>
  </div>
</template>
