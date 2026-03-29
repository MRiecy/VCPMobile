<script setup lang="ts">
import { useRouter } from 'vue-router';
import { useAssistantStore } from '../../core/stores/assistant';
import { useOverlayStore } from '../../core/stores/overlay';

const assistantStore = useAssistantStore();
const overlayStore = useOverlayStore();
const router = useRouter();

const handleCreateAgent = async () => {
  console.info('[AgentsCreator] create-agent clicked');
  overlayStore.openPrompt({
    title: '创建 Agent',
    initialValue: '',
    placeholder: '为你的助手起个名字...',
    onConfirm: async (value: string) => {
      const name = value.trim();
      if (!name) return;

      try {
        const newAgent = await assistantStore.createAgent(name);
        await assistantStore.fetchAgents();
        if (newAgent?.id) {
          await router.push(`/agents/${newAgent.id}`);
        }
      } catch (error) {
        console.error('[AgentsCreator] create-agent failed', error);
        window.alert('创建 Agent 失败');
      }
    }
  });
};

const handleCreateGroup = async () => {
  console.info('[AgentsCreator] create-group clicked');
  overlayStore.openPrompt({
    title: '创建 Group',
    initialValue: '',
    placeholder: '为你的群组起个名字...',
    onConfirm: async (value: string) => {
      const name = value.trim();
      if (!name) return;

      try {
        await assistantStore.createGroup(name);
        await assistantStore.fetchGroups();
      } catch (error) {
        console.error('[AgentsCreator] create-group failed', error);
        window.alert('创建 Group 失败');
      }
    }
  });
};
</script>

<template>
  <div class="flex gap-2">
    <button
      class="flex-1 py-2.5 bg-blue-500/10 dark:bg-blue-500/20 hover:bg-blue-500/20 dark:hover:bg-blue-500/30 text-blue-600 dark:text-blue-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2"
      @click="handleCreateAgent"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      创建 Agent
    </button>

    <button
      class="flex-1 py-2.5 bg-purple-500/10 dark:bg-purple-500/20 hover:bg-purple-500/20 dark:hover:bg-purple-500/30 text-purple-600 dark:text-purple-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2"
      @click="handleCreateGroup"
    >
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <line x1="5" y1="12" x2="19" y2="12"></line>
      </svg>
      创建 Group
    </button>
  </div>
</template>
