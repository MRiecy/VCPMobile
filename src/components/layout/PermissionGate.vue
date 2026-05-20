<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';

const lifecycleStore = useAppLifecycleStore();

interface PermissionStatus {
  notification: boolean;
  storage: boolean;
  battery: boolean;
}

const status = ref<PermissionStatus>({
  notification: false,
  storage: false,
  battery: false
});

const currentStep = ref(1);

const allGranted = computed(() => status.value.notification && status.value.storage && status.value.battery);

const check = async () => {
  try {
    const res = await invoke<PermissionStatus>('plugin:vcp-mobile|check_all_permissions');
    status.value = res;
  } catch (e) {
    console.error('[PermissionGate] Failed to check permissions:', e);
  }
};

const request = async (type: 'notification' | 'storage' | 'battery') => {
  try {
    await invoke('plugin:vcp-mobile|request_android_permission', { pType: type });
  } catch (e) {
    console.error(`[PermissionGate] Failed to request ${type} permission:`, e);
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

onMounted(() => {
  check();
  // 当应用从后台切回前台时重检（用户在设置页操作后返回）
  window.addEventListener('visibilitychange', () => {
    if (!document.hidden) check();
  });
  // Kotlin 侧主动推送的权限变更事件
  window.addEventListener('vcp-permission-change', onPermissionChange);
  // 低频兜底轮询，防止极端情况下事件丢失
  checkTimer = setInterval(check, 10000);
});

onUnmounted(() => {
  if (checkTimer) clearInterval(checkTimer);
  window.removeEventListener('vcp-permission-change', onPermissionChange);
});
</script>

<template>
  <div class="fixed inset-0 z-gate bg-white flex flex-col items-center select-none overflow-hidden no-rubber-band">
    <!-- Top Section: 米白（所有步骤共享） -->
    <div class="w-full bg-[#FAF6EE] flex flex-col items-center pt-12 px-5 pb-3 shrink-0">
      <!-- Top Illustration Area -->
      <div class="relative w-full flex flex-col items-center mb-1">
        <!-- Background Decorative Blobs -->
        <div class="absolute -top-12 -right-6 w-36 h-36 bg-blue-500/8 rounded-full blur-3xl"></div>
        <div class="absolute top-8 -left-12 w-44 h-44 bg-cyan-400/8 rounded-full blur-3xl"></div>

        <div class="w-32 h-32 rounded-3xl flex items-center justify-center mb-1 relative z-10">
          <img src="/vcpmobile.svg" class="w-24 h-24" />
        </div>
        <h1 class="text-xl font-semibold text-gray-900 tracking-[0.05em] mb-1">VCP Mobile Android</h1>
        <p class="text-sm text-[#8B7D6B] text-center leading-relaxed px-4">
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
                { id: 'storage', name: '储存空间权限', desc: '用于保存头像、聊天图片及导出日志', icon: 'i-heroicons-folder-open' },
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
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-6">
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
            <h3 class="text-lg font-semibold text-gray-900 mb-2 px-1 w-full">品牌电池白名单设置</h3>
            <p class="text-sm text-[#8B7D6B] leading-relaxed mb-4 px-1">
              系统后台权限已授予，但这拦不住品牌厂商自己的电池管理。<br>小米、华为、OPPO 等品牌还有独立的电池管理策略
  ，需要额外将本应用加入白名单。如果不进行以下设置，锁屏后 Agent 仍可能掉线。<br><br>建议将 VCP Mobile 加入电池白名单。
            </p>

            <div class="bg-gray-100/50 border border-gray-200 rounded-xl p-4 space-y-2 mb-4">
              <div class="flex items-center gap-2 text-gray-700">
                <div class="i-heroicons-information-circle w-4 h-4"></div>
                <span class="text-xs font-bold uppercase tracking-wider">建议操作</span>
              </div>
              <p class="text-xs text-gray-500 leading-relaxed">
                进入应用后，右滑打开「全局设置 → 电池优化」查看各品牌的详细设置步骤，按步骤添加白名单即可。
              </p>
            </div>

            <!-- Bottom Action -->
            <div class="mt-auto w-full flex flex-col items-center gap-2 pb-6">
              <button
                @click="lifecycleStore.bootstrap(true)"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>进入应用</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
              </button>
            </div>
          </div>

          <!-- ========== Step 3: Placeholder ========== -->
          <div class="w-full h-full flex-shrink-0 flex flex-col px-5 overflow-y-auto">
            <div class="flex-1 flex flex-col items-center justify-center">
              <div class="w-16 h-16 rounded-2xl bg-gray-100 flex items-center justify-center mb-4">
                <div class="i-heroicons-rocket-launch text-2xl text-gray-400"></div>
              </div>
              <h3 class="text-lg font-semibold text-gray-900 mb-2">更多功能即将上线</h3>
              <p class="text-sm text-[#8B7D6B] text-center leading-relaxed px-4">
                后续版本将提供更多初始化引导功能
              </p>
            </div>

            <!-- Bottom Action -->
            <div class="w-full flex flex-col items-center gap-2 pb-6">
              <button
                @click="lifecycleStore.bootstrap(true)"
                class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
              >
                <span>进入应用</span>
                <div class="i-heroicons-arrow-right text-lg"></div>
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
</style>
