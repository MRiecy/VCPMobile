<script setup lang="ts">
/**
 * MailComposeView.vue — 写信 / 回复（滑入子页，依赖上游补丁端点）。
 *
 * send 模式：to/cc/bcc/subject/body；reply 模式：仅正文
 * （服务端自动带原邮件上下文并标读原邮件）。
 */
import { computed, reactive } from 'vue';
import { ArrowLeft } from 'lucide-vue-next';
import { useMailStore } from './mailStore';
import type { MailSummary } from './mailTypes';

const props = withDefaults(
  defineProps<{ mode: 'send' | 'reply'; replyTo?: MailSummary | null }>(),
  { replyTo: null },
);
const emit = defineEmits<{ close: [] }>();

const store = useMailStore();

const draft = reactive({
  to: props.mode === 'reply' ? (props.replyTo?.fromText ?? '') : '',
  cc: '',
  bcc: '',
  subject: props.mode === 'reply' ? `Re: ${props.replyTo?.subject ?? ''}` : '',
  body: '',
});

const validationError = computed(() => {
  if (props.mode === 'send') {
    if (!draft.to.trim()) return '收件人不能为空';
    if (!draft.subject.trim()) return '主题不能为空';
  }
  if (!draft.body.trim()) return '正文不能为空';
  return null;
});

async function submit(): Promise<void> {
  if (validationError.value) return;
  const ok =
    props.mode === 'reply' && props.replyTo
      ? await store.replyMail(props.replyTo.mailId, draft.body)
      : await store.sendMail({
          to: draft.to,
          cc: draft.cc || undefined,
          bcc: draft.bcc || undefined,
          subject: draft.subject,
          body: draft.body,
        });
  if (ok) emit('close');
}
</script>

<template>
  <div class="mail-compose">
    <header class="mc-header">
      <button type="button" class="mc-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <span class="mc-title">{{ mode === 'reply' ? '回复邮件' : '写邮件' }}</span>
      <span v-if="store.selectedMailbox" class="mc-from">
        {{ store.selectedMailbox.user }}
      </span>
    </header>

    <div class="mc-scroll vcp-scrollable no-rubber-band">
      <section v-if="mode === 'send'" class="mc-section">
        <label class="mc-field">
          <span class="mc-label">收件人（多个用英文逗号分隔）</span>
          <input v-model="draft.to" type="text" class="mc-input" placeholder="someone@example.com"
            autocapitalize="off" autocorrect="off" spellcheck="false" />
        </label>
        <label class="mc-field">
          <span class="mc-label">抄送（选填）</span>
          <input v-model="draft.cc" type="text" class="mc-input"
            autocapitalize="off" autocorrect="off" spellcheck="false" />
        </label>
        <label class="mc-field">
          <span class="mc-label">密送（选填）</span>
          <input v-model="draft.bcc" type="text" class="mc-input"
            autocapitalize="off" autocorrect="off" spellcheck="false" />
        </label>
        <label class="mc-field">
          <span class="mc-label">主题</span>
          <input v-model="draft.subject" type="text" class="mc-input" placeholder="邮件主题" />
        </label>
      </section>

      <section v-else class="mc-section">
        <p class="mc-reply-meta">
          回复 <strong>{{ replyTo?.fromText || '未知发件人' }}</strong>
          · 主题「{{ draft.subject }}」
        </p>
      </section>

      <section class="mc-section">
        <h3 class="mc-section-title">正文</h3>
        <textarea
          v-model="draft.body"
          class="mc-textarea"
          rows="12"
          placeholder="输入正文…"
        />
      </section>

      <div class="mc-footer-spacer" />
    </div>

    <footer class="mc-footer">
      <p v-if="validationError" class="mc-validation">{{ validationError }}</p>
      <button
        type="button"
        class="mc-send-btn"
        :disabled="!!validationError || store.sending"
        @click="submit"
      >
        {{ store.sending ? '发送中…' : '发送' }}
      </button>
    </footer>
  </div>
</template>

<style scoped>
.mail-compose {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.mc-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.mc-title {
  flex: 1;
  font-size: 16px;
  font-weight: 800;
}

.mc-from {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mc-icon-btn {
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

.mc-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.mc-section {
  margin-bottom: 18px;
}

.mc-section-title {
  margin: 0 0 10px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.mc-field {
  display: block;
  margin-bottom: 12px;
}

.mc-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.6;
  margin-bottom: 6px;
}

.mc-input {
  width: 100%;
  box-sizing: border-box;
  height: 40px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 13px;
  outline: none;
}

.mc-reply-meta {
  margin: 0;
  font-size: 12px;
  opacity: 0.6;
}

.mc-textarea {
  width: 100%;
  box-sizing: border-box;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 13px;
  line-height: 1.6;
  resize: vertical;
  outline: none;
  font-family: inherit;
}

.mc-footer-spacer {
  height: 8px;
}

.mc-footer {
  flex-shrink: 0;
  padding: 10px 14px calc(var(--vcp-safe-bottom, 48px) + 10px);
  border-top: 1px solid var(--border-color);
}

.mc-validation {
  margin: 0 0 8px;
  font-size: 11px;
  color: #f59e0b;
}

.mc-send-btn {
  width: 100%;
  min-height: 44px;
  border: none;
  border-radius: 10px;
  background: var(--highlight-text);
  color: #fff;
  font-size: 14px;
  font-weight: 800;
}

.mc-send-btn:disabled {
  opacity: 0.4;
}

@media (min-width: 768px) {
  .mc-scroll,
  .mc-footer {
    max-width: 640px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
