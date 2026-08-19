<script setup lang="ts">
/**
 * ForumDetailView.vue — 帖子详情（主帖 + 楼层线性流 + 底部快捷回复条）。
 *
 * 渲染走 renderForumMarkdown（marked → filterTrustedRichHtml，唯一 v-html 边界）。
 * UID/楼层号 Monospace；回复署名默认取设置中的用户名。
 */
import { computed, onMounted, ref, watch } from 'vue';
import { ArrowLeft, RefreshCw, Send } from 'lucide-vue-next';
import { useForumStore } from './forumStore';
import { useSettingsStore } from '../../core/stores/settings';
import { useKeyboardInsets } from '../../core/composables/useKeyboardInsets';
import {
  authorHue,
  relativeTime,
  renderForumMarkdown,
  type PostMeta,
} from './forumTypes';

const props = defineProps<{ uid: string; postMeta: PostMeta }>();
const emit = defineEmits<{ close: [] }>();

const store = useForumStore();
const settingsStore = useSettingsStore();
const { keyboardHeight } = useKeyboardInsets();

const detail = computed(() => store.detailCache.get(props.uid) ?? null);

onMounted(() => {
  void store.loadDetail(props.uid);
});

// 列表元数据变化（回帖后 lastReply 更新）时穿透刷新详情
watch(
  () => store.posts.find((post) => post.uid === props.uid)?.mtimeMs,
  (mtime, previous) => {
    if (mtime && previous && mtime !== previous) {
      void store.loadDetail(props.uid, true);
    }
  },
);

// ---------- 快捷回复 ----------
const replyMaid = ref('');
const replyContent = ref('');

onMounted(() => {
  replyMaid.value = settingsStore.settings?.userName ?? '';
});

const canReply = computed(
  () => replyMaid.value.trim().length > 0 && replyContent.value.trim().length > 0 && !store.replying,
);

async function sendReply(): Promise<void> {
  if (!canReply.value) return;
  const ok = await store.reply(props.uid, replyMaid.value.trim(), replyContent.value.trim());
  if (ok) replyContent.value = '';
}

function refresh(): void {
  void store.loadDetail(props.uid, true);
  void store.loadPosts();
}
</script>

<template>
  <div class="forum-detail">
    <header class="fd-header">
      <button type="button" class="fd-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <div class="fd-title-block">
        <span class="fd-title">{{ postMeta.title.replace('[置顶]', '').trim() }}</span>
        <span class="fd-subtitle">{{ postMeta.board }} · {{ postMeta.uid }}</span>
      </div>
      <button
        type="button"
        class="fd-icon-btn"
        aria-label="刷新帖子"
        title="刷新帖子"
        @click="refresh"
      >
        <RefreshCw :size="17" :class="{ 'custom-spin': store.detailLoading }" />
      </button>
    </header>

    <div class="fd-scroll vcp-scrollable no-rubber-band" data-forum-role="post-detail">
      <!-- 加载/错误态 -->
      <div v-if="!detail && store.detailLoading" class="fd-status">正在读取帖子…</div>
      <div v-else-if="!detail && store.detailError" class="fd-status">
        <p>{{ store.detailError }}</p>
        <button type="button" class="fd-retry-btn" @click="refresh">重试</button>
      </div>

      <template v-else-if="detail">
        <!-- 主帖 -->
        <article class="fd-main">
          <div class="fd-author-row">
            <span
              class="fd-avatar"
              :style="{ backgroundColor: `hsl(${authorHue(postMeta.author)} 55% 55%)` }"
            >
              {{ postMeta.author.slice(0, 1) }}
            </span>
            <span class="fd-author">{{ postMeta.author }}</span>
            <span class="fd-time">{{ relativeTime(postMeta.timestampMs) }}</span>
          </div>
          <!-- eslint-disable-next-line vue/no-v-html -->
          <div class="fd-body markdown-body" v-html="renderForumMarkdown(detail.mainBody)" />
        </article>

        <!-- 楼层 -->
        <div v-if="detail.floors.length > 0" class="fd-floors-head">
          评论区 · {{ detail.floors.length }} 楼
        </div>
        <article v-for="floor in detail.floors" :key="floor.index" class="fd-floor">
          <div class="fd-author-row">
            <span
              class="fd-avatar fd-avatar-sm"
              :style="{ backgroundColor: `hsl(${authorHue(floor.author)} 55% 55%)` }"
            >
              {{ floor.author.slice(0, 1) }}
            </span>
            <span class="fd-author">{{ floor.author }}</span>
            <span class="fd-time">{{ relativeTime(floor.timeMs) }}</span>
            <span class="fd-floor-no">#{{ floor.index }}</span>
          </div>
          <!-- eslint-disable-next-line vue/no-v-html -->
          <div class="fd-body markdown-body" v-html="renderForumMarkdown(floor.body)" />
        </article>
      </template>

      <div class="fd-bottom-spacer" />
    </div>

    <!-- 快捷回复条（键盘弹出时随 --keyboard-offset 抬升，不顶起整个页面） -->
    <footer class="fd-reply-bar" :style="{ marginBottom: `${keyboardHeight}px` }">
      <input
        v-model="replyMaid"
        type="text"
        class="fd-reply-maid"
        placeholder="署名"
        maxlength="50"
      />
      <textarea
        v-model="replyContent"
        class="fd-reply-input"
        rows="1"
        placeholder="回复…"
        enterkeyhint="send"
        @keydown.enter.exact.prevent="sendReply"
      />
      <button
        type="button"
        class="fd-send-btn"
        :disabled="!canReply"
        aria-label="发送回复"
        @click="sendReply"
      >
        <Send :size="16" />
      </button>
    </footer>
  </div>
</template>

<style scoped>
.forum-detail {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.fd-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.fd-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.fd-title {
  font-size: 15px;
  font-weight: 800;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fd-subtitle {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fd-icon-btn {
  width: 40px;
  height: 40px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.65;
  flex-shrink: 0;
}

.fd-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 10px 14px 12px;
}

.fd-status {
  padding: 40px 24px;
  text-align: center;
  font-size: 12px;
  opacity: 0.6;
}

.fd-retry-btn {
  margin-top: 10px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

.fd-main {
  padding: 6px 2px 12px;
  border-bottom: 1px solid var(--border-color);
}

.fd-author-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}

.fd-avatar {
  width: 30px;
  height: 30px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  color: #fff;
  font-size: 13px;
  font-weight: 800;
}

.fd-avatar-sm {
  width: 24px;
  height: 24px;
  font-size: 11px;
}

.fd-author {
  font-size: 13px;
  font-weight: 700;
}

.fd-time {
  font-size: 11px;
  opacity: 0.45;
}

.fd-floor-no {
  margin-left: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.4;
}

.fd-body {
  font-size: 13.5px;
  line-height: 1.7;
  word-break: break-word;
}

.fd-body :deep(img) {
  max-width: 100%;
  border-radius: 6px;
}

.fd-body :deep(pre) {
  overflow-x: auto;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--secondary-bg);
  font-size: 12px;
}

.fd-body :deep(code) {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.92em;
}

.fd-body :deep(blockquote) {
  margin: 8px 0;
  padding-left: 10px;
  border-left: 2px solid var(--border-color);
  opacity: 0.75;
}

.fd-floors-head {
  padding: 12px 2px 4px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.fd-floor {
  padding: 10px 2px 12px 12px;
  border-left: 2px solid var(--border-color);
  border-bottom: 1px solid var(--border-color);
}

.fd-bottom-spacer {
  height: 8px;
}

/* ---- 快捷回复条 ---- */
.fd-reply-bar {
  flex-shrink: 0;
  display: flex;
  align-items: flex-end;
  gap: 8px;
  padding: 8px 12px calc(var(--vcp-safe-bottom, 48px) + 8px);
  border-top: 1px solid var(--border-color);
}

.fd-reply-maid {
  width: 84px;
  flex-shrink: 0;
  height: 38px;
  padding: 0 10px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  outline: none;
}

.fd-reply-input {
  flex: 1;
  min-width: 0;
  min-height: 38px;
  max-height: 120px;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 13px;
  line-height: 1.5;
  resize: none;
  outline: none;
  font-family: inherit;
}

.fd-send-btn {
  width: 38px;
  height: 38px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: var(--highlight-text);
  color: #fff;
}

.fd-send-btn:disabled {
  opacity: 0.4;
}

@media (min-width: 768px) {
  .fd-scroll,
  .fd-reply-bar {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
