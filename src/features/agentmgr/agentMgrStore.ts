/**
 * agentMgrStore.ts — Agent 管理状态机。
 *
 * 读：agentmgr_get_config（原始 config，无 envelope）。
 * 写：agentmgr_save_config（Rust 侧 read-modify-write + 防御性校验）。
 * 并发防护：加载时记录 agents 指纹；保存前重新拉取比对，若他端改过则返回
 * 'conflict' 由视图二次确认（force 覆盖）。
 * 联动：保存成功后失效任务调度中心的 agent 快选缓存。
 */
import { defineStore } from 'pinia';
import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useNotificationStore } from '../../core/stores/notification';
import { useTaskCenterStore } from '../taskcenter/taskCenterStore';
import {
  GLOBAL_DEFAULTS,
  collectTaskReferences,
  draftToAgentObject,
  normalizeAgentList,
  normalizeGlobalConfig,
  type AgentDraft,
  type AgentEntry,
  type GlobalConfig,
} from './agentMgrTypes';

export type SaveResult = 'ok' | 'conflict' | 'error';

export const useAgentMgrStore = defineStore('agentMgr', () => {
  const notificationStore = useNotificationStore();

  const toast = (type: 'info' | 'success' | 'warning' | 'error', message: string) => {
    notificationStore.addNotification({
      type,
      title: 'Agent 管理',
      message,
      toastOnly: true,
    });
  };

  // ---------- 状态 ----------
  const agents = ref<AgentEntry[]>([]);
  const globalConfig = ref<GlobalConfig>({ ...GLOBAL_DEFAULTS });
  const configLoaded = ref(false);
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const saving = ref(false);
  const deleting = ref(false);

  /** 模型选择器数据源（懒加载一次）。 */
  const models = ref<string[]>([]);
  const modelsLoaded = ref(false);
  const modelsError = ref(false);

  /** 加载时 agents 数组的指纹（并发脏检测基线）。 */
  let loadedAgentsFingerprint = '';

  function fingerprintOf(rawAgents: unknown): string {
    return JSON.stringify(rawAgents ?? []);
  }

  function applyConfig(raw: Record<string, unknown>): void {
    agents.value = normalizeAgentList(raw.agents);
    globalConfig.value = normalizeGlobalConfig(raw);
    loadedAgentsFingerprint = fingerprintOf(raw.agents);
    configLoaded.value = true;
  }

  // ---------- 读 ----------
  async function loadConfig(): Promise<void> {
    if (isLoading.value) return;
    isLoading.value = true;
    try {
      const raw = await invoke<Record<string, unknown>>('agentmgr_get_config');
      applyConfig(raw);
      error.value = null;
    } catch (rawErr) {
      error.value = rawErr instanceof Error ? rawErr.message : String(rawErr);
    } finally {
      isLoading.value = false;
    }
  }

  async function loadModels(): Promise<void> {
    if (modelsLoaded.value) return;
    modelsLoaded.value = true;
    modelsError.value = false;
    try {
      const list = await invoke<unknown[]>('agentmgr_list_models');
      models.value = (Array.isArray(list) ? list : [])
        .filter((item): item is string => typeof item === 'string' && item.length > 0);
    } catch (rawErr) {
      modelsError.value = true;
      console.warn(
        '[AgentMgr] 模型列表加载失败:',
        rawErr instanceof Error ? rawErr.message : String(rawErr),
      );
    }
  }

  /** 页面关闭时复位（重开重新全量）。 */
  function resetSession(): void {
    agents.value = [];
    globalConfig.value = { ...GLOBAL_DEFAULTS };
    configLoaded.value = false;
    isLoading.value = false;
    error.value = null;
    saving.value = false;
    deleting.value = false;
    loadedAgentsFingerprint = '';
    // models 缓存保留：模型列表与 config 会话无关
  }

  // ---------- 写 ----------
  /**
   * 保存（payload 为要提交的顶层键集合；不含 agents 时服务端保留现状）。
   * force=false 且服务端 agents 与本 store 加载基线不一致时返回 'conflict'。
   */
  async function saveConfig(
    payload: Record<string, unknown>,
    options: { force?: boolean } = {},
  ): Promise<SaveResult> {
    if (saving.value) return 'error';
    saving.value = true;
    try {
      if (!options.force) {
        const latest = await invoke<Record<string, unknown>>('agentmgr_get_config');
        if (fingerprintOf(latest.agents) !== loadedAgentsFingerprint) {
          return 'conflict';
        }
      }
      await invoke('agentmgr_save_config', { config: payload });
      // 保存后重新拉取作为新基线（同时验证热重载生效）。
      const fresh = await invoke<Record<string, unknown>>('agentmgr_get_config');
      applyConfig(fresh);
      error.value = null;
      invalidateTaskAgentCache();
      return 'ok';
    } catch (rawErr) {
      const message = rawErr instanceof Error ? rawErr.message : String(rawErr);
      error.value = message;
      toast('error', `保存失败：${message}`);
      return 'error';
    } finally {
      saving.value = false;
    }
  }

  /** 保存单个 Agent（新建或按 originalName 替换）。 */
  async function saveAgent(
    draft: AgentDraft,
    options: { force?: boolean } = {},
  ): Promise<SaveResult> {
    // 以现有条目重建提交数组（draftToAgentObject 保证 extras 透传）。
    const rebuilt = agents.value.map((entry) =>
      draftToAgentObject({
        originalName: entry.chineseName,
        ...entry,
      }),
    );
    const object = draftToAgentObject(draft);
    const index = draft.originalName
      ? agents.value.findIndex((entry) => entry.chineseName === draft.originalName)
      : -1;
    if (index >= 0) rebuilt[index] = object;
    else rebuilt.push(object);

    const result = await saveConfig({ agents: rebuilt }, options);
    if (result === 'ok') {
      toast('success', draft.originalName ? `Agent「${object.chineseName}」已保存` : `Agent「${object.chineseName}」已创建`);
    }
    return result;
  }

  /** 删除 Agent（从数组移除后整体回写）。 */
  async function deleteAgent(
    chineseName: string,
    options: { force?: boolean } = {},
  ): Promise<SaveResult> {
    if (deleting.value) return 'error';
    deleting.value = true;
    try {
      const rebuilt = agents.value
        .filter((entry) => entry.chineseName !== chineseName)
        .map((entry) => draftToAgentObject({ originalName: entry.chineseName, ...entry }));
      const result = await saveConfig({ agents: rebuilt }, options);
      if (result === 'ok') toast('success', `Agent「${chineseName}」已删除`);
      return result;
    } finally {
      deleting.value = false;
    }
  }

  /** 保存全局字段（不提交 agents 键，服务端浅合并保留现状）。 */
  async function saveGlobalConfig(config: GlobalConfig): Promise<SaveResult> {
    const result = await saveConfig({ ...config });
    if (result === 'ok') toast('success', '全局设置已保存');
    return result;
  }

  // ---------- 引用扫描（改名/删除前） ----------
  /** 扫描任务调度中心对该 Agent 的引用，返回受影响任务名列表。失败降级为空列表。 */
  async function scanTaskReferences(chineseName: string): Promise<string[]> {
    try {
      const raw = await invoke<Record<string, unknown>>('task_get_config');
      return collectTaskReferences(raw, chineseName);
    } catch {
      return [];
    }
  }

  /** 联动：失效任务调度中心的 agent 快选缓存（下次打开选择器时重拉）。 */
  function invalidateTaskAgentCache(): void {
    try {
      const taskCenter = useTaskCenterStore();
      taskCenter.agentsLoaded = false;
    } catch {
      // 任务中心 store 未激活等场景静默跳过
    }
  }

  return {
    agents,
    globalConfig,
    configLoaded,
    isLoading,
    error,
    saving,
    deleting,
    models,
    modelsLoaded,
    modelsError,
    loadConfig,
    loadModels,
    resetSession,
    saveConfig,
    saveAgent,
    deleteAgent,
    saveGlobalConfig,
    scanTaskReferences,
  };
});
