import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, disposePinia, getActivePinia, setActivePinia } from 'pinia';
import { useChatHistoryStore } from '@/core/stores/chatHistoryStore';
import { useChatSessionStore } from '@/core/stores/chatSessionStore';
import { installWindow, mockWindowRead, windowPage } from '@/tests/helpers/chatWindow';
import { invokeMock, mockInvoke } from '@/tests/mocks/tauri';

const owner = { id: 'a', type: 'agent' as const };
const row = (id: string, timestamp: number) => ({ id, timestamp, role: 'user', content: id });
beforeEach(() => setActivePinia(createPinia()));
afterEach(() => disposePinia(getActivePinia()!));

describe('accepted chat window', () => {
  it('holds the most recent anchored bubble until the user explicitly requests latest', async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    installWindow(session, owner, 't', 'a');
    mockWindowRead([row('a', 1)]);
    await history.ensureWindow();
    history.followingLatest = true;
    history.projectLiveMessage(session.currentConversationKey!, row('b', 2));
    expect(history.currentChatHistory.map(m => m.id)).toEqual(['a']);
    expect(history.hasEvictedNewer).toBe(true);
    const navigate = vi.spyOn(session, 'selectTopicById').mockImplementation(async () => {
      installWindow(session, owner, 't');
      return 'accepted';
    });
    mockWindowRead([row('a', 1), row('b', 2)]);
    await history.returnToLatest();
    expect(navigate).toHaveBeenCalledOnce();
    expect(history.anchored).toBe(false);
    expect(history.currentChatHistory.map(m => m.id)).toEqual(['a', 'b']);
  });

  it('coalesces mounting with navigation and ignores a previous anchor page in the same topic', async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    installWindow(session, owner, 't', 'old');
    let resolve!: (page: ReturnType<typeof windowPage>) => void;
    mockInvoke('read_chat_window', () => new Promise(done => { resolve = done; }));
    const mounting = history.ensureWindow();
    const navigating = history.ensureWindow();
    expect(invokeMock.mock.calls.filter(([cmd]) => cmd === 'read_chat_window')).toHaveLength(1);
    installWindow(session, owner, 't', 'new');
    mockWindowRead([row('new', 2)]);
    await history.ensureWindow();
    resolve(windowPage([row('old', 1)]));
    expect(await mounting).toMatchObject({ aborted: true });
    expect(await navigating).toMatchObject({ aborted: true });
    expect(history.currentChatHistory.map(m => m.id)).toEqual(['new']);
  });

  it('does not insert live rows into a retained older page that previously reached the newest row', async () => {
    const session = useChatSessionStore();
    const history = useChatHistoryStore();
    installWindow(session, owner, 't');
    mockWindowRead([row('tail', 2)], { hasMoreOlder: true });
    await history.ensureWindow();
    mockWindowRead([row('old', 1), row('tail', 2)], { pinned: true });
    await history.loadMoreHistory();
    expect(invokeMock).toHaveBeenCalledWith('read_chat_window', expect.objectContaining({
      older: true, before: { id: 'tail', timestamp: 2 },
    }));
    expect(history.hasEvictedNewer).toBe(false);
    history.projectLiveMessage(session.currentConversationKey!, row('new', 3));
    expect(history.currentChatHistory.map(m => m.id)).toEqual(['old', 'tail']);
    expect(history.hasEvictedNewer).toBe(true);
  });
});
