<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useAssistantStore } from "../../core/stores/assistant";
import { useLayoutStore } from "../../core/stores/layout";
import { useNotificationStore } from "../../core/stores/notification";

const topicStore = useTopicStore();
const sessionStore = useChatSessionStore();
const assistantStore = useAssistantStore();
const layoutStore = useLayoutStore();
const notificationStore = useNotificationStore();
const router = useRouter();

const isCreating = ref(false);

const currentItemId = computed(
  () =>
    sessionStore.currentSelectedItem?.id || assistantStore.agents[0]?.id || null,
);
const canCreateTopic = computed(
  () => Boolean(currentItemId.value) && !isCreating.value,
);

const selectTopic = async (
  itemId: string,
  topicId: string,
  topicName: string,
  ownerType: string,
  expectedEpoch: number,
  allowUnselectedOwner: boolean,
) => {
  if (router.currentRoute.value.path !== "/chat") {
    await router.push("/chat");
  }

  const currentOwner = sessionStore.currentSelectedItem;
  if (
    sessionStore.sessionEpoch !== expectedEpoch ||
    (currentOwner
      ? currentOwner.id !== itemId || currentOwner.type !== ownerType
      : !allowUnselectedOwner)
  ) {
    return;
  }

  // 使用统一的 sessionStore 选择话题，历史加载由 ChatView 的 watcher 响应
  await sessionStore.selectTopicById(
    itemId,
    ownerType as "agent" | "group",
    topicId,
  );

  const createdTopic = topicStore.topics.find((topic) => topic.id === topicId);
  if (createdTopic) {
    createdTopic.name = topicName;
  }

  layoutStore.setLeftDrawer(false);
};

const handleCreateTopic = async () => {
  if (isCreating.value) return;

  console.info(
    "[TopicCreator] create-topic clicked",
    sessionStore.currentSelectedItem,
  );

  const owner = sessionStore.currentSelectedItem;
  const allowUnselectedOwner = !owner;
  const ownerId = owner?.id || currentItemId.value;
  if (!ownerId) {
    notificationStore.addNotification({
      type: 'warning',
      message: '请先选择一个助手或群组',
      toastOnly: true
    });
    return;
  }

  isCreating.value = true;
  const ownerType = owner?.type || (
    assistantStore.agents.some((a) => a.id === ownerId) ? "agent" : "group"
  );
  const selectionEpoch = sessionStore.sessionEpoch;

  const newTopicName = `新话题 ${new Date().toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  })}`;

  try {
    const newTopic = await topicStore.createTopic(
      ownerId,
      ownerType,
      newTopicName,
    );
    if (
      newTopic?.id &&
      sessionStore.sessionEpoch === selectionEpoch &&
      (sessionStore.currentSelectedItem
        ? sessionStore.currentSelectedItem.id === ownerId &&
          sessionStore.currentSelectedItem.type === ownerType
        : allowUnselectedOwner)
    ) {
      await selectTopic(
        ownerId,
        newTopic.id,
        newTopic.name,
        ownerType,
        selectionEpoch,
        allowUnselectedOwner,
      );
    }
  } catch (error) {
    console.error("[TopicCreator] create-topic failed", error);
    // 错误通知已在 store 层处理
  } finally {
    isCreating.value = false;
  }
};
</script>

<template>
  <button
    class="w-full py-2.5 bg-green-500/10 dark:bg-green-500/20 hover:bg-green-500/20 dark:hover:bg-green-500/30 text-green-600 dark:text-green-400 rounded-xl text-sm font-bold transition-all flex items-center justify-center gap-2 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-green-500/10 disabled:dark:hover:bg-green-500/20"
    :disabled="!canCreateTopic" @click="handleCreateTopic">
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <line x1="12" y1="5" x2="12" y2="19"></line>
      <line x1="5" y1="12" x2="19" y2="12"></line>
    </svg>
    新建话题
  </button>
</template>
