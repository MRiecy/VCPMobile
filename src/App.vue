<script setup lang="ts">
import { onMounted, onUnmounted, computed, ref } from "vue";
import { useRouter } from "vue-router";
import { listen } from "@tauri-apps/api/event";
import { useSwipe } from "@vueuse/core";
import { useThemeStore } from "./core/stores/theme";
import { useAppLifecycleStore } from "./core/stores/appLifecycle";
import { useLayoutStore } from "./core/stores/layout";
import { useModalHistory } from "./core/composables/useModalHistory";
import { useNotificationStore } from "./core/stores/notification";
import { useNotificationProcessor } from "./core/composables/useNotificationProcessor";
import { useEmoticonFixer } from "./core/composables/useEmoticonFixer";
import { useAutoUpdate } from "./core/composables/useAutoUpdate";

// Layout Components
import BootScreen from "./components/layout/BootScreen.vue";
import AgentSidebar from "./components/layout/AgentSidebar.vue";
import RightSidebar from "./components/layout/RightSidebar.vue";
import GlobalOverlayManager from "./components/GlobalOverlayManager.vue";
import FeatureOverlays from "./components/FeatureOverlays.vue";
import UpdatePrompt from "./components/ui/UpdatePrompt.vue";

const themeStore = useThemeStore();
const lifecycleStore = useAppLifecycleStore();
const notificationStore = useNotificationStore();
const layoutStore = useLayoutStore();
const { processPayload } = useNotificationProcessor();
const { initGlobalFixer } = useEmoticonFixer();
const { isPromptOpen, updateInfo, handleConfirm, handleDismiss } = useAutoUpdate();
const router = useRouter();

const { initRootHistory } = useModalHistory();

// --- Global Swipe Logic for Sidebar ---
const appRootRef = ref<HTMLElement | null>(null);
const { direction, lengthX, lengthY } = useSwipe(appRootRef, {
  threshold: 30, // 稍微提高阈值，防止微小误触
  onSwipeEnd: (e: TouchEvent | MouseEvent) => {
    // 只有在抽屉关闭时才从左往右滑开启
    if (!layoutStore.leftDrawerOpen && !layoutStore.rightDrawerOpen) {
      // 检查是否从受限区域发起
      if (e.target instanceof Element && e.target.closest(".no-swipe")) return;

      const absX = Math.abs(lengthX.value);
      const absY = Math.abs(lengthY.value);

      // 从左往右划 (开启左侧边栏)
      if (direction.value === "right" && absX > 60 && absY / absX < 0.6) {
        layoutStore.setLeftDrawer(true);
      }
    }
  },
});

const bootstrapApp = async () => {
  try {
    await lifecycleStore.bootstrap();
  } catch (error) {
    console.error("[App] Bootstrap failed:", error);
  }
};

const backgroundStyle = computed(() => {
  const themeInfo = themeStore.availableThemes.find(
    (t) => t.fileName === themeStore.currentTheme,
  );
  if (!themeInfo) return {};

  const isLight = !themeStore.isDarkResolved;
  let rawValue = isLight
    ? themeInfo.variables.light?.["--chat-wallpaper-light"]
    : themeInfo.variables.dark?.["--chat-wallpaper-dark"];

  // Fallback: if current mode has no wallpaper, try the other mode
  if (!rawValue || rawValue === "none") {
    rawValue = isLight
      ? themeInfo.variables.dark?.["--chat-wallpaper-dark"]
      : themeInfo.variables.light?.["--chat-wallpaper-light"];
  }

  if (!rawValue || rawValue === "none") return {};

  // Extract filename and clean it robustly
  const match = rawValue.match(/url\(['"]?(.*?)['"]?\)/);
  let filename = match ? match[1] : rawValue;

  // 1. Strip path
  filename = filename.replace(/^.*[\\\/]/, "").replace(/['"]/g, "");
  // 2. Strip ANY existing extension and force .jpg (matching optimized public/wallpaper)
  filename = filename.split(".")[0] + ".jpg";

  return { backgroundImage: `url("/wallpaper/${filename}")` };
});

// 用于取消监听的清理函数
let unlistenLog: (() => void) | null = null;


const handleVisibilityChange = () => {
  if (document.hidden) {
    document.documentElement.classList.add("vcp-paused-animations");
  } else {
    document.documentElement.classList.remove("vcp-paused-animations");
  }
};

onMounted(async () => {
  // 初始化全局表情包修复器
  initGlobalFixer();

  await bootstrapApp();

  // 启动 VCP Log IPC 监听 (使用 1:1 移植的解析大脑)
  unlistenLog = await listen("vcp-system-event", (event: any) => {
    const payload = event.payload;
    const processed = processPayload(payload);

    if (processed && !processed.silent) {
      notificationStore.addNotification(processed);
    }
  });

  // Operation Dummy Root: Wait for router and inject dummy layer
  await router.isReady();
  initRootHistory();

  // 监听页面可见性，切到后台时暂停所有 CSS 动画以节省 GPU
  document.addEventListener("visibilitychange", handleVisibilityChange);
});

onUnmounted(() => {
  if (unlistenLog) unlistenLog();
  document.removeEventListener("visibilitychange", handleVisibilityChange);
});
</script>

<template>
  <div ref="appRootRef" class="vcp-app-root h-full w-full overflow-hidden flex flex-col select-none relative">
    <!-- 0. 全局初始化加载层 & 错误看板 -->
    <BootScreen />

    <!-- 1. 背景底层 -->
    <Transition name="bg-fade">
      <div :key="backgroundStyle.backgroundImage" class="vcp-background-layer" :style="backgroundStyle"></div>
    </Transition>
    <div class="vcp-background-overlay absolute inset-0 pointer-events-none transition-colors duration-700"
      :class="themeStore.isDarkResolved ? 'bg-black/12' : 'bg-transparent'"></div>

    <!-- 2. 主内容区先渲染，抽屉与遮罩在后声明，靠 DOM 顺位自然覆盖 -->
    <main class="flex-1 min-w-0 relative overflow-hidden">
      <router-view v-slot="{ Component }">
        <component v-if="Component" :is="Component" />
      </router-view>
    </main>

    <!-- 3. 抽屉遮罩层位于主内容之后、抽屉之前，点击空白即可关闭 -->
    <Transition name="fade">
      <div v-if="layoutStore.leftDrawerOpen || layoutStore.rightDrawerOpen"
        class="vcp-overlay fixed inset-0 bg-black/12 backdrop-blur-[1px] md:hidden" @click.self="
          layoutStore.setLeftDrawer(false);
        layoutStore.setRightDrawer(false);
        "></div>
    </Transition>

    <!-- 4. 左右抽屉在遮罩之后声明，不写 z-index 也能稳定压过主内容 -->
    <AgentSidebar />
    <RightSidebar class="pointer-events-auto shrink-0" :is-open="layoutStore.rightDrawerOpen"
      @close="layoutStore.setRightDrawer(false)" />

    <!-- 5. 全局覆盖层管理器 -->
    <GlobalOverlayManager />

    <!-- 6. 业务 Feature 视图挂载点 -->
    <FeatureOverlays />

    <!-- 7. 自动更新提示弹窗 -->
    <UpdatePrompt
      v-model:is-open="isPromptOpen"
      :version="updateInfo?.latestVersion || ''"
      :release-notes="updateInfo?.releaseNotes"
      :apk-size="updateInfo?.apkSize"
      @confirm="handleConfirm"
      @dismiss="handleDismiss"
    />
  </div>
</template>

<style>
/* 全局基础样式保持不变 */
:root {
  --vcp-safe-top: 0px;
  --vcp-safe-bottom: 0px;
}

html,
body,
#app {
  height: 100%;
  margin: 0;
  padding: 0;
  overflow: hidden;
  background-color: #000;
}

.vcp-app-root {
  background-color: transparent;
  color: var(--primary-text);
  height: 100%;
}

.vcp-background-layer {
  position: absolute;
  inset: 0;
  background-size: cover;
  background-position: center;
  background-repeat: no-repeat;
  transition: background-image 0.8s ease-in-out;
}

/* Transitions */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.bg-fade-enter-active,
.bg-fade-leave-active {
  transition: opacity 0.8s ease-in-out;
}

.bg-fade-enter-from,
.bg-fade-leave-to {
  opacity: 0;
}

.pt-safe {
  padding-top: var(--vcp-safe-top, 24px);
}

.mb-safe {
  margin-bottom: var(--vcp-safe-bottom, 20px);
}

/* 移动端适配：安全区域 */
@supports (padding-top: env(safe-area-inset-top)) {
  :root {
    --vcp-safe-top: env(safe-area-inset-top);
    --vcp-safe-bottom: env(safe-area-inset-bottom);
  }
}

/* 全局动画暂停：切到后台时由 JS 添加此 class 到 <html> */
.vcp-paused-animations *,
.vcp-paused-animations *::before,
.vcp-paused-animations *::after {
  animation-play-state: paused !important;
}
</style>
