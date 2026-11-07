<script setup lang="ts">
/**
 * V工程 · Git 模式（对齐桌面 VCPChat「V工程 / Git」侧栏）。
 *
 * 数据通道：桌面 VCPChat 的 Git 面板走 Electron 内部 IPC（手机桥不暴露），
 * 这里改用桌面插件 `PTYShellExecutor`（Windows 侧为 Git Bash）经桥接 `/invoke` 执行 git 命令，
 * 具体命令构造/解析/安全白名单在 `useProjectForgeGit.ts`，本组件只负责移动端交互与展示。
 *
 * 只读：工作区探测、status 分组、分支/ahead/behind、文件 diff、最近提交；
 * 写操作：勾选文件 + 提交信息 → 二次确认 → `git add -- <paths>` + `git commit -m`；
 *         推送 → 二次确认 → `git push`（无上游时 `git push --set-upstream origin <branch>`）。
 * 禁止 reset/checkout/clean 等破坏性命令（由数据层白名单强制）。
 */
import { computed, onMounted, ref, watch } from 'vue';
import {
  ArrowDown,
  ArrowUp,
  Check,
  ChevronLeft,
  FolderGit2,
  GitBranch,
  GitPullRequest,
  Loader2,
  Plus,
  RefreshCw,
  Upload,
  X,
} from "@lucide/vue";
import { pfInvoke, ptyExec, describeProjectForgeError } from '../projectForgeApi';
import {
  PF_GIT_GROUPS,
  createProjectForgeGit,
  type PfGitFileEntry,
  type PfGitGroup,
  type PfGitRuntimeDeps,
} from '../useProjectForgeGit';
import { useNotificationStore } from '../../../core/stores/notification';
import { useOverlayStore } from '../../../core/stores/overlay';
import PfGitDiff from './PfGitDiff.vue';

interface Props {
  /** 可选：由页面 shell 注入工作区列表；不传则自行调用 ProjectForge ListWorkspaces。 */
  workspaces?: { id?: string; alias?: string; path: string }[] | null;
  /** 可选：预选工作区路径（例如工程模式当前筛选的工作区）。 */
  workspacePath?: string;
  /** 可选：覆盖运行时依赖（供单测/嵌入使用；生产环境无需传入）。 */
  runtime?: PfGitRuntimeDeps | null;
}

const props = withDefaults(defineProps<Props>(), {
  workspaces: null,
  workspacePath: '',
  runtime: null,
});

const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();

function notify(type: 'success' | 'error' | 'info' | 'warning', message: string): void {
  notificationStore.addNotification({
    id: `pf-git-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    title: 'V工程 · Git',
    message,
    type,
    duration: type === 'error' ? 4200 : 2400,
    toastOnly: true,
  });
}

/** 生产运行时：PTYShellExecutor + ListWorkspaces + 全局确认弹层。 */
function buildDefaultRuntime(): PfGitRuntimeDeps {
  return {
    exec: async (command, options) => {
      try {
        return await ptyExec(command, options);
      } catch (error) {
        // 桥接错误（网络/鉴权/协议）转成中文可读文案，交给数据层展示
        throw new Error(describeProjectForgeError(error));
      }
    },
    listWorkspaces: async () => {
      if (props.workspaces !== null) {
        return props.workspaces.map((item) => ({
          id: item.id,
          alias: item.alias,
          path: item.path,
        }));
      }
      const result = await pfInvoke<{ workspaces?: { id?: string; alias?: string; path?: string }[] }>(
        'ListWorkspaces',
      );
      const raw = Array.isArray(result?.workspaces) ? result.workspaces : [];
      return raw.map((item) => ({
        id: item?.id,
        alias: item?.alias,
        path: String(item?.path ?? ''),
      }));
    },
    confirm: (options) =>
      overlayStore.showConfirm({
        title: options.title,
        message: options.message,
        isDanger: options.isDanger,
      }),
  };
}

const git = createProjectForgeGit(props.runtime ?? buildDefaultRuntime());

const {
  workspaces,
  workspaceScanning,
  workspacesError,
  pathValidating,
  currentShellPath,
  status,
  statusLoading,
  statusError,
  notice,
  opOutput,
  busy,
  busyOp,
  commitMessage,
  activeDiff,
  diffLoading,
  diffError,
  diffText,
  isRepo,
  groups,
  selectedCount,
  canCommit,
  canPush,
  loadWorkspaces,
  ensureWorkspaces,
  addWorkspaceByPath,
  selectWorkspace,
  refreshStatus,
  isSelected,
  toggleFile,
  setGroupSelected,
  toggleGroupCollapsed,
  isGroupCollapsed,
  openDiff,
  closeDiff,
  clearOpOutput,
  confirmAndCommit,
  confirmAndPush,
  newBranchMode,
  newBranchName,
  autoCreatePr,
  prBusy,
  prResult,
  ghStatus,
  ghAccount,
  githubContext,
  canCreatePr,
  enableNewBranchMode,
  setNewBranchName,
  regenerateNewBranchName,
  setAutoCreatePr,
  openPullRequest,
} = git;

// —— 本地 UI 状态 ——
const manualOpen = ref(false);
const manualPath = ref('');
const manualError = ref('');
const submittingManual = ref(false);

const visibleGroups = computed(() =>
  PF_GIT_GROUPS.map((meta) => {
    const entries: PfGitFileEntry[] = groups.value[meta.key] ?? [];
    let count = 0;
    for (const entry of entries) {
      if (isSelected(meta.key, entry.path)) count += 1;
    }
    return {
      ...meta,
      entries,
      selectedCount: count,
      allSelected: entries.length > 0 && count === entries.length,
      someSelected: count > 0,
    };
  }).filter((group) => group.entries.length > 0),
);

const canAddManual = computed(() => Boolean(manualPath.value.trim()) && !submittingManual.value);

function branchLabel(): string {
  const branch = status.value?.branch;
  if (!branch) return '';
  if (branch.detached) return '分离 HEAD';
  return branch.head || '（未知分支）';
}

function stripTrailingSlash(path: string): string {
  return path.endsWith('/') ? path.replace(/\/+$/, '') : path;
}

function baseName(path: string): string {
  const normalized = stripTrailingSlash(path);
  const index = normalized.lastIndexOf('/');
  return index === -1 ? normalized : normalized.slice(index + 1);
}

function dirName(path: string): string {
  const normalized = stripTrailingSlash(path);
  const index = normalized.lastIndexOf('/');
  return index === -1 ? '' : normalized.slice(0, index);
}

function statusClass(letter: string): string {
  if (letter === '!') return 'conflict';
  if (letter === '?') return 'untracked';
  return letter.toLowerCase();
}

// —— 工作区交互 ——
function onWorkspaceChange(event: Event): void {
  const value = (event.target as HTMLSelectElement | null)?.value ?? '';
  if (value) selectWorkspace(value);
}

async function onRefreshWorkspaces(): Promise<void> {
  await loadWorkspaces();
  if (!workspacesError.value && !workspaces.value.length) {
    notify('info', '没有检测到 Git 仓库，可手动输入仓库路径');
  }
}

function toggleManual(): void {
  manualOpen.value = !manualOpen.value;
  manualError.value = '';
}

async function submitManual(): Promise<void> {
  if (!canAddManual.value) return;
  submittingManual.value = true;
  manualError.value = '';
  try {
    const result = await addWorkspaceByPath(manualPath.value);
    if (!result.ok) {
      manualError.value = result.message;
      return;
    }
    manualPath.value = '';
    manualOpen.value = false;
    notify('success', result.message);
  } finally {
    submittingManual.value = false;
  }
}

// —— 变更分组交互 ——
function onFileCheck(group: PfGitGroup, path: string, event: Event): void {
  const checked = (event.target as HTMLInputElement | null)?.checked ?? false;
  toggleFile(group, path, checked);
}

function onGroupCheck(group: PfGitGroup, event: Event): void {
  const checked = (event.target as HTMLInputElement | null)?.checked ?? false;
  setGroupSelected(group, checked);
}

function openEntry(group: PfGitGroup, entry: PfGitFileEntry): void {
  void openDiff(group, entry.path);
}

/** 差异图例（对齐桌面端「改动前 / 改动后」语义）。 */
function activeDiffLegend(): string {
  if (!activeDiff.value) return '';
  const meta = PF_GIT_GROUPS.find((group) => group.key === activeDiff.value?.group);
  return meta ? `${meta.legendLeft} → ${meta.legendRight}` : '';
}

/** 差异为空时的提示（区分未跟踪目录 / 冲突 / 二进制等场景）。 */
function diffEmptyHint(): string {
  if (!activeDiff.value) return '';
  if (activeDiff.value.group === 'untracked') {
    if (activeDiff.value.path.endsWith('/')) {
      return '这是一个未跟踪的目录（git status 默认按目录聚合显示），可在源码模式展开查看具体文件。';
    }
    return '未跟踪的新文件没有文本差异（空文件或二进制文件）。';
  }
  return '没有可显示的差异（可能是二进制文件、空文件或仅权限/模式变化）。';
}

// —— 提交 / 推送 ——
async function onCommit(): Promise<void> {
  if (!canCommit.value) return;
  try {
    const ok = await confirmAndCommit();
    if (ok) {
      notify('success', '提交成功');
    } else if (notice.value?.type === 'error') {
      notify('error', notice.value.text);
    }
  } catch (error) {
    notify('error', `提交失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

async function onPush(): Promise<void> {
  if (!canPush.value) return;
  try {
    const ok = await confirmAndPush();
    if (ok) {
      notify(notice.value?.type === 'warning' ? 'warning' : 'success', notice.value?.text || '推送成功');
    } else if (notice.value?.type === 'error') {
      notify('error', notice.value.text);
    }
  } catch (error) {
    notify('error', `推送失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

// —— 分支 + PR（2026-11-07） ——
function onToggleNewBranch(event: Event): void {
  enableNewBranchMode(Boolean((event.target as HTMLInputElement | null)?.checked));
}

function onBranchNameInput(event: Event): void {
  setNewBranchName((event.target as HTMLInputElement | null)?.value ?? '');
}

function onRegenerateBranchName(): void {
  regenerateNewBranchName();
}

function onToggleAutoPr(event: Event): void {
  setAutoCreatePr(Boolean((event.target as HTMLInputElement | null)?.checked));
}

async function onCreatePr(): Promise<void> {
  if (!canCreatePr.value) return;
  try {
    const result = await openPullRequest();
    notify(result.ok ? 'success' : 'warning', result.message);
  } catch (error) {
    notify('error', `创建 PR 失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

onMounted(() => {
  void ensureWorkspaces({ preferredPath: props.workspacePath });
});

watch(
  () => props.workspacePath,
  (value) => {
    if (value) void ensureWorkspaces({ preferredPath: value });
  },
);

// 供页面 shell（顶部刷新按钮）调用
defineExpose({
  refresh: () => loadWorkspaces(),
  refreshStatus: () => refreshStatus(),
  ensureWorkspaces,
  openWorkspace: selectWorkspace,
});
</script>

<template>
  <div class="pf-git" :class="{ 'is-busy': busy }" aria-label="Git 模式">
    <!-- 顶部：工作区选择 / 刷新 / 手动输入路径 -->
    <header class="pf-git-head">
      <div class="pf-git-head-row">
        <FolderGit2 :size="18" class="pf-git-head-icon" aria-hidden="true" />
        <select
          v-if="workspaces.length"
          class="pf-git-select"
          :value="currentShellPath"
          :disabled="busy || workspaceScanning"
          aria-label="Git 工作区"
          @change="onWorkspaceChange"
        >
          <option v-for="workspace in workspaces" :key="workspace.shellPath" :value="workspace.shellPath">
            {{ workspace.alias }} · {{ workspace.path }}
          </option>
        </select>
        <span v-else class="pf-git-head-empty">未检测到 Git 工作区</span>
        <button
          type="button"
          class="pf-git-icon-btn"
          :disabled="busy || workspaceScanning"
          aria-label="重新检测工作区中的 Git 仓库"
          title="重新检测工作区中的 Git 仓库"
          @click="onRefreshWorkspaces"
        >
          <Loader2 v-if="workspaceScanning" :size="18" class="pf-git-spin" />
          <RefreshCw v-else :size="18" />
        </button>
        <button
          type="button"
          class="pf-git-icon-btn"
          :class="{ 'is-active': manualOpen }"
          aria-label="手动输入 Git 仓库路径"
          title="手动输入 Git 仓库路径"
          @click="toggleManual"
        >
          <Plus :size="18" />
        </button>
      </div>

      <form v-if="manualOpen" class="pf-git-manual" @submit.prevent="submitManual">
        <input
          v-model="manualPath"
          class="pf-git-manual-input"
          type="text"
          placeholder="Git 仓库路径，如 D:\Workspace\Project"
          autocomplete="off"
          spellcheck="false"
          :disabled="submittingManual"
          aria-label="Git 仓库路径"
        />
        <button type="submit" class="pf-git-manual-btn" :disabled="!canAddManual || pathValidating">
          {{ pathValidating ? '检测中…' : '添加' }}
        </button>
      </form>

      <p v-if="manualError" class="pf-git-notice error">{{ manualError }}</p>
      <p v-if="workspacesError" class="pf-git-notice error">{{ workspacesError }}</p>
      <p v-else-if="!workspaces.length && !workspaceScanning" class="pf-git-head-hint">
        ProjectForge 登记的工作区里没有 Git 仓库；可点「＋」手动输入仓库路径。
      </p>
      <p v-else-if="workspaceScanning" class="pf-git-head-hint">正在检测工作区中的 Git 仓库…</p>
    </header>

    <!-- 差异子页 -->
    <template v-if="activeDiff">
      <div class="pf-git-diff-head">
        <button type="button" class="pf-git-back" aria-label="返回变更列表" @click="closeDiff">
          <ChevronLeft :size="18" />
          <span>返回变更</span>
        </button>
        <span class="pf-git-diff-status" :class="`st-${statusClass(activeDiff.status)}`">
          {{ activeDiff.status }}
        </span>
        <span class="pf-git-diff-label">{{ activeDiff.label }}</span>
      </div>
      <PfGitDiff
        :text="diffText"
        :loading="diffLoading"
        :error="diffError"
        :title="activeDiff.displayPath"
        :group-label="activeDiffLegend()"
        :empty-hint="diffEmptyHint()"
      />
    </template>

    <!-- 变更列表 + 提交区 -->
    <template v-else>
      <div v-if="isRepo && status" class="pf-git-branch">
        <GitBranch :size="15" class="pf-git-branch-icon" aria-hidden="true" />
        <span class="pf-git-branch-name">{{ branchLabel() }}</span>
        <span v-if="status.branch.upstream" class="pf-git-upstream" :title="status.branch.upstream">
          {{ status.branch.upstream }}
        </span>
        <span v-else class="pf-git-upstream is-muted">无上游</span>
        <span v-if="status.branch.ahead > 0" class="pf-git-ahead" title="领先上游">
          <ArrowUp :size="12" />{{ status.branch.ahead }}
        </span>
        <span v-if="status.branch.behind > 0" class="pf-git-behind" title="落后上游">
          <ArrowDown :size="12" />{{ status.branch.behind }}
        </span>
        <span v-if="status.branch.initial" class="pf-git-initial">尚无提交</span>
        <span v-if="status.prefix" class="pf-git-prefix" :title="status.topLevel">子目录：{{ status.prefix }}</span>
      </div>

      <div v-if="statusError" class="pf-git-notice error">{{ statusError }}</div>
      <div v-else-if="notice" class="pf-git-notice" :class="notice.type">{{ notice.text }}</div>

      <!-- 操作输出面板：展示 stdout/stderr 原文 -->
      <div v-if="opOutput" class="pf-git-op" :class="opOutput.ok ? 'success' : 'error'">
        <div class="pf-git-op-head">
          <span class="pf-git-op-title">
            {{ opOutput.title }}
          </span>
          <button type="button" class="pf-git-icon-btn" aria-label="关闭输出" @click="clearOpOutput">
            <X :size="16" />
          </button>
        </div>
        <div class="pf-git-op-command">$ {{ opOutput.command }}</div>
        <pre class="pf-git-op-text">{{ opOutput.text }}</pre>
      </div>

      <div class="pf-git-body">
        <div v-if="!currentShellPath" class="pf-git-empty">
          <p>请在上方选择 Git 工作区，或手动输入仓库路径。</p>
        </div>
        <div v-else-if="statusLoading && !status" class="pf-git-empty">
          <Loader2 :size="16" class="pf-git-spin" />
          <p>正在读取 Git 状态…</p>
        </div>
        <div v-else-if="status && !status.isRepo" class="pf-git-empty">
          <p>{{ status.message || statusError || '当前工作区不是 Git 仓库' }}</p>
          <p class="is-muted">可点上方「＋」手动输入其它仓库路径。</p>
        </div>
        <div v-else-if="status && status.total === 0" class="pf-git-empty">
          <p>干净的工作区，没有未提交的变更。</p>
          <p v-if="status.branch.initial" class="is-muted">当前分支还没有任何提交，填写提交信息后可创建首个提交。</p>
        </div>
        <div v-else-if="status" class="pf-git-groups">
          <section v-for="group in visibleGroups" :key="group.key" class="pf-git-group">
            <div class="pf-git-group-head">
              <input
                class="pf-git-check"
                type="checkbox"
                :checked="group.allSelected"
                :indeterminate.prop="group.someSelected && !group.allSelected"
                :aria-label="`全选 ${group.title}`"
                @change="onGroupCheck(group.key, $event)"
              />
              <button
                type="button"
                class="pf-git-group-toggle"
                :aria-expanded="!isGroupCollapsed(group.key)"
                @click="toggleGroupCollapsed(group.key)"
              >
                <span class="pf-git-chev" :class="{ 'is-collapsed': isGroupCollapsed(group.key) }">▾</span>
                <span class="pf-git-group-title">{{ group.title }}</span>
                <span class="pf-git-count">{{ group.entries.length }}</span>
              </button>
              <span v-if="group.selectedCount" class="pf-git-sel-chip">已选 {{ group.selectedCount }}</span>
            </div>
            <ul v-show="!isGroupCollapsed(group.key)" class="pf-git-list">
              <li
                v-for="entry in group.entries"
                :key="`${group.key}:${entry.path}`"
                class="pf-git-row"
                :class="{ 'is-selected': isSelected(group.key, entry.path) }"
              >
                <input
                  class="pf-git-check"
                  type="checkbox"
                  :checked="isSelected(group.key, entry.path)"
                  :aria-label="`选择 ${entry.displayPath}`"
                  @change="onFileCheck(group.key, entry.path, $event)"
                />
                <button
                  type="button"
                  class="pf-git-open"
                  :title="entry.origDisplayPath ? `${entry.origDisplayPath} → ${entry.displayPath}` : entry.path"
                  @click="openEntry(group.key, entry)"
                >
                  <span class="pf-git-status" :class="`st-${statusClass(entry.status)}`" :title="entry.label">
                    {{ entry.status }}
                  </span>
                  <span class="pf-git-file">
                    <span class="pf-git-name" :class="{ 'is-deleted': entry.status === 'D' }">
                      {{ baseName(entry.displayPath) }}{{ entry.path.endsWith('/') ? '/' : '' }}
                    </span>
                    <span v-if="dirName(entry.displayPath)" class="pf-git-dir">
                      {{ dirName(entry.displayPath) }}
                    </span>
                  </span>
                </button>
              </li>
            </ul>
          </section>

          <details v-if="status.log.length" class="pf-git-log">
            <summary>最近提交（{{ status.log.length }}）</summary>
            <ul>
              <li v-for="item in status.log" :key="item.hash">
                <code>{{ item.hash.slice(0, 7) }}</code>
                <span>{{ item.subject }}</span>
              </li>
            </ul>
          </details>
        </div>
      </div>

      <!-- 提交区（固定在面板底部） -->
      <div class="pf-git-commit">
        <textarea
          v-model="commitMessage"
          class="pf-git-message"
          rows="2"
          maxlength="20000"
          placeholder="提交信息（必填）"
          aria-label="提交信息"
          :disabled="busy"
        ></textarea>
        <div class="pf-git-branchbar">
          <label class="pf-git-toggle">
            <input
              type="checkbox"
              :checked="newBranchMode"
              :disabled="busy || prBusy"
              @change="onToggleNewBranch"
            />
            <span>新建修改分支</span>
          </label>
          <template v-if="newBranchMode">
            <input
              class="pf-git-branch-input"
              :value="newBranchName"
              :disabled="busy || prBusy"
              placeholder="分支名（默认 vcp/时间戳）"
              aria-label="新分支名"
              @input="onBranchNameInput"
            />
            <button
              type="button"
              class="pf-git-mini"
              :disabled="busy || prBusy"
              title="重新生成分支名"
              @click="onRegenerateBranchName"
            >
              生成
            </button>
          </template>
        </div>
        <div v-if="githubContext" class="pf-git-branchbar">
          <label v-if="newBranchMode" class="pf-git-toggle">
            <input
              type="checkbox"
              :checked="autoCreatePr"
              :disabled="busy || prBusy"
              @change="onToggleAutoPr"
            />
            <span>推送后自动创建 PR</span>
          </label>
          <span class="pf-git-sub">
            {{ githubContext.isFork ? `fork → ${githubContext.baseRepo.owner}/${githubContext.baseRepo.repo}` : `${githubContext.origin.owner}/${githubContext.origin.repo}` }}
            <template v-if="ghStatus === 'available' && ghAccount"> · gh: {{ ghAccount }}</template>
          </span>
        </div>
        <div v-if="prResult" class="pf-git-prresult" :class="{ 'is-error': !prResult.ok }">
          {{ prResult.message }}
        </div>
        <div class="pf-git-actions">
          <button type="button" class="pf-git-btn primary" :disabled="!canCommit" @click="onCommit">
            <Loader2 v-if="busyOp === 'commit'" :size="16" class="pf-git-spin" />
            <Check v-else :size="16" />
            <span>{{ busyOp === 'commit' ? '提交中…' : `提交${selectedCount ? `（${selectedCount}）` : ''}` }}</span>
          </button>
          <button type="button" class="pf-git-btn" :disabled="!canPush" @click="onPush">
            <Loader2 v-if="busyOp === 'push'" :size="16" class="pf-git-spin" />
            <Upload v-else :size="16" />
            <span>{{ busyOp === 'push' ? '推送中…' : '推送' }}</span>
          </button>
          <button
            v-if="githubContext && newBranchMode"
            type="button"
            class="pf-git-btn"
            :disabled="!canCreatePr"
            @click="onCreatePr"
          >
            <Loader2 v-if="prBusy" :size="16" class="pf-git-spin" />
            <GitPullRequest v-else :size="16" />
            <span>{{ prBusy ? '创建 PR 中…' : '创建 PR' }}</span>
          </button>
        </div>
        <p class="pf-git-commit-hint">
          勾选文件 → 填写提交信息 → 提交（git add + git commit）；可勾选「新建修改分支」避免直推默认分支；推送前会二次确认，GitHub 远端可自动创建 PR。
        </p>
      </div>
    </template>
  </div>
</template>

<style scoped>
.pf-git {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
  background: var(--primary-bg, #1c1c1e);
  color: var(--primary-text, #e0e0e0);
  font-size: 14px;
}

.pf-git.is-busy {
  cursor: progress;
}

/* ---------- 顶部工作区行 ---------- */
.pf-git-head {
  flex-shrink: 0;
  padding: 8px 10px 6px;
  border-bottom: 1px solid var(--border-color, #3a3a3e);
}

.pf-git-head-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.pf-git-head-icon {
  color: var(--highlight-text, #3498db);
  flex-shrink: 0;
}

.pf-git-select {
  flex: 1;
  min-width: 0;
  height: 44px;
  padding: 0 8px;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
  font-size: 13px;
}

.pf-git-select:focus {
  outline: none;
  border-color: var(--highlight-text, #3498db);
}

.pf-git-select option {
  background: var(--primary-bg, #1c1c1e);
  color: var(--primary-text, #e0e0e0);
}

.pf-git-head-empty {
  flex: 1;
  min-width: 0;
  height: 44px;
  display: flex;
  align-items: center;
  color: var(--secondary-text, rgba(224, 224, 224, 0.6));
  font-size: 13px;
}

.pf-git-icon-btn {
  flex-shrink: 0;
  width: 44px;
  height: 44px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid transparent;
  border-radius: 6px;
  background: transparent;
  color: var(--primary-text, #e0e0e0);
  cursor: pointer;
}

.pf-git-icon-btn:hover:not(:disabled) {
  background: var(--secondary-bg, #28282c);
}

.pf-git-icon-btn.is-active {
  color: var(--highlight-text, #3498db);
  border-color: var(--highlight-text, #3498db);
}

.pf-git-icon-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.pf-git-head-hint {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.55));
}

/* ---------- 手动路径输入 ---------- */
.pf-git-manual {
  display: flex;
  gap: 6px;
  margin-top: 8px;
}

.pf-git-manual-input {
  flex: 1;
  min-width: 0;
  height: 44px;
  padding: 0 10px;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
  font-size: 13px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.pf-git-manual-input:focus {
  outline: none;
  border-color: var(--highlight-text, #3498db);
}

.pf-git-manual-btn {
  flex-shrink: 0;
  min-width: 64px;
  height: 44px;
  border: 1px solid var(--highlight-text, #3498db);
  border-radius: 6px;
  background: transparent;
  color: var(--highlight-text, #3498db);
  font-size: 13px;
  cursor: pointer;
}

.pf-git-manual-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

/* ---------- 提示 / 输出 ---------- */
.pf-git-notice {
  flex-shrink: 0;
  margin: 6px 10px 0;
  padding: 8px 10px;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  font-size: 12px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

.pf-git-notice.error {
  color: #f85149;
  border-color: rgba(248, 81, 73, 0.55);
}

.pf-git-notice.warning {
  color: #d29922;
  border-color: rgba(210, 153, 34, 0.55);
}

.pf-git-notice.success {
  color: #3fb950;
  border-color: rgba(63, 185, 80, 0.55);
}

.pf-git-notice.info {
  color: var(--secondary-text, rgba(224, 224, 224, 0.7));
}

.pf-git-op {
  flex-shrink: 0;
  margin: 6px 10px 0;
  border: 1px solid rgba(248, 81, 73, 0.55);
  border-radius: 6px;
  overflow: hidden;
}

.pf-git-op.success {
  border-color: rgba(63, 185, 80, 0.55);
}

.pf-git-op-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 2px 0 10px;
  min-height: 40px;
}

.pf-git-op-title {
  font-size: 12px;
  font-weight: 600;
  color: #f85149;
}

.pf-git-op.success .pf-git-op-title {
  color: #3fb950;
}

.pf-git-op-command {
  padding: 0 10px 4px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.55));
  word-break: break-all;
}

.pf-git-op-text {
  margin: 0;
  padding: 8px 10px;
  max-height: 168px;
  overflow: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
}

/* ---------- 分支行 ---------- */
.pf-git-branch {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 10px;
  border-bottom: 1px solid var(--border-color, #3a3a3e);
  font-size: 12px;
}

.pf-git-branch-icon {
  color: var(--highlight-text, #3498db);
  flex-shrink: 0;
}

.pf-git-branch-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-weight: 600;
  font-size: 13px;
  max-width: 46%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-git-upstream {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  color: var(--secondary-text, rgba(224, 224, 224, 0.6));
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-git-upstream.is-muted {
  font-family: inherit;
}

.pf-git-ahead {
  display: inline-flex;
  align-items: center;
  gap: 1px;
  color: #3fb950;
}

.pf-git-behind {
  display: inline-flex;
  align-items: center;
  gap: 1px;
  color: #f85149;
}

.pf-git-initial {
  color: #d29922;
}

.pf-git-prefix {
  color: var(--secondary-text, rgba(224, 224, 224, 0.55));
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ---------- 变更列表 ---------- */
.pf-git-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  -webkit-overflow-scrolling: touch;
  overscroll-behavior: contain;
}

.pf-git-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 32px 20px;
  text-align: center;
  color: var(--secondary-text, rgba(224, 224, 224, 0.65));
  font-size: 13px;
  line-height: 1.6;
}

.pf-git-empty p {
  margin: 0;
}

.pf-git-empty .is-muted {
  color: var(--secondary-text, rgba(224, 224, 224, 0.45));
  font-size: 12px;
}

.pf-git-groups {
  padding: 4px 4px 12px;
}

.pf-git-group + .pf-git-group {
  margin-top: 2px;
}

.pf-git-group-head {
  display: flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 6px 0 10px;
  border-radius: 6px;
}

.pf-git-group-toggle {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 44px;
  border: none;
  background: transparent;
  color: var(--primary-text, #e0e0e0);
  font-size: 13px;
  font-weight: 600;
  text-align: left;
  cursor: pointer;
}

.pf-git-chev {
  display: inline-block;
  width: 12px;
  font-size: 12px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.55));
  transition: transform 0.15s ease;
}

.pf-git-chev.is-collapsed {
  transform: rotate(-90deg);
}

.pf-git-group-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pf-git-count {
  flex-shrink: 0;
  min-width: 20px;
  padding: 0 5px;
  border-radius: 8px;
  background: var(--secondary-bg, #28282c);
  color: var(--secondary-text, rgba(224, 224, 224, 0.65));
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  text-align: center;
  line-height: 18px;
}

.pf-git-sel-chip {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--highlight-text, #3498db);
}

.pf-git-check {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  margin: 0;
  accent-color: var(--highlight-text, #3498db);
  cursor: pointer;
}

.pf-git-list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.pf-git-row {
  display: flex;
  align-items: center;
  gap: 4px;
  min-height: 44px;
  padding: 0 6px 0 10px;
  border-radius: 6px;
}

.pf-git-row:hover {
  background: var(--secondary-bg, #28282c);
}

.pf-git-row.is-selected {
  box-shadow: inset 2px 0 0 var(--highlight-text, #3498db);
}

.pf-git-open {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 44px;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--primary-text, #e0e0e0);
  text-align: left;
  cursor: pointer;
}

.pf-git-status {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid currentColor;
  border-radius: 3px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  font-weight: 700;
  line-height: 1;
}

.pf-git-status.st-m {
  color: #d29922;
}

.pf-git-status.st-a {
  color: #3fb950;
}

.pf-git-status.st-d {
  color: #f85149;
}

.pf-git-status.st-r,
.pf-git-status.st-c {
  color: #58a6ff;
}

.pf-git-status.st-t {
  color: #bc8cff;
}

.pf-git-status.st-u,
.pf-git-status.st-conflict {
  color: #f85149;
  background: rgba(248, 81, 73, 0.14);
}

.pf-git-status.st-untracked {
  color: #8b949e;
  border-style: dashed;
}

.pf-git-file {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.pf-git-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pf-git-name.is-deleted {
  text-decoration: line-through;
  opacity: 0.75;
}

.pf-git-dir {
  min-width: 0;
  color: var(--secondary-text, rgba(224, 224, 224, 0.45));
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ---------- 最近提交 ---------- */
.pf-git-log {
  margin: 8px 6px 0;
  border-top: 1px solid var(--border-color, #3a3a3e);
  padding-top: 6px;
  font-size: 12px;
}

.pf-git-log summary {
  min-height: 44px;
  display: flex;
  align-items: center;
  color: var(--secondary-text, rgba(224, 224, 224, 0.65));
  cursor: pointer;
}

.pf-git-log ul {
  list-style: none;
  margin: 0;
  padding: 0 0 6px;
}

.pf-git-log li {
  display: flex;
  gap: 8px;
  padding: 4px 0;
  min-width: 0;
}

.pf-git-log code {
  flex-shrink: 0;
  color: var(--highlight-text, #3498db);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}

.pf-git-log li span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--secondary-text, rgba(224, 224, 224, 0.8));
}

/* ---------- 提交区 ---------- */
.pf-git-commit {
  flex-shrink: 0;
  padding: 8px 10px calc(8px + var(--vcp-safe-bottom, 0px));
  border-top: 1px solid var(--border-color, #3a3a3e);
  background: var(--primary-bg, #1c1c1e);
}

.pf-git-message {
  width: 100%;
  box-sizing: border-box;
  min-height: 64px;
  max-height: 140px;
  padding: 8px 10px;
  resize: vertical;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
  font-size: 13px;
  line-height: 1.5;
  font-family: inherit;
}

.pf-git-message:focus {
  outline: none;
  border-color: var(--highlight-text, #3498db);
}

.pf-git-message:disabled {
  opacity: 0.55;
}

.pf-git-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}

.pf-git-btn {
  flex: 1;
  min-height: 44px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 12px;
  border: 1px solid var(--border-color, #3a3a3e);
  border-radius: 6px;
  background: var(--secondary-bg, #28282c);
  color: var(--primary-text, #e0e0e0);
  font-size: 14px;
  cursor: pointer;
}

.pf-git-btn.primary {
  border-color: var(--highlight-text, #3498db);
  color: var(--highlight-text, #3498db);
  font-weight: 600;
}

.pf-git-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.pf-git-branchbar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin: 0 0 6px;
}

.pf-git-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--primary-text, #e0e0e0);
  cursor: pointer;
  user-select: none;
}

.pf-git-toggle input[type='checkbox'] {
  width: 15px;
  height: 15px;
  accent-color: var(--pf-ok, #3ecf8e);
}

.pf-git-branch-input {
  flex: 1 1 150px;
  min-width: 120px;
  height: 30px;
  padding: 0 8px;
  border-radius: 7px;
  border: 1px solid rgba(128, 128, 128, 0.35);
  background: rgba(127, 127, 127, 0.12);
  color: var(--primary-text, #e0e0e0);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
}

.pf-git-mini {
  flex: 0 0 auto;
  height: 30px;
  padding: 0 10px;
  border-radius: 7px;
  border: 1px solid rgba(128, 128, 128, 0.35);
  background: transparent;
  color: var(--primary-text, #e0e0e0);
  font-size: 12px;
  cursor: pointer;
}

.pf-git-mini:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.pf-git-sub {
  font-size: 11px;
  opacity: 0.65;
  word-break: break-all;
}

.pf-git-prresult {
  margin: 0 0 6px;
  padding: 6px 8px;
  border-radius: 7px;
  font-size: 11.5px;
  line-height: 1.45;
  word-break: break-all;
  background: color-mix(in srgb, var(--pf-ok, #3ecf8e) 12%, transparent);
  color: var(--pf-ok, #3ecf8e);
}

.pf-git-prresult.is-error {
  background: color-mix(in srgb, var(--pf-warn, #f5a623) 14%, transparent);
  color: var(--pf-warn, #f5a623);
}

.pf-git-commit-hint {
  margin: 6px 0 0;
  font-size: 11px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.45));
}

/* ---------- 差异子页头部 ---------- */
.pf-git-diff-head {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 48px;
  padding: 0 6px;
  border-bottom: 1px solid var(--border-color, #3a3a3e);
}

.pf-git-back {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  min-height: 44px;
  min-width: 44px;
  padding: 0 8px 0 4px;
  border: none;
  background: transparent;
  color: var(--highlight-text, #3498db);
  font-size: 13px;
  cursor: pointer;
}

.pf-git-diff-status {
  width: 18px;
  height: 18px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid currentColor;
  border-radius: 3px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  font-weight: 700;
}

.pf-git-diff-status.st-m {
  color: #d29922;
}

.pf-git-diff-status.st-a {
  color: #3fb950;
}

.pf-git-diff-status.st-d {
  color: #f85149;
}

.pf-git-diff-status.st-r,
.pf-git-diff-status.st-c {
  color: #58a6ff;
}

.pf-git-diff-status.st-t {
  color: #bc8cff;
}

.pf-git-diff-status.st-u,
.pf-git-diff-status.st-conflict {
  color: #f85149;
}

.pf-git-diff-status.st-untracked {
  color: #8b949e;
  border-style: dashed;
}

.pf-git-diff-label {
  font-size: 12px;
  color: var(--secondary-text, rgba(224, 224, 224, 0.6));
}

.pf-git-spin {
  animation: pf-git-spin 0.9s linear infinite;
}

@keyframes pf-git-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
</style>
