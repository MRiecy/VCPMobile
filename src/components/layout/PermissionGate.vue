<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';

const lifecycleStore = useAppLifecycleStore();

// 仅保活核心权限进入门禁；存储（全媒体读取）等非核心权限由对应功能按需申请
interface PermissionStatus {
  notification: boolean;
  battery: boolean;
}

const status = ref<PermissionStatus>({
  notification: false,
  battery: false
});

const currentStep = ref(1);

// Step 2 状态
const autoStartStatus = ref<'true' | 'false' | 'unsupported'>('unsupported');
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

const allGranted = computed(() => status.value.notification && status.value.battery);

// 自启动是否配置好 (小米/红米/VIVO/魅族支持自动感应，非支持机型依赖用户勾选)
const isAutoStartReady = computed(() => {
  if (autoStartStatus.value === 'true') return true;
  return userConfirmedAutoStart.value;
});

// 后台省电是否配置好 (优先自动感应电池优化白名单和无后台限制状态)
const isPowerReady = computed(() => {
  return status.value.battery || userConfirmedPower.value;
});

// 通知监听是否配置好
const isListenerReady = computed(() => {
  if (isNotificationListenerReady.value) return true;
  return userConfirmedListener.value;
});

// Step 2 是否满足要求
const step2Ready = computed(() => isAutoStartReady.value && isPowerReady.value && isListenerReady.value);

// Step 3 存储检测是否合格 (要求 >= 5.0 GB)
const isStorageSpaceOk = computed(() => freeDiskSpaceGB.value >= 5.0);

const check = async () => {
  try {
    const res = await invoke<PermissionStatus>('plugin:vcp-mobile|check_all_permissions');
    status.value = res;
    
    // 检测通知栏监听服务
    const listenerRes = await invoke<{ enabled: boolean }>('plugin:vcp-mobile|check_notification_listener_permission');
    isNotificationListenerReady.value = listenerRes.enabled;
    
    // 如果已经授予存储权限，检测内部存储空间和自启动状态
    await checkDiskSpace();
    await checkAutoStart();
  } catch (e) {
    console.error('[PermissionGate] Failed to check permissions:', e);
  }
};

const checkAutoStart = async () => {
  try {
    const res = await invoke<string>('plugin:vcp-mobile|check_auto_start_permission');
    autoStartStatus.value = res as any;
  } catch (e) {
    console.error('[PermissionGate] Failed to check auto start permission:', e);
  }
};

const checkDiskSpace = async () => {
  try {
    const res = await invoke<{ freeBytes: number, freeGb: number, totalBytes: number, totalGb: number }>('plugin:vcp-mobile|get_free_disk_space');
    freeDiskSpaceGB.value = res.freeGb;
    totalDiskSpaceGB.value = res.totalGb;
    isDiskCheckError.value = false;
  } catch (e) {
    console.error('[PermissionGate] Failed to check disk space:', e);
    isDiskCheckError.value = true;
  }
};

const request = async (type: 'notification' | 'battery') => {
  try {
    await invoke('plugin:vcp-mobile|request_android_permission', { pType: type });
  } catch (e) {
    console.error(`[PermissionGate] Failed to request ${type} permission:`, e);
  }
};

const triggerAutoStartSettings = async () => {
  try {
    await invoke('plugin:vcp-mobile|request_auto_start_permission');
    hasClickedAutoStart.value = true;
  } catch (e) {
    console.error('[PermissionGate] Failed to request auto start settings:', e);
  }
};

const triggerPowerManagementSettings = async () => {
  try {
    await invoke('plugin:vcp-mobile|request_power_management_permission');
    hasClickedPower.value = true;
  } catch (e) {
    console.error('[PermissionGate] Failed to request power management settings:', e);
  }
};

const triggerNotificationListenerSettings = async () => {
  try {
    await invoke('plugin:vcp-mobile|request_notification_listener_permission');
    hasClickedListener.value = true;
  } catch (e) {
    console.error('[PermissionGate] Failed to request notification listener settings:', e);
  }
};

const exitApp = async () => {
  try {
    await invoke('plugin:vcp-mobile|move_task_to_back');
  } catch (e) {
    console.error('[PermissionGate] Failed to move task to back:', e);
  }
};

const goNext = () => {
  if (currentStep.value < 3) {
    currentStep.value++;
  }
};

let checkTimer: any = null;

const onPermissionChange = (e: Event) => {
  status.value = (e as CustomEvent).detail;
};

const onVisibilityChange = () => {
  if (!document.hidden) check();
};

onMounted(() => {
  check();
  // 当应用从后台切回前台时重检（用户在设置页操作后返回）
  window.addEventListener('visibilitychange', onVisibilityChange);
  // Kotlin 侧主动推送的权限变更事件
  window.addEventListener('vcp-permission-change', onPermissionChange);
  // 低频兜底轮询，防止极端情况下事件丢失
  checkTimer = setInterval(check, 10000);
});

onUnmounted(() => {
  if (checkTimer) clearInterval(checkTimer);
  window.removeEventListener('visibilitychange', onVisibilityChange);
  window.removeEventListener('vcp-permission-change', onPermissionChange);
});
</script>

<template>
  <div class="vcp-permission-gate vcp-safe-inline fixed inset-0 z-gate bg-white flex flex-col items-center select-none overflow-hidden no-rubber-band">
    <!-- Top Section: 米白（所有步骤共享） -->
    <div class="vcp-permission-hero w-full bg-[#FAF6EE] flex flex-col items-center px-5 pb-3 shrink-0">
      <!-- Top Illustration Area -->
      <div class="relative w-full flex flex-col items-center mb-1">
        <!-- Background Decorative Blobs -->
        <div class="absolute -top-12 -right-6 w-36 h-36 bg-blue-500/8 rounded-full blur-3xl"></div>
        <div class="absolute top-8 -left-12 w-44 h-44 bg-cyan-400/8 rounded-full blur-3xl"></div>

        <div class="vcp-permission-logo w-32 h-32 rounded-3xl flex items-center justify-center mb-1 relative z-10">
          <img src="/vcpmobile.svg" class="w-24 h-24" />
        </div>
        <h1 class="text-xl font-semibold text-gray-900 tracking-[0.05em] mb-1">VCP Mobile Android</h1>
        <p class="vcp-permission-description text-sm text-[#8B7D6B] text-center leading-relaxed px-4">
          将 VCPMobile 部署到你的手机，通过这台手机和智能体对话，建议使用闲置手机
        </p>
      </div>
    </div>

    <!-- Bottom Section -->
    <div class="w-full flex-1 flex flex-col min-h-0">
      <!-- Progress Indicator -->
      <div class="flex items-center w-full px-5 pt-4 mb-2 shrink-0">
        <!-- Step 1 -->
        <div class="w-6 h-6 rounded-full border-2 flex items-center justify-center shrink-0 transition-colors duration-300"
          :class="currentStep === 1 ? 'border-gray-900' : (currentStep > 1 ? 'border-gray-900 bg-gray-900' : 'border-gray-100')">
          <div v-if="currentStep === 1" class="w-2 h-2 bg-gray-900 rounded-full"></div>
          <span v-else-if="currentStep > 1" class="text-[10px] font-bold text-white">1</span>
        </div>
        <!-- Line 1→2 -->
        <div class="h-[1px] flex-1 mx-4 transition-colors duration-300"
          :class="currentStep > 1 ? 'bg-gray-900' : 'bg-gray-100'"></div>
        <!-- Step 2 -->
        <div class="w-6 h-6 rounded-full border-2 flex items-center justify-center text-[10px] font-bold shrink-0 transition-colors duration-300"
          :class="currentStep === 2 ? 'border-gray-900 text-gray-900' : (currentStep > 2 ? 'border-gray-900 bg-gray-900 text-white' : 'border-gray-100 text-gray-300')">
          2
        </div>
        <!-- Line 2→3 -->
        <div class="h-[1px] flex-1 mx-4 transition-colors duration-300"
          :class="currentStep > 2 ? 'bg-gray-900' : 'bg-gray-100'"></div>
        <!-- Step 3 -->
        <div class="w-6 h-6 rounded-full border-2 flex items-center justify-center text-[10px] font-bold shrink-0 transition-colors duration-300"
          :class="currentStep === 3 ? 'border-gray-900 text-gray-900' : 'border-gray-100 text-gray-300'">
          3
        </div>
      </div>

      <!-- Slide Container -->
      <div class="flex-1 relative overflow-hidden">
        <div class="flex h-full transition-transform duration-500 ease-in-out"
             :style="{ transform: `translateX(-${(currentStep - 1) * 100}%)` }">

          <!-- ========== Step 1: Permissions ========== -->
          <div class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">授予权限</h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              VCP Mobile 需要以下权限才能稳定运行<br>VCP Mobile Core
            </p>
            <div class="w-full space-y-3 mb-4">
              <!-- Permission Cards -->
              <div v-for="item in [
                { id: 'notification', name: '系统通知', desc: '显示 Agent 运行状态和即时提醒', icon: 'i-heroicons-bell' },
                { id: 'battery', name: '后台运行权限', desc: '切换到后台时保持连接不被系统中断', icon: 'i-heroicons-arrow-path' }
              ]" :key="item.id"
                class="group flex items-center gap-4 px-4 py-3 rounded-2xl bg-gray-100/50 active:bg-gray-200/60 transition-all"
              >
                <div class="w-10 h-10 rounded-xl bg-blue-50 flex items-center justify-center shrink-0">
                  <div :class="[item.icon, status[item.id as keyof PermissionStatus] ? 'text-green-500' : 'text-blue-500']" class="text-xl transition-colors duration-500"></div>
                </div>
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-semibold text-gray-900">{{ item.name }}</span>
                    <Transition name="fade">
                      <span v-if="status[item.id as keyof PermissionStatus]" class="text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-black uppercase tracking-wider">OK</span>
                    </Transition>
                  </div>
                  <p class="text-xs text-gray-500 opacity-70 leading-relaxed">{{ item.desc }}</p>
                </div>
                <button v-if="!status[item.id as keyof PermissionStatus]"
                  @click="request(item.id as any)"
                  class="px-3 py-1.5 bg-gray-900 text-white text-[13px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                >
                  去授权
                </button>
              </div>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]">
              <button v-if="allGranted && currentStep === 1"
                @click="goNext"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>下一步</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
              <button v-else-if="!allGranted" @click="exitApp" class="text-xs font-bold text-gray-400 active:text-gray-900 transition-colors py-2 px-4">
                暂不授权，退出应用
              </button>
            </div>
          </div>

          <!-- ========== Step 2: Battery Optimization ========== -->
          <div class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">品牌厂商自启动与后台管理</h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              由于国产手机系统拥有极其激进的后台强杀机制，必须强制前往设置开启自启动和后台完全允许运行。
            </p>

            <div class="w-full space-y-4 mb-4">
              <!-- 卡片 1：自启动 -->
              <div class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200">
                <div class="flex items-center gap-3">
                  <div class="w-8 h-8 rounded-lg bg-indigo-50 flex items-center justify-center shrink-0">
                    <div class="i-heroicons-arrow-path text-indigo-500 text-lg"></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm">开机与后台自启动</span>
                    <span v-if="autoStartStatus === 'true'" class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider">自动感应 OK</span>
                  </div>
                  <div v-if="autoStartStatus === 'true'" class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0">
                    已开启
                  </div>
                  <button v-else
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
                <!-- 非支持设备，或者检测不成功，在点击跳转后，显示勾选防呆机制 -->
                <div v-if="autoStartStatus !== 'true' && hasClickedAutoStart" class="mt-2 pl-11 flex items-center gap-2">
                  <input type="checkbox" id="chkAutoStart" v-model="userConfirmedAutoStart" class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer" />
                  <label for="chkAutoStart" class="text-xs text-gray-700 font-bold select-none cursor-pointer">我已允许自启动</label>
                </div>
              </div>

              <!-- 卡片 2：省电策略 (完全允许后台行为) -->
              <div class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200">
                <div class="flex items-center gap-3">
                  <div class="w-8 h-8 rounded-lg bg-amber-50 flex items-center justify-center shrink-0">
                    <div class="i-heroicons-bolt text-amber-500 text-lg"></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm">省电无限制策略</span>
                    <span v-if="status.battery" class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider">自动感应 OK</span>
                  </div>
                  <div v-if="status.battery" class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0">
                    已开启
                  </div>
                  <button v-else
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
                <div v-if="!status.battery && hasClickedPower" class="mt-2 pl-11 flex items-center gap-2">
                  <input type="checkbox" id="chkPower" v-model="userConfirmedPower" class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer" />
                  <label for="chkPower" class="text-xs text-gray-700 font-bold select-none cursor-pointer">我已将本应用省电策略改为「无限制」</label>
                </div>
              </div>

              <!-- 卡片 3：通知栏监听守护 (极度稳定的后台常驻) -->
              <div class="flex flex-col gap-2 p-4 rounded-2xl bg-gray-100/50 border border-gray-200">
                <div class="flex items-center gap-3">
                  <div class="w-8 h-8 rounded-lg bg-blue-50 flex items-center justify-center shrink-0">
                    <div class="i-heroicons-shield-check text-blue-500 text-lg"></div>
                  </div>
                  <div class="flex-1">
                    <span class="font-semibold text-gray-900 text-sm">通知栏监听守护</span>
                    <span v-if="isNotificationListenerReady" class="ml-2 text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-bold uppercase tracking-wider">自动感应 OK</span>
                  </div>
                  <div v-if="isNotificationListenerReady" class="px-3 py-1.5 text-green-600 text-[12px] font-bold shrink-0">
                    已开启
                  </div>
                  <button v-else
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
                <div v-if="!isNotificationListenerReady && hasClickedListener" class="mt-2 pl-11 flex items-center gap-2">
                  <input type="checkbox" id="chkListener" v-model="userConfirmedListener" class="rounded border-gray-300 text-gray-900 focus:ring-gray-900 w-4 h-4 cursor-pointer" />
                  <label for="chkListener" class="text-xs text-gray-700 font-bold select-none cursor-pointer">我已开启通知栏监听</label>
                </div>
              </div>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]">
              <button v-if="step2Ready && currentStep === 2"
                @click="goNext"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>下一步</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
              <button v-else-if="currentStep === 2" class="w-full py-4 bg-gray-300 text-gray-500 text-[15px] font-bold rounded-2xl cursor-not-allowed flex items-center justify-center gap-2" disabled>
                <span>请先跳转完成保活设置</span>
              </button>
            </div>
          </div>

          <!-- ========== Step 3: Hardened Keep-Alive & Storage Check ========== -->
          <div class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">系统运行环境与存储空间检测</h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              完成应用锁定配置并确保充足的系统存储空间，以保障后台服务的高可用性。
            </p>

            <div class="w-full space-y-4 mb-4">
              <!-- 多任务卡片加锁 -->
              <div class="p-4 rounded-2xl bg-gray-100/50 border border-gray-200 space-y-2">
                <div class="flex items-center gap-2 text-gray-800">
                  <div class="i-heroicons-lock-closed text-gray-600 text-lg"></div>
                  <span class="font-bold text-sm">1. 后台多任务锁定（推荐）</span>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed">
                  在系统『多任务管理器』中，找到 VCP Mobile 并点击『加锁』图标。这能避免应用在系统一键清理时被强制关闭。
                </p>
              </div>

              <!-- 存储空间防爆检测 -->
              <div class="p-4 rounded-2xl bg-gray-100/50 border border-gray-200 space-y-3">
                <div class="flex items-center gap-2 text-gray-800">
                  <div class="i-heroicons-cpu-chip text-gray-600 text-lg"></div>
                  <span class="font-bold text-sm">2. 系统可用存储空间校验</span>
                </div>
                <p class="text-xs text-gray-500 leading-relaxed">
                  Android 进程管理机制在系统可用存储低于 5.0 GB 时会执行更激进的后台资源回收。为确保连接稳定性，本应用需预留至少 **5.0 GB** 可用存储。
                </p>

                <!-- Disk Space Status Bar -->
                <div class="space-y-1" v-if="freeDiskSpaceGB > 0">
                  <div class="flex justify-between text-xs font-semibold">
                    <span :class="isStorageSpaceOk ? 'text-green-600' : 'text-red-500'">
                      {{ isStorageSpaceOk ? '✅ 系统可用存储充足' : '⚠️ 可用存储空间不足 5.0 GB' }}
                    </span>
                    <span class="text-gray-600">
                      {{ freeDiskSpaceGB.toFixed(2) }} GB / {{ totalDiskSpaceGB.toFixed(1) }} GB 可用
                    </span>
                  </div>
                  <!-- Progress bar -->
                  <div class="w-full h-2.5 bg-gray-200 rounded-full overflow-hidden">
                    <div class="h-full transition-all duration-500" 
                      :class="isStorageSpaceOk ? 'bg-green-500' : 'bg-red-500'"
                      :style="{ width: `${Math.min(100, (freeDiskSpaceGB / totalDiskSpaceGB) * 100)}%` }"
                    ></div>
                  </div>
                </div>

                <p v-if="!isStorageSpaceOk && freeDiskSpaceGB > 0" class="text-[11px] text-red-500 font-bold leading-normal">
                  ⚠️ 系统可用存储空间低于 5.0 GB，后台进程可能会被系统终止运行。请清理设备存储以解除阻断。
                </p>
              </div>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]">
              <button v-if="isStorageSpaceOk"
                @click="lifecycleStore.bootstrap(true)"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>激活并进入应用</span>
                <div class="i-heroicons-rocket-launch text-lg"></div>
              </button>
              <button v-else class="w-full py-4 bg-red-100 text-red-400 text-[15px] font-bold rounded-2xl cursor-not-allowed flex items-center justify-center gap-2" disabled>
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
/* 顶部安全区内边距落在米色 hero 上（而非白色容器），
   保证刘海/状态栏区域与头图一体米色 */
.vcp-permission-hero {
  padding-top: calc(var(--vcp-safe-top, 0px) + 3rem);
}

@media (max-height: 520px) {
  .vcp-permission-hero {
    padding-top: calc(var(--vcp-safe-top, 0px) + 0.75rem);
    padding-bottom: 0.25rem;
  }

  .vcp-permission-logo {
    width: 4.5rem;
    height: 4.5rem;
  }

  .vcp-permission-logo img {
    width: 3.5rem;
    height: 3.5rem;
  }

  .vcp-permission-description {
    font-size: 0.75rem;
    line-height: 1.25;
  }
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
