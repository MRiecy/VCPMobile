/**
 * mailStore.ts — clawEmail 邮箱状态机。
 *
 * 读：startSession 时 GET state（mailboxes/wsStates）→ 选中邮箱的列表；
 * 前台 45s 轮询 state（轻量热缓存），updatedAt 变化则重拉当前列表头部；
 * 退后台停轮询（复用 appLifecycleStore）。
 * 写：移入垃圾箱（软删除）；标读 = 读详情带 markRead=true（用户显式操作）。
 * 分页：limit=20 + start offset 增量加载。
 */
import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { useNotificationStore } from '../../core/stores/notification';
import { saveToDownloads } from '../../../src-tauri/plugins/vcp-mobile/guest-js';
import {
  addressingOf,
  normalizeDetail,
  normalizeFolders,
  normalizeMailboxes,
  normalizeMailList,
  normalizeWsStates,
  type FolderInfo,
  type MailboxInfo,
  type MailDetail,
  type MailSummary,
  type WsState,
} from './mailTypes';

const PAGE_SIZE = 20;
const POLL_MS = 45_000;

const PLUGIN_UNAVAILABLE_PREFIX = 'PLUGIN_UNAVAILABLE:';

function toMessage(raw: unknown): string {
  const message = raw instanceof Error ? raw.message : String(raw);
  return message.startsWith(PLUGIN_UNAVAILABLE_PREFIX)
    ? message.slice(PLUGIN_UNAVAILABLE_PREFIX.length)
    : message;
}

function isPluginUnavailable(raw: unknown): boolean {
  const message = raw instanceof Error ? raw.message : String(raw);
  return message.startsWith(PLUGIN_UNAVAILABLE_PREFIX);
}

export const useMailStore = defineStore('mail', () => {
  const notificationStore = useNotificationStore();

  const toast = (type: 'info' | 'success' | 'warning' | 'error', message: string) => {
    notificationStore.addNotification({
      type,
      title: '邮箱',
      message,
      toastOnly: true,
    });
  };

  // ---------- 状态 ----------
  const mailboxes = ref<MailboxInfo[]>([]);
  const wsStates = ref<WsState[]>([]);
  const serverError = ref<string | null>(null);
  /** state.updatedAt 指纹（轮询脏检查键）。 */
  const stateUpdatedAt = ref<string | null>(null);

  /** 当前选中邮箱（mailbox 槽位或 user 地址作 key）。 */
  const selectedKey = ref('');
  const mails = ref<MailSummary[]>([]);
  const unreadOnly = ref(false);

  const stateLoaded = ref(false);
  const isLoading = ref(false);
  const listLoading = ref(false);
  const loadingMore = ref(false);
  const error = ref<string | null>(null);
  const pluginUnavailable = ref(false);

  /** 详情（当前打开的邮件）。 */
  const detailMailId = ref<string | null>(null);
  const detail = ref<MailDetail | null>(null);
  const detailLoading = ref(false);
  const detailError = ref<string | null>(null);

  const trashing = ref(false);

  // ---------- V1.1：文件夹 / 搜索 / 发送 ----------
  const folders = ref<FolderInfo[]>([]);
  /** 服务器是否提供 folders/search 等补丁端点（404 时降级隐藏）。 */
  const extendedApiSupported = ref(true);
  /** 当前文件夹 fid（null = 默认收件箱）。 */
  const currentFid = ref<string | null>(null);
  /** 搜索模式：非 null 时列表展示搜索结果。 */
  const searchKeyword = ref('');
  const searchResults = ref<MailSummary[] | null>(null);
  const searching = ref(false);
  const sending = ref(false);

  /** 展示中的列表（搜索模式优先）。 */
  const displayedMails = computed(() => searchResults.value ?? mails.value);

  const selectedMailbox = computed(
    () => mailboxes.value.find((box) => keyOf(box) === selectedKey.value) ?? null,
  );

  const wsConnected = computed(() => {
    const box = selectedMailbox.value;
    if (!box) return false;
    return wsStates.value.find((state) => state.user === box.user)?.connected ?? false;
  });

  function keyOf(box: MailboxInfo): string {
    return box.mailbox.startsWith('mail') ? box.mailbox : box.user;
  }

  // ---------- 轮询引擎 ----------
  const sessionActive = ref(false);
  let pollTimer: ReturnType<typeof setTimeout> | null = null;

  function clearTimer(): void {
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  function scheduleNext(): void {
    clearTimer();
    const lifecycle = useAppLifecycleStore();
    if (!sessionActive.value || lifecycle.isBackground) return;
    pollTimer = setTimeout(async () => {
      await pollTick();
      scheduleNext();
    }, POLL_MS);
  }

  /** 轻量轮询：state（不穿透），updatedAt 变化则重拉当前列表。 */
  async function pollTick(): Promise<void> {
    try {
      const previous = stateUpdatedAt.value;
      const state = await loadState(false);
      const next = state && typeof state.updatedAt === 'string' ? state.updatedAt : null;
      if (state && previous && next && next !== previous) {
        await loadList(true);
      }
    } catch {
      // 轮询失败静默——下次再试；主动操作才会展示错误
    }
  }

  // ---------- 读 ----------
  /** 拉取 state；返回原始 payload 供轮询比对。 */
  async function loadState(refresh: boolean): Promise<Record<string, unknown> | null> {
    try {
      const payload = await invoke<Record<string, unknown>>('mail_state', { refresh });
      mailboxes.value = normalizeMailboxes(payload.mailboxes);
      wsStates.value = normalizeWsStates(payload.wsStates);
      serverError.value = typeof payload.lastError === 'string' ? payload.lastError : null;
      stateUpdatedAt.value = typeof payload.updatedAt === 'string' ? payload.updatedAt : null;
      stateLoaded.value = true;
      pluginUnavailable.value = false;

      // 默认选中第一个启用的邮箱
      if (!selectedKey.value && mailboxes.value.length > 0) {
        const first = mailboxes.value.find((box) => box.enabled) ?? mailboxes.value[0];
        selectedKey.value = keyOf(first);
      }
      return payload;
    } catch (raw) {
      pluginUnavailable.value = isPluginUnavailable(raw);
      error.value = toMessage(raw);
      return null;
    }
  }

  /** 拉取当前邮箱列表（reset=true 从头；否则 start 增量加载更多）。 */
  async function loadList(reset: boolean): Promise<void> {
    const box = selectedMailbox.value;
    if (!box) return;
    if (reset) {
      if (listLoading.value) return;
      listLoading.value = true;
    } else {
      if (loadingMore.value) return;
      loadingMore.value = true;
    }
    try {
      const start = reset ? 0 : mails.value.length;
      const payload = await invoke<Record<string, unknown>>('mail_list', {
        ...addressingOf(box),
        limit: PAGE_SIZE,
        start,
        unreadOnly: unreadOnly.value,
        fid: currentFid.value ?? undefined,
      });
      const batch = normalizeMailList(payload.emails);
      mails.value = reset ? batch : [...mails.value, ...batch];
      error.value = null;
    } catch (raw) {
      pluginUnavailable.value = pluginUnavailable.value || isPluginUnavailable(raw);
      error.value = toMessage(raw);
    } finally {
      listLoading.value = false;
      loadingMore.value = false;
    }
  }

  /** 手动刷新：state 穿透刷新 + 列表重拉。 */
  async function refresh(): Promise<void> {
    if (isLoading.value) return;
    isLoading.value = true;
    await loadState(true);
    await loadList(true);
    isLoading.value = false;
  }

  async function selectMailbox(key: string): Promise<void> {
    if (key === selectedKey.value) return;
    selectedKey.value = key;
    mails.value = [];
    folders.value = [];
    currentFid.value = null;
    clearSearch();
    await loadList(true);
    void loadFolders();
  }

  async function toggleUnreadOnly(): Promise<void> {
    unreadOnly.value = !unreadOnly.value;
    await loadList(true);
  }

  /** 打开详情（markRead=false——阅读不产生副作用）。 */
  async function openDetail(mailId: string): Promise<void> {
    const box = selectedMailbox.value;
    detailMailId.value = mailId;
    detail.value = null;
    detailError.value = null;
    if (!box) return;
    detailLoading.value = true;
    try {
      const payload = await invoke<Record<string, unknown>>('mail_read', {
        mailId,
        ...addressingOf(box),
        markRead: false,
      });
      detail.value = normalizeDetail(payload);
    } catch (raw) {
      detailError.value = toMessage(raw);
    } finally {
      detailLoading.value = false;
    }
  }

  function closeDetail(): void {
    detailMailId.value = null;
    detail.value = null;
    detailError.value = null;
  }

  /** 显式标记已读/未读（补丁端点 mail_mark）。 */
  async function setRead(read: boolean): Promise<void> {
    const box = selectedMailbox.value;
    const mailId = detailMailId.value;
    if (!box || !mailId) return;
    try {
      await invoke('mail_mark', {
        mailId,
        read,
        ...addressingOf(box),
      });
      const target = mails.value.find((mail) => mail.mailId === mailId);
      if (target) target.readState = read ? 'read' : 'unread';
      toast('success', read ? '已标记为已读' : '已标记为未读');
    } catch (raw) {
      toast('error', `标记失败：${toMessage(raw)}`);
    }
  }

  /** 移入垃圾箱（软删除）。 */
  async function trash(mailId: string): Promise<boolean> {
    const box = selectedMailbox.value;
    if (!box || trashing.value) return false;
    trashing.value = true;
    try {
      await invoke('mail_trash', { mailId, ...addressingOf(box) });
      toast('success', '已移入垃圾箱');
      closeDetail();
      await loadList(true);
      return true;
    } catch (raw) {
      toast('error', `操作失败：${toMessage(raw)}`);
      return false;
    } finally {
      trashing.value = false;
    }
  }

  // ---------- V1.1：文件夹 ----------
  /** 懒加载文件夹列表（404 = 服务器未打补丁，降级隐藏文件夹 UI）。 */
  async function loadFolders(): Promise<void> {
    const box = selectedMailbox.value;
    if (!box || !extendedApiSupported.value) return;
    try {
      const payload = await invoke<Record<string, unknown>>('mail_folders', {
        ...addressingOf(box),
      });
      folders.value = normalizeFolders(payload.folders);
    } catch (raw) {
      const message = toMessage(raw);
      if (message.includes('404') || message.includes('Cannot')) {
        extendedApiSupported.value = false;
      }
      console.warn('[Mail] 文件夹列表不可用:', message);
    }
  }

  async function selectFolder(fid: string | null): Promise<void> {
    if (fid === currentFid.value) return;
    currentFid.value = fid;
    clearSearch();
    await loadList(true);
  }

  // ---------- V1.1：搜索 ----------
  async function search(keyword: string): Promise<void> {
    const box = selectedMailbox.value;
    const trimmed = keyword.trim();
    if (!box || !trimmed || searching.value) return;
    searching.value = true;
    searchKeyword.value = trimmed;
    try {
      const payload = await invoke<Record<string, unknown>>('mail_search', {
        keyword: trimmed,
        ...addressingOf(box),
        limit: 50,
      });
      searchResults.value = normalizeMailList(payload.emails);
    } catch (raw) {
      const message = toMessage(raw);
      if (message.includes('404') || message.includes('Cannot')) {
        extendedApiSupported.value = false;
      }
      toast('error', `搜索失败：${message}`);
    } finally {
      searching.value = false;
    }
  }

  function clearSearch(): void {
    searchKeyword.value = '';
    searchResults.value = null;
  }

  // ---------- V1.1：发送 / 回复 ----------
  async function sendMail(draft: {
    to: string;
    cc?: string;
    bcc?: string;
    subject: string;
    body: string;
  }): Promise<boolean> {
    const box = selectedMailbox.value;
    if (!box || sending.value) return false;
    sending.value = true;
    try {
      await invoke('mail_send', { ...addressingOf(box), ...draft });
      toast('success', '邮件已发送');
      await loadList(true);
      return true;
    } catch (raw) {
      toast('error', `发送失败：${toMessage(raw)}`);
      return false;
    } finally {
      sending.value = false;
    }
  }

  async function replyMail(mailId: string, body: string): Promise<boolean> {
    const box = selectedMailbox.value;
    if (!box || sending.value) return false;
    sending.value = true;
    try {
      await invoke('mail_reply', { mailId, ...addressingOf(box), body });
      toast('success', '回复已发送');
      const target = mails.value.find((mail) => mail.mailId === mailId);
      if (target) target.readState = 'read';
      return true;
    } catch (raw) {
      toast('error', `回复失败：${toMessage(raw)}`);
      return false;
    } finally {
      sending.value = false;
    }
  }

  // ---------- V1.1：附件下载 ----------
  async function downloadAttachment(
    mailId: string,
    partId: string,
    filename: string,
  ): Promise<void> {
    const box = selectedMailbox.value;
    if (!box) return;
    try {
      const payload = await invoke<Record<string, unknown>>('mail_attachment', {
        mailId,
        partId,
        ...addressingOf(box),
      });
      const dataBase64 = String(payload.dataBase64 ?? '');
      const name = String(payload.filename ?? filename);
      if (!dataBase64) throw new Error('附件内容为空');
      const result = await saveToDownloads(
        name,
        dataBase64,
        typeof payload.contentType === 'string' ? payload.contentType : undefined,
      );
      toast('success', `附件已保存到下载目录：${result.displayName}`);
    } catch (raw) {
      toast('error', `附件下载失败：${toMessage(raw)}`);
    }
  }

  // ---------- 会话 ----------
  async function startSession(): Promise<void> {
    if (sessionActive.value) return;
    sessionActive.value = true;
    isLoading.value = true;
    await loadState(false);
    await loadList(true);
    void loadFolders();
    isLoading.value = false;
    scheduleNext();
  }

  function stopSession(): void {
    sessionActive.value = false;
    clearTimer();
  }

  function resetSession(): void {
    stopSession();
    mailboxes.value = [];
    wsStates.value = [];
    serverError.value = null;
    stateUpdatedAt.value = null;
    selectedKey.value = '';
    mails.value = [];
    unreadOnly.value = false;
    stateLoaded.value = false;
    isLoading.value = false;
    error.value = null;
    pluginUnavailable.value = false;
    folders.value = [];
    extendedApiSupported.value = true;
    currentFid.value = null;
    clearSearch();
    sending.value = false;
    closeDetail();
  }

  // ---------- 前后台感知 ----------
  const lifecycleStore = useAppLifecycleStore();
  watch(
    () => lifecycleStore.isBackground,
    (isBackground) => {
      if (!sessionActive.value) return;
      if (isBackground) {
        clearTimer();
      } else {
        void pollTick().then(scheduleNext);
      }
    },
  );

  return {
    mailboxes,
    wsStates,
    serverError,
    selectedKey,
    selectedMailbox,
    wsConnected,
    mails,
    unreadOnly,
    stateLoaded,
    isLoading,
    listLoading,
    loadingMore,
    error,
    pluginUnavailable,
    detailMailId,
    detail,
    detailLoading,
    detailError,
    trashing,
    folders,
    extendedApiSupported,
    currentFid,
    searchKeyword,
    searchResults,
    searching,
    sending,
    displayedMails,
    keyOf,
    loadState,
    loadList,
    refresh,
    selectMailbox,
    toggleUnreadOnly,
    openDetail,
    closeDetail,
    setRead,
    trash,
    loadFolders,
    selectFolder,
    search,
    clearSearch,
    sendMail,
    replyMail,
    downloadAttachment,
    startSession,
    stopSession,
    resetSession,
  };
});
