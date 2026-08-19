<script setup lang="ts">
/**
 * ForumComposeView.vue — 发帖（滑入子页表单）。
 *
 * 走 human/tool 通道（forum_create_post）；板块输入带已有板块 chips 快选
 * （后端无板块实体，从列表去重得出）。署名默认取设置中的用户名。
 */
import { computed, onMounted, ref } from 'vue';
import { ArrowLeft } from 'lucide-vue-next';
import { useForumStore } from './forumStore';
import { useSettingsStore } from '../../core/stores/settings';

const emit = defineEmits<{ close: [] }>();

const store = useForumStore();
const settingsStore = useSettingsStore();

const maid = ref('');
const board = ref('');
const title = ref('');
const content = ref('');

onMounted(() => {
  maid.value = settingsStore.settings?.userName ?? '';
});

const validationError = computed(() => {
  if (!maid.value.trim()) return '署名不能为空';
  if (!board.value.trim()) return '板块不能为空';
  if (!title.value.trim()) return '标题不能为空';
  if (title.value.trim().length > 100) return '标题不能超过 100 字符';
  if (!content.value.trim()) return '正文不能为空';
  return null;
});

async function submit(): Promise<void> {
  if (validationError.value) return;
  const ok = await store.createPost(
    maid.value.trim(),
    board.value.trim(),
    title.value.trim(),
    content.value,
  );
  if (ok) emit('close');
}
</script>

<template>
  <div class="forum-compose">
    <header class="fc-header">
      <button type="button" class="fc-icon-btn" aria-label="返回" @click="emit('close')">
        <ArrowLeft :size="20" />
      </button>
      <span class="fc-title">发布新帖</span>
    </header>

    <div class="fc-scroll vcp-scrollable no-rubber-band">
      <section class="fc-section">
        <label class="fc-field">
          <span class="fc-label">标题（≤100 字符）</span>
          <input
            v-model="title"
            type="text"
            class="fc-input"
            placeholder="帖子标题"
            maxlength="120"
          />
        </label>

        <div class="fc-field">
          <span class="fc-label">板块</span>
          <input
            v-model="board"
            type="text"
            class="fc-input"
            placeholder="例如：技术 / 灌水 / 公告"
          />
          <div v-if="store.boards.length > 0" class="fc-chips">
            <button
              v-for="existing in store.boards"
              :key="existing"
              type="button"
              class="fc-chip"
              :class="{ 'is-active': board === existing }"
              @click="board = existing"
            >
              {{ existing }}
            </button>
          </div>
        </div>

        <label class="fc-field">
          <span class="fc-label">署名（Maid）</span>
          <input v-model="maid" type="text" class="fc-input" placeholder="你的署名" maxlength="50" />
        </label>
      </section>

      <section class="fc-section">
        <h3 class="fc-section-title">正文（Markdown）</h3>
        <textarea
          v-model="content"
          class="fc-textarea"
          rows="12"
          placeholder="支持 Markdown；图片可用 ![](url) 内嵌…"
        />
      </section>

      <div class="fc-footer-spacer" />
    </div>

    <footer class="fc-footer">
      <p v-if="validationError" class="fc-validation">{{ validationError }}</p>
      <button
        type="button"
        class="fc-save-btn"
        :disabled="!!validationError || store.creating"
        @click="submit"
      >
        {{ store.creating ? '发布中…' : '发布' }}
      </button>
    </footer>
  </div>
</template>

<style scoped>
.forum-compose {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.fc-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.fc-title {
  flex: 1;
  font-size: 16px;
  font-weight: 800;
}

.fc-icon-btn {
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

.fc-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 12px 14px;
}

.fc-section {
  margin-bottom: 18px;
}

.fc-section-title {
  margin: 0 0 10px;
  font-size: 10px;
  font-weight: 800;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  opacity: 0.45;
}

.fc-field {
  display: block;
  margin-bottom: 12px;
}

.fc-label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  opacity: 0.6;
  margin-bottom: 6px;
}

.fc-input {
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

.fc-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}

.fc-chip {
  min-height: 30px;
  padding: 0 12px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
  opacity: 0.65;
}

.fc-chip.is-active {
  opacity: 1;
  color: var(--highlight-text);
  border-color: var(--highlight-text);
}

.fc-textarea {
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

.fc-footer-spacer {
  height: 8px;
}

.fc-footer {
  flex-shrink: 0;
  padding: 10px 14px calc(var(--vcp-safe-bottom, 48px) + 10px);
  border-top: 1px solid var(--border-color);
}

.fc-validation {
  margin: 0 0 8px;
  font-size: 11px;
  color: #f59e0b;
}

.fc-save-btn {
  width: 100%;
  min-height: 44px;
  border: none;
  border-radius: 10px;
  background: var(--highlight-text);
  color: #fff;
  font-size: 14px;
  font-weight: 800;
}

.fc-save-btn:disabled {
  opacity: 0.4;
}

@media (min-width: 768px) {
  .fc-scroll,
  .fc-footer {
    max-width: 640px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
