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
  ChevronDown,
  CircleAlert,
  Mail,
  Paperclip,
  PenLine,
  Search,
  X,
} from 'lucide-vue-next';
import SlidePage from '../../components/ui/SlidePage.vue';
import RefreshButton from '../../components/ui/RefreshButton.vue';
import MailDetailView from './MailDetailView.vue';
import MailComposeView from './MailComposeView.vue';
import { useModalHistory } from '../../core/composables/useModalHistory';
import { useMailStore } from './mailStore';
import { folderDisplayName, mailTimeLabel, type MailSummary } from './mailTypes';

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
const BOXES_MODAL_ID = 'Mail:Boxes';

watch(activeMail, (mail) => {
  if (mail) registerModal(DETAIL_MODAL_ID, () => closeDetail());
  else unregisterModal(DETAIL_MODAL_ID);
});

watch(isComposeOpen, (open) => {
  if (open) registerModal(COMPOSE_MODAL_ID, () => (isComposeOpen.value = false));
  else unregisterModal(COMPOSE_MODAL_ID);
});

// ---------- 邮箱切换面板 ----------
const isBoxSheetOpen = ref(false);

watch(isBoxSheetOpen, (open) => {
  if (open) registerModal(BOXES_MODAL_ID, () => (isBoxSheetOpen.value = false));
  else unregisterModal(BOXES_MODAL_ID);
});

function pickMailbox(key: string): void {
  isBoxSheetOpen.value = false;
  void store.selectMailbox(key);
}

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
  unregisterModal(BOXES_MODAL_ID);
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
        <RefreshButton label="刷新邮箱（穿透云端）" :loading="store.isLoading" @refresh="store.refresh()" />
      </header>

      <!-- 账户切换条 + 未读过滤 -->
      <section v-if="store.mailboxes.length > 0" class="ml-top">
        <div class="ml-account-row">
          <button
            type="button"
            class="ml-account-btn"
            aria-label="切换邮箱"
            @click="isBoxSheetOpen = true"
          >
            <span
              class="ml-ws-dot"
              :class="{ 'is-on': store.wsConnected }"
              :title="store.wsConnected ? '推送在线' : '推送离线'"
            />
            <span class="ml-account-main">
              <span class="ml-account-name">
                {{ store.selectedMailbox?.agentName ?? store.selectedMailbox?.label ?? '选择邮箱' }}
                <span v-if="store.selectedMailbox" class="ml-account-type">
                  {{ store.selectedMailbox.mailbox.startsWith('mail') ? '子邮箱' : '公共邮箱' }}
                </span>
              </span>
              <span class="ml-account-addr">{{ store.selectedMailbox?.user }}</span>
            </span>
            <ChevronDown :size="16" class="ml-account-chevron" />
          </button>
          <button
            type="button"
            class="ml-unread-toggle"
            :class="{ 'is-active': store.unreadOnly }"
            :aria-pressed="store.unreadOnly"
            title="只看未读邮件"
            @click="store.toggleUnreadOnly()"
          >
            <span class="ml-unread-toggle-dot" aria-hidden="true" />
            未读
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

        <!-- 文件夹（收件箱去重 + 系统文件夹语义化排序） -->
        <div v-if="store.displayFolders.length > 0" class="ml-folder-row vcp-scrollable">
          <button
            type="button"
            class="ml-folder-chip"
            :class="{ 'is-active': store.currentFid === null }"
            @click="store.selectFolder(null)"
          >
            收件箱
          </button>
          <button
            v-for="folder in store.displayFolders"
            :key="folder.id"
            type="button"
            class="ml-folder-chip"
            :class="{ 'is-active': store.currentFid === folder.id }"
            @click="store.selectFolder(folder.id)"
          >
            {{ folderDisplayName(folder.name) }}
          </button>
        </div>
      </section>

      <!-- 服务器最近错误（可关闭；仅提示，不代表当前操作失败） -->
      <div v-if="store.visibleServerError" class="ml-server-note" role="status">
        <CircleAlert :size="14" class="ml-server-note-icon" />
        <span class="ml-server-note-text">
          服务器最近报告了一个错误（可能来自后台同步）：{{ store.visibleServerError }}
        </span>
        <button
          type="button"
          class="ml-server-note-close"
          aria-label="知道了"
          @click="store.dismissServerError()"
        >
          <X :size="13" />
        </button>
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

      <!-- 邮箱切换面板（底部滑升） -->
      <Transition name="ml-fade">
        <div
          v-if="isBoxSheetOpen"
          class="ml-sheet-mask"
          @click="isBoxSheetOpen = false"
          @touchmove.prevent
        />
      </Transition>
      <Transition name="ml-sheet-slide">
        <section v-if="isBoxSheetOpen" class="ml-sheet" role="dialog" aria-label="切换邮箱">
          <header class="ml-sheet-header">
            <span class="ml-sheet-title">切换邮箱</span>
            <button
              type="button"
              class="ml-icon-btn"
              aria-label="关闭"
              @click="isBoxSheetOpen = false"
            >
              <X :size="17" />
            </button>
          </header>
          <div class="ml-sheet-list vcp-scrollable">
            <button
              v-for="box in store.mailboxes"
              :key="store.keyOf(box)"
              type="button"
              class="ml-sheet-row"
              :class="{ 'is-active': store.selectedKey === store.keyOf(box), 'is-disabled': !box.enabled }"
              @click="pickMailbox(store.keyOf(box))"
            >
              <span
                class="ml-ws-dot"
                :class="{ 'is-on': store.wsStates.find((s) => s.user === box.user)?.connected }"
              />
              <span class="ml-sheet-row-main">
                <span class="ml-sheet-row-name">{{ box.agentName ?? box.label }}</span>
                <span class="ml-sheet-row-addr">{{ box.user }}</span>
              </span>
              <span class="ml-sheet-row-side">
                <span
                  class="ml-sheet-row-type"
                  :class="{ 'is-sub': box.mailbox.startsWith('mail') }"
                >{{ box.mailbox.startsWith('mail') ? '子邮箱' : '公共' }}</span>
                <span class="ml-sheet-row-meta">
                  {{ store.wsStates.find((s) => s.user === box.user)?.connected ? '推送在线' : '推送离线' }} · 缓存 {{ box.cachedCount }}
                </span>
              </span>
            </button>
          </div>
        </section>
      </Transition>

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

.ml-top {
  flex-shrink: 0;
  padding: 8px 14px 10px;
  border-bottom: 1px solid var(--border-color);
}

/* ---- 账户切换条 ---- */
.ml-account-row {
  display: flex;
  align-items: stretch;
  gap: 8px;
}

.ml-account-btn {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 46px;
  padding: 6px 12px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  text-align: left;
  transition: opacity 0.15s ease;
}

.ml-account-btn:active {
  opacity: 0.75;
}

.ml-account-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.ml-account-name {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 800;
  overflow: hidden;
  white-space: nowrap;
}

.ml-account-type {
  flex-shrink: 0;
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid var(--border-color);
  font-size: 9px;
  font-weight: 700;
  opacity: 0.6;
}

.ml-account-addr {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-account-chevron {
  flex-shrink: 0;
  opacity: 0.4;
}

/* ---- 未读过滤开关 ---- */
.ml-unread-toggle {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 0 14px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: transparent;
  color: var(--primary-text);
  font-size: 12px;
  font-weight: 700;
  opacity: 0.6;
}

.ml-unread-toggle-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--border-color);
}

.ml-unread-toggle.is-active {
  opacity: 1;
  color: var(--highlight-text);
  border-color: var(--highlight-text);
}

.ml-unread-toggle.is-active .ml-unread-toggle-dot {
  background: var(--highlight-text);
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

/* ---- 服务器最近错误（琥珀色提示条，可关闭） ---- */
.ml-server-note {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 8px 14px 0;
  padding: 8px 10px 8px 12px;
  border: 1px solid rgba(245, 158, 11, 0.35);
  border-radius: 10px;
  background: rgba(245, 158, 11, 0.08);
  font-size: 11px;
  color: var(--primary-text);
  flex-shrink: 0;
}

.ml-server-note-icon {
  color: #f59e0b;
  flex-shrink: 0;
}

.ml-server-note-text {
  flex: 1;
  min-width: 0;
  opacity: 0.8;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.ml-server-note-close {
  width: 26px;
  height: 26px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: var(--primary-text);
  opacity: 0.5;
}

/* ---- 邮箱切换底部面板 ---- */
.ml-sheet-mask {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  z-index: var(--layer-local);
}

.ml-sheet {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  max-height: 62%;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border-color);
  border-radius: 16px 16px 0 0;
  background: var(--primary-bg);
  z-index: calc(var(--layer-local) + 1);
  padding-bottom: var(--vcp-safe-bottom, 48px);
}

.ml-sheet-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px 8px;
}

.ml-sheet-title {
  font-size: 14px;
  font-weight: 800;
}

.ml-sheet-list {
  overflow-y: auto;
  padding: 0 12px 12px;
}

.ml-sheet-row {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  margin-bottom: 6px;
  padding: 11px 12px;
  border: 1px solid var(--border-color);
  border-radius: 12px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  text-align: left;
}

.ml-sheet-row.is-active {
  border-color: var(--highlight-text);
}

.ml-sheet-row.is-disabled {
  opacity: 0.4;
}

.ml-sheet-row-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.ml-sheet-row-name {
  font-size: 13px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-sheet-row-addr {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 10px;
  opacity: 0.45;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ml-sheet-row-side {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 2px;
}

.ml-sheet-row-type {
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid var(--border-color);
  font-size: 9px;
  font-weight: 700;
  opacity: 0.6;
}

.ml-sheet-row-type.is-sub {
  color: var(--highlight-text);
  border-color: var(--highlight-text);
  opacity: 0.9;
}

.ml-sheet-row-meta {
  font-size: 9.5px;
  opacity: 0.45;
}

/* 面板动画 */
.ml-fade-enter-active,
.ml-fade-leave-active {
  transition: opacity 0.2s ease;
}

.ml-fade-enter-from,
.ml-fade-leave-to {
  opacity: 0;
}

.ml-sheet-slide-enter-active,
.ml-sheet-slide-leave-active {
  transition: transform 0.28s cubic-bezier(0.32, 0.72, 0, 1);
}

.ml-sheet-slide-enter-from,
.ml-sheet-slide-leave-to {
  transform: translateY(100%);
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
  .ml-top {
    max-width: 860px;
    width: 100%;
    margin: 0 auto;
  }
}
</style>
