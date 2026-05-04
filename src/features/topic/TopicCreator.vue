<script setup lang="ts">
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useAssistantStore } from "../../core/stores/assistant";
import { useLayoutStore } from "../../core/stores/layout";

const topicStore = useTopicStore();
const sessionStore = useChatSessionStore();
const assistantStore = useAssistantStore();
const layoutStore = useLayoutStore();
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
) => {
  if (router.currentRoute.value.path !== "/chat") {
    await router.push("/chat");
  }

  // 使用统一的 sessionStore 选择话题，历史加载由 ChatView 的 watcher 响应
  await sessionStore.selectTopicById(itemId, topicId);

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

  if (!currentItemId.value) {
    window.alert("请先选择一个助手或群组");
    return;
  }

  isCreating.value = true;

  const newTopicName = `新话题 ${new Date().toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  })}`;

  try {
    const ownerType = assistantStore.agents.some((a) => a.id === currentItemId.value)
      ? "agent"
      : "group";

    const newTopic = await topicStore.createTopic(
      currentItemId.value,
      ownerType,
      newTopicName,
    );
    if (newTopic?.id) {
      await selectTopic(currentItemId.value, newTopic.id, newTopic.name);
    }
  } catch (error) {
    console.error("[TopicCreator] create-topic failed", error);
    // 错误通知已在 store 层处理
  } finally {
    // 1秒防抖/锁定，防止快速连击
    setTimeout(() => {
      isCreating.value = false;
    }, 1000);
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
