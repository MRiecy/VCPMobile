/**
 * globalSearchStore.ts — 全局消息搜索 Store。
 *
 * 设计要点：
 * 1. 纯本地 FTS 检索（search_messages_fts），无远程依赖、离线可用；
 * 2. generation 竞态防护：每次新搜索递增 generation，迟到响应直接丢弃
 *    （本地 SQLite 毫秒级返回，无需日记那套远程 cancel command）；
 * 3. keyset 分页：时间倒序模式下以末条完整消息身份为游标加载更多；
 * 4. 当前 baseline 的消息写入与 FTS 索引在同一事务内维护，不在搜索页启动额外回填任务。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import {
  SEARCH_PAGE_SIZE,
  SEARCH_MIN_CHARS,
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
  /** 具体发言 Agent；区别于会话归属 owner 和消息协议 role */
  const speakerAgentId = ref<string | null>(null);
  const role = ref<RoleFilter>('all');
  const timeRange = ref<TimeFilter>('all');
  const customStartDate = ref('');
  const customEndDate = ref('');
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

  let generation = 0;

  const canSearch = computed(() => [...query.value.trim()].length >= SEARCH_MIN_CHARS);

  // ---------- 搜索 ----------
  const localDateBoundary = (value: string, endOfDay: boolean): number | null => {
    if (!value) return null;
    const [year, month, day] = value.split('-').map(Number);
    const date = new Date(
      year,
      month - 1,
      day,
      endOfDay ? 23 : 0,
      endOfDay ? 59 : 0,
      endOfDay ? 59 : 0,
      endOfDay ? 999 : 0,
    );
    return Number.isNaN(date.getTime()) ? null : date.getTime();
  };

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
      case 'custom':
        return {
          startTime: localDateBoundary(customStartDate.value, false),
          endTime: localDateBoundary(customEndDate.value, true),
        };
      default:
        return { startTime: null, endTime: null };
    }
  };

  const customRangeInvalid = computed(() => {
    if (timeRange.value !== 'custom') return false;
    const { startTime, endTime } = timeBounds();
    return startTime !== null && endTime !== null && startTime > endTime;
  });

  const buildFilter = (cursor?: FtsSearchResultItem) => {
    const bounds = timeBounds();
    const scopedOwner = scope.value === 'topic' || scope.value === 'owner';
    return {
      query: query.value.trim(),
      topicId: scope.value === 'topic' ? scopeTopicId.value : null,
      ownerId: scopedOwner ? scopeOwnerId.value : null,
      ownerType: scopedOwner ? scopeOwnerType.value : null,
      agentId: speakerAgentId.value,
      role: role.value === 'all' ? null : role.value,
      startTime: bounds.startTime,
      endTime: bounds.endTime,
      limit: SEARCH_PAGE_SIZE,
      beforeTimestamp: cursor?.timestamp ?? null,
      beforeOwnerType: cursor?.ownerType ?? null,
      beforeOwnerId: cursor?.ownerId ?? null,
      beforeTopicId: cursor?.topicId ?? null,
      beforeMessageId: cursor?.msgId ?? null,
      sort: sort.value,
    };
  };

  /** 执行新搜索（重置结果集）。竞态防护：generation 单调递增，迟到响应作废。 */
  const search = async () => {
    const g = ++generation;
    error.value = null;
    loadingMore.value = false;
    if (!canSearch.value) {
      results.value = [];
      limited.value = false;
      searching.value = false;
      hasSearched.value = false;
      return;
    }
    if (customRangeInvalid.value) {
      results.value = [];
      limited.value = false;
      searching.value = false;
      hasSearched.value = true;
      error.value = '开始日期不能晚于结束日期';
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
        filter: buildFilter(last),
      });
      if (g !== generation) return;
      const additions = items.filter(
        (item) =>
          !results.value.some(
            (existing) =>
              existing.ownerType === item.ownerType &&
              existing.ownerId === item.ownerId &&
              existing.topicId === item.topicId &&
              existing.msgId === item.msgId,
          ),
      );
      results.value = [...results.value, ...additions];
      limited.value = items.length >= SEARCH_PAGE_SIZE;
    } catch (e) {
      if (g !== generation) return;
      error.value = String(e);
    } finally {
      if (g === generation) loadingMore.value = false;
    }
  };

  /** 关闭页面时清空本次搜索会话，防止全局入口继承旧范围。 */
  const reset = () => {
    generation += 1;
    query.value = '';
    scope.value = 'all';
    scopeTopicId.value = null;
    scopeTopicTitle.value = '';
    scopeOwnerId.value = null;
    scopeOwnerType.value = null;
    scopeOwnerLabel.value = '';
    speakerAgentId.value = null;
    role.value = 'all';
    timeRange.value = 'all';
    customStartDate.value = '';
    customEndDate.value = '';
    sort.value = 'time';
    results.value = [];
    searching.value = false;
    loadingMore.value = false;
    error.value = null;
    limited.value = false;
    hasSearched.value = false;
  };

  return {
    // state
    query, scope, scopeTopicId, scopeTopicTitle,
    scopeOwnerId, scopeOwnerType, scopeOwnerLabel,
    speakerAgentId, role, timeRange, customStartDate, customEndDate, sort,
    results, searching, loadingMore, error, limited, hasSearched,
    canSearch,
    // actions
    search, loadMore, reset,
  };
});
