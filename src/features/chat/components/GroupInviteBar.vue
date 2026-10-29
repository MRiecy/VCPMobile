<script setup lang="ts">
import { computed, ref } from 'vue';
import { useAssistantStore } from '../../../core/stores/assistant';
import { useChatSessionStore } from '../../../core/stores/chatSessionStore';
import { useChatHistoryStore } from '../../../core/stores/chatHistoryStore';
import { useChatStreamStore } from '../../../core/stores/chatStreamStore';
import VcpAvatar from '../../../components/ui/VcpAvatar.vue';

/**
 * 群组手动发言入口。JEV 运行中点按成员向 Rust owner 排队，
 * 阶段、轮数和待发言列表只呈现后端快照。
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
const isJev = computed(() => (streamStore.currentGroupTurn?.mode ?? currentGroup.value?.mode) === 'jev');

const members = computed(() => {
  const group = currentGroup.value;
  if (!group) return [];
  return group.members
    .map((id) => assistantStore.agents.find((a) => a.id === id))
    .filter((a): a is NonNullable<typeof a> => !!a);
});

const isGenerating = computed(() => streamStore.isGroupGenerating);
const pendingOperations = ref(new Set<string>());
const operationKey = computed(() => {
  const key = sessionStore.currentConversationKey;
  return key?.ownerType === 'group' ? JSON.stringify([key.ownerId, key.topicId]) : null;
});
const isPending = computed(() => !!operationKey.value && pendingOperations.value.has(operationKey.value));
const isPreparing = computed(() => {
  const key = sessionStore.currentConversationKey;
  return !!key && !isGenerating.value && streamStore.isConversationPreparing(key.ownerId, key.ownerType, key.topicId);
});
const controlsDisabled = computed(() => !operationKey.value || isPending.value || isPreparing.value);
const queuedMembers = computed(() => new Map((streamStore.currentGroupTurn?.queuedAgentIds ?? [])
  .map((id, index) => [id, index + 1])));
const currentSpeaker = computed(() => streamStore.currentGroupTurn?.currentAgentId);
const statusLabel = computed(() => {
  const turn = streamStore.currentGroupTurn;
  const currentName = members.value.find(member => member.id === turn?.currentAgentId)?.name;
  switch (turn?.phase) {
    case 'deciding': return '正在安排发言';
    case 'speaking': return currentName ? `${currentName} 正在发言` : '成员正在发言';
    case 'waiting': return '准备下一轮';
    case 'stopping': return '正在停止';
    default: return isGenerating.value ? '群聊进行中' : 'JEV 群聊';
  }
});

const perform = async (operation: () => Promise<unknown>) => {
  const key = operationKey.value;
  if (!key || controlsDisabled.value) return;
  pendingOperations.value.add(key);
  try { await operation(); }
  finally { pendingOperations.value.delete(key); }
};
const invite = (agentId: string) => {
  if ((!isJev.value && isGenerating.value) || queuedMembers.value.has(agentId) || currentSpeaker.value === agentId) return;
  void perform(() => historyStore.inviteGroupMember(agentId));
};
const controlSession = (action: 'start' | 'continue') => {
  if (isGenerating.value) return;
  void perform(() => historyStore.controlGroupSession(action));
};
</script>

<template>
  <div
    v-if="(isInviteOnly || isJev) && members.length > 0"
    class="flex flex-col gap-1 px-2 select-none"
  >
    <div v-if="isJev" class="flex items-center gap-2 min-w-0 text-xs text-primary-text">
      <span class="min-w-0 flex-1 truncate opacity-60" role="status" aria-live="polite">{{ statusLabel }}</span>
      <span v-if="streamStore.currentGroupTurn?.autonomousRounds" class="shrink-0 font-mono opacity-40">
        第 {{ streamStore.currentGroupTurn.autonomousRounds }} 轮
      </span>
      <template v-if="!isGenerating">
        <button
          type="button"
          class="shrink-0 px-2.5 py-1 rounded-lg border border-black/10 dark:border-white/10 bg-black/5 dark:bg-white/5 active:bg-blue-500/10 active:border-blue-500/30 text-xs font-semibold text-primary-text disabled:opacity-40 transition-colors"
          :disabled="controlsDisabled"
          aria-label="发起群聊"
          @click="controlSession('start')"
        >
          发起群聊
        </button>
        <button
          type="button"
          class="shrink-0 px-2.5 py-1 rounded-lg border border-black/10 dark:border-white/10 bg-black/5 dark:bg-white/5 active:bg-blue-500/10 active:border-blue-500/30 text-xs font-semibold text-primary-text disabled:opacity-40 transition-colors"
          :disabled="controlsDisabled"
          aria-label="继续群聊"
          @click="controlSession('continue')"
        >
          继续群聊
        </button>
      </template>
    </div>
    <div class="flex items-center gap-1.5 overflow-x-auto scrollbar-hide">
      <span class="text-[10px] font-bold text-primary-text opacity-40 shrink-0 pr-1 select-none">
        {{ isJev && isGenerating ? '排入发言' : '邀其发言' }}
      </span>
      <button
        v-for="member in members"
        :key="member.id"
        type="button"
        class="flex items-center gap-1.5 pl-1 pr-2.5 py-1 shrink-0 rounded-lg border border-black/10 dark:border-white/10 bg-black/5 dark:bg-white/5 active:bg-blue-500/10 active:border-blue-500/30 disabled:opacity-40 transition-colors"
        :disabled="controlsDisabled || (!isJev && isGenerating) || queuedMembers.has(member.id) || currentSpeaker === member.id"
        :aria-label="currentSpeaker === member.id ? `${member.name}正在发言` : queuedMembers.has(member.id) ? `${member.name}等待发言` : `邀请${member.name}发言`"
        @click="invite(member.id)"
      >
        <VcpAvatar owner-type="agent" :owner-id="member.id" :fallback-name="member.name" size="w-5 h-5" rounded="rounded-md" />
        <span class="text-xs font-semibold text-primary-text whitespace-nowrap">{{ member.name }}</span>
        <span v-if="currentSpeaker === member.id" class="text-[10px] text-primary-text opacity-50">发言中</span>
        <span v-else-if="queuedMembers.has(member.id)" class="text-[10px] text-primary-text opacity-50">待发言 <span class="font-mono">{{ queuedMembers.get(member.id) }}</span></span>
      </button>
    </div>
  </div>
</template>
