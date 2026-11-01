<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { isAndroid, isIOS } from '../../core/utils/platform';
import { useSystemNotificationPermission } from '../../features/notification/composables/useSystemNotificationPermission';
import {
  androidStartupPermissionIds,
  hasMissingStartupPermission,
  type AndroidStartupPermission,
  type DiskSpaceDto,
  type PermissionStatusDto,
} from '../../core/types/native';

const lifecycleStore = useAppLifecycleStore();
const notificationPermission = useSystemNotificationPermission();

const androidLabels = {
  notification: { name: '系统通知', desc: '允许 VCP Mobile 显示运行状态和任务提醒。', icon: 'i-heroicons-bell' },
  battery: { name: '电池优化豁免', desc: '允许 VCP Mobile 忽略系统电池优化，减少待机对连接的影响。', icon: 'i-heroicons-bolt' },
  backgroundAllowed: { name: '允许后台运行', desc: '在应用详情的电池设置中解除后台运行限制。', icon: 'i-heroicons-arrow-path' },
  notificationListener: { name: '通知使用权', desc: '在通知使用权列表中为「VCP 连接守护助手」开启访问权限。', icon: 'i-heroicons-shield-check' },
} satisfies Record<AndroidStartupPermission, { name: string; desc: string; icon: string }>;

const status = ref<PermissionStatusDto | null>(null);
const permissionItems = computed(() => {
  if (isIOS) return [{ id: 'notification' as const, ...androidLabels.notification, name: '系统通知（可选）' }];
  return androidStartupPermissionIds
    .filter(id => status.value?.[id] != null)
    .map(id => ({ id, ...androidLabels[id] }));
});

const currentStep = ref(1);
const permissionReadFailed = ref(false);
const openingAndroidSettings = ref(false);
const settingsError = ref('');
const freeDiskSpaceGB = ref<number | null>(null);
const totalDiskSpaceGB = ref(0);
const isDiskCheckError = ref(false);

const allGranted = computed(() => status.value !== null && !hasMissingStartupPermission(status.value));
const isGranted = (id: AndroidStartupPermission) =>
  isIOS ? notificationPermission.allowed.value : status.value?.[id] === true;
const continuing = ref(false);
const continueToApp = async () => {
  if (continuing.value) return;
  continuing.value = true;
  try {
    await lifecycleStore.continueFromPermissionGuide();
    // A permission may have been revoked between displaying the button and startup's final read.
    if (isAndroid && lifecycleStore.state === 'PERMISSIONS') {
      currentStep.value = 1;
      await checkPermissions();
    }
  } catch (cause) {
    // The startup owner records ERROR and displays the existing retry screen.
    console.error('[PermissionGate] Failed to continue startup:', cause);
  } finally {
    continuing.value = false;
  }
};

const isStorageSpaceOk = computed(() => freeDiskSpaceGB.value !== null && freeDiskSpaceGB.value >= 5.0);

let latestRead = 0;
const checkPermissions = async () => {
  const read = ++latestRead;
  try {
    const snapshot = await invoke<PermissionStatusDto>('plugin:vcp-mobile|check_all_permissions');
    if (read !== latestRead) return;
    status.value = snapshot;
    permissionReadFailed.value = false;
    if (!allGranted.value) currentStep.value = 1;
    else if (permissionItems.value.length === 0) currentStep.value = 2;
  } catch (e) {
    if (read !== latestRead) return;
    permissionReadFailed.value = true;
    console.error('[PermissionGate] Failed to check permissions:', e);
  }
};

const checkDiskSpace = async () => {
  try {
    const res = await invoke<DiskSpaceDto>('plugin:vcp-mobile|get_free_disk_space');
    freeDiskSpaceGB.value = res.freeGb;
    totalDiskSpaceGB.value = res.totalGb;
    isDiskCheckError.value = false;
  } catch (e) {
    console.error('[PermissionGate] Failed to check disk space:', e);
    freeDiskSpaceGB.value = null;
    isDiskCheckError.value = true;
  }
};

const request = async (type: AndroidStartupPermission) => {
  if (isIOS) {
    await notificationPermission.act();
    return;
  }
  if (openingAndroidSettings.value) return;
  openingAndroidSettings.value = true;
  settingsError.value = '';
  try {
    if (type === 'notificationListener') {
      await invoke('plugin:vcp-mobile|request_notification_listener_permission');
    } else if (type === 'backgroundAllowed') {
      const opened = await invoke<boolean>('plugin:vcp-mobile|request_power_management_permission');
      if (!opened) throw new Error('Settings activity unavailable');
    } else {
      await invoke('plugin:vcp-mobile|request_android_permission', { pType: type === 'notification' ? 'notificationSettings' : type });
    }
  } catch (e) {
    settingsError.value = `未能打开「${androidLabels[type].name}」设置，请在系统设置中调整后返回。`;
    console.error(`[PermissionGate] Failed to open ${type} settings:`, e);
  } finally {
    openingAndroidSettings.value = false;
  }
};

const exitApp = async () => {
  try {
    await invoke('plugin:vcp-mobile|move_task_to_back');
  } catch (e) {
    console.error('[PermissionGate] Failed to move task to back:', e);
  }
};

let checkTimer: ReturnType<typeof setInterval> | null = null;

const refreshWhenVisible = () => {
  if (document.hidden) return;
  void checkPermissions();
  if (currentStep.value === 2) void checkDiskSpace();
};

watch(currentStep, step => { if (step === 2) void checkDiskSpace(); });
watch(() => lifecycleStore.lifecycleSnapshot?.phase, phase => {
  if (isAndroid && phase === 'active') refreshWhenVisible();
});

onMounted(() => {
  if (!isAndroid && !isIOS) {
    void lifecycleStore.bootstrap(true).catch(error => {
      console.error('[PermissionGate] Failed to continue startup:', error);
    });
    return;
  }
  if (isIOS) return; // Its actual notification observer owns refresh and cleanup.
  void checkPermissions();
  document.addEventListener('visibilitychange', refreshWhenVisible);
  window.addEventListener('vcp-permission-change', refreshWhenVisible);
  checkTimer = setInterval(refreshWhenVisible, 10000);
});

onUnmounted(() => {
  if (checkTimer) clearInterval(checkTimer);
  latestRead++;
  document.removeEventListener('visibilitychange', refreshWhenVisible);
  window.removeEventListener('vcp-permission-change', refreshWhenVisible);
});
</script>

<template>
  <div v-if="isAndroid || isIOS" class="vcp-viewport-surface vcp-permission-gate vcp-safe-inline fixed inset-0 z-gate bg-white flex flex-col items-center select-none overflow-hidden no-rubber-band">
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
        <h1 class="text-xl font-semibold text-gray-900 tracking-[0.05em] mb-1">VCP Mobile</h1>
        <p class="vcp-permission-description text-sm text-[#8B7D6B] text-center leading-relaxed px-4">
          将 VCPMobile 部署到你的手机，通过这台手机和智能体对话，建议使用闲置手机
        </p>
      </div>
    </div>

    <!-- Bottom Section -->
    <div class="w-full flex-1 flex flex-col min-h-0">
      <!-- Progress Indicator -->
      <div v-if="isAndroid" class="flex items-center w-full px-5 pt-4 mb-2 shrink-0">
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
          :class="currentStep === 2 ? 'border-gray-900 text-gray-900' : 'border-gray-100 text-gray-300'">
          2
        </div>
      </div>

      <!-- Slide Container -->
      <div class="flex-1 relative overflow-hidden">
        <div class="flex h-full transition-transform duration-500 ease-in-out"
             :style="{ transform: `translateX(-${(currentStep - 1) * 100}%)` }">

          <!-- ========== Step 1: Permissions ========== -->
          <div :inert="currentStep !== 1" class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">授予权限</h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              <template v-if="isIOS">选择是否接收系统通知。此设置可稍后在通知中心修改。</template>
              <template v-else-if="permissionReadFailed">暂时无法读取权限。<button class="underline" @click="checkPermissions">重新检测</button></template>
              <template v-else-if="!status">正在读取系统权限…</template>
              <template v-else>开启以下权限后返回，应用会自动检查授权结果。</template>
            </p>
            <div class="w-full space-y-3 mb-4">
              <!-- Permission Cards -->
              <div v-for="item in permissionItems" :key="item.id"
                class="group flex items-center gap-4 px-4 py-3 rounded-2xl bg-gray-100/50 active:bg-gray-200/60 transition-all"
              >
                <div class="w-10 h-10 rounded-xl bg-blue-50 flex items-center justify-center shrink-0">
                  <div :class="[item.icon, isGranted(item.id) ? 'text-green-500' : 'text-blue-500']" class="text-xl transition-colors duration-500"></div>
                </div>
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="font-semibold text-gray-900">{{ item.name }}</span>
                    <Transition name="fade">
                      <span v-if="isGranted(item.id)" class="text-[9px] px-1.5 py-0.5 bg-green-500/10 text-green-600 rounded-md font-black uppercase tracking-wider">OK</span>
                    </Transition>
                  </div>
                  <p class="text-xs text-gray-500 opacity-70 leading-relaxed">{{ isIOS ? notificationPermission.description.value : item.desc }}</p>
                </div>
                <button v-if="isIOS || !isGranted(item.id)"
                  @click="request(item.id)"
                  :disabled="isIOS ? (notificationPermission.busy.value || notificationPermission.openingSettings.value) : openingAndroidSettings"
                  class="px-3 py-1.5 bg-gray-900 text-white text-[13px] font-bold rounded-lg active:scale-95 transition-all shrink-0"
                >
                  {{ isIOS ? notificationPermission.actionLabel.value : item.id === 'battery' ? '去授权' : '去设置' }}
                </button>
              </div>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]">
              <p v-if="settingsError" role="alert" class="text-xs text-red-600">{{ settingsError }}</p>
              <button v-if="isIOS" @click="continueToApp" :disabled="continuing"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl disabled:opacity-50">
                {{ continuing ? '正在进入…' : notificationPermission.allowed.value ? '继续进入应用' : '暂不启用，继续进入应用' }}
              </button>
              <button v-else-if="allGranted && !permissionReadFailed && currentStep === 1"
                @click="currentStep = 2"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>下一步</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
              <button v-else-if="status && !allGranted" @click="exitApp" class="text-xs font-bold text-gray-400 active:text-gray-900 transition-colors py-2 px-4">
                暂不授权，退出应用
              </button>
            </div>
          </div>

          <!-- Step 2: Storage -->
          <div v-if="isAndroid" :inert="currentStep !== 2" class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
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
                <div class="space-y-1" v-if="freeDiskSpaceGB !== null">
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

                <p v-if="!isStorageSpaceOk && freeDiskSpaceGB !== null" class="text-[11px] text-red-500 font-bold leading-normal">
                  ⚠️ 系统可用存储空间低于 5.0 GB，后台进程可能会被系统终止运行。请清理设备存储以解除阻断。
                </p>
              </div>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-[calc(var(--vcp-safe-bottom,48px)+24px)]">
              <button v-if="currentStep === 2 && isStorageSpaceOk" :disabled="continuing"
                @click="continueToApp"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>{{ continuing ? '正在进入…' : '激活并进入应用' }}</span>
                <div class="i-heroicons-rocket-launch text-lg"></div>
              </button>
              <button v-else-if="isDiskCheckError" @click="checkDiskSpace" class="w-full py-4 bg-gray-100 text-gray-900 text-[15px] font-bold rounded-2xl">
                未能读取存储空间，重新检测
              </button>
              <p v-else-if="freeDiskSpaceGB === null" class="py-4 text-sm text-gray-500">正在检测可用存储空间…</p>
              <button v-else-if="!isStorageSpaceOk" class="w-full py-4 bg-red-100 text-red-400 text-[15px] font-bold rounded-2xl cursor-not-allowed flex items-center justify-center gap-2" disabled>
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
