<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { getVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
import SettingsCard from '../../../components/settings/SettingsCard.vue';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import UpdateSection from './UpdateSection.vue';
import { useNotificationStore } from '../../../core/stores/notification';
import { useThemeStore } from '../../../core/stores/theme';
import WebGLFluidBackground from '../../../components/ui/WebGLFluidBackground.vue';

const themeStore = useThemeStore();

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
  openUrl('https://github.com/MRiecy/VCPMobile');
};

const openFeedback = () => {
  openUrl('https://github.com/MRiecy/VCPMobile/issues');
};
</script>

<template>
  <div 
    class="flex flex-col flex-1 h-full relative bg-transparent overflow-hidden transition-colors duration-300"
    :class="themeStore.isDarkResolved ? 'theme-dark' : 'theme-light'"
  >
    <!-- WebGL Fluid Background promoted to fullscreen backdrop (Z-0) -->
    <WebGLFluidBackground class="absolute inset-0 w-full h-full pointer-events-none z-0" />

    <!-- Immersive Back Button -->
    <button 
      @click="$emit('back')"
      class="absolute left-4 z-20 p-2 active:scale-90 transition-all flex items-center justify-center opacity-70 active:opacity-100"
      style="top: calc(var(--vcp-safe-top, 24px) + 12px);"
    >
      <svg
        width="22"
        height="22"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        :class="themeStore.isDarkResolved ? 'text-white' : 'text-slate-800'"
      >
        <path d="m15 18-6-6 6-6"/>
      </svg>
    </button>

    <!-- 电影级胶片噪点层 (Film Grain Dithering) - 采用 0KB 纯 SVG 分形噪声物理抹平大模糊带来的色彩渐变断层(等高线条纹) -->
    <div 
      class="noise-overlay absolute inset-0 pointer-events-none z-1 overflow-hidden transition-opacity duration-300"
      :class="themeStore.isDarkResolved ? '' : 'light-mode-noise'"
    />

    <!-- Header with 3D Logo (Invisible Hitbox Layer) -->
    <div 
      ref="hitboxRef"
      class="relative pt-16 pb-12 flex flex-col items-center justify-center z-10"
      @mousemove="handleMove"
      @mouseleave="resetRotation"
      @touchmove.prevent="handleMove"
      @touchend="resetRotation"
      @mousedown="handlePress"
      @touchstart.passive="handlePress"
    >
      <!-- 3D Logo Container (Removed preserve-3d to fix clipping) -->
      <div 
        class="relative z-10 w-48 h-48 flex items-center justify-center pointer-events-none"
        :style="{
          transform: `perspective(1000px) rotateX(${rotation.x}deg) rotateY(${rotation.y}deg)`,
          transition: transitionStyle
        }"
      >
        <!-- Logo Image -->
        <img 
          src="/vcpmobile.svg" 
          alt="VCPMobile" 
          decoding="async"
          class="w-[116px] h-[116px] drop-shadow-[0_30px_60px_rgba(0,0,0,0.6)] select-none z-20"
        />
      </div>

      <!-- App Info -->
      <div class="mt-1 text-center z-10 pointer-events-none">
        <h1 class="text-[26px] font-black tracking-tighter text-transparent bg-clip-text bg-gradient-to-r from-[#00e5ff] via-[#3b82f6] to-[#ff3366] cursor-default drop-shadow-sm pb-1">VCPMobile</h1>
      </div>
    </div>

    <!-- Actions List -->
    <div class="px-4 space-y-4 relative z-10">
      <SettingsCard class="!py-1.5 !backdrop-blur-3xl shadow-2xl transition-all duration-300">
        <UpdateSection />
      </SettingsCard>

      <SettingsCard class="!backdrop-blur-3xl shadow-2xl transition-all duration-300">
        <div class="divide-y transition-colors duration-300" :class="themeStore.isDarkResolved ? 'divide-white/10' : 'divide-black/5'">
          <SettingsRow 
            title="功能介绍" 
            clickable
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
            clickable
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
            clickable
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
    <div class="footer-section mt-auto pt-8 pb-4 text-center space-y-1 opacity-30 relative z-10 pointer-events-none transition-colors duration-300">
      <p class="text-[9px] font-mono tracking-[0.2em] uppercase">Powered by Tauri & Vue 3</p>
      <p class="text-[9px] opacity-80">2024-2026 © VCPMobile PROJECT AVATAR</p>
    </div>
  </div>
</template>

<style scoped>


/* 胶片颗粒噪点层，使用 0KB 纯 SVG 分形噪声物理抹除大渐变色带 */
.noise-overlay {
  opacity: 0.06; /* 从 0.045 提升至 0.06，增强像素打散强度以抵抗物理干涉，颗粒质感更细腻高级 */
  mix-blend-mode: overlay;
  background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noiseFilter'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.85' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noiseFilter)'/%3E%3C/svg%3E");
  transition: opacity 0.5s ease;
}

/* 亮色模式下的胶片颗粒 - 呈 multiply 模式，不透明度略降，提供 pure white 磨砂玻璃颗粒触感 */
.noise-overlay.light-mode-noise {
  mix-blend-mode: multiply;
  opacity: 0.038;
}

/* ==========================================================================
   Theme Adaptations (Dynamic Dark/Light Overrides via Scoped Deep Selectors)
   ========================================================================== */

/* 1. Dark Theme Base Variables */
.theme-dark {
  color: #ffffff;
  background-color: #0f172a !important; /* 强制接管关于页面暗色底盘背景 */
}

.theme-dark :deep(.settings-card) {
  background-color: rgba(255, 255, 255, 0.1) !important;
  border-color: rgba(255, 255, 255, 0.1) !important;
  color: #ffffff !important;
}

.theme-dark :deep(.settings-row .text-primary-text) {
  color: #ffffff !important;
}

.theme-dark .footer-section {
  color: rgba(255, 255, 255, 0.9) !important;
}

/* 2. Light Theme Refined Adaptations (Surgical deep-selector overrides) */
.theme-light {
  color: #1e293b; /* text-slate-800 */
  background-color: #f8fafc !important; /* 强制接管关于页面亮色底盘背景 */
}

/* 亮色半透明毛玻璃卡片 */
.theme-light :deep(.settings-card) {
  background-color: rgba(255, 255, 255, 0.65) !important;
  border-color: rgba(0, 0, 0, 0.05) !important;
  backdrop-filter: blur(24px) !important;
  box-shadow: 0 10px 30px -10px rgba(0, 0, 0, 0.06), 0 1px 3px rgba(0, 0, 0, 0.02) !important;
  color: #1e293b !important;
}

/* 亮色卡片文字 */
.theme-light :deep(.settings-row .text-primary-text) {
  color: #1e293b !important; /* 强制 row 标题为 slate-800 */
}

.theme-light :deep(.settings-row .opacity-40) {
  color: #64748b !important; /* 强制 row 描述文字为 slate-500 */
  opacity: 0.85 !important;
}

/* 亮色卡片内部精细分割线 */
.theme-light :deep(.divide-white\/10) {
  border-color: rgba(0, 0, 0, 0.06) !important;
}

/* 亮色操作右箭头及小图标高保真提亮 */
.theme-light :deep(.i-carbon-chevron-right),
.theme-light :deep(.i-carbon-logo-github),
.theme-light :deep(.i-carbon-debug),
.theme-light :deep(.opacity-20 svg) {
  color: #475569 !important; /* slate-600 */
  opacity: 0.65 !important;
}

/* UpdateSection.vue (更新模块内部亮色深度重置) */
.theme-light :deep(.bg-white\/5) {
  background-color: rgba(0, 0, 0, 0.04) !important; /* 亮色更新信息框底色 */
  color: #1e293b !important;
}

.theme-light :deep(.bg-white\/10) {
  background-color: rgba(0, 0, 0, 0.06) !important; /* 进度条轨道 */
}

.theme-light :deep(.settings-inline-status) {
  color: #334155 !important;
}

.theme-light :deep(.settings-action-button.secondary) {
  background-color: rgba(0, 0, 0, 0.05) !important;
  color: #1e293b !important;
  border-color: rgba(0, 0, 0, 0.05) !important;
}

/* Footer & 版权声明自适应 */
.theme-light .footer-section {
  color: #475569 !important; /* slate-600 */
}
</style>
