<script setup lang="ts">
import { computed } from 'vue';
import { useRouter } from 'vue-router';
import { useTopicStore, type Topic } from '../../core/stores/topicListManager';
import { useChatManagerStore } from '../../core/stores/chatManager';
import { useAssistantStore } from '../../core/stores/assistant';
import { useLayoutStore } from '../../core/stores/layout';
import { useOverlayStore } from '../../core/stores/overlay';
import { Edit3, Lock, LockOpen, CheckCircle, Trash2 } from 'lucide-vue-next';

const emit = defineEmits<{
  (e: 'select-topic'): void;
}>();

const topicListStore = useTopicStore();
const chatStore = useChatManagerStore();
const assistantStore = useAssistantStore();
const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const router = useRouter();

type TopicViewModel = Topic & { pinned?: boolean; updatedAt?: number };

const currentTopics = computed<TopicViewModel[]>(() => {
  return topicListStore.filteredTopics as TopicViewModel[];
});

const showTopicContextMenu = (topic: Topic) => {
  const itemId = chatStore.currentSelectedItem?.id || 'default_agent';
  
  overlayStore.openContextMenu([
    {
      label: '修改标题',
      icon: Edit3,
      handler: () => {
        overlayStore.openPrompt({
          title: '修改话题标题',
          initialValue: topic.name,
          placeholder: '请输入新的话题标题...',
          onConfirm: (newTitle: string) => {
            if (newTitle && newTitle.trim()) {
              topicListStore.updateTopicTitle(itemId, topic.id, newTitle.trim());
            }
          }
        });
      }
    },
    {
      label: topic.locked ? '解锁话题' : '锁定话题',
      icon: topic.locked ? LockOpen : Lock,
      handler: () => {
        topicListStore.toggleTopicLock(itemId, topic.id);
      }
    },
    {
      label: topic.unread ? '标为已读' : '标为未读',
      icon: CheckCircle,
      handler: () => {
        topicListStore.setTopicUnread(itemId, topic.id, !topic.unread);
      }
    },
    {
      label: '删除话题',
      icon: Trash2,
      danger: true,
      handler: () => {
        if (window.confirm(`确定要删除话题 "${topic.name}" 吗？此操作不可逆转。`)) {
          if (window.confirm(`【最终确认】真的要永久删除 "${topic.name}" 吗？`)) {
            topicListStore.deleteTopic(itemId, topic.id);
          }
        }
      }
    }
  ], 'Topic Options');
};

const selectTopic = async (itemId: string, topicId: string, topicName: string) => {
  if (router.currentRoute.value.path !== '/chat') {
    await router.push('/chat');
  }
  await chatStore.loadHistory(itemId, topicId);
  
  // 更新当前选中项的名称 (保持 type)
  if (!chatStore.currentSelectedItem || chatStore.currentSelectedItem.id !== itemId) {
     const agent = assistantStore.agents.find((a: any) => a.id === itemId);
     if (agent) {
       chatStore.currentSelectedItem = { id: agent.id, name: agent.name, type: 'agent' };
     } else {
       const group = assistantStore.groups.find(g => g.id === itemId);
       if (group) {
         chatStore.currentSelectedItem = { id: group.id, name: group.name, type: 'group' };
       }
     }
  } else {
     chatStore.currentSelectedItem.name = topicName;
  }

  // 在移动端，选择话题后自动关闭侧边栏
  if (window.innerWidth < 768) {
    layoutStore.setLeftDrawer(false);
  }
  
  emit('select-topic');
};
</script>

<template>
  <div v-if="!topicListStore.topics || topicListStore.topics.length === 0"
       class="p-8 opacity-30 text-center flex flex-col items-center gap-2">
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
    </svg>
    <span class="text-xs">暂无话题，请先选择助手</span>
  </div>
  
  <div v-else v-for="topic in currentTopics" :key="topic.id"
       @click="selectTopic(chatStore.currentSelectedItem?.id || 'default_agent', topic.id, topic.name)"
       v-longpress="() => showTopicContextMenu(topic)"
       class="relative p-3 glass-panel rounded-xl flex items-center gap-3 active:scale-95 transition-all border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5"
       :class="chatStore.currentTopicId === topic.id ? 'border-green-500/50 bg-green-500/10 dark:bg-green-500/20' : 'border-black/5 dark:border-white/5'">
    
    <div v-if="topic.unreadCount === -1 || topic.unread"
         class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm animate-pulse"
         style="background: #ff6b6b;"></div>

    <div class="w-10 h-10 rounded-xl bg-black/5 dark:bg-white/5 flex items-center justify-center shrink-0 border border-black/5 dark:border-white/5">
      <svg v-if="topic.locked" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="opacity-40"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"></rect><path d="M7 11V7a5 5 0 0 1 10 0v4"></path></svg>
      <span v-else class="text-[10px] font-bold opacity-30">{{ topic.messageCount || 0 }}</span>
    </div>
    <div class="flex flex-col overflow-hidden flex-1">
      <span class="font-bold text-sm truncate text-primary-text">{{ topic.name }}</span>
      <span class="text-[9px] opacity-40 uppercase tracking-tighter">
        {{ new Date(topic.createdAt).toLocaleDateString() }}
      </span>
    </div>
  </div>
</template>
