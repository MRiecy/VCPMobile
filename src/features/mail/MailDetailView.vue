<script setup lang="ts">
/**
 * MailDetailView.vue — 邮件详情（滑入子页）。
 *
 * 正文渲染 renderMailMarkdown（共享安全管线，唯一 v-html 边界）。
 * 阅读默认不标读（markRead=false）；「标为已读」与「移入垃圾箱」为显式操作。
 * mailId 等标识使用 Monospace。
 */
import { computed } from 'vue';
import { ArrowLeft, MailCheck, Trash2 } from 'lucide-vue-next';
import { useOverlayStore } from '../../core/stores/overlay';
import { useMailStore } from './mailStore';
import { mailTimeLabel, renderMailMarkdown, type MailSummary } from './mailTypes';

const props = defineProps<{ mail: MailSummary }>();
const emit = defineEmits<{ close: [] }>();

const store = useMailStore();
const overlayStore = useOverlayStore();

const canMarkRead = computed(() => props.mail.readState === 'unread');

async function markRead(): Promise<void> {
  await store.markRead();
}

async function trash(): Promise<void> {
  const confirmed = await overlayStore.showConfirm({
    title: '移入垃圾箱',
    message: `确定把「${props.mail.subject}」移入垃圾箱吗？（软删除，可在网页端恢复）`,
    isDanger: true,
  });
  if (!confirmed) return;
  const ok = await store.trash(props.mail.mailId);
  if (ok) emit('close');
}
</script>

<template>
  <div class="mail-detail">
    <header class="md-header">
      <button type="button" class="md-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <div class="md-title-block">
        <span class="md-title">{{ mail.subject }}</span>
        <span class="md-subtitle">{{ mail.mailId }}</span>
      </div>
      <button
        v-if="canMarkRead"
        type="button"
        class="md-icon-btn"
        aria-label="标为已读"
        title="标为已读"
        @click="markRead"
      >
        <MailCheck :size="17" />
      </button>
      <button
        type="button"
        class="md-icon-btn md-trash-btn"
        aria-label="移入垃圾箱"
        title="移入垃圾箱"
        :disabled="store.trashing"
        @click="trash"
      >
        <Trash2 :size="17" />
      </button>
    </header>

    <!-- 头信息 -->
    <section class="md-meta">
      <div class="md-meta-row">
        <span class="md-meta-key">发件人</span>
        <span class="md-meta-value">{{ mail.fromText || '未知' }}</span>
      </div>
      <div v-if="mail.toText" class="md-meta-row">
        <span class="md-meta-key">收件人</span>
        <span class="md-meta-value">{{ mail.toText }}</span>
      </div>
      <div class="md-meta-row">
        <span class="md-meta-key">时间</span>
        <span class="md-meta-value md-mono">{{ mailTimeLabel(mail.dateMs) }}</span>
      </div>
    </section>

    <!-- 正文 -->
    <div class="md-scroll vcp-scrollable no-rubber-band" data-mail-role="mail-detail">
      <div v-if="store.detailLoading" class="md-status">正在读取邮件…</div>
      <div v-else-if="store.detailError" class="md-status">
        <p>{{ store.detailError }}</p>
        <button type="button" class="md-retry-btn" @click="store.openDetail(mail.mailId)">
          重试
        </button>
      </div>
      <!-- eslint-disable-next-line vue/no-v-html -->
      <div v-else class="md-body markdown-body" v-html="renderMailMarkdown(store.detailMarkdown)" />
    </div>
  </div>
</template>

<style scoped>
.mail-detail {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.md-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.md-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.md-title {
  font-size: 15px;
  font-weight: 800;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.md-subtitle {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.md-icon-btn {
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

.md-trash-btn {
  color: #ef4444;
  opacity: 0.9;
}

.md-meta {
  flex-shrink: 0;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.md-meta-row {
  display: flex;
  gap: 10px;
  min-width: 0;
}

.md-meta-key {
  flex-shrink: 0;
  width: 44px;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.5;
}

.md-meta-value {
  font-size: 12px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.md-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.md-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px calc(var(--vcp-safe-bottom, 48px) + 12px);
}

.md-status {
  padding: 40px 24px;
  text-align: center;
  font-size: 12px;
  opacity: 0.6;
}

.md-retry-btn {
  margin-top: 10px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

.md-body {
  font-size: 13.5px;
  line-height: 1.7;
  word-break: break-word;
}

.md-body :deep(img) {
  max-width: 100%;
  border-radius: 6px;
}

.md-body :deep(pre) {
  overflow-x: auto;
  padding: 10px 12px;
  border-radius: 8px;
  background: var(--secondary-bg);
  font-size: 12px;
}

.md-body :deep(code) {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.92em;
}

.md-body :deep(blockquote) {
  margin: 8px 0;
  padding-left: 10px;
  border-left: 2px solid var(--border-color);
  opacity: 0.75;
}

.md-body :deep(table) {
  border-collapse: collapse;
  font-size: 12px;
}

.md-body :deep(th),
.md-body :deep(td) {
  border: 1px solid var(--border-color);
  padding: 4px 8px;
}

@media (min-width: 768px) {
  .md-scroll,
  .md-meta {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
