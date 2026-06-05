<script setup lang="ts">
import { onMounted, onUnmounted, computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow, WebviewWindow } from "@tauri-apps/api/webviewWindow";
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
import { useAssistantStore } from "./core/stores/assistant";
import { useSettingsStore } from "./core/stores/settings";
import { reapplyScreenKeepIfActive, suspendPhysicalScreenKeep } from "./core/composables/useScreenKeeper";

// Layout Components
import PermissionGate from "./components/layout/PermissionGate.vue";
import BootScreen from "./components/layout/BootScreen.vue";
import AgentSidebar from "./components/layout/AgentSidebar.vue";
import RightSidebar from "./components/layout/RightSidebar.vue";
import GlobalOverlayManager from "./components/GlobalOverlayManager.vue";
import FeatureOverlays from "./components/FeatureOverlays.vue";
import UpdatePrompt from "./components/ui/UpdatePrompt.vue";
import ShareAgentSelector from "./features/chat/components/ShareAgentSelector.vue";


interface SharedFileEntry {
  cachePath: string;
  mimeType: string;
  fileName: string;
  size: number;
}

interface SharedContentData {
  text: string;
  files: SharedFileEntry[];
}

interface PickedFileInfo {
  path: string;
  name: string;
  mime: string;
  size: number;
  hash: string;
  thumbnailPath?: string;
}

const themeStore = useThemeStore();
const lifecycleStore = useAppLifecycleStore();
const notificationStore = useNotificationStore();
const layoutStore = useLayoutStore();
const sessionStore = useChatSessionStore();
const assistantStore = useAssistantStore();
const settingsStore = useSettingsStore();
const { processPayload } = useNotificationProcessor();
const { initGlobalFixer } = useEmoticonFixer();
const { isPromptOpen, updateInfo, handleConfirm, handleDismiss } = useAutoUpdate();
const router = useRouter();

const { initRootHistory } = useModalHistory();

const isAssistant = ref(false);

// --- Share Intent State ---
const sharedContent = ref<SharedContentData>({ text: "", files: [] });
const showShareSelector = ref(false);
const pendingSharedFiles = ref<PickedFileInfo[]>([]);

const handleShareIntent = (e: Event) => {
  processSharedIntent((e as CustomEvent).detail);
};

const processSharedIntent = async (detail: any) => {
  console.log("[App] Share intent received:", detail);

  const text = typeof detail?.text === "string" ? detail.text : "";
  const files: SharedFileEntry[] = Array.isArray(detail?.files) ? detail.files : [];

  sharedContent.value = { text, files };

  // Wait for core to be ready, then process files
  if (lifecycleStore.state !== "READY") {
    console.log("[App] Core not ready yet, deferring share intent processing...");
    const unwatch = watch(
      () => lifecycleStore.state,
      async (state) => {
        if (state === "READY") {
          unwatch();
          await prepareShareFiles();
        }
      },
    );
    return;
  }

  await prepareShareFiles();
};

const prepareShareFiles = async () => {
  const files = sharedContent.value.files;
  if (files.length > 0) {
    try {
      console.log(`[App] Registering ${files.length} shared file(s)...`);
      const results = await invoke<PickedFileInfo[]>("plugin:vcp-mobile|register_shared_files", {
        files: files.map((f) => ({
          cachePath: f.cachePath,
          mimeType: f.mimeType,
          fileName: f.fileName,
        })),
      });
      pendingSharedFiles.value = results;
      console.log("[App] Shared files registered:", results);
    } catch (err) {
      console.error("[App] Failed to register shared files:", err);
      pendingSharedFiles.value = [];
    }
  } else {
    pendingSharedFiles.value = [];
  }

  // Ensure agents are loaded
  if (assistantStore.agents.length === 0) {
    try {
      await assistantStore.fetchAgents();
    } catch (e) {
      console.error("[App] Failed to fetch agents for share selector:", e);
    }
  }

  showShareSelector.value = true;
};

const handleShareAgentSelected = async (agent: any) => {
  showShareSelector.value = false;

  try {
    await sessionStore.startShareSession(
      agent.id,
      sharedContent.value.text,
      pendingSharedFiles.value,
    );
  } catch (err) {
    console.error("[App] Failed to start share session:", err);
  }

  // Clear share state
  sharedContent.value = { text: "", files: [] };
  pendingSharedFiles.value = [];
};

const handleShareSelectorClose = () => {
  showShareSelector.value = false;
};

// --- Global Swipe Logic for Sidebar ---
const appRootRef = ref<HTMLElement | null>(null);
useSidebarSwipe(appRootRef, { type: "global" });

const bootstrapApp = async () => {
  try {
    if (isAssistant.value) {
      // 划词助手窗口：采用静默快速启动，跳过权限检查和核心就绪等待（假设主窗口已完成）
      lifecycleStore.state = "READY";
      // 仅后台静默同步必要的 UI 资源
      await themeStore.initTheme();
      // 异步拉取配置，不阻塞渲染
      settingsStore.fetchSettings().catch(() => {});
      return;
    }
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

let isAppBackground = false;

const handleVcpLifecycle = (e: Event) => {
  if (isAssistant.value) return;

  const detail = (e as CustomEvent).detail;
  const state = detail?.state;
  
  if (state === "stop" || state === "pause") {
    if (isAppBackground) return;
    isAppBackground = true;
    console.log("[Lifecycle] App moved to background, tuning heartbeat to 120s...");
    suspendPhysicalScreenKeep(); // 休眠物理亮屏，达到省电效果
    invoke("set_vcp_log_heartbeat", { intervalMs: 120000 }).catch((err) => {
      console.error("[Lifecycle] Failed to set background heartbeat:", err);
    });
  } else if (state === "resume") {
    if (!isAppBackground) return;
    isAppBackground = false;
    console.log("[Lifecycle] App moved to foreground, restoring heartbeat to 15s...");
    reapplyScreenKeepIfActive(); // 唤醒时自动校准和恢复可能丢失的物理亮屏 FLAG
    invoke("set_vcp_log_heartbeat", { intervalMs: 15000 }).catch((err) => {
      console.error("[Lifecycle] Failed to restore foreground heartbeat:", err);
    });
    lifecycleStore.hydrateSystemStatus().catch((err) => {
      console.error("[Lifecycle] Failed to hydrate system status:", err);
    });
  }
};

const handleFloatingBallClick = async () => {
  console.log("[App] Floating ball clicked. Resolving assistant window...");
  try {
    let win = await WebviewWindow.getByLabel("assistant");
    if (win) {
      console.log("[App] Assistant window already exists, showing and focusing...");
      await win.show();
      await win.setFocus();
      return;
    }

    const newWin = new WebviewWindow("assistant", {
      url: "/#/assistant",
      title: "VCP 划词助手",
      transparent: true,
      decorations: false,
      visible: true,
    });

    newWin.once("tauri://created", () => {
      console.log("[App] Assistant window created successfully!");
    });

    newWin.once("tauri://error", (e) => {
      console.error("[App] Failed to create assistant window:", e);
    });
  } catch (err) {
    console.error("[App] Failed to resolve assistant window:", err);
  }
};

onMounted(async () => {
  // 环境探测：若是原生悬浮窗模式，Tauri API 可能不可用，优先通过 URL 判断
  isAssistant.value = window.location.search.includes("mode=floating");

  if (!isAssistant.value) {
    try {
      const win = getCurrentWebviewWindow();
      isAssistant.value = win.label === "assistant";
    } catch (e) {
      // 忽略非 Tauri 环境下的错误
    }
  }

  // 1. 同步挂载基础物理按键与系统事件监听 (混合应用黄金铁律：物理拦截最优先挂载，杜绝初始化阻塞失效)
  window.addEventListener("vcp-exit-requested", handleExitRequest);
  window.addEventListener("vcp-hardware-back", handleExitRequest);
  document.addEventListener("visibilitychange", handleVisibilityChange);
  window.addEventListener("vcp-lifecycle", handleVcpLifecycle);
  window.addEventListener("vcp-floating-ball-click", handleFloatingBallClick);
  window.addEventListener("vcp-share-intent", handleShareIntent);

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
  window.removeEventListener("vcp-floating-ball-click", handleFloatingBallClick);
  window.removeEventListener("vcp-share-intent", handleShareIntent);
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
    <AgentSidebar v-if="lifecycleStore.state === 'READY'" />
    <RightSidebar v-if="lifecycleStore.state === 'READY'" class="pointer-events-auto shrink-0" :is-open="layoutStore.rightDrawerOpen"
      @close="layoutStore.setRightDrawer(false)" />

    <!-- 5. 全局覆盖层管理器 -->
    <GlobalOverlayManager v-if="lifecycleStore.state === 'READY'" />

    <!-- 6. 业务 Feature 视图挂载点 -->
    <FeatureOverlays v-if="lifecycleStore.state === 'READY'" />

    <!-- 7. 分享意图 Agent 选择器 -->
    <ShareAgentSelector v-if="lifecycleStore.state === 'READY'"
      :is-open="showShareSelector"
      :shared-text="sharedContent.text"
      :shared-file-count="sharedContent.files.length"
      @close="handleShareSelectorClose"
      @selected="handleShareAgentSelected"
    />

    <!-- 8. 自动更新提示弹窗 -->
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
