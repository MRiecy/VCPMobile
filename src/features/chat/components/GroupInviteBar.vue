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
    class="flex flex-col gap-1 px-2 pt-0.5 pb-1 select-none"
  >
    <div v-if="isJev" class="flex items-center justify-between gap-2 min-w-0 h-6.5">
      <div class="flex items-center gap-1.5 min-w-0 flex-1">
        <span
          class="w-1.5 h-1.5 rounded-full shrink-0 transition-all duration-300"
          :class="{
            'bg-amber-500 animate-pulse': streamStore.currentGroupTurn?.phase === 'deciding',
            'bg-emerald-500': streamStore.currentGroupTurn?.phase === 'speaking',
            'bg-[var(--primary-text)] opacity-25': streamStore.currentGroupTurn?.phase !== 'deciding' && streamStore.currentGroupTurn?.phase !== 'speaking'
          }"
          aria-hidden="true"
        />
        <span class="text-[11px] font-medium text-[var(--primary-text)] opacity-70 truncate" role="status" aria-live="polite">
          {{ statusLabel }}
        </span>
        <span
          v-if="streamStore.currentGroupTurn?.autonomousRounds"
          class="shrink-0 font-mono text-[10px] px-1 py-0.5 rounded bg-black/5 dark:bg-white/8 text-[var(--primary-text)] opacity-50"
        >
          第 {{ streamStore.currentGroupTurn.autonomousRounds }} 轮
        </span>
      </div>
      <template v-if="!isGenerating">
        <div class="flex items-center gap-1.5 shrink-0">
          <button
            type="button"
            class="h-6 px-2 rounded-md border border-[var(--border-color)] bg-[var(--secondary-bg)] hover:bg-[var(--primary-bg)] active:bg-blue-500/10 active:border-blue-500/30 text-[11px] font-medium text-[var(--primary-text)] disabled:opacity-40 transition-colors flex items-center gap-1 shrink-0"
            :disabled="controlsDisabled"
            aria-label="发起群聊"
            @click="controlSession('start')"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="opacity-70 shrink-0" aria-hidden="true">
              <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
            </svg>
            <span>发起群聊</span>
          </button>
          <button
            type="button"
            class="h-6 px-2 rounded-md border border-[var(--border-color)] bg-[var(--secondary-bg)] hover:bg-[var(--primary-bg)] active:bg-blue-500/10 active:border-blue-500/30 text-[11px] font-medium text-[var(--primary-text)] disabled:opacity-40 transition-colors flex items-center gap-1 shrink-0"
            :disabled="controlsDisabled"
            aria-label="继续群聊"
            @click="controlSession('continue')"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="opacity-70 shrink-0" aria-hidden="true">
              <path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67" />
            </svg>
            <span>继续群聊</span>
          </button>
        </div>
      </template>
    </div>
    <div class="flex items-center gap-1.5 overflow-x-auto scrollbar-hide py-0.5">
      <span class="text-[10px] font-bold text-[var(--primary-text)] opacity-40 shrink-0 pr-0.5 select-none tracking-wider">
        {{ isJev && isGenerating ? '排入发言' : '邀请发言' }}
      </span>
      <button
        v-for="member in members"
        :key="member.id"
        type="button"
        class="flex items-center gap-1.5 pl-1 pr-2.5 h-7 shrink-0 rounded-md border text-xs transition-colors"
        :class="{
          'border-blue-500/50 bg-blue-500/12 dark:bg-blue-500/20 text-blue-600 dark:text-blue-400 font-medium': currentSpeaker === member.id,
          'border-amber-500/40 bg-amber-500/8 text-[var(--primary-text)]': queuedMembers.has(member.id),
          'border-[var(--border-color)] bg-[var(--secondary-bg)] text-[var(--primary-text)] active:bg-blue-500/10 active:border-blue-500/30': currentSpeaker !== member.id && !queuedMembers.has(member.id),
          'opacity-40 pointer-events-none': controlsDisabled || (!isJev && isGenerating) || queuedMembers.has(member.id) || currentSpeaker === member.id
        }"
        :disabled="controlsDisabled || (!isJev && isGenerating) || queuedMembers.has(member.id) || currentSpeaker === member.id"
        :aria-label="currentSpeaker === member.id ? `${member.name}正在发言` : queuedMembers.has(member.id) ? `${member.name}等待发言` : `邀请${member.name}发言`"
        @click="invite(member.id)"
      >
        <VcpAvatar
          owner-type="agent"
          :owner-id="member.id"
          :fallback-name="member.name"
          size="w-5 h-5"
          rounded="rounded"
        />
        <span class="font-medium whitespace-nowrap">{{ member.name }}</span>
        <span v-if="currentSpeaker === member.id" class="text-[10px] text-blue-500 dark:text-blue-400 opacity-80 flex items-center gap-0.5">
          <span class="w-1 h-1 rounded-full bg-blue-500 inline-block animate-pulse" />
          发言中
        </span>
        <span v-else-if="queuedMembers.has(member.id)" class="text-[10px] text-[var(--secondary-text)]">
          等待 <span class="font-mono font-semibold text-amber-500/90">{{ queuedMembers.get(member.id) }}</span>
        </span>
      </button>
    </div>
  </div>
</template>
