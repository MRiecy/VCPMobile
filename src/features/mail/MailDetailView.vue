<script setup lang="ts">
/**
 * MailDetailView.vue — 邮件详情（滑入子页）。
 *
 * 正文渲染 renderMailMarkdown（共享安全管线，唯一 v-html 边界）。
 * 阅读默认不标读；操作：回复 / 标为已读或未读 / 附件下载 / 移入垃圾箱。
 * mailId 等标识使用 Monospace。
 */
import { computed, ref } from 'vue';
import {
  ArrowLeft,
  Download,
  MailCheck,
  MailPlus,
  Reply,
  Trash2,
} from 'lucide-vue-next';
import { useOverlayStore } from '../../core/stores/overlay';
import { filterTrustedRichHtml } from '../../core/utils/astRenderer';
import { useMailStore } from './mailStore';
import MailComposeView from './MailComposeView.vue';
import {
  attachmentSizeLabel,
  mailBodyOf,
  mailTimeLabel,
  renderMailMarkdown,
  type MailSummary,
} from './mailTypes';

const props = defineProps<{ mail: MailSummary }>();
const emit = defineEmits<{ close: [] }>();

const store = useMailStore();
const overlayStore = useOverlayStore();

const isReplyOpen = ref(false);

/**
 * 详情正文：优先结构化 html/text（补丁服务器），旧服务器回退到
 * 从 AI 导向 markdown 中提取的正文段落。html 走与聊天一致的护栏管线。
 */
const body = computed(() => (store.detail ? mailBodyOf(store.detail) : null));

const renderedBody = computed(() => {
  const current = body.value;
  if (!current) return '';
  if (current.kind === 'html') return filterTrustedRichHtml(current.html);
  if (current.kind === 'markdown') return renderMailMarkdown(current.markdown);
  return '';
});

async function toggleRead(): Promise<void> {
  await store.setRead(props.mail.readState !== 'read');
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

async function download(partId: string, filename: string): Promise<void> {
  await store.downloadAttachment(props.mail.mailId, partId, filename);
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

    <!-- 操作条（图标 + 小字说明） -->
    <nav class="md-actions" aria-label="邮件操作">
      <button type="button" class="md-action" @click="isReplyOpen = true">
        <Reply :size="18" />
        <span class="md-action-label">回复</span>
      </button>
      <button type="button" class="md-action" @click="toggleRead">
        <MailPlus v-if="mail.readState === 'read'" :size="18" />
        <MailCheck v-else :size="18" />
        <span class="md-action-label">{{ mail.readState === 'read' ? '标为未读' : '标为已读' }}</span>
      </button>
      <button
        type="button"
        class="md-action md-action-danger"
        :disabled="store.trashing"
        @click="trash"
      >
        <Trash2 :size="18" />
        <span class="md-action-label">垃圾箱</span>
      </button>
    </nav>

    <!-- 正文 -->
    <div class="md-scroll vcp-scrollable no-rubber-band" data-mail-role="mail-detail">
      <div v-if="store.detailLoading" class="md-status">正在读取邮件…</div>
      <div v-else-if="store.detailError" class="md-status">
        <p>{{ store.detailError }}</p>
        <button type="button" class="md-retry-btn" @click="store.openDetail(mail.mailId)">
          重试
        </button>
      </div>
      <template v-else-if="store.detail">
        <!-- 正文：html/markdown 走护栏渲染；纯文本预格式化展示 -->
        <pre v-if="body?.kind === 'text'" class="md-text">{{ body.text }}</pre>
        <!-- eslint-disable-next-line vue/no-v-html -->
        <div v-else-if="renderedBody" class="md-body markdown-body" v-html="renderedBody" />
        <div v-else class="md-status">这封邮件没有可显示的正文。</div>

        <!-- 附件列表 -->
        <section v-if="store.detail.attachments.length > 0" class="md-attachments">
          <h3 class="md-attachments-title">附件 · {{ store.detail.attachments.length }}</h3>
          <div
            v-for="att in store.detail.attachments"
            :key="att.partId"
            class="md-attachment-row"
          >
            <div class="md-attachment-info">
              <span class="md-attachment-name">{{ att.filename }}</span>
              <span class="md-attachment-meta">
                {{ att.contentType }} · {{ attachmentSizeLabel(att.size) }}{{ att.inline ? ' · 内嵌' : '' }}
              </span>
            </div>
            <button
              type="button"
              class="md-attachment-btn"
              :aria-label="`下载 ${att.filename}`"
              @click="download(att.partId, att.filename)"
            >
              <Download :size="15" />
            </button>
          </div>
        </section>
      </template>
    </div>

    <!-- 回复（滑入子页） -->
    <Transition name="md-compose-slide">
      <MailComposeView
        v-if="isReplyOpen"
        mode="reply"
        :reply-to="mail"
        @close="isReplyOpen = false"
      />
    </Transition>
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

/* ---- 操作条（图标 + 小字） ---- */
.md-actions {
  flex-shrink: 0;
  display: flex;
  border-bottom: 1px solid var(--border-color);
}

.md-action {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  padding: 9px 0 8px;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.75;
}

.md-action:active {
  opacity: 1;
  background: var(--secondary-bg);
}

.md-action:disabled {
  opacity: 0.3;
}

.md-action-label {
  font-size: 10px;
  font-weight: 700;
  opacity: 0.8;
}

.md-action-danger {
  color: #ef4444;
}

.md-text {
  margin: 0;
  font-family: inherit;
  font-size: 13.5px;
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
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

/* ---- 附件 ---- */
.md-attachments {
  margin-top: 14px;
  border-top: 1px solid var(--border-color);
  padding-top: 10px;
}

.md-attachments-title {
  margin: 0 0 8px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.md-attachment-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 2px;
  border-bottom: 1px solid var(--border-color);
}

.md-attachment-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.md-attachment-name {
  font-size: 12.5px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.md-attachment-meta {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
}

.md-attachment-btn {
  width: 36px;
  height: 36px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border-color);
  border-radius: 50%;
  background: var(--secondary-bg);
  color: var(--highlight-text);
}

/* 回复子页滑入动画 */
.md-compose-slide-enter-active,
.md-compose-slide-leave-active {
  transition:
    transform 0.3s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.3s ease;
}

.md-compose-slide-enter-from,
.md-compose-slide-leave-to {
  transform: translateX(100%);
  opacity: 0.6;
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
