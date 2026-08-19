<script setup lang="ts">
/**
 * MailListView.vue — clawEmail 邮箱主视图（邮箱切换 + 邮件列表）。
 *
 * MVP（10 篇 §9）：账户切换 chips（含 WS 在线指示）+ 仅未读开关 +
 * 高密度邮件列表（未读点/附件标记/preview）+ start offset 加载更多 +
 * 下拉等效手动刷新（state 穿透）。详情为滑入子页。
 */
import { onBeforeUnmount, ref, watch } from 'vue';
import {
  ArrowLeft,
  CircleAlert,
  Mail,
  Paperclip,
  PenLine,
  RefreshCw,
  Search,
  X,
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import MailDetailView from './MailDetailView.vue';
import MailComposeView from './MailComposeView.vue';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useMailStore } from './mailStore';
import { mailTimeLabel, type MailSummary } from './mailTypes';

const props = withDefaults(defineProps<{ isOpen?: boolean; zIndex?: number }>(), {
  isOpen: false,
  zIndex: 40,
});

const emit = defineEmits<{ close: [] }>();

const store = useMailStore();

// ---------- 详情 / 写信子页 ----------
const activeMail = ref<MailSummary | null>(null);
const isComposeOpen = ref(false);

const { registerModal, unregisterModal } = useModalHistory();
const DETAIL_MODAL_ID = 'Mail:Detail';
const COMPOSE_MODAL_ID = 'Mail:Compose';

watch(activeMail, (mail) => {
  if (mail) registerModal(DETAIL_MODAL_ID, () => closeDetail());
  else unregisterModal(DETAIL_MODAL_ID);
});

watch(isComposeOpen, (open) => {
  if (open) registerModal(COMPOSE_MODAL_ID, () => (isComposeOpen.value = false));
  else unregisterModal(COMPOSE_MODAL_ID);
});

function openDetail(mail: MailSummary): void {
  activeMail.value = mail;
  void store.openDetail(mail.mailId);
}

function closeDetail(): void {
  activeMail.value = null;
  store.closeDetail();
}

// ---------- 搜索 ----------
const searchInput = ref('');

function submitSearch(): void {
  const keyword = searchInput.value.trim();
  if (keyword) void store.search(keyword);
}

function clearSearch(): void {
  searchInput.value = '';
  store.clearSearch();
}

// ---------- 会话 ----------
watch(
  () => props.isOpen,
  (open) => {
    if (open) void store.startSession();
    else store.stopSession();
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
    <div class="mail">
      <!-- 顶栏 -->
      <header class="ml-header">
        <button type="button" class="ml-icon-btn" aria-label="返回" @click="emit('close')">
          <ArrowLeft :size="20" />
        </button>
        <div class="ml-title-block">
          <span class="ml-title">邮箱</span>
          <span class="ml-subtitle">clawEmail</span>
        </div>
        <button
          type="button"
          class="ml-icon-btn"
          aria-label="刷新邮箱"
          title="刷新邮箱（穿透云端）"
          @click="store.refresh()"
        >
          <RefreshCw :size="17" :class="{ 'custom-spin': store.isLoading }" />
        </button>
      </header>

      <!-- 邮箱切换 + 仅未读 -->
      <section v-if="store.mailboxes.length > 0" class="ml-boxes">
        <div class="ml-box-row vcp-scrollable">
          <button
            v-for="box in store.mailboxes"
            :key="store.keyOf(box)"
            type="button"
            class="ml-box-chip"
            :class="{ 'is-active': store.selectedKey === store.keyOf(box), 'is-disabled': !box.enabled }"
            @click="store.selectMailbox(store.keyOf(box))"
          >
            <span
              class="ml-ws-dot"
              :class="{
                'is-on': store.wsStates.find((s) => s.user === box.user)?.connected,
              }"
              :title="store.wsStates.find((s) => s.user === box.user)?.connected ? '推送在线' : '推送离线'"
            />
            {{ box.agentName ?? box.label }}
          </button>
          <button
            type="button"
            class="ml-box-chip ml-unread-chip"
            :class="{ 'is-active': store.unreadOnly }"
            @click="store.toggleUnreadOnly()"
          >
            仅未读
          </button>
        </div>

        <!-- 搜索（补丁端点；不可用时隐藏） -->
        <div v-if="store.extendedApiSupported" class="ml-search">
          <Search :size="14" class="ml-search-icon" />
          <input
            v-model="searchInput"
            type="search"
            class="ml-search-input"
            placeholder="搜索主题 / 发件人 / 收件人…"
            enterkeyhint="search"
            @keyup.enter="submitSearch"
          />
          <button
            v-if="store.searchResults !== null"
            type="button"
            class="ml-search-clear"
            aria-label="清除搜索"
            @click="clearSearch"
          >
            <X :size="14" />
          </button>
        </div>

        <!-- 文件夹 chips（补丁端点；不可用时隐藏） -->
        <div v-if="store.folders.length > 0" class="ml-folder-row vcp-scrollable">
          <button
            type="button"
            class="ml-folder-chip"
            :class="{ 'is-active': store.currentFid === null }"
            @click="store.selectFolder(null)"
          >
            收件箱
          </button>
          <button
            v-for="folder in store.folders"
            :key="folder.id"
            type="button"
            class="ml-folder-chip"
            :class="{ 'is-active': store.currentFid === folder.id }"
            @click="store.selectFolder(folder.id)"
          >
            {{ folder.name }}<template v-if="folder.unreadCount"> ({{ folder.unreadCount }})</template>
          </button>
        </div>
      </section>

      <!-- 服务端错误横幅 -->
      <div v-if="store.serverError" class="ml-banner" role="alert">
        <CircleAlert :size="14" />
        <span>{{ store.serverError }}</span>
      </div>

      <!-- 整页空态 -->
      <div v-if="store.pluginUnavailable" class="ml-empty">
        <Mail :size="28" class="ml-empty-icon" />
        <p class="ml-empty-title">VCPClawMail 插件未加载</p>
        <p class="ml-empty-detail">请在 VCPToolBox 服务器上启用 VCPClawMail 插件后重试。</p>
        <button type="button" class="ml-retry-btn" @click="store.refresh()">重试</button>
      </div>
      <div v-else-if="store.error && !store.stateLoaded" class="ml-empty">
        <Mail :size="28" class="ml-empty-icon" />
        <p class="ml-empty-title">连接失败</p>
        <p class="ml-empty-detail">{{ store.error }}</p>
        <button type="button" class="ml-retry-btn" @click="store.refresh()">重试</button>
      </div>
      <div v-else-if="store.stateLoaded && store.mailboxes.length === 0" class="ml-empty">
        <Mail :size="28" class="ml-empty-icon" />
        <p class="ml-empty-title">尚未配置邮箱</p>
        <p class="ml-empty-detail">请在 VCPToolBox 的 VCPClawMail 插件配置 ClawMailUsers。</p>
      </div>

      <!-- 邮件列表 -->
      <div v-else class="ml-scroll vcp-scrollable no-rubber-band" data-mail-role="mail-list">
        <div v-if="store.searchResults !== null" class="ml-search-meta">
          搜索「{{ store.searchKeyword }}」 · {{ store.searchResults.length }} 封
        </div>
        <div v-if="store.displayedMails.length === 0" class="ml-empty">
          <p class="ml-empty-title">
            {{ store.searching || store.listLoading ? '正在读取邮件…' : store.searchResults !== null ? '没有匹配的邮件' : store.unreadOnly ? '没有未读邮件' : '这个邮箱是空的' }}
          </p>
        </div>

        <button
          v-for="mail in store.displayedMails"
          :key="mail.mailId"
          type="button"
          class="ml-row"
          :class="{ 'is-unread': mail.readState === 'unread' }"
          @click="openDetail(mail)"
        >
          <span class="ml-row-head">
            <span
              class="ml-unread-dot"
              :class="{ 'is-on': mail.readState === 'unread' }"
              aria-hidden="true"
            />
            <span class="ml-from">{{ mail.fromText || '未知发件人' }}</span>
            <Paperclip v-if="mail.hasAttachments" :size="12" class="ml-attach" aria-label="含附件" />
            <span class="ml-time">{{ mailTimeLabel(mail.dateMs) }}</span>
          </span>
          <span class="ml-subject">{{ mail.subject }}</span>
          <span v-if="mail.preview" class="ml-preview">{{ mail.preview }}</span>
        </button>

        <button
          v-if="store.searchResults === null && store.mails.length > 0"
          type="button"
          class="ml-more-btn"
          :disabled="store.loadingMore"
          @click="store.loadList(false)"
        >
          {{ store.loadingMore ? '加载中…' : '加载更多' }}
        </button>
      </div>

      <!-- 写信 FAB（子页打开时隐藏，避免浮在详情/写信页之上） -->
      <button
        v-if="store.extendedApiSupported && store.stateLoaded && !activeMail && !isComposeOpen"
        type="button"
        class="ml-fab"
        aria-label="写邮件"
        @click="isComposeOpen = true"
      >
        <PenLine :size="20" />
      </button>

      <!-- 已加载后的轮询错误横幅（不打断浏览） -->
      <div v-if="store.error && store.stateLoaded" class="ml-banner" role="alert">
        <CircleAlert :size="14" />
        <span>{{ store.error }}</span>
      </div>

      <!-- 详情（滑入子页） -->
      <Transition name="ml-detail-slide">
        <MailDetailView v-if="activeMail" :mail="activeMail" @close="closeDetail" />
      </Transition>

      <!-- 写信（滑入子页） -->
      <Transition name="ml-detail-slide">
        <MailComposeView v-if="isComposeOpen" mode="send" @close="isComposeOpen = false" />
      </Transition>
    </div>
  </SlidePage>
</template>

<style scoped>
.mail {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--primary-bg);
  color: var(--primary-text);
}

.ml-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: calc(var(--vcp-safe-top, 24px) + 10px) 12px 10px;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.ml-title-block {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.ml-title {
  font-size: 16px;
  font-weight: 800;
}

.ml-subtitle {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  opacity: 0.45;
  text-transform: uppercase;
}

.ml-icon-btn {
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

.ml-icon-btn:active {
  opacity: 1;
}

.ml-boxes {
  flex-shrink: 0;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-color);
}

.ml-box-row {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  white-space: nowrap;
}

.ml-box-chip {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
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

.ml-box-chip.is-active {
  opacity: 1;
  color: var(--highlight-text);
  border-color: var(--highlight-text);
}

.ml-box-chip.is-disabled {
  opacity: 0.35;
}

.ml-unread-chip {
  margin-left: auto;
}

.ml-search {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 36px;
  margin-top: 8px;
  padding: 0 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  background: var(--secondary-bg);
}

.ml-search-icon {
  opacity: 0.45;
  flex-shrink: 0;
}

.ml-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--primary-text);
  font-size: 13px;
}

.ml-search-clear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.5;
  flex-shrink: 0;
}

.ml-folder-row {
  display: flex;
  gap: 8px;
  margin-top: 8px;
  overflow-x: auto;
  white-space: nowrap;
}

.ml-folder-chip {
  flex-shrink: 0;
  min-height: 28px;
  padding: 0 11px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  font-size: 11.5px;
  font-weight: 700;
  opacity: 0.6;
}

.ml-folder-chip.is-active {
  opacity: 1;
  color: var(--highlight-text);
  border-color: var(--highlight-text);
}

.ml-search-meta {
  padding: 8px 2px 4px;
  font-size: 11px;
  opacity: 0.5;
}

.ml-fab {
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

.ml-fab:active {
  opacity: 0.85;
}

.ml-ws-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--border-color);
}

.ml-ws-dot.is-on {
  background: #10b981;
}

.ml-banner {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  font-size: 11px;
  color: #ef4444;
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.ml-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 4px 12px calc(var(--vcp-safe-bottom, 48px) + 12px);
}

.ml-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px 24px;
  text-align: center;
}

.ml-empty-icon {
  opacity: 0.35;
}

.ml-empty-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  opacity: 0.75;
}

.ml-empty-detail {
  margin: 0;
  font-size: 12px;
  opacity: 0.5;
  max-width: 28rem;
  word-break: break-all;
}

.ml-retry-btn {
  margin-top: 6px;
  padding: 8px 22px;
  border-radius: 999px;
  border: 1px solid var(--border-color);
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
}

/* ---- 邮件行（高密度线性 + 2px accent） ---- */
.ml-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 100%;
  padding: 10px 10px 10px 12px;
  border: none;
  border-left: 2px solid transparent;
  border-bottom: 1px solid var(--border-color);
  background: transparent;
  color: var(--primary-text);
  text-align: left;
}

.ml-row:active {
  border-left-color: var(--highlight-text);
  background: var(--secondary-bg);
}

.ml-row.is-unread {
  border-left-color: var(--highlight-text);
}

.ml-row-head {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.ml-unread-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
  background: transparent;
}

.ml-unread-dot.is-on {
  background: var(--highlight-text);
}

.ml-from {
  font-size: 12.5px;
  font-weight: 700;
  opacity: 0.85;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-row.is-unread .ml-from {
  opacity: 1;
}

.ml-attach {
  opacity: 0.45;
  flex-shrink: 0;
}

.ml-time {
  margin-left: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10.5px;
  opacity: 0.45;
  flex-shrink: 0;
}

.ml-subject {
  font-size: 13.5px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-preview {
  font-size: 11.5px;
  opacity: 0.5;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-more-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  min-height: 42px;
  margin-top: 8px;
  border: 1px dashed var(--border-color);
  border-radius: 10px;
  background: transparent;
  color: var(--highlight-text);
  font-size: 12px;
  font-weight: 700;
}

.ml-more-btn:disabled {
  opacity: 0.4;
}

/* 子页滑入动画（内敛：位移 + 透明度） */
.ml-detail-slide-enter-active,
.ml-detail-slide-leave-active {
  transition:
    transform 0.3s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.3s ease;
}

.ml-detail-slide-enter-from,
.ml-detail-slide-leave-to {
  transform: translateX(100%);
  opacity: 0.6;
}

@media (min-width: 768px) {
  .ml-scroll,
  .ml-boxes {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
