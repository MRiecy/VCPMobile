import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, disposePinia, type Pinia } from 'pinia';
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate';
import { useAssistantStore } from '@/core/stores/assistant';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useTopicStore, type Topic } from '@/core/stores/topicListManager';
import { useOverlayStore } from '@/core/stores/overlay';
import { checkAndPromptPendingConfigRestore } from '@/core/stores/syncSession';
import { applyImport, writeStagedEntityBackup } from '@/features/settings/useConfigBackup';
import { useGuideStore } from '@/features/guide/stores/guideStore';
import { useDiaryStore } from '@/features/diary/diaryStore';
import type { AssistantsSnapshotDto, TopicDto } from '@/core/types/assistant';
import { invokeMock, mockInvoke, type MockChannel } from '@/tests/mocks/tauri';
import { makeConfigSnapshot } from '@/tests/helpers/config';

const activePiniaInstances = new Set<Pinia>();
const ownerId = 'shared-owner-id';

function startFrontend() {
  // Use the same real plugin registration order as main.ts, without mounting
  // the application or replacing its stores with testing doubles.
  const app = createApp({ render: () => null });
  const pinia = createPinia();
  pinia.use(piniaPluginPersistedstate);
  app.use(pinia);
  activePiniaInstances.add(pinia);
  return {
    pinia,
    assistants: useAssistantStore(pinia),
    session: useChatSessionStore(pinia),
    topics: useTopicStore(pinia),
  };
}

function stopFrontend(pinia: Pinia) {
  disposePinia(pinia);
  activePiniaInstances.delete(pinia);
}

function ownerSnapshot(groupName = 'Saved group'): AssistantsSnapshotDto {
  return {
    agents: [{ id: ownerId, name: 'Agent', model: 'fixture-model', avatarCalculatedColor: null }],
    groups: [{ id: ownerId, name: groupName, members: [], mode: 'sequential', avatarCalculatedColor: null }],
    unreadCounts: {},
  };
}

function groupTopic(id: string, name = id): Topic {
  return {
    id, name, ownerId, ownerType: 'group', createdAt: 1, updatedAt: 2,
    locked: false, unread: false, unreadCount: 0, msgCount: 0,
  };
}

function serveGroupTopics(topics: TopicDto[]) {
  mockInvoke('get_topics_streamed', args => {
    const channel = args?.onChunk as MockChannel<TopicDto[]>;
    channel.emit(topics);
  });
}

beforeEach(() => {
  localStorage.clear();
  mockInvoke('get_assistants_snapshot', () => ownerSnapshot());
  let revision = 0;
  const empty = { target: { kind: 'welcome' }, focusRevision: 0, windowRevision: 0, anchorId: null };
  mockInvoke('connect_chat_view', () => ({ documentId: 1, focus: empty }));
  mockInvoke('register_chat_navigation_intent', () => ({ focus: empty }));
  mockInvoke('request_chat_navigation', args => ({
    status: 'accepted', focus: { target: args?.target, focusRevision: ++revision, windowRevision: revision, anchorId: args?.anchorId ?? null },
  }));
  mockInvoke('restore_chat_view', args => ({
    status: 'accepted', focus: { target: args?.hint, focusRevision: ++revision, windowRevision: revision, anchorId: null },
  }));
});

afterEach(() => {
  for (const pinia of activePiniaInstances) disposePinia(pinia);
  activePiniaInstances.clear();
  localStorage.clear();
});

describe('real Pinia persisted-state conversation restoration', () => {
  it.each([
    { entry: 'import', ownerType: 'agent' },
    { entry: 'import', ownerType: 'group' },
    { entry: 'staged', ownerType: 'agent' },
    { entry: 'staged', ownerType: 'group' },
  ] as const)('refreshes the current $ownerType after $entry without changing the accepted conversation', async ({ entry, ownerType }) => {
    const { pinia, assistants, session } = startFrontend();
    await assistants.fetchAgentsAndGroups();
    await session.selectTopicById(ownerId, ownerType, 'active-topic', undefined, 'kept-anchor');
    const conversation = session.currentConversationKey;
    const window = session.acceptedWindow;
    const epoch = session.sessionEpoch;
    const snapshot = makeConfigSnapshot();
    mockInvoke('read_settings', () => snapshot);
    mockInvoke('import_settings_backup', () => ({
      result: { status: 'saved', snapshot }, missingGroups: false, missingAgents: false,
    }));
    const refreshed = ownerSnapshot('Refreshed group');
    refreshed.agents[0].name = 'Refreshed agent';
    mockInvoke('get_assistants_snapshot', () => refreshed);
    const preferences = ownerType === 'agent'
      ? { agentPreferences: { [ownerId]: { mobileSystemPrompt: 'restored' } } }
      : { groupPreferences: { [ownerId]: { contextMessageWindowSize: 60 } } };

    if (entry === 'import') {
      const result = await applyImport({ domains: {}, exportedAt: '', ...preferences });
      expect(result.failed).toEqual([]);
    } else {
      writeStagedEntityBackup(preferences);
      vi.spyOn(useOverlayStore(pinia), 'showConfirm').mockResolvedValue(true);
      expect(await checkAndPromptPendingConfigRestore()).toBe(true);
    }

    expect(session.currentSelectedItem?.name).toBe(`Refreshed ${ownerType}`);
    expect(session.headerTitle).toBe(`Refreshed ${ownerType}`);
    expect(session.currentConversationKey).toBe(conversation);
    expect(session.acceptedWindow).toBe(window);
    expect(session.sessionEpoch).toBe(epoch);
    expect(invokeMock.mock.calls.filter(([name]) => name === 'request_chat_navigation')).toHaveLength(1);
  });

  it('restores the admitted call topic and anchor ahead of a stale local preference', async () => {
    localStorage.setItem('chatSession', JSON.stringify({
      currentSelectedItem: { id: ownerId, type: 'group' }, currentTopicId: 'stale', lastActiveTopicMap: {},
    }));
    mockInvoke('restore_chat_view', args => {
      expect(args?.hint).toMatchObject({ topicId: 'stale' });
      return { status: 'accepted', focus: { target: { kind: 'topic', ownerId, ownerType: 'agent', topicId: 'call' },
        anchorId: 'older-message', focusRevision: 3, windowRevision: 8 } };
    });
    const { session } = startFrontend();
    await session.restoreFocus();
    expect(session.currentConversationKey).toMatchObject({ ownerId, ownerType: 'agent', topicId: 'call' });
    expect(session.acceptedWindow).toMatchObject({ focusRevision: 3, windowRevision: 8, anchorId: 'older-message' });
    expect(invokeMock.mock.calls.some(([name]) => name === 'end_voice_conversation')).toBe(false);
  });

  it('projects the committed focus captured by a newer declined intent while the older reply is delayed', async () => {
    const { session } = startFrontend();
    const focus = { target: { kind: 'topic', ownerId, ownerType: 'group', topicId: 'committed' },
      focusRevision: 1, windowRevision: 1, anchorId: 'kept' };
    let finish!: (outcome: unknown) => void;
    mockInvoke('request_chat_navigation', args => args?.sequence === 1
      ? new Promise(resolve => { finish = resolve; }) : { status: 'call_in_progress' });
    const older = session.selectTopicById(ownerId, 'group', 'committed');
    await vi.waitFor(() => expect(finish).toBeDefined());
    mockInvoke('register_chat_navigation_intent', () => ({ focus }));
    await expect(session.selectTopicById(ownerId, 'group', 'blocked')).resolves.toBe('call_in_progress');
    expect(session.currentTopicId).toBe('committed');
    const window = session.acceptedWindow;
    finish({ status: 'accepted', focus });
    await expect(older).resolves.toBe('stale');
    expect(session.acceptedWindow).toBe(window);
  });

  it('repairs malformed preference leaves and waits for Rust before projecting a saved topic', async () => {
    const pinned = JSON.stringify(['group', ownerId, 'saved-topic']);
    localStorage.setItem('chatSession', JSON.stringify({
      currentSelectedItem: { id: ownerId, type: 'group', name: false },
      currentTopicId: 'saved-topic',
      lastActiveTopicMap: { [`group:${ownerId}`]: 'saved-topic', 'agent:bad': null },
    }));
    localStorage.setItem('topic', JSON.stringify({ sortMode: false, pinnedTopicKeys: [pinned] }));
    localStorage.setItem('guide', JSON.stringify({ completed: null, lastSeenAppVersion: '1.1.6' }));
    localStorage.setItem('diary', JSON.stringify({ hiddenFolders: ['kept'], collapsedCategories: ['unknown'], folderOrder: 4 }));
    const restored = startFrontend();
    const guide = useGuideStore(restored.pinia);
    const diary = useDiaryStore(restored.pinia);
    expect(restored.session.currentConversationKey).toBeNull();
    await restored.session.restoreFocus();
    expect(restored.session.currentConversationKey?.topicId).toBe('saved-topic');
    expect(restored.session.lastActiveTopicMap).toEqual({ [`group:${ownerId}`]: 'saved-topic' });
    expect(restored.topics.sortMode).toBe('created');
    expect(restored.topics.pinnedTopicKeys).toEqual([pinned]);
    expect(guide.isCompleted('anything')).toBe(false);
    expect(guide.lastSeenAppVersion).toBe('1.1.6');
    expect(diary.visibleFolders).toEqual([]);
    expect(diary.hiddenFolders).toEqual(['kept']);
    expect(diary.collapsedCategories).toEqual([]);
    expect(diary.folderOrder).toEqual([]);
    expect(JSON.parse(localStorage.getItem('guide')!)).toEqual({ completed: [], lastSeenAppVersion: '1.1.6' });
    expect(JSON.parse(localStorage.getItem('diary')!)).toEqual({ hiddenFolders: ['kept'], collapsedCategories: [], folderOrder: [] });
  });

  it('persists current defaults for damaged JSON without persisting runtime or draft fields', () => {
    for (const owner of ['chatSession', 'topic', 'guide', 'diary']) localStorage.setItem(owner, '{broken');
    const restored = startFrontend();
    const guide = useGuideStore(restored.pinia);
    const diary = useDiaryStore(restored.pinia);
    expect(restored.session.currentSelectedItem).toBeNull();
    expect(restored.session.currentTopicId).toBeNull();
    expect(restored.session.lastActiveTopicMap).toEqual({});
    expect(restored.topics.sortMode).toBe('created');
    expect(restored.topics.pinnedTopicKeys).toEqual([]);
    expect(guide.completed).toEqual([]);
    expect(guide.lastSeenAppVersion).toBeNull();
    expect(diary.visibleFolders).toEqual([]);
    expect(Object.keys(JSON.parse(localStorage.getItem('chatSession')!))).toEqual(['currentSelectedItem', 'currentTopicId', 'lastActiveTopicMap']);
    expect(Object.keys(JSON.parse(localStorage.getItem('diary')!))).toEqual(['hiddenFolders', 'collapsedCategories', 'folderOrder']);
  });

  it('restores owner-scoped selection and preferences, then reconciles with fresh server data', async () => {
    const first = startFrontend();
    await first.assistants.fetchAgentsAndGroups();
    await first.session.selectTopicById(ownerId, 'agent', 'agent-topic');
    await first.session.selectTopicById(ownerId, 'group', 'group-topic');
    first.topics.setSortMode('updated');
    first.topics.toggleTopicPinned(ownerId, 'agent', 'agent-topic');
    first.topics.toggleTopicPinned(ownerId, 'group', 'group-topic');
    first.topics.topics = [groupTopic('transient-topic')];
    first.topics.searchTerm = 'transient search';
    first.topics.loading = true;
    expect(first.session.sessionEpoch).toBeGreaterThan(0);
    await nextTick();
    stopFrontend(first.pinia);

    const restored = startFrontend();
    expect(restored.session.currentConversationKey).toBeNull();
    await restored.session.restoreFocus();
    expect(restored.session.currentConversationKey).toMatchObject({ ownerId, ownerType: 'group', topicId: 'group-topic' });
    expect(restored.session.lastActiveTopicMap).toEqual({
      [`agent:${ownerId}`]: 'agent-topic',
      [`group:${ownerId}`]: 'group-topic',
    });
    expect(restored.topics.effectiveSortMode).toBe('updated');
    expect(restored.topics.isTopicPinned(ownerId, 'agent', 'agent-topic')).toBe(true);
    expect(restored.topics.isTopicPinned(ownerId, 'group', 'group-topic')).toBe(true);
    expect(restored.topics.isTopicPinned(ownerId, 'group', 'agent-topic')).toBe(false);
    expect(restored.topics.topics).toEqual([]);
    expect(restored.topics.searchTerm).toBe('');
    expect(restored.topics.loading).toBe(false);
    expect(restored.assistants.groups).toEqual([]);

    mockInvoke('get_assistants_snapshot', () => ownerSnapshot('Renamed on server'));
    serveGroupTopics([groupTopic('group-topic', 'Current server topic')]);
    await restored.assistants.fetchAgentsAndGroups();
    restored.session.reconcileCurrentConversation();
    await restored.topics.loadTopicList(ownerId, 'group');
    expect(restored.session.currentSelectedItem?.name).toBe('Renamed on server');
    expect(restored.session.headerTitle).toBe('Current server topic');
    expect(restored.topics.topicSections.pinned.map(topic => topic.id)).toEqual(['group-topic']);
    expect(invokeMock).toHaveBeenCalledWith('get_topics_streamed', expect.objectContaining({
      ownerId, ownerType: 'group', sortMode: 'updated',
    }));
  });

  it('does not resurrect a restored topic that the authoritative list has deleted', async () => {
    const first = startFrontend();
    await first.assistants.fetchAgentsAndGroups();
    await first.session.selectTopicById(ownerId, 'group', 'deleted-topic');
    first.topics.toggleTopicPinned(ownerId, 'group', 'deleted-topic');
    await nextTick();
    stopFrontend(first.pinia);

    const restored = startFrontend();
    expect(restored.session.currentTopicId).toBeNull();
    mockInvoke('restore_chat_view', () => ({ status: 'accepted',
      focus: { target: { kind: 'owner', ownerId, ownerType: 'group' }, focusRevision: 1, windowRevision: 1, anchorId: null },
    }));
    await restored.session.restoreFocus();
    serveGroupTopics([groupTopic('available-topic')]);
    await restored.assistants.fetchAgentsAndGroups();
    restored.session.reconcileCurrentConversation();
    await restored.topics.loadTopicList(ownerId, 'group');
    expect(restored.session.currentTopicId).toBeNull();
    expect(restored.session.lastActiveTopicMap[`group:${ownerId}`]).toBeUndefined();
    expect(restored.topics.isTopicPinned(ownerId, 'group', 'deleted-topic')).toBe(false);

    mockInvoke('request_chat_navigation', args => {
      expect(args).toMatchObject({ target: { kind: 'owner', ownerId, ownerType: 'group' }, selectRecent: true });
      expect(args?.rememberedTopicId).toBeUndefined();
      return { status: 'accepted', focus: { target: { kind: 'topic', ownerId, ownerType: 'group', topicId: 'available-topic' },
        focusRevision: 2, windowRevision: 2, anchorId: null } };
    });
    await restored.session.selectItem({ ...restored.assistants.groups[0], type: 'group' });
    expect(restored.session.currentConversationKey?.topicId).toBe('available-topic');
    await nextTick();
    stopFrontend(restored.pinia);
    expect(startFrontend().session.coldHint.currentTopicId).toBe('available-topic');
  });
});
