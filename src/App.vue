<script setup lang="ts">
import { onMounted, onUnmounted, computed, nextTick, ref, watch, type WatchStopHandle } from "vue";
import { useRouter } from "vue-router";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useSidebarSwipe } from "./core/composables/useSidebarSwipe";
import { useThemeStore } from "./core/stores/theme";
import { useAppLifecycleStore } from "./core/stores/appLifecycle";
import { isAndroid, isIOS } from "./core/utils/platform";
import { useLayoutStore } from "./core/stores/layout";
import { useOverlayStore } from "./core/stores/overlay";
import { useModalHistory } from "./core/composables/useModalHistory";
import { useNotificationStore } from "./core/stores/notification";
import { useNotificationProcessor } from "./core/composables/useNotificationProcessor";
import { useEmoticonFixer } from "./core/composables/useEmoticonFixer";
import { useAutoUpdate } from "./core/composables/useAutoUpdate";
import { useChatSessionStore } from "./core/stores/chatSessionStore";
import { useAppLifecycle } from "./core/composables/useAppLifecycle";
import { retainNativeInsetsBridge } from "./core/composables/useKeyboardInsets";
import { useShareStore } from "./core/stores/shareStore";
import {
  useVcpCliStore,
  type VcpCliNotificationTarget,
} from "./features/cli/vcpCliStore";
import type { AgentListItemDto, TopicDto } from "./core/types/assistant";

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


const themeStore = useThemeStore();
const lifecycleStore = useAppLifecycleStore();
const notificationStore = useNotificationStore();
const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const sessionStore = useChatSessionStore();
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

// One recoverable share; native acquisition and message commits are owned by Rust.
const shareStore = useShareStore();
let pendingAndroidShare: unknown = null;
const handleShareIntent = (event: Event) => {
  const detail = (event as CustomEvent<unknown>).detail;
  if (lifecycleStore.state !== "READY") pendingAndroidShare = detail;
  else shareStore.receiveAndroid(detail);
};
const refreshShare = () => {
  if (lifecycleStore.state !== "READY" || (!isAndroid && !isIOS)) return;
  if (pendingAndroidShare) {
    shareStore.receiveAndroid(pendingAndroidShare);
    pendingAndroidShare = null;
  } else void shareStore.refresh().catch(shareStore.notify);
};
watch(() => lifecycleStore.state, state => { if (state === "READY") refreshShare(); });
watch(() => lifecycleStore.lifecycleSnapshot?.phase, (phase, previous) => {
  if (phase === "active" && previous !== "active") refreshShare();
});
const handleShareAgentSelected = (agent: AgentListItemDto, allowPartial = false) => {
  void shareStore.openDraft(agent.id, allowPartial);
};

// --- Notification Click Routing State & Logic ---
type NotificationClickTarget =
  | { kind: "app" }
  | {
      kind: "cli_job";
      action: "open" | "confirm_stop";
      jobId: string;
      attemptId: string;
      runtimeGeneration: number;
    }
  | {
      kind: "chat";
      ownerId: string;
      ownerType: "agent" | "group";
      topicId: string;
      requestId: string;
    };

const parseNotificationClickTarget = (value: unknown): NotificationClickTarget | null => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const source = value as Record<string, unknown>;
  if (source.kind === "app") return { kind: "app" };
  if (source.kind === "cli_job") {
    if (
      (source.action !== "open" && source.action !== "confirm_stop") ||
      typeof source.jobId !== "string" ||
      source.jobId.length === 0 ||
      source.jobId.length > 256 ||
      typeof source.attemptId !== "string" ||
      source.attemptId.length === 0 ||
      source.attemptId.length > 256 ||
      typeof source.runtimeGeneration !== "number" ||
      !Number.isSafeInteger(source.runtimeGeneration) ||
      source.runtimeGeneration <= 0
    ) {
      return null;
    }
    return {
      kind: "cli_job",
      action: source.action,
      jobId: source.jobId,
      attemptId: source.attemptId,
      runtimeGeneration: source.runtimeGeneration,
    };
  }
  if (
    typeof source.ownerId !== "string" ||
    source.ownerId.length === 0 ||
    (source.ownerType !== "agent" && source.ownerType !== "group") ||
    typeof source.topicId !== "string" ||
    source.topicId.length === 0
  ) {
    return null;
  }
  return {
    kind: "chat",
    ownerId: source.ownerId,
    ownerType: source.ownerType,
    topicId: source.topicId,
    requestId: typeof source.requestId === "string" ? source.requestId : "",
  };
};

let notificationNavigationGeneration = 0;
let stopNotificationReadyWatch: WatchStopHandle | null = null;
let appUnmounted = false;
let lastIOSNotificationSequence = 0;
let lastIOSNotificationClickId = "";
const isCurrentNotificationNavigation = (generation: number) =>
  !appUnmounted && generation === notificationNavigationGeneration;

const routeNotificationClick = (rawDetail: unknown) => {
  if (appUnmounted) return;
  const detail = parseNotificationClickTarget(rawDetail);
  if (!detail) return;
  if (isIOS) {
    const response = rawDetail as Record<string, unknown>;
    if (typeof response.clickId !== 'string' || !response.clickId || response.clickId.length > 256
      || typeof response.sequence !== 'number' || !Number.isSafeInteger(response.sequence)
      || response.sequence <= lastIOSNotificationSequence
      || response.clickId === lastIOSNotificationClickId) return;
    // Native assigns order at the actual click. A late cold-start read cannot
    // supersede a newer live click, and both deliveries of one click are consumed once.
    lastIOSNotificationSequence = response.sequence;
    lastIOSNotificationClickId = response.clickId;
  }
  const generation = ++notificationNavigationGeneration;
  stopNotificationReadyWatch?.();
  stopNotificationReadyWatch = null;
  // A plain notification opens the app without inventing a chat destination.
  if (detail.kind === 'app') return;
  void processNotificationClick(detail, generation).catch(error => {
    if (!isCurrentNotificationNavigation(generation)) return;
    console.error('[App] Notification routing failed:', error);
    notificationStore.addNotification({
      type: 'error', title: '无法打开通知', message: '请从应用内重新打开目标页面。', toastOnly: true,
    });
  });
};

const handleNotificationClick = (e: Event) => {
  routeNotificationClick((e as CustomEvent<unknown>).detail);
};

const processNotificationClick = async (
  detail: NotificationClickTarget,
  generation: number,
) => {
  if (!isCurrentNotificationNavigation(generation) || detail.kind === 'app') return;
  console.log("[App] Notification click received:", detail);

  if (lifecycleStore.state !== "READY") {
    console.log("[App] Core not ready yet, deferring notification click routing...");
    stopNotificationReadyWatch = watch(
      () => lifecycleStore.state,
      (state) => {
        if (state === "READY" && isCurrentNotificationNavigation(generation)) {
          stopNotificationReadyWatch?.();
          stopNotificationReadyWatch = null;
          void processNotificationClick(detail, generation).catch(error => {
            if (isCurrentNotificationNavigation(generation)) {
              console.error('[App] Deferred notification routing failed:', error);
              notificationStore.addNotification({
                type: 'error', title: '无法打开通知', message: '请从应用内重新打开目标页面。', toastOnly: true,
              });
            }
          });
        }
      }
    );
    return;
  }


  const closeNavigationSurfaces = () => {
    const { closeTopModal, modalStackLength } = useModalHistory();
    while (modalStackLength() > 0) {
      const before = modalStackLength();
      closeTopModal();
      // 不可 dismiss 的长任务页持有导航权，通知点击不得强制卸载。
      if (modalStackLength() >= before) break;
    }
    layoutStore.setLeftDrawer(false);
    layoutStore.setRightDrawer(false);
    return modalStackLength() === 0;
  };

  if (detail.kind === "cli_job") {
    const target: VcpCliNotificationTarget = {
      jobId: detail.jobId,
      attemptId: detail.attemptId,
      runtimeGeneration: detail.runtimeGeneration,
    };
    if (!closeNavigationSurfaces()) return;
    overlayStore.openCliManifest();
    await nextTick();
    if (!isCurrentNotificationNavigation(generation)) return;
    const cliStore = useVcpCliStore();
    const opened = await cliStore.openJobFromNotification(target);
    if (!isCurrentNotificationNavigation(generation)) return;
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
    if (!confirmed || !isCurrentNotificationNavigation(generation)) return;
    const revalidated = await cliStore.openJobFromNotification(target);
    if (!isCurrentNotificationNavigation(generation)) return;
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

  // 聊天通知先只读验证目标；过期通知不得关闭当前界面或清空当前 Topic。
  try {
    const topics = await invoke<TopicDto[]>("get_topics", {
      ownerId: detail.ownerId,
      ownerType: detail.ownerType,
    });
    if (!isCurrentNotificationNavigation(generation)) return;
    const targetExists = topics.some((topic) => topic.id === detail.topicId);
    if (!targetExists) {
      notificationStore.addNotification({
        type: "warning",
        title: "通知已过期",
        message: "目标话题已不存在，当前会话未受影响",
        toastOnly: true,
      });
      return;
    }

    if (!closeNavigationSurfaces()) return;
    if (!isCurrentNotificationNavigation(generation)) return;
    await sessionStore.selectTopicById(
      detail.ownerId,
      detail.ownerType,
      detail.topicId,
    );
  } catch (error) {
    if (!isCurrentNotificationNavigation(generation)) return;
    console.error("[App] Failed to open notification target:", error);
    notificationStore.addNotification({
      type: "error",
      title: "无法打开通知",
      message: error instanceof Error ? error.message : String(error),
      toastOnly: true,
    });
  }
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
let stopRouterHistory: (() => void) | null = null;

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

  // 3. 只有具备原生后台切换动作的平台才在根部提供双击退出。
  if (!isAndroid) return;

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

let pendingNotificationRead: Promise<void> | null = null;
let pendingNotificationRefreshQueued = false;
let androidPendingNotificationRead = false;
let unlistenNativeNotification: (() => void) | null = null;
let notificationSubscription: Promise<void> | null = null;
let notificationSubscriptionSettled = isAndroid;

const readPendingNotification = (): Promise<void> => {
  if (appUnmounted || lifecycleStore.state !== 'READY' || !notificationSubscriptionSettled
    || (!isAndroid && !isIOS) || (isAndroid && androidPendingNotificationRead)) return Promise.resolve();
  if (pendingNotificationRead) {
    if (isIOS) pendingNotificationRefreshQueued = true;
    return pendingNotificationRead;
  }
  if (isAndroid) androidPendingNotificationRead = true;
  pendingNotificationRead = invoke<unknown>('plugin:vcp-mobile|get_pending_notification')
    .then(pending => { if (!appUnmounted) routeNotificationClick(pending); })
    .catch(error => console.warn('[App] Failed to fetch pending notification click:', error))
    .finally(() => {
      pendingNotificationRead = null;
      if (!appUnmounted && pendingNotificationRefreshQueued) {
        pendingNotificationRefreshQueued = false;
        void readPendingNotification();
      }
    });
  return pendingNotificationRead;
};

const subscribeNativeNotifications = (): Promise<void> => {
  if (appUnmounted || !isIOS) return Promise.resolve();
  if (notificationSubscription) return notificationSubscription;
  if (unlistenNativeNotification) return readPendingNotification();
  notificationSubscription = listen<unknown>('vcp-notification-click', event => {
    routeNotificationClick(event.payload);
  }).then(unlisten => {
    if (appUnmounted) unlisten();
    else unlistenNativeNotification = unlisten;
  }).catch(error => {
    console.warn('[App] Failed to subscribe to native notification clicks:', error);
  }).then(async () => {
    notificationSubscriptionSettled = true;
    await readPendingNotification();
  }).finally(() => { notificationSubscription = null; });
  return notificationSubscription;
};

watch(
  () => lifecycleStore.state,
  state => {
    if (state === 'READY') void readPendingNotification();
  },
);
watch(() => lifecycleStore.lifecycleSnapshot?.phase, (phase, previous) => {
  if (isIOS && phase === 'active' && previous !== 'active') void subscribeNativeNotifications();
});

onMounted(async () => {
  // 1. 同步挂载基础物理按键与系统事件监听 (混合应用黄金铁律：物理拦截最优先挂载，杜绝初始化阻塞失效)
  window.addEventListener("vcp-exit-requested", handleExitRequest);
  window.addEventListener("vcp-hardware-back", handleExitRequest);
  if (isAndroid) {
    window.addEventListener("vcp-share-intent", handleShareIntent);
    window.addEventListener("vcp-notification-click", handleNotificationClick);
  }
  if (isIOS) void subscribeNativeNotifications();
  if (isIOS) window.addEventListener("vcp-share-inbox-changed", refreshShare);
  leftSidebarPersistent.addEventListener("change", reconcileDrawerPresentation);
  rightSidebarPersistent.addEventListener("change", reconcileDrawerPresentation);
  reconcileDrawerPresentation();


  // 初始化全局表情包修复器
  initGlobalFixer();

  // 1.5. 启动 VCP Log IPC 监听 (必须在 bootstrapApp 前挂载，防止 bootstrap 期间的 ready 事件丢失)
  const stopLogListener = await listen<unknown>("vcp-system-event", (event) => {
    if (appUnmounted) return;
    const payload = event.payload;
    const processed = processPayload(payload);

    if (processed && !processed.silent) {
      notificationStore.addNotification(processed);
    }
  });
  if (appUnmounted) {
    stopLogListener();
    return;
  }
  unlistenLog = stopLogListener;

  // 2. 异步执行重度核心资源加载 (启动引导)
  await bootstrapApp();
  if (appUnmounted) return;

  // Operation Dummy Root: Wait for router and inject dummy layer
  await router.isReady();
  if (appUnmounted) return;
  initRootHistory();

  // 路由后置守护：在任何路由切换（包括重定向、刷新）完成后，自动校准防护盾，100% 确保栈顶处于防护状态
  stopRouterHistory = router.afterEach(() => {
    initRootHistory();
  });

});

onUnmounted(() => {
  appUnmounted = true;
  notificationNavigationGeneration++;
  stopNotificationReadyWatch?.();
  stopNotificationReadyWatch = null;
  if (unlistenLog) unlistenLog();
  stopRouterHistory?.();
  unlistenNativeNotification?.();
  window.removeEventListener("vcp-exit-requested", handleExitRequest);
  window.removeEventListener("vcp-hardware-back", handleExitRequest);
  window.removeEventListener("vcp-share-intent", handleShareIntent);
  window.removeEventListener("vcp-share-inbox-changed", refreshShare);
  window.removeEventListener("vcp-notification-click", handleNotificationClick);
  leftSidebarPersistent.removeEventListener("change", reconcileDrawerPresentation);
  rightSidebarPersistent.removeEventListener("change", reconcileDrawerPresentation);
  releaseNativeInsetsBridge();
  if (exitTimer) clearTimeout(exitTimer);

});
</script>

<template>
  <div ref="appRootRef" class="vcp-app-root vcp-viewport-surface h-full w-full overflow-hidden flex flex-col select-none relative">
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
      :is-open="shareStore.selectorOpen"
      :shared-text="shareStore.current?.text || ''"
      :shared-file-count="shareStore.current?.attachments.length || 0"
      :failures="shareStore.failures"
      :busy="shareStore.busy"
      :error="shareStore.current?.error"
      :is-draft="shareStore.current?.state === 'draft'"
      @close="shareStore.selectorOpen = false"
      @discard="shareStore.discard"
      @retry="shareStore.prepare().catch(shareStore.notify)"
      @selected="handleShareAgentSelected"
    />

    <!-- 8. 自动更新提示弹窗 -->
    <UpdatePrompt
      v-if="isAndroid"
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
