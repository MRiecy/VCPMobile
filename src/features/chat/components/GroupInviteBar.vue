<script setup lang="ts">
import { computed } from 'vue';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import VcpAvatar from '../../../components/ui/VcpAvatar.vue';

/**
 * 群组「邀请发言」横条（invite_only 模式核心入口）。
 *
 * 当前会话为 invite_only 群组时，在输入框上方展示成员 chip 列表；
 * 点按成员即触发其单人发言回合。
 */
const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const historyStore = useChatHistoryStore();
const streamStore = useChatStreamStore();

const currentGroup = computed(() => {
  const item = sessionStore.currentSelectedItem;
  if (!item || item.type !== 'group') return null;
  return assistantStore.groups.find((g) => g.id === item.id) ?? null;
});

const isInviteOnly = computed(() => currentGroup.value?.mode === 'invite_only');

const members = computed(() => {
  const group = currentGroup.value;
  if (!group) return [];
  return group.members
    .map((id) => assistantStore.agents.find((a) => a.id === id))
    .filter((a): a is NonNullable<typeof a> => !!a);
});

const isGenerating = computed(() => streamStore.isGroupGenerating);

const invite = (agentId: string) => {
  if (isGenerating.value) return;
  historyStore.inviteGroupMember(agentId);
};
</script>

<template>
  <div
    v-if="isInviteOnly && members.length > 0"
    class="flex items-center gap-1.5 px-2 overflow-x-auto scrollbar-hide"
    :class="{ 'opacity-50 pointer-events-none': isGenerating }"
  >
    <span class="text-[10px] font-bold text-primary-text opacity-40 shrink-0 pr-1 select-none">
      邀其发言
    </span>
    <button
      v-for="member in members"
      :key="member.id"
      class="flex items-center gap-1.5 pl-1 pr-2.5 py-1 shrink-0 rounded-lg border border-black/10 dark:border-white/10 bg-black/5 dark:bg-white/5 active:bg-blue-500/10 active:border-blue-500/30 transition-colors"
      :disabled="isGenerating"
      @click="invite(member.id)"
    >
      <VcpAvatar owner-type="agent" :owner-id="member.id" :fallback-name="member.name" size="w-5 h-5" rounded="rounded-full" />
      <span class="text-xs font-semibold text-primary-text whitespace-nowrap">{{ member.name }}</span>
    </button>
  </div>
</template>
