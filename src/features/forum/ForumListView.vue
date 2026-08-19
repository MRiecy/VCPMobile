<script setup lang="ts">
/**
 * ForumListView.vue — VCP 论坛主视图（板块筛选 + 搜索 + 线性列表）。
 *
 * 高密度线性列表（非瀑布流，09 篇 §7.3）：板块徽标 + 标题（置顶📌）+
 * 署名 + 相对时间 + 最后回复者。详情/发帖为滑入子页。
 */
import { onBeforeUnmount, ref, watch } from 'vue';
import {
  ArrowLeft,
  MessageSquareText,
  Pin,
  Plus,
  RefreshCw,
  Search,
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import ForumDetailView from './ForumDetailView.vue';
import ForumComposeView from './ForumComposeView.vue';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useForumStore } from './forumStore';
import { authorHue, relativeTime, type PostMeta } from './forumTypes';

const props = withDefaults(defineProps<{ isOpen?: boolean; zIndex?: number }>(), {
  isOpen: false,
  zIndex: 40,
});

const emit = defineEmits<{ close: [] }>();

const store = useForumStore();

// ---------- 子页（详情 / 发帖） ----------
const activePostUid = ref<string | null>(null);
const isComposeOpen = ref(false);

const { registerModal, unregisterModal } = useModalHistory();
const DETAIL_MODAL_ID = 'Forum:Detail';
const COMPOSE_MODAL_ID = 'Forum:Compose';

watch(activePostUid, (uid) => {
  if (uid) registerModal(DETAIL_MODAL_ID, () => closeDetail());
  else unregisterModal(DETAIL_MODAL_ID);
});

watch(isComposeOpen, (open) => {
  if (open) registerModal(COMPOSE_MODAL_ID, () => closeCompose());
  else unregisterModal(COMPOSE_MODAL_ID);
});

const activePost = ref<PostMeta | null>(null);

function openDetail(post: PostMeta): void {
  activePost.value = post;
  activePostUid.value = post.uid;
}

function closeDetail(): void {
  activePostUid.value = null;
  activePost.value = null;
}

function openCompose(): void {
  isComposeOpen.value = true;
}

function closeCompose(): void {
  isComposeOpen.value = false;
}

// ---------- 会话 ----------
watch(
  () => props.isOpen,
  (open) => {
    if (open) void store.loadPosts();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  unregisterModal(DETAIL_MODAL_ID);
  unregisterModal(COMPOSE_MODAL_ID);
  store.resetSession();
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div class="forum">
      <!-- 顶栏 -->
      <header class="fm-header">
        <button type="button" class="fm-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <div class="fm-title-block">
          <span class="fm-title">VCP 论坛</span>
          <span class="fm-subtitle">Forum</span>
        </div>
        <button
          type="button"
          class="fm-icon-btn"
          aria-label="刷新帖子列表"
          title="刷新帖子列表"
          @click="store.loadPosts()"
        >
          <RefreshCw :size="17" :class="{ 'custom-spin': store.isLoading }" />
        </button>
      </header>

      <!-- 搜索 + 板块 chips -->
      <section class="fm-filter">
        <div class="fm-search">
          <Search :size="14" class="fm-search-icon" />
          <input
            v-model="store.searchKeyword"
            type="search"
            class="fm-search-input"
            placeholder="搜索标题 / 作者…"
            enterkeyhint="search"
          />
        </div>
        <div v-if="store.boards.length > 0" class="fm-boards vcp-scrollable">
          <button
            type="button"
            class="fm-board-chip"
            :class="{ 'is-active': store.activeBoard === '' }"
            @click="store.activeBoard = ''"
          >
            全部
          </button>
          <button
            v-for="board in store.boards"
            :key="board"
            type="button"
            class="fm-board-chip"
            :class="{ 'is-active': store.activeBoard === board }"
            @click="store.activeBoard = board"
          >
            {{ board }}
          </button>
        </div>
      </section>

      <!-- 整页空态 -->
      <div v-if="store.error && !store.listLoaded" class="fm-empty">
        <MessageSquareText :size="28" class="fm-empty-icon" />
        <p class="fm-empty-title">连接失败</p>
        <p class="fm-empty-detail">{{ store.error }}</p>
        <button type="button" class="fm-retry-btn" @click="store.loadPosts()">重试</button>
      </div>

      <!-- 帖子列表 -->
      <div v-else class="fm-scroll vcp-scrollable no-rubber-band" data-forum-role="post-list">
        <div v-if="store.filteredPosts.length === 0" class="fm-empty">
          <p class="fm-empty-title">
            {{ store.isLoading ? '正在读取帖子…' : store.searchKeyword || store.activeBoard ? '没有匹配的帖子' : '论坛还没有帖子' }}
          </p>
          <p v-if="!store.isLoading && !store.searchKeyword && !store.activeBoard" class="fm-empty-detail">
            点击右下角「+」发布第一个帖子。
          </p>
        </div>

        <button
          v-for="post in store.filteredPosts"
          :key="post.uid"
          type="button"
          class="fm-row"
          @click="openDetail(post)"
        >
          <span class="fm-row-head">
            <Pin v-if="post.pinned" :size="12" class="fm-pin" aria-label="置顶" />
            <span class="fm-row-board">{{ post.board }}</span>
            <span class="fm-row-title">{{ post.title.replace('[置顶]', '').trim() }}</span>
          </span>
          <span class="fm-row-meta">
            <span
              class="fm-author-dot"
              :style="{ backgroundColor: `hsl(${authorHue(post.author)} 55% 55%)` }"
            />
            {{ post.author }} · {{ relativeTime(post.timestampMs) }}
            <template v-if="post.lastReplyBy">
              · 最后回复 {{ post.lastReplyBy }}
            </template>
          </span>
        </button>
      </div>

      <!-- 发帖 FAB -->
      <button type="button" class="fm-fab" aria-label="发布新帖" @click="openCompose">
        <Plus :size="22" />
      </button>

      <!-- 详情（滑入子页） -->
      <Transition name="fm-detail-slide">
        <ForumDetailView
          v-if="activePostUid && activePost"
          :uid="activePostUid"
          :post-meta="activePost"
          @close="closeDetail"
        />
      </Transition>

      <!-- 发帖（滑入子页） -->
      <Transition name="fm-detail-slide">
        <ForumComposeView v-if="isComposeOpen" @close="closeCompose" />
      </Transition>
    </div>
  </SlidePage>
</template>

<style scoped>
.forum {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.fm-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.fm-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.fm-title {
  font-size: 16px;
  font-weight: 800;
}

.fm-subtitle {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  opacity: 0.45;
  text-transform: uppercase;
}

.fm-icon-btn {
  width: 40px;
  height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.65;
}

.fm-icon-btn:active {
  opacity: 1;
}

.fm-filter {
  flex-shrink: 0;
  padding: 10px 14px 8px;
  border-bottom: 1px solid var(--border-color);
}

.fm-search {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 38px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
}

.fm-search-icon {
  opacity: 0.45;
  flex-shrink: 0;
}

.fm-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
}

.fm-boards {
  display: flex;
  gap: 8px;
  margin-top: 8px;
  overflow-x: auto;
  white-space: nowrap;
}

.fm-board-chip {
  flex-shrink: 0;
  min-height: 30px;
  padding: 0 12px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
  opacity: 0.6;
}

.fm-board-chip.is-active {
  opacity: 1;
  color: var(--highlight-text);
  border-color: var(--highlight-text);
}

.fm-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px 12px calc(var(--vcp-safe-bottom, 48px) + 76px);
}

.fm-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px 24px;
  text-align: center;
}

.fm-empty-icon {
  opacity: 0.35;
}

.fm-empty-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  opacity: 0.75;
}

.fm-empty-detail {
  margin: 0;
  font-size: 12px;
  opacity: 0.5;
  max-width: 28rem;
  word-break: break-all;
}

.fm-retry-btn {
  margin-top: 6px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

/* ---- 帖子行（高密度线性 + 2px accent） ---- */
.fm-row {
  display: flex;
  flex-direction: column;
  gap: 3px;
  width: 100%;
  padding: 10px 10px 10px 12px;
  border: none;
  border-left: 2px solid transparent;
  border-bottom: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.fm-row:active {
  border-left-color: var(--highlight-text);
  background: var(--secondary-bg);
}

.fm-row-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

.fm-pin {
  color: #f59e0b;
  flex-shrink: 0;
  align-self: center;
}

.fm-row-board {
  flex-shrink: 0;
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.08em;
  padding: 2px 6px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  opacity: 0.6;
}

.fm-row-title {
  font-size: 14px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fm-row-meta {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  opacity: 0.55;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fm-author-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}

/* ---- 发帖 FAB ---- */
.fm-fab {
  position: absolute;
  right: 18px;
  bottom: calc(var(--vcp-safe-bottom, 48px) + 20px);
  width: 52px;
  height: 52px;
  border-radius: 50%;
  border: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--highlight-text);
  color: #fff;
  z-index: var(--layer-local);
}

.fm-fab:active {
  opacity: 0.85;
}

/* 子页滑入动画（内敛：位移 + 透明度） */
.fm-detail-slide-enter-active,
.fm-detail-slide-leave-active {
  transition:
    transform 0.3s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.3s ease;
}

.fm-detail-slide-enter-from,
.fm-detail-slide-leave-to {
  transform: translateX(100%);
  opacity: 0.6;
}

@media (min-width: 768px) {
  .fm-scroll,
  .fm-filter {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
