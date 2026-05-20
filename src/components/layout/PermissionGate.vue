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
    <!-- Top Section: 米白 -->
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

    <!-- Bottom Section: 纯白 -->
    <div class="w-full flex-1 px-5 pt-4 pb-6 flex flex-col min-h-0">
      <!-- Progress Indicator (Recreating the dots from screenshot) -->
      <div class="flex items-center w-full mb-4">
        <div class="w-6 h-6 rounded-full border-2 border-gray-900 flex items-center justify-center shrink-0">
          <div class="w-2 h-2 bg-gray-900 rounded-full"></div>
        </div>
        <div class="h-[1px] flex-1 bg-gray-100 mx-4"></div>
        <div class="w-6 h-6 rounded-full border-2 border-gray-100 flex items-center justify-center text-[10px] font-bold text-gray-300 shrink-0">2</div>
        <div class="h-[1px] flex-1 bg-gray-100 mx-4"></div>
        <div class="w-6 h-6 rounded-full border-2 border-gray-100 flex items-center justify-center text-[10px] font-bold text-gray-300 shrink-0">3</div>
      </div>

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
      <div class="mt-auto w-full flex flex-col items-center gap-2">
        <button v-if="allGranted" 
          @click="lifecycleStore.bootstrap(true)" 
          class="w-full py-4 bg-gray-900 text-white text-[15px] font-bold rounded-2xl active:scale-95 transition-all shadow-lg shadow-gray-900/10 flex items-center justify-center gap-2"
        >
          <span>进入应用</span>
          <div class="i-heroicons-arrow-right text-lg"></div>
        </button>
        <button v-else @click="exitApp" class="text-xs font-bold text-gray-400 active:text-gray-900 transition-colors py-2 px-4">
          暂不授权，退出应用
        </button>
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
