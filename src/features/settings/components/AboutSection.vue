<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import UpdateSection from './UpdateSection.vue';
import { useNotificationStore } from '../../../core/stores/notification';

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

const currentVersion = ref('');
const notificationStore = useNotificationStore();

defineEmits(['back']);

onMounted(async () => {
  try {
    currentVersion.value = await getVersion();
  } catch (e) {
    console.error('[AboutSection] Failed to get version:', e);
  }
});

// 3D Parallax & Animation logic
const hitboxRef = ref<HTMLElement | null>(null);
const rotation = ref({ x: 0, y: 0 });
const glowPos = ref({ x: 50, y: 50 });
const isMixing = ref(false);
const transitionStyle = ref('transform 0.2s ease-out');

let animSessionId = 0;

const startAnimationSequence = async () => {
  animSessionId++;
  const sessionId = animSessionId;
  const checkAbort = () => sessionId !== animSessionId;

  const animateTo = async (x: number, y: number, durationMs: number, easing = 'ease-in-out') => {
    if (checkAbort()) return;
    transitionStyle.value = `transform ${durationMs}ms ${easing}`;
    rotation.value = { x, y };
    await sleep(durationMs);
  };

  // Wait a moment after initial interaction before starting sequence
  await sleep(800);
  if (checkAbort()) return;

  // 1. 先向右缓慢旋转翻面
  await animateTo(0, 180, 2000, 'ease-in-out');
  await sleep(1500);
  if (checkAbort()) return;

  // 2. 然后向左缓慢旋转复原
  await animateTo(0, 0, 2000, 'ease-in-out');
  await sleep(1500);
  if (checkAbort()) return;

  // 3. 随后向左旋转70~80度，并轻微偏转 (展现左侧)
  await animateTo(10, -75, 1200, 'ease-out');
  await sleep(1500);
  if (checkAbort()) return;

  // 4. 然后快速向右翻转 (展现右侧)
  await animateTo(-10, 75, 600, 'ease-in-out');
  await sleep(1500);
  if (checkAbort()) return;

  // 5. 快速复原，并轻微扭动 (准备动作)
  await animateTo(0, 0, 400, 'ease-out');
  await animateTo(0, 15, 100, 'linear');
  await animateTo(0, -15, 100, 'linear');
  await animateTo(0, 10, 100, 'linear');
  await animateTo(0, 0, 100, 'ease-out');
  await sleep(1500);
  if (checkAbort()) return;

  // 6. 最后快速上下翻转一遍，临场的抖动
  await animateTo(360, 0, 800, 'ease-in');
  await animateTo(360, 0, 50, 'linear'); // hit ground
  await animateTo(340, 5, 80, 'linear');
  await animateTo(370, -5, 80, 'linear');
  await animateTo(355, 2, 60, 'linear');
  await animateTo(360, 0, 60, 'ease-out');
  
  if (checkAbort()) return;
  // Reset internally without transition to allow normal dragging again cleanly
  transitionStyle.value = 'none';
  rotation.value = { x: 0, y: 0 };
  await sleep(50);
  if (!checkAbort()) {
    transitionStyle.value = 'transform 0.2s ease-out';
  }
};

const handlePress = () => {
  if (!isMixing.value) {
    isMixing.value = true;
    startAnimationSequence();
  }
};

const handleMove = (e: MouseEvent | TouchEvent) => {
  if (!hitboxRef.value) return;
  
  // Intervene: Cancel sequence and track mouse/finger instantly
  animSessionId++;
  isMixing.value = true;
  transitionStyle.value = 'none'; // 0延迟，极致跟手
  
  const rect = hitboxRef.value.getBoundingClientRect();
  let clientX, clientY;
  
  if ('touches' in e) {
    if (e.touches.length === 0) return;
    clientX = e.touches[0].clientX;
    clientY = e.touches[0].clientY;
  } else {
    clientX = (e as MouseEvent).clientX;
    clientY = (e as MouseEvent).clientY;
  }
  
  const x = clientX - rect.left;
  const y = clientY - rect.top;
  
  const centerX = rect.width / 2;
  const centerY = rect.height / 2;
  
  // Calculate target rotation with tuned sensitivity
  let targetRotY = ((x - centerX) / centerX) * 150;
  let targetRotX = ((centerY - y) / centerY) * 80;
  
  // Hard clamp to prevent flipping out or gimbal lock if dragged far off-center
  rotation.value.y = Math.max(-180, Math.min(180, targetRotY));
  rotation.value.x = Math.max(-90, Math.min(90, targetRotX));
  
  // Update glow position (0-100%)
  glowPos.value.x = (x / rect.width) * 100;
  glowPos.value.y = (y / rect.height) * 100;
};

const resetRotation = () => {
  animSessionId++;
  transitionStyle.value = 'transform 0.5s ease-out';
  rotation.value = { x: 0, y: 0 };
};

const showFeatures = () => {
  notificationStore.addNotification({
    type: 'info',
    title: '功能介绍',
    message: 'VCPMobile 正在不断进化中...',
    toastOnly: true
  });
};

const openGitHub = () => {
  openUrl('https://github.com/VCPChat/VCPMobile');
};

const openFeedback = () => {
  openUrl('https://github.com/VCPChat/VCPMobile/issues');
};
</script>

<template>
  <div class="flex flex-col flex-1 h-full relative bg-transparent overflow-hidden">
    <!-- Immersive Back Button -->
    <button 
      @click="$emit('back')"
      class="absolute top-4 left-4 z-20 p-2.5 bg-white/5 border border-white/10 rounded-full active:scale-90 transition-all flex items-center justify-center backdrop-blur-md shadow-lg"
    >
      <div class="i-carbon-chevron-left text-xl opacity-90" />
    </button>

    <!-- Header with 3D Logo (Invisible Hitbox Layer) -->
    <div 
      ref="hitboxRef"
      class="relative pt-24 pb-12 flex flex-col items-center justify-center z-10"
      @mousemove="handleMove"
      @mouseleave="resetRotation"
      @touchmove.prevent="handleMove"
      @touchend="resetRotation"
      @mousedown="handlePress"
      @touchstart.passive="handlePress"
    >
      <!-- Atmosphere Background (Restricted to Logo Area) -->
      <div 
        class="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-full h-full pointer-events-none transition-all duration-300 z-0"
        :class="[isMixing ? 'mixing' : '']"
      >
        <div class="blob blob-cyan" />
        <div class="blob blob-blue" />
        <div class="blob blob-pink" />
      </div>

      <!-- 3D Logo Container (Removed preserve-3d to fix clipping) -->
      <div 
        class="relative z-10 w-48 h-48 flex items-center justify-center pointer-events-none"
        :style="{
          transform: `perspective(1000px) rotateX(${rotation.x}deg) rotateY(${rotation.y}deg)`,
          transition: transitionStyle
        }"
      >
        <!-- Soft Circular Aura -->
        <div 
          class="absolute inset-0 rounded-full opacity-60 blur-[60px] transition-opacity duration-500"
          :style="{
            background: `radial-gradient(circle at ${glowPos.x}% ${glowPos.y}%, rgba(0, 229, 255, 0.8), rgba(59, 130, 246, 0.6), transparent)`
          }"
        />
        
        <!-- Inner Glow for depth -->
        <div class="absolute w-32 h-32 bg-white/15 rounded-full blur-3xl" />
        
        <!-- Logo Image -->
        <img 
          src="/vcpmobile.svg" 
          alt="VCPMobile" 
          decoding="async"
          class="w-[116px] h-[116px] drop-shadow-[0_30px_60px_rgba(0,0,0,0.6)] select-none z-20"
        />
      </div>

      <!-- App Info -->
      <div class="mt-4 text-center z-10 pointer-events-none">
        <h1 class="text-[26px] font-black tracking-tighter text-transparent bg-clip-text bg-gradient-to-r from-[#00e5ff] via-[#3b82f6] to-[#ff3366] cursor-default drop-shadow-sm pb-1">VCPMobile</h1>
      </div>
    </div>

    <!-- Actions List -->
    <div class="px-4 space-y-4 relative z-10">
      <SettingsCard class="!py-1.5 !bg-white/10 !backdrop-blur-3xl !border-white/10 shadow-2xl">
        <UpdateSection />
      </SettingsCard>

      <SettingsCard class="!bg-white/10 !backdrop-blur-3xl !border-white/10 shadow-2xl">
        <div class="divide-y divide-white/10">
          <SettingsRow 
            title="功能介绍" 
            description="了解 VCPMobile 的核心特性"
            class="py-3"
            @click="showFeatures"
          >
            <template #action>
              <div class="i-carbon-chevron-right opacity-30" />
            </template>
          </SettingsRow>
          
          <SettingsRow 
            title="项目主页" 
            description="GitHub 开源仓库"
            class="py-3"
            @click="openGitHub"
          >
            <template #action>
              <div class="i-carbon-logo-github opacity-30" />
            </template>
          </SettingsRow>
          
          <SettingsRow 
            title="意见反馈" 
            description="报告 Bug 或提交建议"
            class="py-3"
            @click="openFeedback"
          >
            <template #action>
              <div class="i-carbon-debug opacity-30" />
            </template>
          </SettingsRow>
        </div>
      </SettingsCard>
    </div>

    <!-- Footer -->
    <div class="mt-auto pt-8 pb-4 text-center space-y-1 opacity-30 relative z-10 pointer-events-none">
      <p class="text-[9px] font-mono tracking-[0.2em] uppercase text-white">Powered by Tauri & Vue 3</p>
      <p class="text-[9px] text-white/80">2024-2026 © VCPMobile PROJECT AVATAR</p>
    </div>
  </div>
</template>

<style scoped>
/* Aerosol blobs base - Restricted to Logo Area */
.blob {
  position: absolute;
  border-radius: 50%;
  mix-blend-mode: screen;
  transition: opacity 0.3s ease-in-out, filter 0.3s ease-in-out;
  opacity: 0.35;
}

/* Static positions - Logo area */
.blob-cyan {
  background: #00e5ff;
  width: 250px; height: 250px;
  top: 50%; left: 50%;
  margin: -125px 0 0 -125px; /* Center perfectly */
  transform: translate(-30px, -30px);
  filter: blur(50px);
}
.blob-blue {
  background: #3b82f6;
  width: 200px; height: 200px;
  top: 50%; left: 50%;
  margin: -100px 0 0 -100px;
  transform: translate(20px, -40px);
  filter: blur(40px);
}
.blob-pink {
  background: #ff3366;
  width: 275px; height: 275px; /* RE-ADJUSTED PINK BLOB SIZE */
  top: 50%; left: 50%;
  margin: -137.5px 0 0 -137.5px;
  transform: translate(0px, 30px);
  filter: blur(80px);
}

/* Animated mixing states - Deep crossing movement */
.mixing .blob {
  opacity: 0.7; /* Increased opacity for stronger fusion */
}
.mixing .blob-cyan {
  animation: float-cyan 8s infinite alternate ease-in-out;
}
.mixing .blob-blue {
  animation: float-blue 10s infinite alternate ease-in-out;
}
.mixing .blob-pink {
  animation: float-pink 12s infinite alternate ease-in-out;
}

@keyframes float-cyan {
  0% { transform: translate(-40px, -40px) scale(1); }
  50% { transform: translate(30px, 10px) scale(1.1); }
  100% { transform: translate(-10px, 40px) scale(1.2); }
}
@keyframes float-blue {
  0% { transform: translate(30px, -40px) scale(1); }
  50% { transform: translate(-30px, 20px) scale(1.2); }
  100% { transform: translate(30px, 40px) scale(1.1); }
}
@keyframes float-pink {
  0% { transform: translate(0px, 30px) scale(1); }
  50% { transform: translate(-40px, -20px) scale(1.1); }
  100% { transform: translate(40px, -10px) scale(1.2); }
}
</style>
