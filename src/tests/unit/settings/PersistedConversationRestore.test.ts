import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, disposePinia, type Pinia } from 'pinia';
import piniaPluginPersistedstate from 'pinia-plugin-persistedstate';
import { useAssistantStore } from '@/core/stores/assistant';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { useTopicStore, type Topic } from '@/core/stores/topicListManager';
import { useGuideStore } from '@/features/guide/stores/guideStore';
import { useDiaryStore } from '@/features/diary/diaryStore';
import type { AssistantsSnapshotDto, TopicDto } from '@/core/types/assistant';
import { invokeMock, mockInvoke, type MockChannel } from '@/tests/mocks/tauri';

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
});

afterEach(() => {
  for (const pinia of activePiniaInstances) disposePinia(pinia);
  activePiniaInstances.clear();
  localStorage.clear();
});

describe('real Pinia persisted-state conversation restoration', () => {
  it('repairs malformed preference leaves before consumers run and preserves valid references', () => {
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
    expect(restored.session.currentConversationKey).toEqual({
      ownerId, ownerType: 'group', topicId: 'group-topic', epoch: 0,
    });
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
    expect(restored.session.currentTopicId).toBe('deleted-topic');
    serveGroupTopics([groupTopic('available-topic')]);
    await restored.assistants.fetchAgentsAndGroups();
    restored.session.reconcileCurrentConversation();
    await restored.topics.loadTopicList(ownerId, 'group');
    expect(restored.session.currentTopicId).toBeNull();
    expect(restored.session.lastActiveTopicMap[`group:${ownerId}`]).toBeUndefined();
    expect(restored.topics.isTopicPinned(ownerId, 'group', 'deleted-topic')).toBe(false);

    await restored.session.selectItem({ ...restored.assistants.groups[0], type: 'group' });
    expect(restored.session.currentConversationKey?.topicId).toBe('available-topic');
    await nextTick();
    stopFrontend(restored.pinia);
    expect(startFrontend().session.currentTopicId).toBe('available-topic');
  });
});
