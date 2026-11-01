<script setup lang="ts">
import { ref } from 'vue';
import { useAssistantStore } from '../../core/stores/assistant';
import { useOverlayStore } from '../../core/stores/overlay';
import { useChatSessionStore } from '../../core/stores/chatSessionStore';
import { useTopicStore } from '../../core/stores/topicListManager';
import { useLayoutStore } from '../../core/stores/layout';
import { useNotificationStore } from '../../core/stores/notification';

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const topicListStore = useTopicStore();
const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const isCreating = ref(false);

const openCreatedOwner = async (id: string, name: string, type: 'agent' | 'group', intent: number) => {
  try {
    await assistantStore.fetchAgentsAndGroups();
    if (!sessionStore.isNavigationCurrent(intent)) return;
    const result = await sessionStore.selectOwnerOnly({ id, name, type }, intent);
    if (!sessionStore.isNavigationCurrent(intent)) return;
    if (result !== 'accepted') {
      sessionStore.notifyNavigationResult(result);
      return;
    }
    layoutStore.setLeftDrawer(false);
    if (type === 'agent') overlayStore.openAgentSettings(id);
    else overlayStore.openGroupSettings(id);
    // The list store reports its own load failures.
    void topicListStore.loadTopicList(id, type).catch(() => {});
  } catch (error) {
    if (sessionStore.isNavigationCurrent(intent)) {
      notificationStore.addNotification({ type: 'error', title: '创建成功，打开设置失败', message: String(error), toastOnly: true });
    }
  }
};

const handleCreateAgent = async () => {
  if (isCreating.value) return;
  console.info('[AgentsCreator] create-agent clicked');
  overlayStore.openPrompt({
    title: '创建 Agent',
    initialValue: '',
    placeholder: '为你的助手起个名字...',
    onConfirm: async (value: string) => {
      const name = value.trim();
      if (!name) return;

      isCreating.value = true;
      const intent = sessionStore.beginNavigation();
      try {
        const newAgent = await assistantStore.createAgent(name);
        if (newAgent?.id) await openCreatedOwner(newAgent.id, newAgent.name, 'agent', intent);
      } catch (error) {
        console.error('[AgentsCreator] create-agent failed', error);
        notificationStore.addNotification({
          type: 'error',
          message: '创建 Agent 失败',
          toastOnly: true
        });
      } finally {
        isCreating.value = false;
      }
    }
  });
};

const handleCreateGroup = async () => {
  if (isCreating.value) return;
  console.info('[AgentsCreator] create-group clicked');
  overlayStore.openPrompt({
    title: '创建 Group',
    initialValue: '',
    placeholder: '为你的群组起个名字...',
    onConfirm: async (value: string) => {
      const name = value.trim();
      if (!name) return;

      isCreating.value = true;
      const intent = sessionStore.beginNavigation();
      try {
        const newGroup = await assistantStore.createGroup(name);
        if (newGroup?.id) await openCreatedOwner(newGroup.id, newGroup.name, 'group', intent);
      } catch (error) {
        console.error('[AgentsCreator] create-group failed', error);
        notificationStore.addNotification({
          type: 'error',
          message: '创建群组失败',
          toastOnly: true
        });
      } finally {
        isCreating.value = false;
      }
    }
  });
};
</script>

<template>
  <div class="flex gap-2">
    <button
      :disabled="isCreating"
      class="flex-1 py-2.5 bg-blue-500/10 dark:bg-blue-500/20 hover:bg-blue-500/20 dark:hover:bg-blue-500/30 text-blue-600 dark:text-blue-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2"
      @click="handleCreateAgent">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      创建 Agent
    </button>

    <button
      :disabled="isCreating"
      class="flex-1 py-2.5 bg-purple-500/10 dark:bg-purple-500/20 hover:bg-purple-500/20 dark:hover:bg-purple-500/30 text-purple-600 dark:text-purple-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2"
      @click="handleCreateGroup">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      创建 Group
    </button>
  </div>
</template>
