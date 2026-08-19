<script setup lang="ts">
/**
 * FeatureOverlays.vue
 *
 * 职责：作为所有全局业务 Feature 视图的统一挂载点。
 *
 * 架构说明：
 * 1. Settings/Agent/Group 等低频页面首次打开时才挂载，挂载后保持常驻 DOM，
 *    以保留组件本地状态（表单草稿等）并确保 SlidePage 的 leave 动画正常完成。
 * 2. SyncSessionView 使用 v-if 按需渲染，因其状态完全由 syncSessionStore 管理，
 *    且已纳入 OverlayStore pageStack 统一管控。
 *
 * 注意：此组件内的视图通过 SlidePage 管理滑入/滑出动画，
 * 物理上它们会渲染在 GlobalOverlayManager 提供的容器中。
 */
import { defineAsyncComponent, ref, watch } from 'vue';
import { useOverlayStore } from '../core/stores/overlay';
import { useSettingsStore } from '../core/stores/settings';
import ToolInteractionOverlay from '../features/distributed/ToolInteractionOverlay.vue';

// 相对低频的设置页按需懒加载：用户首次打开时才下载 chunk，SlidePage 动画天然遮盖加载延迟
const AgentSettingsView = defineAsyncComponent(() => import('../features/agent/AgentSettingsView.vue'));
const GroupSettingsView = defineAsyncComponent(() => import('../features/agent/GroupSettingsView.vue'));

// 其余页面同样按需异步加载，状态由 Store 完全托管
const SyncSessionView = defineAsyncComponent(() => import('../features/sync/SyncSessionView.vue'));
const RebuildSessionView = defineAsyncComponent(() => import('../features/settings/components/RebuildSessionView.vue'));
const DistributedView = defineAsyncComponent(() => import('../features/distributed/DistributedView.vue'));
const SettingsView = defineAsyncComponent(() => import('../features/settings/SettingsView.vue'));
const RagObserverView = defineAsyncComponent(() => import('../features/rag/RagObserver.vue'));
const DiaryCenterView = defineAsyncComponent(() => import('../features/diary/DiaryCenterView.vue'));
const LogCenterView = defineAsyncComponent(() => import('../features/logcenter/LogCenterView.vue'));
const TaskCenterView = defineAsyncComponent(() => import('../features/taskcenter/TaskCenterView.vue'));
const AgentMgrView = defineAsyncComponent(() => import('../features/agentmgr/AgentMgrView.vue'));
const ForumListView = defineAsyncComponent(() => import('../features/forum/ForumListView.vue'));
const MailListView = defineAsyncComponent(() => import('../features/mail/MailListView.vue'));
const VcpCliManifestView = defineAsyncComponent(() => import('../features/cli/components/VcpCliManifestView.vue'));
const TarvenSettingsView = defineAsyncComponent(() => import('../features/chat/components/TarvenSettings.vue'));


const overlayStore = useOverlayStore();
const settingsStore = useSettingsStore();

const createFirstOpenLatch = (isOpen: () => boolean) => {
  const mounted = ref(isOpen());
  watch(isOpen, (open) => {
    if (open) mounted.value = true;
  });
  return mounted;
};

const settingsMounted = createFirstOpenLatch(() => overlayStore.isSettingsOpen);
const agentSettingsMounted = createFirstOpenLatch(() => overlayStore.isAgentSettingsOpen);
const groupSettingsMounted = createFirstOpenLatch(() => overlayStore.isGroupSettingsOpen);
const tarvenSettingsMounted = createFirstOpenLatch(() => overlayStore.isTarvenSettingsOpen);
const distributedMounted = createFirstOpenLatch(() => overlayStore.isDistributedOpen);
const ragObserverMounted = createFirstOpenLatch(() => overlayStore.isRagObserverOpen);
const diaryMounted = createFirstOpenLatch(() => overlayStore.isDiaryCenterOpen);
const cliManifestMounted = createFirstOpenLatch(() => overlayStore.isCliManifestOpen);
const logCenterMounted = createFirstOpenLatch(() => overlayStore.isLogCenterOpen);
const taskCenterMounted = createFirstOpenLatch(() => overlayStore.isTaskCenterOpen);
const agentMgrMounted = createFirstOpenLatch(() => overlayStore.isAgentMgrOpen);
const forumMounted = createFirstOpenLatch(() => overlayStore.isForumOpen);
const mailMounted = createFirstOpenLatch(() => overlayStore.isMailOpen);
</script>

<template>
  <div>
    <SettingsView
      v-if="settingsMounted"
      :is-open="overlayStore.isSettingsOpen"
      :z-index="overlayStore.getPageZIndex('settings')"
      @close="overlayStore.closeSettings()"
    />

    <AgentSettingsView
      v-if="agentSettingsMounted"
      :is-open="overlayStore.isAgentSettingsOpen"
      :id="overlayStore.agentSettingsId"
      :z-index="overlayStore.getPageZIndex('agentSettings')"
      @close="overlayStore.closeAgentSettings()"
    />

    <GroupSettingsView
      v-if="groupSettingsMounted"
      :is-open="overlayStore.isGroupSettingsOpen"
      :id="overlayStore.groupSettingsId"
      :z-index="overlayStore.getPageZIndex('groupSettings')"
      @close="overlayStore.closeGroupSettings()"
    />

    <TarvenSettingsView
      v-if="tarvenSettingsMounted"
      :is-open="overlayStore.isTarvenSettingsOpen"
      :z-index="overlayStore.getPageZIndex('tarvenSettings')"
      @close="overlayStore.closeTarvenSettings()"
    />

    <SyncSessionView
      v-if="overlayStore.isSyncSessionOpen"
      :z-index="overlayStore.getPageZIndex('syncSession')"
    />
    <RebuildSessionView
      v-if="overlayStore.isRebuildSessionOpen"
      :z-index="overlayStore.getPageZIndex('rebuildSession')"
    />

    <DistributedView
      v-if="distributedMounted"
      :is-open="overlayStore.isDistributedOpen"
      :z-index="overlayStore.getPageZIndex('distributed')"
      @close="overlayStore.closeDistributed()"
    />

    <RagObserverView
      v-if="ragObserverMounted"
      :is-open="overlayStore.isRagObserverOpen"
      :z-index="overlayStore.getPageZIndex('ragObserver')"
      @close="overlayStore.closeRagObserver()"
    />

    <VcpCliManifestView
      v-if="cliManifestMounted"
      :is-open="overlayStore.isCliManifestOpen"
      :z-index="overlayStore.getPageZIndex('cliManifest')"
      @close="overlayStore.closeCliManifest()"
    />

    <LogCenterView
      v-if="logCenterMounted"
      :is-open="overlayStore.isLogCenterOpen"
      :z-index="overlayStore.getPageZIndex('logCenter')"
      @close="overlayStore.closeLogCenter()"
    />

    <TaskCenterView
      v-if="taskCenterMounted"
      :is-open="overlayStore.isTaskCenterOpen"
      :z-index="overlayStore.getPageZIndex('taskCenter')"
      @close="overlayStore.closeTaskCenter()"
    />

    <AgentMgrView
      v-if="agentMgrMounted"
      :is-open="overlayStore.isAgentMgrOpen"
      :z-index="overlayStore.getPageZIndex('agentMgr')"
      @close="overlayStore.closeAgentMgr()"
    />

    <ForumListView
      v-if="forumMounted"
      :is-open="overlayStore.isForumOpen"
      :z-index="overlayStore.getPageZIndex('forum')"
      @close="overlayStore.closeForum()"
    />

    <MailListView
      v-if="mailMounted"
      :is-open="overlayStore.isMailOpen"
      :z-index="overlayStore.getPageZIndex('mail')"
      @close="overlayStore.closeMail()"
    />

    <Suspense v-if="diaryMounted">
      <DiaryCenterView
        :is-open="overlayStore.isDiaryCenterOpen"
        :z-index="overlayStore.getPageZIndex('diaryCenter')"
        :open-target="overlayStore.diaryOpenTarget"
        @target-consumed="overlayStore.clearDiaryOpenTarget()"
        @close="overlayStore.closeDiaryCenter()"
      />
      <template #fallback>
        <div
          v-if="overlayStore.isDiaryCenterOpen"
          class="fixed inset-0 pointer-events-auto grid place-items-center bg-[var(--primary-bg)] text-sm text-[var(--primary-text)]"
          :style="{ zIndex: overlayStore.getPageZIndex('diaryCenter') }"
        >
          正在打开日记中心…
        </div>
      </template>
    </Suspense>

    <!-- 仅当用户已启用分布式计算时才挂载事件监听器，避免常驻不必要的后台监听 -->
    <ToolInteractionOverlay v-if="settingsStore.settings?.distributedEnabled" />
  </div>
</template>
