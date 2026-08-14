<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { AppSettings } from "../../../core/stores/settings";
import { useDistributed } from "../../distributed/composables/useDistributed";

const props = defineProps<{
  settings: AppSettings;
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

const hasRemoteEndpoint = computed(() =>
  Boolean(props.settings.distributedWsUrl),
);
const hasRemoteKey = computed(() =>
  Boolean(props.settings.distributedVcpKey),
);

const loadToolAuthorization = async (generation: number) => {
  try {
    const tools = await invoke<RegisteredToolMetadata[]>(
      "get_registered_tools_metadata",
    );
    if (generation !== preflightGeneration) return;

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
    if (generation !== preflightGeneration) return;
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

    if (generation !== preflightGeneration) return;
    await loadToolAuthorization(generation);
  } catch (error) {
    if (generation !== preflightGeneration) return;
    toolAuthorization.value = "unavailable";
    toolAuthorizationDetail.value = `预检状态读取失败：${String(error)}`;
  } finally {
    if (
      generation === preflightGeneration &&
      toolAuthorization.value === "loading"
    ) {
      toolAuthorization.value = "unavailable";
      toolAuthorizationDetail.value = "预检未返回可用状态，请重试";
    }
  }
};

onMounted(() => {
  void runRemotePreflight();
});

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
    <div
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
