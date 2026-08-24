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
      try {
        const newAgent = await assistantStore.createAgent(name);
        await assistantStore.fetchAgentsAndGroups();
        if (newAgent?.id) {
          // 选中新创建的 Agent
          sessionStore.setConversation({
            id: newAgent.id,
            name: newAgent.name,
            type: 'agent'
          }, null);
          // 加载话题列表
          try {
            await topicListStore.loadTopicList(newAgent.id, 'agent');
          } catch {
            // Topic Store 已统一显示加载失败提示；Agent 创建本身仍然成功。
          }
          // 关闭侧边栏
          layoutStore.setLeftDrawer(false);
          // 开启配置 Overlay
          overlayStore.openAgentSettings(newAgent.id);
        }
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
      try {
        const newGroup = await assistantStore.createGroup(name);
        await assistantStore.fetchAgentsAndGroups();
        if (newGroup?.id) {
          // 选中新创建 of Group
          sessionStore.setConversation({
            id: newGroup.id,
            name: newGroup.name,
            type: 'group'
          }, null);
          // 加载话题列表
          try {
            await topicListStore.loadTopicList(newGroup.id, 'group');
          } catch {
            // Topic Store 已统一显示加载失败提示；Group 创建本身仍然成功。
          }
          // 关闭侧边栏
          layoutStore.setLeftDrawer(false);
          // 开启配置 Overlay
          overlayStore.openGroupSettings(newGroup.id);
          }
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
