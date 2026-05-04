<script setup lang="ts">
import { computed, watch, ref, nextTick } from "vue";
import { useRouter } from "vue-router";
import { useVirtualList } from "@vueuse/core";
import { useTopicStore, type Topic } from "../../core/stores/topicListManager";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useAssistantStore } from "../../core/stores/assistant";
import { useLayoutStore } from "../../core/stores/layout";
import { useOverlayStore } from "../../core/stores/overlay";
import { Edit3, Lock, LockOpen, CheckCircle, Trash2 } from "lucide-vue-next";

const emit = defineEmits<{
  (e: "select-topic"): void;
}>();

const topicListStore = useTopicStore();
const sessionStore = useChatSessionStore();
const assistantStore = useAssistantStore();
const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const router = useRouter();

type TopicViewModel = Topic & { pinned?: boolean; updatedAt?: number };

const currentTopics = computed<TopicViewModel[]>(() => {
  return topicListStore.filteredTopics as TopicViewModel[];
});

// 虚拟列表实现
const { list, containerProps, wrapperProps } = useVirtualList(currentTopics, {
  itemHeight: 74, // 10(h-10) + padding/margins, 约 74px
});

// 拦截容器引用，用于数据变化时手动控制滚动位置
const scrollContainerRef = ref<HTMLElement | null>(null);
const bindContainerRef = (el: unknown) => {
  const htmlEl = el as HTMLElement | null;
  containerProps.ref.value = htmlEl;
  scrollContainerRef.value = htmlEl;
};

// 新建话题后自动滚动到顶部，强制虚拟列表重新计算并让用户看到新话题
watch(
  () => topicListStore.topics.length,
  async (newLen, oldLen) => {
    if (newLen > oldLen && scrollContainerRef.value) {
      await nextTick();
      scrollContainerRef.value.scrollTop = 0;
    }
  },
);

const showTopicContextMenu = (topicId: string) => {
  // 每次打开菜单时，从 store 中获取最新的 topic 状态，避免闭包捕获旧状态
  const topic = topicListStore.topics.find((t) => t.id === topicId);
  if (!topic) return;

  const itemId =
    topic.ownerId ||
    topicListStore.currentAgentId ||
    sessionStore.currentSelectedItem?.id ||
    "default_agent";
  const ownerType = assistantStore.agents.some((a) => a.id === itemId)
    ? "agent"
    : "group";

  const menuItems: any[] = [
    {
      label: "修改标题",
      icon: Edit3,
      handler: () => {
        overlayStore.openPrompt({
          title: "修改话题标题",
          initialValue: topic.name,
          placeholder: "请输入新的话题标题...",
          onConfirm: (newTitle: string) => {
            if (newTitle && newTitle.trim()) {
              topicListStore.updateTopicTitle(
                itemId,
                ownerType,
                topic.id,
                newTitle.trim(),
              );
            }
          },
        });
      },
    },
  ];

  // 仅在 Agent 模式下显示锁定和未读切换（Group 模式固定为 Locked/Read）
  if (ownerType === "agent") {
    menuItems.push(
      {
        label: topic.locked ? "解锁话题" : "锁定话题",
        icon: topic.locked ? LockOpen : Lock,
        handler: () => {
          topicListStore.toggleTopicLock(itemId, ownerType, topic.id);
        },
      },
      {
        label: topic.unread ? "标为已读" : "标为未读",
        icon: CheckCircle,
        handler: () => {
          topicListStore.setTopicUnread(
            itemId,
            ownerType,
            topic.id,
            !topic.unread,
          );
        },
      },
    );
  }

  menuItems.push({
    label: "删除话题",
    icon: Trash2,
    danger: true,
    handler: () => {
      if (
        window.confirm(`确定要删除话题 "${topic.name}" 吗？此操作不可逆转。`)
      ) {
        if (window.confirm(`【最终确认】真的要永久删除 "${topic.name}" 吗？`)) {
          topicListStore.deleteTopic(itemId, ownerType, topic.id);
        }
      }
    },
  });

  overlayStore.openContextMenu(menuItems, "Topic Options");
};

// 兜底同步：当聊天上下文的选中项变化时，自动重新加载对应 Agent/Group 的话题列表
watch(
  () => sessionStore.currentSelectedItem?.id,
  (newId) => {
    if (newId) {
      const ownerType = assistantStore.agents.some((a) => a.id === newId)
        ? "agent"
        : "group";
      topicListStore.loadTopicList(newId, ownerType);
    }
  },
  { immediate: true },
);

const selectTopic = async (
  itemId: string,
  topicId: string,
  topicName: string,
) => {
  if (router.currentRoute.value.path !== "/chat") {
    await router.push("/chat");
  }

  await sessionStore.selectTopicById(itemId, topicId);

  // 顶部栏显示话题标题
  if (sessionStore.currentSelectedItem) {
    sessionStore.currentSelectedItem.name = topicName;
  }

  // 在移动端，选择话题后自动关闭侧边栏
  if (window.innerWidth < 768) {
    layoutStore.setLeftDrawer(false);
  }

  emit("select-topic");
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

  <div v-else :ref="bindContainerRef" :style="containerProps.style" @scroll="containerProps.onScroll" class="h-full overflow-y-auto vcp-scrollable px-4 py-4">
    <div v-bind="wrapperProps" class="flex flex-col">
      <div v-for="item in list" :key="item.data.id" class="pb-2" @click="
        selectTopic(
          item.data.ownerId || sessionStore.currentSelectedItem?.id || 'default_agent',
          item.data.id,
          item.data.name,
        )
        " v-longpress="() => showTopicContextMenu(item.data.id)">
        <div class="relative p-3 glass-panel rounded-xl flex items-center gap-3 active:scale-95 transition-all border shadow-sm cursor-pointer hover:bg-black/5 dark:hover:bg-white/5"
          :class="sessionStore.currentTopicId === item.data.id
              ? 'border-green-500/50 bg-green-500/10 dark:bg-green-500/20'
              : 'border-black/5 dark:border-white/5'
            ">
          <!-- 未读小红点 / 计数角标 (基于桌面端主题同步) -->
          <div v-if="item.data.unreadCount === -1 || item.data.unread"
            class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm shrink-0"
            style="background: #ff6b6b"></div>
          <div v-else-if="item.data.unreadCount && item.data.unreadCount > 0"
            class="absolute -top-1.5 -right-1.5 min-w-[18px] h-[18px] px-1 rounded-full border-2 border-white dark:border-gray-900 text-[9px] font-bold text-white flex items-center justify-center z-10 shadow-sm"
            style="background: linear-gradient(135deg, #ff6b6b 0%, #ee5a6f 100%)">
            {{ item.data.unreadCount > 99 ? "99+" : item.data.unreadCount }}
          </div>

          <div
            class="relative w-10 h-10 rounded-xl bg-gradient-to-br from-green-500/10 to-emerald-500/10 flex items-center justify-center shrink-0 border border-black/10 dark:border-white/10">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
            </svg>
          </div>
          <div class="flex flex-col overflow-hidden flex-1">
            <div class="flex justify-between items-center w-full">
              <span class="font-bold text-sm truncate text-primary-text">{{
                item.data.name
                }}</span>
              <span v-if="item.data.msgCount !== undefined"
                class="text-[11px] font-bold shrink-0 ml-2 px-[8px] py-[3px] rounded-[10px]" style="
                  background-color: var(--accent-bg);
                  color: var(--highlight-text);
                  font-family: 'Arial Rounded MT Bold', 'Helvetica Rounded', Arial, sans-serif;
                ">
                {{ item.data.msgCount }}
              </span>
            </div>
            <span class="text-[9px] opacity-40 truncate font-mono tracking-tighter">{{
              item.data.id
              }}</span>
          </div>

          <!-- 解锁状态标签 (桌面端还原) -->
          <div v-if="!item.data.locked"
            class="absolute bottom-1 right-2 flex items-center gap-[2px] bg-black/5 dark:bg-white/10 px-1 py-[1px] rounded text-[9px] text-yellow-600 dark:text-yellow-400 border border-yellow-600/20 dark:border-yellow-400/20">
            <LockOpen :size="8" />
            <span class="scale-90 font-bold uppercase tracking-tighter leading-none pt-[1px]">Unlock</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
