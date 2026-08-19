/**
 * globalSearchStore.ts — 全局消息搜索 Store。
 *
 * 设计要点：
 * 1. 纯本地 FTS 检索（search_messages_fts），无远程依赖、离线可用；
 * 2. generation 竞态防护：每次新搜索递增 generation，迟到响应直接丢弃
 *    （本地 SQLite 毫秒级返回，无需日记那套远程 cancel command）；
 * 3. keyset 分页：时间倒序模式下以末条 (timestamp, msgId) 为游标加载更多；
 * 4. 索引构建：首开搜索页时检测覆盖率（决策 G），不足则触发 rebuild_messages_fts
 *    并监听 vcp-system-event / vcp-fts-rebuild 进度事件驱动进度 UI；
 * 5. 进度事件监听为模块级单例注册，避免重复 listen。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  SEARCH_PAGE_SIZE,
  SEARCH_MIN_CHARS,
  type FtsIndexStatus,
  type FtsSearchResultItem,
  type RoleFilter,
  type SearchScope,
  type SortMode,
  type TimeFilter,
} from './types';

export const useGlobalSearchStore = defineStore('globalSearch', () => {
  // ---------- 输入与过滤器 ----------
  const query = ref('');
  const scope = ref<SearchScope>('all');
  /** scope='topic' 时的目标话题（打开时可由外部预填） */
  const scopeTopicId = ref<string | null>(null);
  const scopeTopicTitle = ref<string>('');
  /** scope='owner' 时的会话归属（决策 B：经 topics.owner 过滤） */
  const scopeOwnerId = ref<string | null>(null);
  const scopeOwnerType = ref<'agent' | 'group' | null>(null);
  const scopeOwnerLabel = ref<string>('');
  const role = ref<RoleFilter>('all');
  const timeRange = ref<TimeFilter>('all');
  const sort = ref<SortMode>('time');

  // ---------- 结果与状态 ----------
  const results = ref<FtsSearchResultItem[]>([]);
  const searching = ref(false);
  const loadingMore = ref(false);
  const error = ref<string | null>(null);
  /** 结果可能达到上限（最后一页满页） */
  const limited = ref(false);
  /** 是否已执行过至少一次搜索（区分初始空态与无结果空态） */
  const hasSearched = ref(false);

  // ---------- 索引构建状态 ----------
  const indexStatus = ref<FtsIndexStatus | null>(null);
  let rebuildListenerReady = false;

  let generation = 0;

  const indexReady = computed(() => {
    const st = indexStatus.value;
    if (!st) return false;
    return st.totalMessages === 0 || st.indexedMessages >= st.totalMessages;
  });

  const indexProgressPct = computed(() => {
    const st = indexStatus.value;
    if (!st || st.totalMessages <= 0) return 100;
    return Math.min(100, Math.round((st.indexedMessages / st.totalMessages) * 100));
  });

  const canSearch = computed(() => query.value.trim().length >= SEARCH_MIN_CHARS);

  // ---------- 索引构建 ----------
  const registerRebuildListener = () => {
    if (rebuildListenerReady) return;
    rebuildListenerReady = true;
    void listen<{ type: string; indexedMessages?: number; totalMessages?: number }>(
      'vcp-system-event',
      (event) => {
        const payload = event.payload;
        if (payload?.type !== 'vcp-fts-rebuild') return;
        const current = indexStatus.value;
        indexStatus.value = {
          totalMessages: payload.totalMessages ?? current?.totalMessages ?? 0,
          indexedMessages: payload.indexedMessages ?? current?.indexedMessages ?? 0,
          rebuilding: true,
        };
      },
    );
  };

  const refreshIndexStatus = async (): Promise<FtsIndexStatus | null> => {
    try {
      const status = await invoke<FtsIndexStatus>('get_fts_index_status');
      indexStatus.value = status;
      return status;
    } catch (e) {
      console.error('[GlobalSearch] Failed to query fts index status:', e);
      return null;
    }
  };

  /**
   * 首开搜索页时调用：检测覆盖率，不足则触发后台分批回填。
   * 回填期间搜索照常可用（已索引部分立即可搜）。
   */
  const ensureIndex = async () => {
    registerRebuildListener();
    const status = await refreshIndexStatus();
    if (!status) return;
    if (status.totalMessages > status.indexedMessages && !status.rebuilding) {
      try {
        indexStatus.value = { ...status, rebuilding: true };
        const finalStatus = await invoke<FtsIndexStatus>('rebuild_messages_fts');
        indexStatus.value = finalStatus;
      } catch (e) {
        console.error('[GlobalSearch] FTS rebuild failed:', e);
        indexStatus.value = { ...status, rebuilding: false };
      }
    }
  };

  // ---------- 搜索 ----------
  const timeBounds = (): { startTime: number | null; endTime: number | null } => {
    const now = Date.now();
    switch (timeRange.value) {
      case 'today': {
        const d = new Date();
        d.setHours(0, 0, 0, 0);
        return { startTime: d.getTime(), endTime: null };
      }
      case 'week':
        return { startTime: now - 7 * 86400_000, endTime: null };
      case 'month':
        return { startTime: now - 30 * 86400_000, endTime: null };
      default:
        return { startTime: null, endTime: null };
    }
  };

  const buildFilter = (cursor?: { beforeTimestamp: number; beforeMessageId: string }) => {
    const bounds = timeBounds();
    return {
      query: query.value.trim(),
      topicId: scope.value === 'topic' ? scopeTopicId.value : null,
      ownerId: scope.value === 'owner' ? scopeOwnerId.value : null,
      ownerType: scope.value === 'owner' ? scopeOwnerType.value : null,
      role: role.value === 'all' ? null : role.value,
      startTime: bounds.startTime,
      endTime: bounds.endTime,
      limit: SEARCH_PAGE_SIZE,
      beforeTimestamp: cursor?.beforeTimestamp ?? null,
      beforeMessageId: cursor?.beforeMessageId ?? null,
      sort: sort.value,
    };
  };

  /** 执行新搜索（重置结果集）。竞态防护：generation 单调递增，迟到响应作废。 */
  const search = async () => {
    const g = ++generation;
    error.value = null;
    if (!canSearch.value) {
      results.value = [];
      limited.value = false;
      searching.value = false;
      hasSearched.value = false;
      return;
    }
    searching.value = true;
    try {
      const items = await invoke<FtsSearchResultItem[]>('search_messages_fts', {
        filter: buildFilter(),
      });
      if (g !== generation) return;
      results.value = items;
      limited.value = items.length >= SEARCH_PAGE_SIZE;
      hasSearched.value = true;
    } catch (e) {
      if (g !== generation) return;
      error.value = String(e);
      results.value = [];
    } finally {
      if (g === generation) searching.value = false;
    }
  };

  /** 加载更多（keyset 游标，仅时间倒序模式） */
  const loadMore = async () => {
    if (!limited.value || loadingMore.value || searching.value) return;
    if (sort.value !== 'time') return; // 相关度排序不支持游标分页
    const last = results.value[results.value.length - 1];
    if (!last) return;
    const g = generation;
    loadingMore.value = true;
    try {
      const items = await invoke<FtsSearchResultItem[]>('search_messages_fts', {
        filter: buildFilter({ beforeTimestamp: last.timestamp, beforeMessageId: last.msgId }),
      });
      if (g !== generation) return;
      results.value = [...results.value, ...items];
      limited.value = items.length >= SEARCH_PAGE_SIZE;
    } catch (e) {
      if (g !== generation) return;
      error.value = String(e);
    } finally {
      if (g === generation) loadingMore.value = false;
    }
  };

  /** 清空搜索状态（关闭页面时调用，保留索引状态缓存） */
  const reset = () => {
    generation += 1;
    query.value = '';
    results.value = [];
    searching.value = false;
    loadingMore.value = false;
    error.value = null;
    limited.value = false;
    hasSearched.value = false;
  };

  /** 打开页面时由外部预填过滤条件（如从话题菜单"搜索此话题"进入） */
  const applyOpenTarget = (target?: {
    topicId?: string;
    topicTitle?: string;
    ownerId?: string;
    ownerType?: 'agent' | 'group';
    ownerLabel?: string;
  } | null) => {
    if (!target) return;
    if (target.topicId) {
      scope.value = 'topic';
      scopeTopicId.value = target.topicId;
      scopeTopicTitle.value = target.topicTitle ?? '';
    } else if (target.ownerId) {
      scope.value = 'owner';
      scopeOwnerId.value = target.ownerId;
      scopeOwnerType.value = target.ownerType ?? 'agent';
      scopeOwnerLabel.value = target.ownerLabel ?? '';
    }
  };

  return {
    // state
    query, scope, scopeTopicId, scopeTopicTitle,
    scopeOwnerId, scopeOwnerType, scopeOwnerLabel,
    role, timeRange, sort,
    results, searching, loadingMore, error, limited, hasSearched,
    indexStatus, indexReady, indexProgressPct, canSearch,
    // actions
    ensureIndex, refreshIndexStatus, search, loadMore, reset, applyOpenTarget,
  };
});
