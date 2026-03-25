<script setup lang="ts">
import { ref, onMounted, computed, watch } from 'vue';
import { useRouter } from 'vue-router';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useThemeStore } from './stores/theme';
import { useTopicStore } from './stores/topicListManager';
import { useChatManagerStore } from './stores/chatManager';
import { useAssistantStore } from './stores/assistant';
import { useSettingsStore } from './stores/settings';
import { useModalHistory } from './composables/useModalHistory';
import SettingsView from './views/SettingsView.vue';
import SyncView from './views/SyncView.vue';
import BottomSheet from './components/BottomSheet.vue';
import VcpPrompt from './components/VcpPrompt.vue';
import NotificationDrawer from './components/NotificationDrawer.vue';
import ToastManager from './components/ToastManager.vue';
import { useNotificationStore } from './stores/notification';
import { useNotificationProcessor } from './composables/useNotificationProcessor';
import { useContextMenu } from './composables/useContextMenu';
import { Edit3, Lock, LockOpen, CheckCircle, Trash2, Users } from 'lucide-vue-next';

const themeStore = useThemeStore();
const topicListStore = useTopicStore();
const chatStore = useChatManagerStore();
const assistantStore = useAssistantStore();
const settingsStore = useSettingsStore();
const notificationStore = useNotificationStore();
const { processPayload } = useNotificationProcessor();
const router = useRouter();

const { registerModal, unregisterModal, showExitToast, initRootHistory } = useModalHistory();

const isLeftDrawerOpen = ref(false);
const isRightDrawerOpen = ref(false);
const isSettingsOpen = ref(false);
const isSyncOpen = ref(false);

// --- History Handling for Overlays ---
watch(isSettingsOpen, (val) => {
  if (val) registerModal('SettingsView', () => { isSettingsOpen.value = false; });
  else unregisterModal('SettingsView');
});

watch(isSyncOpen, (val) => {
  if (val) registerModal('SyncView', () => { isSyncOpen.value = false; });
  else unregisterModal('SyncView');
});

watch(isLeftDrawerOpen, (val) => {
  if (val && window.innerWidth < 768) {
    registerModal('LeftDrawer', () => { isLeftDrawerOpen.value = false; });
  } else if (!val) {
    unregisterModal('LeftDrawer');
  }
});

watch(isRightDrawerOpen, (val) => {
  if (val) {
    notificationStore.markAllRead();
    if (window.innerWidth < 768) {
      registerModal('RightDrawer', () => { isRightDrawerOpen.value = false; });
    }
  } else if (!val) {
    unregisterModal('RightDrawer');
  }
});

const { isOpen: isContextMenuOpen, currentTitle: contextMenuTitle, currentActions: contextMenuActions, openMenu } = useContextMenu();

const activeTab = ref<'agents' | 'topics'>('agents');
const searchQuery = ref('');

// --- Swipe Action Logic (Right Swipe) ---
const activeSwipeId = ref<string | null>(null);
const currentSwipeX = ref(0);
let startX = 0;
let isDragging = false;
const SWIPE_THRESHOLD = 50;
const MAX_SWIPE = 80;

const onTouchStart = (e: TouchEvent, id: string) => {
  if (activeSwipeId.value && activeSwipeId.value !== id) {
    activeSwipeId.value = null;
    currentSwipeX.value = 0;
  }
  startX = e.touches[0].clientX;
  isDragging = true;
};

const onTouchMove = (e: TouchEvent, id: string) => {
  if (!isDragging) return;
  const currentX = e.touches[0].clientX;
  const deltaX = currentX - startX;

  // Only allow rightward swipe (deltaX > 0)
  if (deltaX > 0) {
    activeSwipeId.value = id;
    currentSwipeX.value = Math.min(deltaX, MAX_SWIPE + 20); // Elastic resistance
  } else if (activeSwipeId.value === id) {
    currentSwipeX.value = 0;
  }
};

const onTouchEnd = (id: string) => {
  if (!isDragging) return;
  isDragging = false;
  
  if (activeSwipeId.value === id && currentSwipeX.value > SWIPE_THRESHOLD) {
    currentSwipeX.value = MAX_SWIPE; // Snap open
  } else {
    activeSwipeId.value = null;
    currentSwipeX.value = 0; // Snap closed
  }
};

const goToSettings = (id: string) => {
  activeSwipeId.value = null;
  currentSwipeX.value = 0;
  isLeftDrawerOpen.value = false;
  router.push('/agents/' + id);
};

// VcpPrompt state
const isPromptOpen = ref(false);
const promptTitle = ref('');
const promptInitialValue = ref('');
const promptPlaceholder = ref('');
const promptCallback = ref<(val: string) => void>(() => {});

const openPrompt = (title: string, initialValue: string, placeholder: string, onConfirm: (val: string) => void) => {
  promptTitle.value = title;
  promptInitialValue.value = initialValue;
  promptPlaceholder.value = placeholder;
  promptCallback.value = onConfirm;
  isPromptOpen.value = true;
};

const toggleLeftDrawer = () => {
  isLeftDrawerOpen.value = !isLeftDrawerOpen.value;
  if (isLeftDrawerOpen.value) isRightDrawerOpen.value = false;
};

const toggleRightDrawer = () => {
  isRightDrawerOpen.value = !isRightDrawerOpen.value;
  if (isRightDrawerOpen.value) isLeftDrawerOpen.value = false;
};

const openSettings = () => {
  isSettingsOpen.value = true;
  isLeftDrawerOpen.value = false;
};

const selectAgent = async (agentId: string) => {
  const agent = assistantStore.agents.find(a => a.id === agentId);
  if (agent) {
    chatStore.currentSelectedItem = { id: agent.id, name: agent.name, type: 'agent' };
  }
  await topicListStore.loadTopicList(agentId);
  activeTab.value = 'topics';
};

const selectGroup = async (groupId: string) => {
  const group = assistantStore.groups.find(g => g.id === groupId);
  if (group) {
    chatStore.currentSelectedItem = { id: group.id, name: group.name, type: 'group' };
  }
  await topicListStore.loadTopicList(groupId);
  activeTab.value = 'topics';
};

const showTopicContextMenu = (topic: any) => {
  const itemId = chatStore.currentSelectedItem?.id || 'default_agent';
  
  openMenu([
    {
      label: '修改标题',
      icon: Edit3,
      handler: () => {
        openPrompt('修改话题标题', topic.name, '请输入新的话题标题...', (newTitle) => {
          if (newTitle && newTitle.trim()) {
            topicListStore.updateTopicTitle(itemId, topic.id, newTitle.trim());
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
     const agent = assistantStore.agents.find(a => a.id === itemId);
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
    isLeftDrawerOpen.value = false;
  }
};

const handleCreateTopic = async () => {
  const itemId = chatStore.currentSelectedItem?.id || assistantStore.agents[0]?.id;
  if (!itemId) {
    alert("请先选择一个助手或群组");
    return;
  }
  const newTopicName = `新话题 ${new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}`;
  try {
    const newTopic = await topicListStore.createTopic(itemId, newTopicName);
    if (newTopic && newTopic.id) {
      await selectTopic(itemId, newTopic.id, newTopic.name);
    }
  } catch (err) {
    console.error("创建话题失败", err);
  }
};

const filteredAgents = computed(() => {
  if (!searchQuery.value) return assistantStore.agents;
  return assistantStore.agents.filter(a => a.name.toLowerCase().includes(searchQuery.value.toLowerCase()));
});

const filteredGroups = computed(() => {
  if (!searchQuery.value) return assistantStore.groups;
  return assistantStore.groups.filter(g => g.name.toLowerCase().includes(searchQuery.value.toLowerCase()));
});

const filteredTopics = computed(() => {
  if (!searchQuery.value) return topicListStore.topics;
  return topicListStore.topics.filter(t => t.name.toLowerCase().includes(searchQuery.value.toLowerCase()));
});

const backgroundStyle = computed(() => {
  if (!themeStore.currentTheme) return {};
  
  const themeInfo = themeStore.availableThemes.find(t => t.fileName === themeStore.currentTheme);
  if (!themeInfo) return {};
  
  const isLight = !themeStore.isDarkResolved;
  let rawValue = isLight 
    ? themeInfo.variables.light?.['--chat-wallpaper-light']
    : themeInfo.variables.dark?.['--chat-wallpaper-dark'];

  // Fallback: if current mode has no wallpaper, try the other mode
  if (!rawValue || rawValue === 'none') {
    rawValue = isLight
      ? themeInfo.variables.dark?.['--chat-wallpaper-dark']
      : themeInfo.variables.light?.['--chat-wallpaper-light'];
  }

  if (!rawValue || rawValue === 'none') return {};

  // Extract filename and clean it robustly
  const match = rawValue.match(/url\(['"]?(.*?)['"]?\)/);
  let filename = match ? match[1] : rawValue;
  
  // 1. Strip path
  filename = filename.replace(/^.*[\\\/]/, '').replace(/['"]/g, '');
  // 2. Strip ANY existing extension and force .jpg (matching optimized public/wallpaper)
  filename = filename.split('.')[0] + '.jpg';

  return { backgroundImage: `url("/wallpaper/${filename}")` };
});

onMounted(async () => {
  // 异步加载主题与数据
  await themeStore.initTheme();
  settingsStore.fetchSettings().catch(() => {});
  await assistantStore.fetchAgents();
  await assistantStore.fetchGroups();

  // 启动 VCP Log IPC 监听 (使用 1:1 移植的解析大脑)
  listen('vcp-system-event', (event: any) => {
    const payload = event.payload;
    const processed = processPayload(payload);
    
    if (processed && !processed.silent) {
      notificationStore.addNotification(processed);
    }
  });

  // 监听配置并初始化后端大动脉
  watch(() => [settingsStore.settings?.vcpLogUrl, settingsStore.settings?.vcpLogKey], ([url, key]) => {
    if (url && key) {
      invoke('init_vcp_log_connection', { url: String(url), key: String(key) }).catch(e => {
        console.error('[VCPLog] Failed to init connection:', e);
      });
    }
  }, { immediate: true });

  // Operation Dummy Root: Wait for router and inject dummy layer
  await router.isReady();
  initRootHistory();
});
</script>

<template>
  <div class="vcp-app-root h-full w-full overflow-hidden flex flex-col select-none relative">
    
    <!-- 1. 背景底层 -->
    <Transition name="bg-fade">
      <div :key="backgroundStyle.backgroundImage" class="vcp-background-layer" :style="backgroundStyle"></div>
    </Transition>
    <div class="vcp-background-overlay absolute inset-0 pointer-events-none z-0 transition-colors duration-700"
         :class="themeStore.isDarkResolved ? 'bg-black/25' : 'bg-transparent'"></div>

    <!-- 2. 全局遮罩 (z-index 提高) -->
    <Transition name="fade">
      <div v-if="isLeftDrawerOpen || isRightDrawerOpen" 
           class="vcp-overlay fixed inset-0 bg-black/30 z-[60] backdrop-blur-[1px] md:hidden"
           @click="isLeftDrawerOpen = false; isRightDrawerOpen = false">
      </div>
    </Transition>

    <!-- 3. 左侧抽屉 -->
    <aside class="vcp-drawer vcp-drawer-left flex flex-col z-[100]" 
           :class="{ 'is-open': isLeftDrawerOpen }">
      
      <!-- 顶部 Tabs -->
      <div class="pt-safe px-4 pt-6 pb-2 shrink-0 border-b border-black/5 dark:border-white/5">
        <h2 class="text-xl font-black opacity-90 mb-4 tracking-tighter text-blue-500 dark:text-blue-400 px-2">VCP MOBILE</h2>
        
        <div class="flex p-1 bg-black/5 dark:bg-black/20 rounded-xl mb-4 border border-black/5 dark:border-white/5">
          <button @click="activeTab = 'agents'" 
                  class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all"
                  :class="activeTab === 'agents' ? 'bg-white shadow-sm text-gray-800 dark:bg-white/10 dark:text-white dark:shadow-sm' : 'text-gray-500 hover:text-gray-700 dark:text-white/40 dark:hover:text-white/60'">
            助手
          </button>
          <button @click="activeTab = 'topics'" 
                  class="flex-1 py-1.5 text-sm font-bold rounded-lg transition-all"
                  :class="activeTab === 'topics' ? 'bg-white shadow-sm text-gray-800 dark:bg-white/10 dark:text-white dark:shadow-sm' : 'text-gray-500 hover:text-gray-700 dark:text-white/40 dark:hover:text-white/60'">
            话题
          </button>
        </div>

        <div class="relative">
          <svg class="absolute left-3 top-1/2 -translate-y-1/2 opacity-40 w-4 h-4 text-primary-text" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>
          <input v-model="searchQuery" 
                 type="text" 
                 :placeholder="activeTab === 'agents' ? '搜索助手...' : '搜索话题...'" 
                 class="w-full bg-black/5 dark:bg-black/20 text-primary-text text-sm rounded-xl py-2 pl-9 pr-4 outline-none border border-black/5 dark:border-white/5 focus:border-black/20 dark:focus:border-white/20 transition-colors" />
        </div>
      </div>

      <!-- 列表内容区 -->
      <div class="flex-1 overflow-y-auto px-4 py-4 space-y-2">
        
        <!-- Agents Tab Content -->
        <template v-if="activeTab === 'agents'">
          <div v-if="assistantStore.loading" class="flex justify-center p-8 opacity-50">
            <svg class="animate-spin h-6 w-6 text-primary-text" viewBox="0 0 24 24" fill="none">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
            </svg>
          </div>
          <div v-else-if="filteredAgents.length === 0 && filteredGroups.length === 0" class="text-center p-8 opacity-30 text-sm">
            未找到助手或群组
          </div>
          <div v-else class="space-y-4">
            <!-- 群组列表 (Phase 4) -->
            <div v-if="filteredGroups.length > 0" class="space-y-2">
              <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">Agent Groups</h3>
              <div v-for="group in filteredGroups" :key="group.id" class="relative rounded-xl overflow-hidden w-full">
                <div @click="selectGroup(group.id)"
                     class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5 z-10 w-full active:scale-[0.98] transition-all"
                     :class="chatStore.currentSelectedItem?.id === group.id ? 'border-purple-500/50 bg-purple-500/10 dark:bg-purple-500/20' : 'border-black/5 dark:border-white/5'">
                  
                  <div class="w-10 h-10 rounded-xl bg-gradient-to-br from-purple-500/20 to-pink-500/20 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10 overflow-hidden">
                    <img v-if="group.resolvedAvatarUrl" :src="group.resolvedAvatarUrl" class="w-full h-full object-cover" />
                    <Users v-else class="w-5 h-5 text-purple-500/60" />
                  </div>
                  <div class="flex flex-col overflow-hidden flex-1">
                    <span class="font-bold text-sm truncate text-primary-text">{{ group.name }}</span>
                    <span class="text-[9px] opacity-40 truncate uppercase tracking-tighter">{{ group.members.length }} Members • {{ group.mode }}</span>
                  </div>
                </div>
              </div>
            </div>

            <!-- 助手列表 -->
            <div v-if="filteredAgents.length > 0" class="space-y-2">
              <h3 class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30">Individual Agents</h3>
              <div v-for="agent in filteredAgents" :key="agent.id" class="relative rounded-xl overflow-hidden w-full">
                <!-- 底部操作区 -->
                <div class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start z-0"
                     @click.stop="goToSettings(agent.id)">
                  <div class="w-[80px] h-full flex items-center justify-center text-blue-600/70 dark:text-blue-400/70 hover:text-blue-600 dark:hover:text-blue-400 transition-colors cursor-pointer active:bg-black/5 dark:active:bg-white/5">
                    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                      <circle cx="12" cy="12" r="3"></circle>
                      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
                    </svg>
                  </div>
                </div>

                <!-- 顶层原卡片 (绑定滑动手势) -->
                <div @click="selectAgent(agent.id)"
                     @touchstart="onTouchStart($event, agent.id)"
                     @touchmove="onTouchMove($event, agent.id)"
                     @touchend="onTouchEnd(agent.id)"
                     class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5 z-10 w-full active:scale-[0.98] origin-center"
                     :class="[
                       chatStore.currentSelectedItem?.id === agent.id ? 'border-blue-500/50 bg-blue-500/10 dark:bg-blue-500/20' : 'border-black/5 dark:border-white/5',
                       activeSwipeId === agent.id ? 'transition-none' : 'transition-transform duration-200 ease-out'
                     ]"
                     :style="{ transform: `translateX(${activeSwipeId === agent.id ? currentSwipeX : 0}px)` }">
                     
                  <!-- 未读小红点 -->
                  <div v-if="assistantStore.unreadCounts[agent.id] === -1 || assistantStore.unreadCounts[agent.id] > 0" class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm animate-pulse" style="background: #ff6b6b;"></div>

                  <div class="w-10 h-10 rounded-full bg-gradient-to-br from-blue-500/20 to-purple-500/20 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10 overflow-hidden pointer-events-none">
                    <img v-if="agent.resolvedAvatarUrl" :src="agent.resolvedAvatarUrl" class="w-full h-full object-cover" />
                    <span v-else class="font-bold text-lg text-primary-text opacity-50">{{ agent.name[0] }}</span>
                  </div>
                  <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
                    <span class="font-bold text-sm truncate text-primary-text">{{ agent.name }}</span>
                    <span class="text-[10px] opacity-40 truncate">{{ agent.model }}</span>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </template>

        <!-- Topics Tab Content -->
        <template v-if="activeTab === 'topics'">
          <div v-if="!topicListStore.topics || topicListStore.topics.length === 0" 
               class="p-8 opacity-30 text-center flex flex-col items-center gap-2">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
            </svg>
            <span class="text-xs">暂无话题，请先选择助手</span>
          </div>
          
          <div v-else v-for="topic in filteredTopics" :key="topic.id"
               @click="selectTopic(chatStore.currentSelectedItem?.id || 'default_agent', topic.id, topic.name)"
               v-longpress="() => showTopicContextMenu(topic)"
               class="relative p-3 glass-panel rounded-xl flex items-center gap-3 active:scale-95 transition-all border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5"
               :class="chatStore.currentTopicId === topic.id ? 'border-green-500/50 bg-green-500/10 dark:bg-green-500/20' : 'border-black/5 dark:border-white/5'">
            
            <!-- 未读小红点 / 计数角标 (基于桌面端主题同步) -->
            <div v-if="topic.unreadCount === -1 || topic.unread" 
                 class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm animate-pulse"
                 style="background: #ff6b6b;"></div>
            <div v-else-if="topic.unreadCount && topic.unreadCount > 0" 
                 class="absolute -top-1.5 -right-1.5 min-w-[18px] h-[18px] px-1 rounded-full border-2 border-white dark:border-gray-900 text-[9px] font-bold text-white flex items-center justify-center z-10 shadow-sm"
                 style="background: linear-gradient(135deg, #ff6b6b 0%, #ee5a6f 100%);">
              {{ topic.unreadCount > 99 ? '99+' : topic.unreadCount }}
            </div>
               
            <div class="relative w-10 h-10 rounded-xl bg-gradient-to-br from-green-500/10 to-emerald-500/10 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
              </svg>
            </div>
            <div class="flex flex-col overflow-hidden flex-1">
              <div class="flex justify-between items-center w-full">
                <span class="font-bold text-sm truncate text-primary-text">{{ topic.name }}</span>
                <span v-if="topic.messageCount !== undefined" 
                      class="text-[11px] font-bold shrink-0 ml-2 px-[8px] py-[3px] rounded-[10px]"
                      style="background-color: var(--accent-bg); color: var(--highlight-text); font-family: 'Arial Rounded MT Bold', 'Helvetica Rounded', Arial, sans-serif;">
                  {{ topic.messageCount }}
                </span>
              </div>
              <span class="text-[9px] opacity-40 truncate font-mono tracking-tighter">{{ topic.id }}</span>
            </div>
            
            <!-- 解锁状态标签 (桌面端还原) -->
            <div v-if="!topic.locked" class="absolute bottom-1 right-2 flex items-center gap-[2px] bg-black/5 dark:bg-white/10 px-1 py-[1px] rounded text-[9px] text-yellow-600 dark:text-yellow-400 border border-yellow-600/20 dark:border-yellow-400/20">
              <LockOpen :size="8" />
              <span class="scale-90 font-bold uppercase tracking-tighter leading-none pt-[1px]">Unlock</span>
            </div>
          </div>
        </template>
      </div>
      
      <!-- 底部: 动作区与设置 -->
      <div class="p-4 border-t border-black/5 dark:border-white/5 glass-panel shrink-0 space-y-3 pb-[calc(var(--vcp-safe-bottom,16px)+8px)]">
        <button v-if="activeTab === 'agents'" @click="$router.push('/agents')" 
          class="w-full py-2.5 bg-blue-500/10 dark:bg-blue-500/20 hover:bg-blue-500/20 dark:hover:bg-blue-500/30 text-blue-600 dark:text-blue-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
          创建 Agent
        </button>
        <button v-if="activeTab === 'topics'" @click="handleCreateTopic" class="w-full py-2.5 bg-green-500/10 dark:bg-green-500/20 hover:bg-green-500/20 dark:hover:bg-green-500/30 text-green-600 dark:text-green-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
          新建话题
        </button>
        
        <button class="w-full flex items-center justify-between p-3 bg-black/5 dark:bg-white/5 hover:bg-black/10 dark:hover:bg-white/10 active:scale-95 rounded-xl transition-all border border-black/5 dark:border-white/5 text-primary-text"
                @click="openSettings">
          <div class="flex items-center gap-3">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
            <span class="font-bold text-sm">全局设置</span>
          </div>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="opacity-30">
            <polyline points="9 18 15 12 9 6"></polyline>
          </svg>
        </button>
      </div>
    </aside>

    <!-- 4. 主视图层 (对话容器) -->
    <main class="vcp-main-stage flex-1 relative flex flex-col overflow-hidden w-full h-full z-10">
      <router-view v-slot="{ Component }">
        <keep-alive include="ChatView">
          <component v-if="Component" 
                     :is="Component" 
                     @toggle-left="toggleLeftDrawer" 
                     @toggle-right="toggleRightDrawer" />
        </keep-alive>
      </router-view>
    </main>

    <!-- 5. 右侧抽屉 -->
    <NotificationDrawer :is-open="isRightDrawerOpen" @close="isRightDrawerOpen = false" />

    <!-- 6. 全局 Toast 气泡容器 -->
    <ToastManager />

    <!-- 6. 全屏设置叠加层 (z-index 最高) -->
    <Transition name="slide-up">
      <div v-if="isSettingsOpen" class="fixed inset-0 z-[200]">
        <SettingsView @close="isSettingsOpen = false" @open-sync="isSettingsOpen = false; isSyncOpen = true" />
      </div>
    </Transition>

    <!-- 7. 全屏同步面板叠加层 -->
    <Transition name="slide-up">
      <div v-if="isSyncOpen" class="fixed inset-0 z-[200]">
        <SyncView @close="isSyncOpen = false" />
      </div>
    </Transition>

    <!-- 8. 全局右键 / 长按菜单 ActionSheet -->
    <BottomSheet
      v-model="isContextMenuOpen"
      :title="contextMenuTitle"
      :actions="contextMenuActions"
    />

    <!-- 9. VCP 风格输入弹窗 -->
    <VcpPrompt
      v-model:isOpen="isPromptOpen"
      :title="promptTitle"
      :initialValue="promptInitialValue"
      :placeholder="promptPlaceholder"
      @confirm="promptCallback"
    />

    <!-- 10. 双击退出提示 (Operation Aegis) -->
    <Transition name="toast-fade">
      <div v-if="showExitToast" 
           class="vcp-toast fixed bottom-10 left-1/2 -translate-x-1/2 px-6 py-3 rounded-full bg-gray-900/90 dark:bg-white/90 text-white dark:text-gray-900 text-sm font-bold shadow-2xl z-[9999] backdrop-blur-md flex items-center gap-2 border border-white/10 dark:border-black/10">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
          <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"></path>
          <polyline points="16 17 21 12 16 7"></polyline>
          <line x1="21" y1="12" x2="9" y2="12"></line>
        </svg>
        <span>再按一次退出应用</span>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* Toast Animation */
.toast-fade-enter-active, .toast-fade-leave-active { 
  transition: all 0.3s cubic-bezier(0.16, 1, 0.3, 1); 
}
.toast-fade-enter-from { 
  transform: translate(-50%, 20px) scale(0.9);
  opacity: 0; 
}
.toast-fade-leave-to { 
  transform: translate(-50%, 10px) scale(0.9);
  opacity: 0; 
}

.vcp-app-root {
  /* Remove primary-bg to allow wallpaper to show */
  background-color: transparent;
  /* 解决移动端视口高度问题 */
  height: 100dvh; 
}

@media (min-width: 768px) {
  .vcp-app-root {
    flex-direction: row;
  }
}

.vcp-background-layer {
  position: absolute;
  inset: 0;
  z-index: -1;
  background-size: cover;
  background-position: center;
  
  /* [修复移动端闪烁]: 强制开启独立合成层，并预告 opacity 变化，避免与滚动争用 GPU */
  transform: translateZ(0);
  will-change: opacity, transform;
  backface-visibility: hidden;
}

.vcp-drawer {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 82vw; 
  max-width: 340px;
  background-color: color-mix(in srgb, var(--secondary-bg) 85%, transparent);
  backdrop-filter: blur(20px) saturate(180%);
  -webkit-backdrop-filter: blur(20px) saturate(180%);
  transition: transform 0.4s cubic-bezier(0.16, 1, 0.3, 1);
}

.vcp-drawer-left {
  left: 0;
  transform: translateX(-100%);
  border-right: 1px solid transparent;
}

.vcp-drawer-left.is-open {
  transform: translateX(0);
}

.vcp-drawer-right {
  right: 0;
  transform: translateX(100%);
  border-left: 1px solid transparent;
}

.vcp-drawer-right.is-open {
  transform: translateX(0);
}

@media (min-width: 768px) {
  .vcp-drawer {
    position: relative;
    transform: translateX(0) !important;
    width: 280px;
    max-width: 280px;
    z-index: 10;
  }
  .vcp-drawer-left, .vcp-drawer-right {
    transition: none;
  }
}

.pt-safe { padding-top: var(--vcp-safe-top, 24px); }
.mb-safe { margin-bottom: var(--vcp-safe-bottom, 20px); }

.fade-enter-active, .fade-leave-active { transition: opacity 0.3s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; }

.bg-fade-enter-active, .bg-fade-leave-active { transition: opacity 1s ease-in-out; }
.bg-fade-enter-from, .bg-fade-leave-to { opacity: 0; }
.bg-fade-leave-active { position: absolute; }

.slide-up-enter-active, .slide-up-leave-active { transition: transform 0.5s cubic-bezier(0.16, 1, 0.3, 1); }
.slide-up-enter-from { transform: translateY(100%); }
.slide-up-leave-to { transform: translateY(100%); }

.page-fade-enter-active, .page-fade-leave-active { transition: opacity 0.2s ease; }
.page-fade-enter-from, .page-fade-leave-to { opacity: 0; }
</style>
