/**
 * forumStore.ts — VCP 论坛状态机。
 *
 * 读：进入页面全量拉取（GET /posts 无分页），手动刷新；`mtimeMs` 作脏检查键。
 * 写：回帖（forum_reply）/ 发帖（forum_create_post，human/tool 通道），
 * 成功后乐观重拉。凭据复用设置中的 admin 凭据与 VCP API Key（09 篇 §4 确认）。
 */
import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useNotificationStore } from '../../core/stores/notification';
import {
  normalizePostList,
  parsePostContent,
  type ParsedPost,
  type PostMeta,
} from './forumTypes';

function toMessage(raw: unknown): string {
  return raw instanceof Error ? raw.message : String(raw);
}

export const useForumStore = defineStore('forum', () => {
  const notificationStore = useNotificationStore();

  const toast = (type: 'info' | 'success' | 'warning' | 'error', message: string) => {
    notificationStore.addNotification({
      type,
      title: 'VCP 论坛',
      message,
      toastOnly: true,
    });
  };

  // ---------- 列表状态 ----------
  const posts = ref<PostMeta[]>([]);
  const listLoaded = ref(false);
  const isLoading = ref(false);
  const error = ref<string | null>(null);

  /** 板块筛选（空 = 全部）与搜索关键词。 */
  const activeBoard = ref('');
  const searchKeyword = ref('');

  /** 板块列表：从 posts 去重得出（后端无板块实体）。 */
  const boards = computed(() => {
    const set = new Set<string>();
    for (const post of posts.value) set.add(post.board);
    return Array.from(set);
  });

  const filteredPosts = computed(() => {
    const keyword = searchKeyword.value.trim().toLowerCase();
    return posts.value.filter((post) => {
      if (activeBoard.value && post.board !== activeBoard.value) return false;
      if (!keyword) return true;
      return (
        post.title.toLowerCase().includes(keyword) ||
        post.author.toLowerCase().includes(keyword) ||
        post.board.toLowerCase().includes(keyword)
      );
    });
  });

  // ---------- 详情缓存 ----------
  /** uid → 解析后的帖子（主帖 + 楼层）。 */
  const detailCache = ref<Map<string, ParsedPost>>(new Map());
  const detailLoading = ref(false);
  const detailError = ref<string | null>(null);

  // ---------- 写操作状态 ----------
  const replying = ref(false);
  const creating = ref(false);

  // ---------- 读 ----------
  async function loadPosts(): Promise<void> {
    if (isLoading.value) return;
    isLoading.value = true;
    try {
      const payload = await invoke<Record<string, unknown>>('forum_list_posts');
      posts.value = normalizePostList(payload.posts);
      listLoaded.value = true;
      error.value = null;
    } catch (raw) {
      error.value = toMessage(raw);
    } finally {
      isLoading.value = false;
    }
  }

  /** 拉取详情（force=false 时用缓存；force=true 穿透刷新）。 */
  async function loadDetail(uid: string, force = false): Promise<ParsedPost | null> {
    if (!force && detailCache.value.has(uid)) {
      return detailCache.value.get(uid)!;
    }
    detailLoading.value = true;
    detailError.value = null;
    try {
      const payload = await invoke<Record<string, unknown>>('forum_get_post', { uid });
      const content = typeof payload.content === 'string' ? payload.content : '';
      const parsed = parsePostContent(content);
      detailCache.value = new Map(detailCache.value).set(uid, parsed);
      return parsed;
    } catch (raw) {
      detailError.value = toMessage(raw);
      return null;
    } finally {
      detailLoading.value = false;
    }
  }

  /** 页面关闭时彻底复位。 */
  function resetSession(): void {
    posts.value = [];
    listLoaded.value = false;
    isLoading.value = false;
    error.value = null;
    activeBoard.value = '';
    searchKeyword.value = '';
    detailCache.value = new Map();
    detailLoading.value = false;
    detailError.value = null;
    replying.value = false;
    creating.value = false;
  }

  // ---------- 写 ----------
  /** 回帖：成功后失效缓存并重拉详情 + 列表（lastReply 元数据变化）。 */
  async function reply(uid: string, maid: string, content: string): Promise<boolean> {
    if (replying.value) return false;
    replying.value = true;
    try {
      await invoke('forum_reply', { uid, maid, content });
      toast('success', '回复已发布');
      detailCache.value.delete(uid);
      await Promise.all([loadDetail(uid, true), loadPosts()]);
      return true;
    } catch (raw) {
      toast('error', `回复失败：${toMessage(raw)}`);
      return false;
    } finally {
      replying.value = false;
    }
  }

  /** 发帖：human/tool 通道；成功后重拉列表。 */
  async function createPost(
    maid: string,
    board: string,
    title: string,
    content: string,
  ): Promise<boolean> {
    if (creating.value) return false;
    creating.value = true;
    try {
      await invoke('forum_create_post', { maid, board, title, content });
      toast('success', '帖子已发布');
      await loadPosts();
      return true;
    } catch (raw) {
      toast('error', `发帖失败：${toMessage(raw)}`);
      return false;
    } finally {
      creating.value = false;
    }
  }

  return {
    posts,
    listLoaded,
    isLoading,
    error,
    activeBoard,
    searchKeyword,
    boards,
    filteredPosts,
    detailCache,
    detailLoading,
    detailError,
    replying,
    creating,
    loadPosts,
    loadDetail,
    resetSession,
    reply,
    createPost,
  };
});
