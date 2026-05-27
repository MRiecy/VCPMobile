<script setup lang="ts">
import { onMounted, onUnmounted, computed, ref } from "vue";
import { useRouter } from "vue-router";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useSidebarSwipe } from "./core/composables/useSidebarSwipe";
import { useThemeStore } from "./core/stores/theme";
import { useAppLifecycleStore } from "./core/stores/appLifecycle";
import { useLayoutStore } from "./core/stores/layout";
import { useModalHistory } from "./core/composables/useModalHistory";
import { useNotificationStore } from "./core/stores/notification";
import { useNotificationProcessor } from "./core/composables/useNotificationProcessor";
import { useEmoticonFixer } from "./core/composables/useEmoticonFixer";
import { useAutoUpdate } from "./core/composables/useAutoUpdate";
import { useChatSessionStore } from "./core/stores/chatSessionStore";

// Layout Components
import PermissionGate from "./components/layout/PermissionGate.vue";
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
const sessionStore = useChatSessionStore();
const { processPayload } = useNotificationProcessor();
const { initGlobalFixer } = useEmoticonFixer();
const { isPromptOpen, updateInfo, handleConfirm, handleDismiss } = useAutoUpdate();
const router = useRouter();

const { initRootHistory } = useModalHistory();

// --- Global Swipe Logic for Sidebar ---
const appRootRef = ref<HTMLElement | null>(null);
useSidebarSwipe(appRootRef, { type: "global" });

const bootstrapApp = async () => {
  try {
    await lifecycleStore.bootstrap();
  } catch (error) {
    console.error("[App] Bootstrap failed:", error);
  }
};

const backgroundStyle = computed(() => {
  const themeInfo = themeStore.currentThemeInfo || themeStore.availableThemes.find(
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
  // 2. Strip ANY existing extension and force .webp (matching optimized public/wallpaper)
  filename = filename.split(".")[0] + ".webp";

  return { backgroundImage: `url("/wallpaper/${filename}")` };
});

// 用于取消监听的清理函数
let unlistenLog: (() => void) | null = null;

// --- Root Exit Handler (Double-Tap to Exit with Toast) ---
let exitTimer: number | null = null;
const isWaitingExit = ref(false);

const handleExitRequest = async () => {
  console.log(
    `[ExitRequest] KeyPressed! State: ${lifecycleStore.state}, Item: ${
      sessionStore.currentSelectedItem ? sessionStore.currentSelectedItem.id : 'NULL'
    }, Topic: ${sessionStore.currentTopicId}, Modals: ${useModalHistory().modalStackLength()}`
  );

  // 1. 优先让 Modal Stack 消费返回事件 (支持 Sidebar、Page、Dialog 等 LIFO 退出)
  const { closeTopModal } = useModalHistory();
  if (closeTopModal()) {
    return;
  }

  // 2. 第二级：若当前在 Agent 聊天中（且已就绪），按返回键退回到初始零数据引导欢迎页
  if (lifecycleStore.state === 'READY' && sessionStore.currentSelectedItem !== null) {
    console.log('[ExitRequest] Resetting active session to welcome boot screen.');
    sessionStore.$patch((state) => {
      state.currentSelectedItem = null;
      state.currentTopicId = null;
    });
    return;
  }

  // 3. 第三级：已在初始引导页，触发高精度双击物理退出到后台
  if (isWaitingExit.value) {
    if (exitTimer) {
      clearTimeout(exitTimer);
      exitTimer = null;
    }
    isWaitingExit.value = false;
    
    try {
      await invoke("plugin:vcp-mobile|move_task_to_back");
    } catch (err) {
      console.warn("[Exit] Failed to move task to back, calling window close fallback:", err);
      getCurrentWebviewWindow().close();
    }
  } else {
    isWaitingExit.value = true;
    notificationStore.addNotification({
      id: "vcp-exit-toast",
      title: "再按一次退出应用",
      message: "",
      type: "info",
      duration: 2000,
      toastOnly: true,
    });

    if (typeof navigator !== "undefined" && navigator.vibrate) {
      navigator.vibrate(50);
    }

    exitTimer = window.setTimeout(() => {
      isWaitingExit.value = false;
      exitTimer = null;
    }, 2000);
  }
};


const handleVisibilityChange = () => {
  if (document.hidden) {
    document.documentElement.classList.add("vcp-paused-animations");
  } else {
    document.documentElement.classList.remove("vcp-paused-animations");
  }
};

const handleVcpLifecycle = async (e: Event) => {
  const detail = (e as CustomEvent).detail;
  const state = detail?.state;
  
  if (state === "stop" || state === "pause") {
    console.log("[Lifecycle] App moved to background, tuning heartbeat to 120s...");
    try {
      await invoke("set_vcp_log_heartbeat", { intervalMs: 120000 });
    } catch (err) {
      console.error("[Lifecycle] Failed to set background heartbeat:", err);
    }
  } else if (state === "resume") {
    console.log("[Lifecycle] App moved to foreground, restoring heartbeat to 15s...");
    try {
      await invoke("set_vcp_log_heartbeat", { intervalMs: 15000 });
      // 唤醒时主动校准系统最新状态快照
      await lifecycleStore.hydrateSystemStatus();
    } catch (err) {
      console.error("[Lifecycle] Failed to restore foreground heartbeat:", err);
    }
  }
};

onMounted(async () => {
  // 1. 同步挂载基础物理按键与系统事件监听 (混合应用黄金铁律：物理拦截最优先挂载，杜绝初始化阻塞失效)
  window.addEventListener("vcp-exit-requested", handleExitRequest);
  window.addEventListener("vcp-hardware-back", handleExitRequest);
  document.addEventListener("visibilitychange", handleVisibilityChange);
  window.addEventListener("vcp-lifecycle", handleVcpLifecycle);

  // 初始化全局表情包修复器
  initGlobalFixer();

  // 1.5. 启动 VCP Log IPC 监听 (必须在 bootstrapApp 前挂载，防止 bootstrap 期间的 ready 事件丢失)
  unlistenLog = await listen("vcp-system-event", (event: any) => {
    const payload = event.payload;
    const processed = processPayload(payload);

    if (processed && !processed.silent) {
      notificationStore.addNotification(processed);
    }
  });

  // 2. 异步执行重度核心资源加载 (启动引导)
  await bootstrapApp();

  // Operation Dummy Root: Wait for router and inject dummy layer
  await router.isReady();
  initRootHistory();

  // 路由后置守护：在任何路由切换（包括重定向、刷新）完成后，自动校准防护盾，100% 确保栈顶处于防护状态
  router.afterEach(() => {
    initRootHistory();
  });
});

onUnmounted(() => {
  if (unlistenLog) unlistenLog();
  window.removeEventListener("vcp-exit-requested", handleExitRequest);
  window.removeEventListener("vcp-hardware-back", handleExitRequest);
  document.removeEventListener("visibilitychange", handleVisibilityChange);
  window.removeEventListener("vcp-lifecycle", handleVcpLifecycle);
});
</script>

<template>
  <div ref="appRootRef" class="vcp-app-root h-full w-full overflow-hidden flex flex-col select-none relative">
    <!-- 0. 权限门禁 (仅在 PERMISSIONS 状态显示) -->
    <PermissionGate v-if="lifecycleStore.state === 'PERMISSIONS'" />

    <!-- 0.5. 全局初始化加载层 & 错误看板 -->
    <BootScreen v-else />

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
        class="vcp-overlay fixed inset-0 z-drawer bg-black/12 md:hidden" @click.self="
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
