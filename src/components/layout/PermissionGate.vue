<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAppLifecycleStore } from "../../core/stores/appLifecycle";

const lifecycleStore = useAppLifecycleStore();

const REQUEST_RECHECK_DELAYS_MS = [250, 1000, 2500, 5000] as const;

interface PermissionStatus {
  notification: boolean;
  ring: boolean;
  storage: boolean;
  battery: boolean;
  backgroundRestricted: boolean;
  requiresManualPowerManagement: boolean;
}

type RequestablePermission = "notification" | "ring" | "storage" | "battery";
type AutoStartStatus = "true" | "false" | "unsupported";

interface DiskSpaceSnapshot {
  freeGb: number;
  totalGb: number;
}

interface PermissionItem {
  id: RequestablePermission;
  name: string;
  desc: string;
  icon: string;
  required: boolean;
}

const permissionItems: PermissionItem[] = [
  {
    id: "notification",
    name: "系统通知",
    desc: "显示 Agent 运行状态和即时提醒",
    icon: "i-heroicons-bell",
    required: true,
  },
  {
    id: "ring",
    name: "通知铃声",
    desc: "允许 AgentMessage 通知发声或振动；一加/OPPO 等系统需单独开启",
    icon: "i-heroicons-speaker-wave",
    required: true,
  },
  {
    id: "storage",
    name: "储存空间权限",
    desc: "用于保存头像、聊天图片及导出日志",
    icon: "i-heroicons-folder-open",
    required: true,
  },
  {
    id: "battery",
    name: "后台运行权限",
    desc: "切换到后台时保持连接不被系统中断",
    icon: "i-heroicons-arrow-path",
    required: true,
  },
];

const status = ref<PermissionStatus>({
  notification: false,
  ring: false,
  storage: false,
  battery: false,
  backgroundRestricted: false,
  requiresManualPowerManagement: false,
});

const currentStep = ref(1);
const requesting = ref<RequestablePermission | null>(null);

const requiredGranted = computed(
  () =>
    status.value.notification &&
    status.value.ring &&
    status.value.storage &&
    status.value.battery,
);
const ringBlockingMissing = computed(
  () => status.value.notification && !status.value.ring,
);

let checkSequence = 0;
let requestRecheckTimers: ReturnType<typeof setTimeout>[] = [];
let requestingResetTimer: ReturnType<typeof setTimeout> | null = null;

// Step 2 状态
const autoStartStatus = ref<AutoStartStatus>("unsupported");
const hasClickedAutoStart = ref(false);
const userConfirmedAutoStart = ref(false);
const hasClickedPower = ref(false);
const userConfirmedPower = ref(false);
const isNotificationListenerReady = ref(false);
const hasClickedListener = ref(false);
const userConfirmedListener = ref(false);

// Step 3 状态
const freeDiskSpaceGB = ref(0);
const totalDiskSpaceGB = ref(0);
const isDiskCheckError = ref(false);

// 自启动是否配置好（支持检测的系统自动感应，其余系统依赖用户确认）
const isAutoStartReady = computed(() => {
  if (autoStartStatus.value === "true") return true;
  return userConfirmedAutoStart.value;
});

const isSystemPowerReady = computed(
  () => status.value.battery && !status.value.backgroundRestricted,
);

// 原生 Android 检测无法覆盖部分厂商的额外后台策略，这些设备仍需人工确认。
const isPowerReady = computed(() => {
  if (status.value.requiresManualPowerManagement) {
    return userConfirmedPower.value;
  }
  return isSystemPowerReady.value || userConfirmedPower.value;
});

// 通知监听是否配置好
const isListenerReady = computed(() => {
  if (isNotificationListenerReady.value) return true;
  return userConfirmedListener.value;
});

// Step 2 是否满足要求
const step2Ready = computed(
  () => isAutoStartReady.value && isPowerReady.value && isListenerReady.value,
);

// Step 3 存储检测是否合格 (要求 >= 5.0 GB)
const isStorageSpaceOk = computed(() => freeDiskSpaceGB.value >= 5.0);

const fetchAutoStartStatus = async (): Promise<AutoStartStatus> => {
  try {
    const result = await invoke<unknown>(
      "plugin:vcp-mobile|check_auto_start_permission",
    );
    return result === "true" || result === "false" ? result : "unsupported";
  } catch (e) {
    console.error("[PermissionGate] Failed to check auto start permission:", e);
    return "unsupported";
  }
};

const fetchDiskSpace = async (): Promise<DiskSpaceSnapshot | null> => {
  try {
    const result = await invoke<{
      freeBytes: number;
      freeGb: number;
      totalBytes: number;
      totalGb: number;
    }>("plugin:vcp-mobile|get_free_disk_space");
    return { freeGb: result.freeGb, totalGb: result.totalGb };
  } catch (e) {
    console.error("[PermissionGate] Failed to check disk space:", e);
    return null;
  }
};

const fetchNotificationListenerStatus = async (): Promise<boolean> => {
  try {
    const result = await invoke<{ enabled: boolean }>(
      "plugin:vcp-mobile|check_notification_listener_permission",
    );
    return result.enabled === true;
  } catch (e) {
    console.error(
      "[PermissionGate] Failed to check notification listener permission:",
      e,
    );
    return false;
  }
};

const check = async (): Promise<boolean> => {
  const sequence = ++checkSequence;
  try {
    const [permissionStatus, diskSpace, nextAutoStartStatus, listenerReady] =
      await Promise.all([
        invoke<PermissionStatus>("plugin:vcp-mobile|check_all_permissions"),
        fetchDiskSpace(),
        fetchAutoStartStatus(),
        fetchNotificationListenerStatus(),
      ]);
    if (sequence !== checkSequence) return false;
    status.value = permissionStatus;
    autoStartStatus.value = nextAutoStartStatus;
    isNotificationListenerReady.value = listenerReady;

    if (diskSpace) {
      freeDiskSpaceGB.value = diskSpace.freeGb;
      totalDiskSpaceGB.value = diskSpace.totalGb;
      isDiskCheckError.value = false;
    } else {
      freeDiskSpaceGB.value = 0;
      totalDiskSpaceGB.value = 0;
      isDiskCheckError.value = true;
    }
    return true;
  } catch (e) {
    if (sequence === checkSequence) {
      console.error("[PermissionGate] Failed to check permissions:", e);
    }
    return false;
  }
};

const clearRequestRecheckTimers = () => {
  requestRecheckTimers.forEach((timer) => clearTimeout(timer));
  requestRecheckTimers = [];
};

const scheduleRequestRechecks = () => {
  clearRequestRecheckTimers();
  requestRecheckTimers = REQUEST_RECHECK_DELAYS_MS.map((delay) =>
    setTimeout(check, delay),
  );
};

const resetRequestingLater = (type: RequestablePermission) => {
  if (requestingResetTimer) clearTimeout(requestingResetTimer);
  requestingResetTimer = setTimeout(() => {
    if (requesting.value === type) {
      requesting.value = null;
    }
  }, 6000);
};

const request = async (type: RequestablePermission) => {
  requesting.value = type;
  scheduleRequestRechecks();
  resetRequestingLater(type);
  try {
    await invoke("plugin:vcp-mobile|request_android_permission", {
      pType: type,
    });
    await check();
  } catch (e) {
    console.error(`[PermissionGate] Failed to request ${type} permission:`, e);
  } finally {
    if (requesting.value === type) {
      requesting.value = null;
    }
    scheduleRequestRechecks();
  }
};

const triggerAutoStartSettings = async () => {
  try {
    await invoke("plugin:vcp-mobile|request_auto_start_permission");
    hasClickedAutoStart.value = true;
  } catch (e) {
    console.error("[PermissionGate] Failed to request auto start settings:", e);
  }
};

const triggerPowerManagementSettings = async () => {
  try {
    await invoke("plugin:vcp-mobile|request_power_management_permission");
    hasClickedPower.value = true;
  } catch (e) {
    console.error(
      "[PermissionGate] Failed to request power management settings:",
      e,
    );
  }
};

const triggerNotificationListenerSettings = async () => {
  try {
    await invoke("plugin:vcp-mobile|request_notification_listener_permission");
    hasClickedListener.value = true;
  } catch (e) {
    console.error(
      "[PermissionGate] Failed to request notification listener settings:",
      e,
    );
  }
};

const exitApp = async () => {
  try {
    await invoke("plugin:vcp-mobile|move_task_to_back");
  } catch (e) {
    console.error("[PermissionGate] Failed to move task to back:", e);
  }
};

const goNext = async () => {
  const sourceStep = currentStep.value;
  const refreshed = await check();
  if (!refreshed || currentStep.value !== sourceStep) return;

  if (!requiredGranted.value) return;
  if (sourceStep === 2 && !step2Ready.value) return;

  if (sourceStep < 3) {
    currentStep.value = sourceStep + 1;
  }
};

watch(requiredGranted, (granted) => {
  if (!granted) currentStep.value = 1;
});

let checkTimer: any = null;

const onPermissionChange = (e: Event) => {
  const detail = (e as CustomEvent<PermissionStatus>).detail;
  if (!detail) return;
  checkSequence++;
  status.value = detail;
};

const onVisibilityChange = () => {
  if (!document.hidden) check();
};

const onLifecycleChange = (e: Event) => {
  const state = (e as CustomEvent<{ state?: string }>).detail?.state;
  if (state === "resume" || state === "config-changed") {
    check();
  }
};

const onPageVisible = () => {
  check();
};

onMounted(() => {
  check();
  // 当应用从后台切回前台时重检（用户在设置页操作后返回）
  window.addEventListener("visibilitychange", onVisibilityChange);
  // Android 原生 Activity 恢复比 WebView visibilitychange 更可靠，设置页返回时优先走这里
  window.addEventListener("vcp-lifecycle", onLifecycleChange);
  window.addEventListener("focus", onPageVisible);
  window.addEventListener("pageshow", onPageVisible);
  // Kotlin 侧主动推送的权限变更事件
  window.addEventListener("vcp-permission-change", onPermissionChange);
  // 低频兜底轮询，防止极端情况下事件丢失
  checkTimer = setInterval(check, 10000);
});

onUnmounted(() => {
  if (checkTimer) clearInterval(checkTimer);
  if (requestingResetTimer) clearTimeout(requestingResetTimer);
  clearRequestRecheckTimers();
  window.removeEventListener("visibilitychange", onVisibilityChange);
  window.removeEventListener("vcp-lifecycle", onLifecycleChange);
  window.removeEventListener("focus", onPageVisible);
  window.removeEventListener("pageshow", onPageVisible);
  window.removeEventListener("vcp-permission-change", onPermissionChange);
});
</script>

<template>
  <div
    class="fixed inset-0 z-gate bg-white flex flex-col items-center select-none overflow-hidden no-rubber-band"
  >
    <!-- Top Section: 米白（所有步骤共享） -->
    <div
      class="w-full bg-[#FAF6EE] flex flex-col items-center px-5 pb-3 shrink-0"
      style="
        padding-top: calc(
          3rem + var(--vcp-safe-top, env(safe-area-inset-top, 0px))
        );
      "
    >
      <!-- Top Illustration Area -->
      <div class="relative w-full flex flex-col items-center mb-1">
        <!-- Background Decorative Blobs -->
        <div
          class="absolute -top-12 -right-6 w-36 h-36 bg-blue-500/8 rounded-full blur-3xl"
        ></div>
        <div
          class="absolute top-8 -left-12 w-44 h-44 bg-cyan-400/8 rounded-full blur-3xl"
        ></div>

        <div
          class="w-32 h-32 rounded-3xl flex items-center justify-center mb-1 relative z-10"
        >
          <img src="/vcpmobile.svg" class="w-24 h-24" />
        </div>
        <h1 class="text-xl font-semibold text-gray-900 tracking-[0.05em] mb-1">
          VCP Mobile Android
        </h1>
        <p class="text-sm text-[#8B7D6B] text-center leading-relaxed px-4">
          将 VCPMobile
          部署到你的手机，通过这台手机和智能体对话，建议使用闲置手机
        </p>
      </div>
    </div>

    <!-- Bottom Section -->
    <div class="w-full flex-1 flex flex-col min-h-0">
      <!-- Progress Indicator -->
      <div class="flex items-center w-full px-5 pt-4 mb-2 shrink-0">
        <!-- Step 1 -->
        <div
          class="w-6 h-6 rounded-full border-2 flex items-center justify-center shrink-0 transition-colors duration-300"
          :class="
            currentStep === 1
              ? 'border-gray-900'
              : currentStep > 1
                ? 'border-gray-900 bg-gray-900'
                : 'border-gray-100'
          "
        >
          <div
            v-if="currentStep === 1"
            class="w-2 h-2 bg-gray-900 rounded-full"
          ></div>
          <span
            v-else-if="currentStep > 1"
            class="text-[10px] font-bold text-white"
            >1</span
          >
        </div>
        <!-- Line 1→2 -->
        <div
          class="h-[1px] flex-1 mx-4 transition-colors duration-300"
          :class="currentStep > 1 ? 'bg-gray-900' : 'bg-gray-100'"
        ></div>
        <!-- Step 2 -->
        <div
          class="w-6 h-6 rounded-full border-2 flex items-center justify-center text-[10px] font-bold shrink-0 transition-colors duration-300"
          :class="
            currentStep === 2
              ? 'border-gray-900 text-gray-900'
              : currentStep > 2
                ? 'border-gray-900 bg-gray-900 text-white'
                : 'border-gray-100 text-gray-300'
          "
        >
          2
        </div>
        <!-- Line 2→3 -->
        <div
          class="h-[1px] flex-1 mx-4 transition-colors duration-300"
          :class="currentStep > 2 ? 'bg-gray-900' : 'bg-gray-100'"
        ></div>
        <!-- Step 3 -->
        <div
          class="w-6 h-6 rounded-full border-2 flex items-center justify-center text-[10px] font-bold shrink-0 transition-colors duration-300"
          :class="
            currentStep === 3
              ? 'border-gray-900 text-gray-900'
              : 'border-gray-100 text-gray-300'
          "
        >
          3
        </div>
      </div>

      <!-- Slide Container -->
      <div class="flex-1 relative overflow-hidden">
        <div
          class="flex h-full transition-transform duration-500 ease-in-out"
          :style="{ transform: `translateX(-${(currentStep - 1) * 100}%)` }"
        >
          <!-- ========== Step 1: Permissions ========== -->
          <div
            class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto"
          >
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">
              授予权限
            </h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              VCP Mobile 需要以下权限才能稳定运行<br />VCP Mobile Core
            </p>
            <div class="w-full space-y-3 mb-4">
              <!-- Permission Cards -->
              <div
                v-for="item in permissionItems"
                :key="item.id"
                class="group flex items-center gap-4 px-4 py-3 rounded-2xl bg-gray-100/50 active:bg-gray-200/60 transition-all"
              >
                <div
                  class="w-10 h-10 rounded-xl bg-blue-50 flex items-center justify-center shrink-0"
                >
                  <div
                    :class="[
                      item.icon,
                      status[item.id] ? 'text-green-500' : 'text-blue-500',
                    ]"
                    class="text-xl transition-colors duration-500"
                  ></div>
                </div>
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-semibold text-gray-900">{{
                      item.name
                    }}</span>
                    <Transition name="fade">
                      <span
                        v-if="status[item.id]"
                        class="text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-black uppercase tracking-wider"
                        >OK</span
                      >
                    </Transition>
                    <span
                      v-if="!item.required && !status[item.id]"
                      class="text-[9px] px-1.5 py-0.5 bg-amber-500/10 text-amber-600 rounded-md font-black uppercase tracking-wider"
                      >推荐</span
                    >
                  </div>
                  <p class="text-xs text-gray-500 opacity-70 leading-relaxed">
                    {{ item.desc }}
                  </p>
                </div>
                <button
                  v-if="!status[item.id]"
                  :disabled="requesting === item.id"
                  @click="request(item.id)"
                  class="px-3 py-1.5 bg-gray-900 text-white text-[13px] font-bold rounded-lg active:scale-95 transition-all shrink-0 disabled:opacity-60 disabled:active:scale-100"
                >
                  {{
                    requesting === item.id
                      ? "检查中"
                      : item.id === "ring" && status.notification
                        ? "去设置"
                        : "去授权"
                  }}
                </button>
              </div>
            </div>

            <!-- Ring blocking missing banner -->
            <div
              v-if="ringBlockingMissing"
              class="w-full rounded-2xl bg-amber-50 border border-amber-100 px-4 py-3 text-xs text-amber-700 leading-relaxed mb-4"
            >
              <p>
                通知铃声未开启，必须开启后才能继续引导。一加/OPPO
                等定制系统需在通知设置中单独允许使用铃声进行通知。
              </p>
              <button
                @click="request('ring')"
                :disabled="requesting === 'ring'"
                class="mt-2 px-3 py-1.5 bg-amber-600 text-white text-xs font-bold rounded-lg active:scale-95 transition-all disabled:opacity-60"
              >
                {{ requesting === "ring" ? "跳转中..." : "前往开启铃声" }}
              </button>
            </div>

            <!-- Bottom Action -->
            <div
              class="mt-auto w-full flex flex-col items-center gap-2 permission-gate-bottom-action"
            >
              <button
                v-if="requiredGranted && currentStep === 1"
                @click="goNext"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>下一步</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
              <button
                v-else-if="!requiredGranted"
                @click="exitApp"
                class="text-xs font-bold text-gray-400 active:text-gray-900 transition-colors py-2 px-4"
              >
                暂不授权，退出应用
              </button>
            </div>
          </div>

          <!-- ========== Step 2: Battery Optimization ========== -->
          <div
            class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto"
          >
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">
              品牌厂商自启动与后台管理
            </h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              由于国产手机系统拥有极其激进的后台强杀机制，必须强制前往设置开启自启动和后台完全允许运行。
            </p>

            <div class="w-full space-y-4 mb-4">
              <!-- 卡片 1：自启动 -->
              <div
                class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200"
              >
                <div class="flex items-center gap-3">
                  <div
                    class="w-8 h-8 rounded-lg bg-indigo-50 flex items-center justify-center shrink-0"
                  >
                    <div
                      class="i-heroicons-arrow-path text-indigo-500 text-lg"
                    ></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm"
                      >开机与后台自启动</span
                    >
                    <span
                      v-if="autoStartStatus === 'true'"
                      class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider"
                      >自动感应 OK</span
                    >
                  </div>
                  <div
                    v-if="isAutoStartReady"
                    class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0"
                  >
                    {{ autoStartStatus === "true" ? "已开启" : "已手动确认" }}
                  </div>
                  <button
                    v-else
                    @click="triggerAutoStartSettings"
                    class="px-3 py-1.5 bg-gray-900 text-white text-[12px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                  >
                    去设置
                  </button>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed pl-11">
                  请在打开的系统页面中开启以下选项：
                  <span class="block mt-1 text-[11px] text-[#8B7D6B] font-bold">
                    关联权限名称：自启动 / 开机自动启动 / 关联启动
                  </span>
                </p>
                <!-- 系统不支持检测或检测未通过时，在跳转后显示手动确认 -->
                <div
                  v-if="autoStartStatus !== 'true' && hasClickedAutoStart"
                  class="mt-2 pl-11 flex items-center gap-2"
                >
                  <input
                    type="checkbox"
                    id="chkAutoStart"
                    v-model="userConfirmedAutoStart"
                    class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer"
                  />
                  <label
                    for="chkAutoStart"
                    class="text-xs text-gray-700 font-bold select-none cursor-pointer"
                    >我已允许自启动</label
                  >
                </div>
              </div>

              <!-- 卡片 2：省电策略 (完全允许后台行为) -->
              <div
                class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200"
              >
                <div class="flex items-center gap-3">
                  <div
                    class="w-8 h-8 rounded-lg bg-amber-50 flex items-center justify-center shrink-0"
                  >
                    <div class="i-heroicons-bolt text-amber-500 text-lg"></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm"
                      >省电无限制策略</span
                    >
                    <span
                      v-if="isSystemPowerReady"
                      class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider"
                      >{{
                        status.requiresManualPowerManagement
                          ? "基础检测 OK"
                          : "自动感应 OK"
                      }}</span
                    >
                  </div>
                  <div
                    v-if="isPowerReady"
                    class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0"
                  >
                    {{
                      isSystemPowerReady &&
                      !status.requiresManualPowerManagement
                        ? "已开启"
                        : "已手动确认"
                    }}
                  </div>
                  <button
                    v-else
                    @click="triggerPowerManagementSettings"
                    class="px-3 py-1.5 bg-gray-900 text-white text-[12px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                  >
                    去设置
                  </button>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed pl-11">
                  请在打开的系统页面中将本应用设为不受限：
                  <span class="block mt-1 text-[11px] text-[#8B7D6B] font-bold">
                    关联权限名称：允许完全后台行为 / 不限制 / 应用耗电管理
                  </span>
                </p>
                <div
                  v-if="
                    (status.requiresManualPowerManagement ||
                      !isSystemPowerReady) &&
                    hasClickedPower
                  "
                  class="mt-2 pl-11 flex items-center gap-2"
                >
                  <input
                    type="checkbox"
                    id="chkPower"
                    v-model="userConfirmedPower"
                    class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer"
                  />
                  <label
                    for="chkPower"
                    class="text-xs text-gray-700 font-bold select-none cursor-pointer"
                    >{{
                      status.backgroundRestricted
                        ? "我已在系统中设为「不限制」，仍要继续"
                        : status.requiresManualPowerManagement
                          ? "我已在厂商设置中设为「不限制」"
                          : "我已将本应用省电策略改为「无限制」"
                    }}</label
                  >
                </div>
                <p
                  v-if="status.backgroundRestricted && hasClickedPower"
                  class="pl-11 text-[11px] leading-relaxed text-amber-700"
                >
                  系统仍报告后台受限。仅在厂商设置页已确认“不限制”但状态未及时刷新时手动继续，否则后台连接可能中断。
                </p>
                <p
                  v-else-if="
                    status.requiresManualPowerManagement && hasClickedPower
                  "
                  class="pl-11 text-[11px] leading-relaxed text-amber-700"
                >
                  当前系统包含额外的厂商后台策略，Android
                  基础检测无法确认其状态，请以厂商设置页为准。
                </p>
              </div>

              <!-- 卡片 3：通知栏监听守护 (极度稳定的后台常驻) -->
              <div
                class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200"
              >
                <div class="flex items-center gap-3">
                  <div
                    class="w-8 h-8 rounded-lg bg-blue-50 flex items-center justify-center shrink-0"
                  >
                    <div
                      class="i-heroicons-shield-check text-blue-500 text-lg"
                    ></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm"
                      >通知栏监听守护</span
                    >
                    <span
                      v-if="isNotificationListenerReady"
                      class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider"
                      >自动感应 OK</span
                    >
                  </div>
                  <div
                    v-if="isListenerReady"
                    class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0"
                  >
                    {{ isNotificationListenerReady ? "已开启" : "已手动确认" }}
                  </div>
                  <button
                    v-else
                    @click="triggerNotificationListenerSettings"
                    class="px-3 py-1.5 bg-gray-900 text-white text-[12px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                  >
                    去设置
                  </button>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed pl-11">
                  启用通知使用权，使应用在系统底层获得极高的后台网络与运行豁免特权：
                  <span class="block mt-1 text-[11px] text-[#8B7D6B] font-bold">
                    请在列表中找到『VCP Mobile』并开启『允许访问通知』
                  </span>
                </p>
                <div
                  v-if="!isNotificationListenerReady && hasClickedListener"
                  class="mt-2 pl-11 flex items-center gap-2"
                >
                  <input
                    type="checkbox"
                    id="chkListener"
                    v-model="userConfirmedListener"
                    class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer"
                  />
                  <label
                    for="chkListener"
                    class="text-xs text-gray-700 font-bold select-none cursor-pointer"
                    >我已开启通知栏监听</label
                  >
                </div>
              </div>
            </div>

            <!-- Bottom Action -->
            <div
              class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]"
            >
              <button
                v-if="step2Ready && currentStep === 2"
                @click="goNext"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>下一步</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
              <button
                v-else-if="currentStep === 2"
                class="w-full py-4 bg-gray-300 text-gray-500 text-[15px] font-bold rounded-2xl cursor-not-allowed flex items-center justify-center gap-2"
                disabled
              >
                <span>请先跳转完成保活设置</span>
              </button>
            </div>
          </div>

          <!-- ========== Step 3: Hardened Keep-Alive & Storage Check ========== -->
          <div
            class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto"
          >
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">
              系统运行环境与存储空间检测
            </h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              完成应用锁定配置并确保充足的系统存储空间，以保障后台服务的高可用性。
            </p>

            <div class="w-full space-y-4 mb-4">
              <!-- 多任务卡片加锁 -->
              <div
                class="p-4 rounded-2xl bg-gray-100/50 border border-gray-200 space-y-2"
              >
                <div class="flex items-center gap-2 text-gray-800">
                  <div
                    class="i-heroicons-lock-closed text-gray-600 text-lg"
                  ></div>
                  <span class="font-bold text-sm"
                    >1. 后台多任务锁定（推荐）</span
                  >
                </div>
                <p class="text-xs text-gray-500 leading-relaxed">
                  在系统『多任务管理器』中，找到 VCP Mobile
                  并点击『加锁』图标。这能避免应用在系统一键清理时被强制关闭。
                </p>
              </div>

              <!-- 存储空间防爆检测 -->
              <div
                class="p-4 rounded-2xl bg-gray-100/50 border border-gray-200 space-y-3"
              >
                <div class="flex items-center gap-2 text-gray-800">
                  <div class="i-heroicons-cpu-chip text-gray-600 text-lg"></div>
                  <span class="font-bold text-sm">2. 系统可用存储空间校验</span>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed">
                  Android 进程管理机制在系统可用存储低于 5.0 GB
                  时会执行更激进的后台资源回收。为确保连接稳定性，本应用需预留至少
                  **5.0 GB** 可用存储。
                </p>

                <!-- Disk Space Status Bar -->
                <div class="space-y-1" v-if="freeDiskSpaceGB > 0">
                  <div class="flex justify-between text-xs font-semibold">
                    <span
                      :class="
                        isStorageSpaceOk ? 'text-green-600' : 'text-red-500'
                      "
                    >
                      {{
                        isStorageSpaceOk
                          ? "✅ 系统可用存储充足"
                          : "⚠️ 可用存储空间不足 5.0 GB"
                      }}
                    </span>
                    <span class="text-gray-600">
                      {{ freeDiskSpaceGB.toFixed(2) }} GB /
                      {{ totalDiskSpaceGB.toFixed(1) }} GB 可用
                    </span>
                  </div>
                  <!-- Progress bar -->
                  <div
                    class="w-full h-2.5 bg-gray-200 rounded-full overflow-hidden"
                  >
                    <div
                      class="h-full transition-all duration-500"
                      :class="isStorageSpaceOk ? 'bg-green-500' : 'bg-red-500'"
                      :style="{
                        width: `${Math.min(
                          100,
                          (freeDiskSpaceGB / totalDiskSpaceGB) * 100,
                        )}%`,
                      }"
                    ></div>
                  </div>
                </div>

                <p
                  v-if="!isStorageSpaceOk && freeDiskSpaceGB > 0"
                  class="text-[11px] text-red-500 font-bold leading-normal"
                >
                  ⚠️ 系统可用存储空间低于 5.0
                  GB，后台进程可能会被系统终止运行。请清理设备存储以解除阻断。
                </p>
              </div>
            </div>

            <!-- Bottom Action -->
            <div
              class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]"
            >
              <button
                v-if="isStorageSpaceOk"
                @click="lifecycleStore.bootstrap(true)"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>激活并进入应用</span>
                <div class="i-heroicons-rocket text-lg"></div>
              </button>
              <button
                v-else
                class="w-full py-4 bg-red-100 text-red-400 text-[15px] font-bold rounded-2xl cursor-not-allowed flex items-center justify-center gap-2"
                disabled
              >
                <span>存储空间不足，已阻断进入</span>
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.permission-gate-bottom-action {
  padding-bottom: calc(
    1.5rem + var(--vcp-safe-bottom, env(safe-area-inset-bottom, 0px))
  );
}
</style>
