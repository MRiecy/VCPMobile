<script setup lang="ts">
import { onMounted, onUnmounted, computed, nextTick, ref, watch, type WatchStopHandle } from "vue";
import { useRouter } from "vue-router";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useSidebarSwipe } from "./core/composables/useSidebarSwipe";
import { useThemeStore } from "./core/stores/theme";
import { useAppLifecycleStore } from "./core/stores/appLifecycle";
import { useLayoutStore } from "./core/stores/layout";
import { useOverlayStore } from "./core/stores/overlay";
import { useModalHistory } from "./core/composables/useModalHistory";
import { useNotificationStore } from "./core/stores/notification";
import { useNotificationProcessor } from "./core/composables/useNotificationProcessor";
import { useEmoticonFixer } from "./core/composables/useEmoticonFixer";
import { useAutoUpdate } from "./core/composables/useAutoUpdate";
import { useChatSessionStore } from "./core/stores/chatSessionStore";
import { useAssistantStore } from "./core/stores/assistant";
import { useAppLifecycle } from "./core/composables/useAppLifecycle";
import { retainNativeInsetsBridge } from "./core/composables/useKeyboardInsets";
import { LatestIntentOwner } from "./core/utils/latestIntentOwner";
import {
  useVcpCliStore,
  type VcpCliNotificationTarget,
} from "./features/cli/vcpCliStore";

// 初始化应用生命周期监听
useAppLifecycle();
// 根组件持有唯一的原生 Insets 桥，确保编辑器尚未挂载时四边安全区也已生效。
const releaseNativeInsetsBridge = retainNativeInsetsBridge();

// Layout Components
import PermissionGate from "./components/layout/PermissionGate.vue";
import BootScreen from "./components/layout/BootScreen.vue";
import AgentSidebar from "./components/layout/AgentSidebar.vue";
import RightSidebar from "./components/layout/RightSidebar.vue";
import GlobalOverlayManager from "./components/GlobalOverlayManager.vue";
import FeatureOverlays from "./components/FeatureOverlays.vue";
import GuideOverlay from "./features/guide/components/GuideOverlay.vue";
import UpdatePrompt from "./components/ui/UpdatePrompt.vue";
import ShareAgentSelector from "./features/chat/components/ShareAgentSelector.vue";


interface SharedFileEntry {
  cachePath: string;
  mimeType: string;
  fileName: string;
  size: number;
  stagingTicket: string;
}

interface SharedContentData {
  intentId: string;
  operationId: string;
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
const overlayStore = useOverlayStore();
const sessionStore = useChatSessionStore();
const assistantStore = useAssistantStore();
const { processPayload } = useNotificationProcessor();
const leftSidebarPersistent = window.matchMedia("(min-width: 1024px)");
const rightSidebarPersistent = window.matchMedia("(min-width: 1280px)");

const reconcileDrawerPresentation = () => {
  // 常驻栏不再是 modal：跨过断点时清掉抽屉 intent 和对应 history entry。
  if (leftSidebarPersistent.matches) layoutStore.setLeftDrawer(false);
  if (rightSidebarPersistent.matches) layoutStore.setRightDrawer(false);
};
const { initGlobalFixer } = useEmoticonFixer();
const { isPromptOpen, updateInfo, handleConfirm, handleDismiss, handleSkipVersion } =
  useAutoUpdate();
const router = useRouter();

const { initRootHistory } = useModalHistory();

// --- Share Intent State ---
const sharedContent = ref<SharedContentData>({ intentId: "", operationId: "", text: "", files: [] });
const showShareSelector = ref(false);
const pendingSharedFiles = ref<PickedFileInfo[]>([]);
const shareIntentOwner = new LatestIntentOwner();
let stopShareReadyWatch: WatchStopHandle | null = null;

const handleShareIntent = (e: Event) => {
  processSharedIntent((e as CustomEvent).detail);
};

const processSharedIntent = async (detail: any) => {
  const operationId = shareIntentOwner.begin();
  const intentId = typeof detail?.intentId === "string" ? detail.intentId : "";
  const text = typeof detail?.text === "string" ? detail.text : "";
  const files: SharedFileEntry[] = Array.isArray(detail?.files)
    ? detail.files.filter(
        (file: any) =>
          typeof file?.cachePath === "string" &&
          typeof file?.fileName === "string" &&
          typeof file?.stagingTicket === "string",
      )
    : [];
  console.log("[App] Share intent received", {
    intentId,
    textLength: text.length,
    fileCount: files.length,
  });
  const snapshot: SharedContentData = { intentId, operationId, text, files };

  stopShareReadyWatch?.();
  stopShareReadyWatch = null;
  sharedContent.value = snapshot;
  pendingSharedFiles.value = [];
  showShareSelector.value = false;

  // Wait for core to be ready, then process files
  if (lifecycleStore.state !== "READY") {
    console.log("[App] Core not ready yet, deferring share intent processing...");
    stopShareReadyWatch = watch(
      () => lifecycleStore.state,
      async (state) => {
        if (state === "READY" && shareIntentOwner.isCurrent(operationId)) {
          stopShareReadyWatch?.();
          stopShareReadyWatch = null;
          await prepareShareFiles(snapshot, operationId);
        }
      },
    );
    return;
  }

  await prepareShareFiles(snapshot, operationId);
};

const prepareShareFiles = async (content: SharedContentData, operationId: string) => {
  const files = content.files;
  if (files.length > 0) {
    try {
      console.log(`[App] Registering ${files.length} shared file(s)...`);
      const stagedResults = await invoke<PickedFileInfo[]>("plugin:vcp-mobile|register_shared_files", {
        ownerId: content.intentId,
        files: files.map((f) => ({
          cachePath: f.cachePath,
          mimeType: f.mimeType,
          fileName: f.fileName,
          stagingTicket: f.stagingTicket,
        })),
      });
      if (!shareIntentOwner.isCurrent(operationId)) return;

      const results: PickedFileInfo[] = [];
      for (const staged of stagedResults) {
        if (!shareIntentOwner.isCurrent(operationId)) return;
        const registered = await invoke<any>("register_local_file", {
          localPath: staged.path,
          originalName: staged.name,
          mimeType: staged.mime || "application/octet-stream",
          thumbnailPath: staged.thumbnailPath || null,
          stableId: `share_${operationId}_${results.length}`,
          expectedHash: staged.hash,
        });
        results.push({
          path: registered.internalPath,
          name: registered.name,
          mime: registered.type,
          size: registered.size,
          hash: registered.hash,
          thumbnailPath: registered.thumbnailPath,
        });
      }
      if (!shareIntentOwner.isCurrent(operationId)) return;
      pendingSharedFiles.value = results;
      console.log(`[App] Shared files registered: ${results.length}`);
    } catch {
      if (!shareIntentOwner.isCurrent(operationId)) return;
      console.error("[App] Failed to register shared files");
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

  if (shareIntentOwner.isCurrent(operationId)) {
    showShareSelector.value = true;
  }
};

const handleShareAgentSelected = async (agent: any) => {
  const operationId = sharedContent.value.operationId;
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
  if (shareIntentOwner.isCurrent(operationId)) {
    shareIntentOwner.clear(operationId);
    sharedContent.value = { intentId: "", operationId: "", text: "", files: [] };
    pendingSharedFiles.value = [];
  }
};

const handleShareSelectorClose = () => {
  showShareSelector.value = false;
};

// --- Notification Click Routing State & Logic ---
const handleNotificationClick = (e: Event) => {
  const detail = (e as CustomEvent).detail;
  void processNotificationClick(detail);
};

const processNotificationClick = async (detail: any) => {
  console.log("[App] Notification click received:", detail);
  const isCliJob = detail?.kind === "cli_job";
  if (!isCliJob && (!detail?.ownerId || !detail?.topicId)) return;

  if (lifecycleStore.state !== "READY") {
    console.log("[App] Core not ready yet, deferring notification click routing...");
    const unwatch = watch(
      () => lifecycleStore.state,
      (state) => {
        if (state === "READY") {
          unwatch();
          void processNotificationClick(detail);
        }
      }
    );
    return;
  }

  // 1. 关闭所有弹出的 Modals
  const { closeTopModal, modalStackLength } = useModalHistory();
  while (modalStackLength() > 0) {
    const before = modalStackLength();
    closeTopModal();
    // 不可 dismiss 的长任务页持有导航权，通知点击不得强制卸载。
    if (modalStackLength() >= before) break;
  }
  
  // 2. 关闭侧边栏
  layoutStore.setLeftDrawer(false);
  layoutStore.setRightDrawer(false);

  if (isCliJob) {
    const target: VcpCliNotificationTarget = {
      jobId: String(detail.jobId ?? ""),
      attemptId: String(detail.attemptId ?? ""),
      runtimeGeneration: Number(detail.runtimeGeneration ?? 0),
    };
    if (!target.jobId || !target.attemptId || target.runtimeGeneration <= 0) {
      return;
    }
    overlayStore.openCliManifest();
    await nextTick();
    const cliStore = useVcpCliStore();
    const opened = await cliStore.openJobFromNotification(target);
    if (opened !== "opened") {
      notificationStore.addNotification({
        id: `vcp-cli-notification-stale-${Date.now()}`,
        title: opened === "stale" ? "任务已结束" : "暂时无法打开 CLI Job",
        message: opened === "stale" ? "这条通知已过期，没有影响当前任务。" : "请稍后从 Jobs 页面重试。",
        type: opened === "stale" ? "info" : "error",
        duration: 3000,
        toastOnly: true,
      });
      return;
    }
    if (detail.action !== "confirm_stop") return;
    const confirmed = await overlayStore.showConfirm({
      title: "停止这个 CLI Job？",
      message: `将终止该 Job 的整个进程树，已产生的输出会保留。\nJob ${target.jobId.slice(0, 12)}`,
      isDanger: true,
    });
    if (!confirmed) return;
    const revalidated = await cliStore.openJobFromNotification(target);
    if (revalidated === "opened") {
      await cliStore.cancelSelectedJob();
    } else {
      notificationStore.addNotification({
        id: `vcp-cli-notification-ended-${Date.now()}`,
        title: "任务已结束",
        message: "停止请求未作用于其他 Job。",
        type: "info",
        duration: 3000,
        toastOnly: true,
      });
    }
    return;
  }

  // 3. 切换话题
  sessionStore.selectTopicById(detail.ownerId, detail.topicId);
};

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
    sessionStore.clearConversation();
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

    exitTimer = window.setTimeout(() => {
      isWaitingExit.value = false;
      exitTimer = null;
    }, 2000);
  }
};

onMounted(async () => {
  // 1. 同步挂载基础物理按键与系统事件监听 (混合应用黄金铁律：物理拦截最优先挂载，杜绝初始化阻塞失效)
  window.addEventListener("vcp-exit-requested", handleExitRequest);
  window.addEventListener("vcp-hardware-back", handleExitRequest);
  window.addEventListener("vcp-share-intent", handleShareIntent);
  window.addEventListener("vcp-notification-click", handleNotificationClick);
  leftSidebarPersistent.addEventListener("change", reconcileDrawerPresentation);
  rightSidebarPersistent.addEventListener("change", reconcileDrawerPresentation);
  reconcileDrawerPresentation();


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

  // 3. 处理冷启动的通知栏点击
  try {
    const pending = await invoke<any>("plugin:vcp-mobile|get_pending_notification");
    if (pending && (pending.topicId || pending.kind === "cli_job")) {
      void processNotificationClick(pending);
    }
  } catch (err) {
    console.warn("[App] Failed to fetch pending notification click:", err);
  }
});

onUnmounted(() => {
  stopShareReadyWatch?.();
  stopShareReadyWatch = null;
  if (unlistenLog) unlistenLog();
  window.removeEventListener("vcp-exit-requested", handleExitRequest);
  window.removeEventListener("vcp-hardware-back", handleExitRequest);
  window.removeEventListener("vcp-share-intent", handleShareIntent);
  window.removeEventListener("vcp-notification-click", handleNotificationClick);
  leftSidebarPersistent.removeEventListener("change", reconcileDrawerPresentation);
  rightSidebarPersistent.removeEventListener("change", reconcileDrawerPresentation);
  releaseNativeInsetsBridge();

});
</script>

<template>
  <div ref="appRootRef" class="vcp-app-root h-full w-full overflow-hidden flex flex-col select-none relative">
    <!-- 0. 权限门禁（仅在 PERMISSIONS 状态显示；保活核心权限缺失时阻断进入） -->
    <PermissionGate v-if="lifecycleStore.state === 'PERMISSIONS'" />

    <!-- 0.5. 全局初始化加载层 & 错误看板；存储等非核心权限仍由对应功能按需申请 -->
    <BootScreen v-else />

    <!-- 1. 背景底层 -->
    <Transition name="bg-fade">
      <div :key="backgroundStyle.backgroundImage" class="vcp-background-layer" :style="backgroundStyle"></div>
    </Transition>
    <div class="vcp-background-overlay absolute inset-0 pointer-events-none transition-colors" style="transition-duration: 350ms;"
      :class="themeStore.isDarkResolved ? 'bg-black/12' : 'bg-transparent'"></div>

    <!-- 2. 单一横向工作区：CSS 按可用宽度决定抽屉、单栏或双栏呈现 -->
    <div
      v-if="lifecycleStore.state === 'READY'"
      class="vcp-workspace-row flex-1 min-w-0 min-h-0 relative overflow-hidden"
    >
      <!-- 对应侧栏仍为 drawer 时显示遮罩；DOM 置于侧栏前，确保侧栏稳定位于其上 -->
      <Transition name="fade">
        <div
          v-if="layoutStore.leftDrawerOpen || layoutStore.rightDrawerOpen"
          class="vcp-drawer-overlay absolute inset-0 z-drawer bg-black/12"
          :class="{
            'is-left-open': layoutStore.leftDrawerOpen,
            'is-right-open': layoutStore.rightDrawerOpen,
          }"
          @click.self="
            layoutStore.setLeftDrawer(false);
            layoutStore.setRightDrawer(false);
          "
        ></div>
      </Transition>

      <AgentSidebar class="vcp-workspace-left" />

      <main class="vcp-workspace-main flex-1 min-w-0 min-h-0 relative overflow-hidden">
        <router-view v-slot="{ Component }">
          <component v-if="Component" :is="Component" />
        </router-view>
      </main>

      <RightSidebar
        class="vcp-workspace-right"
        :is-open="layoutStore.rightDrawerOpen"
        @close="layoutStore.setRightDrawer(false)"
      />
    </div>

    <!-- 5. 全局覆盖层管理器 -->
    <GlobalOverlayManager v-if="lifecycleStore.state === 'READY'" />

    <!-- 6. 业务 Feature 视图挂载点 -->
    <FeatureOverlays v-if="lifecycleStore.state === 'READY'" />

    <!-- 6.5 教学引导覆盖层（z-guide，高于一切业务 UI，低于 Boot 启动屏与权限门禁层） -->
    <GuideOverlay v-if="lifecycleStore.state === 'READY'" />

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
      @skip="handleSkipVersion"
    />
  </div>
</template>

<style>
/* 全局基础样式保持不变 */
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

.vcp-workspace-row {
  --vcp-workspace-safe-left: var(--vcp-safe-left, 0px);
  --vcp-workspace-safe-right: var(--vcp-safe-right, 0px);
  display: flex;
  flex-direction: row;
  isolation: isolate;
}

.vcp-workspace-main {
  box-sizing: border-box;
  padding-left: var(--vcp-workspace-safe-left);
  padding-right: var(--vcp-workspace-safe-right);
}

@media (min-width: 1024px) {
  .vcp-workspace-main {
    padding-left: 0;
  }

  .vcp-drawer-overlay:not(.is-right-open) {
    display: none;
  }
}

@media (min-width: 1280px) {
  .vcp-workspace-main {
    padding-right: 0;
  }

  .vcp-drawer-overlay {
    display: none;
  }
}

.vcp-background-layer {
  position: absolute;
  inset: 0;
  background-size: cover;
  background-position: center;
  background-repeat: no-repeat;
  transition: none;
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
  transition: opacity 0.35s ease-in-out;
}

.bg-fade-enter-from,
.bg-fade-leave-to {
  opacity: 0;
}

.pt-safe {
  padding-top: var(--vcp-safe-top, 24px);
}

.mb-safe {
  margin-bottom: var(--vcp-safe-bottom, 48px);
}

.pb-safe {
  padding-bottom: var(--vcp-safe-bottom, 48px);
}

/* 全局动画暂停：切到后台时由 JS 添加此 class 到 <html> */
.vcp-paused-animations *,
.vcp-paused-animations *::before,
.vcp-paused-animations *::after {
  animation-play-state: paused !important;
}


</style>
