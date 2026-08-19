/**
 * agentMgrTypes.ts — Agent 管理（AgentAssistant config）类型与纯函数层。
 *
 * 上游契约（plan/vcpmobile-more-tools-research/08 篇）：
 * - config.json 顶层 7 个全局字段 + agents 数组；POST 为顶层浅合并；
 * - agents 元素 7 个已知字段；**未知键必须透传保留**（AdminPanel-Vue 的
 *   normalizeAgentEntry 重建对象导致扩展字段永久丢失，此处引以为戒）；
 * - 后端无校验：chineseName/modelId 缺失会被插件静默跳过，chineseName 重复
 *   运行时后写覆盖先写——校验全部在本层与 Rust 侧双重把关。
 */

// ---------- Agent 条目 ----------

export interface AgentEntry {
  chineseName: string;
  baseName: string;
  modelId: string;
  description: string;
  systemPrompt: string;
  maxOutputTokens: number;
  temperature: number;
  /** 未知扩展键原样透传（保存时铺回原对象）。 */
  extras: Record<string, unknown>;
}

const KNOWN_AGENT_KEYS = new Set([
  'chineseName',
  'baseName',
  'modelId',
  'description',
  'systemPrompt',
  'maxOutputTokens',
  'temperature',
]);

export const AGENT_DEFAULTS = {
  maxOutputTokens: 40000,
  temperature: 0.7,
  systemPrompt: 'You are a helpful AI assistant named {{MaidName}}.',
} as const;

function asNumber(value: unknown, fallback: number): number {
  const num = Number(value);
  return Number.isFinite(num) ? num : fallback;
}

/** 解析单个 agent 条目；chineseName 缺失/空白返回 null（与插件静默跳过对齐）。 */
export function normalizeAgentEntry(raw: unknown): AgentEntry | null {
  if (!raw || typeof raw !== 'object') return null;
  const record = raw as Record<string, unknown>;
  const chineseName = String(record.chineseName ?? '').trim();
  if (!chineseName) return null;

  const extras: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(record)) {
    if (!KNOWN_AGENT_KEYS.has(key)) extras[key] = value;
  }

  return {
    chineseName,
    baseName: String(record.baseName ?? ''),
    modelId: String(record.modelId ?? ''),
    description: String(record.description ?? ''),
    systemPrompt: String(record.systemPrompt ?? ''),
    maxOutputTokens: asNumber(record.maxOutputTokens, AGENT_DEFAULTS.maxOutputTokens),
    temperature: asNumber(record.temperature, AGENT_DEFAULTS.temperature),
    extras,
  };
}

/** 解析 agents 数组（非法条目丢弃；保持读入顺序——数组顺序即展示顺序）。 */
export function normalizeAgentList(raw: unknown): AgentEntry[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map(normalizeAgentEntry)
    .filter((entry): entry is AgentEntry => entry !== null);
}

// ---------- 全局配置 ----------

export interface GlobalConfig {
  maxHistoryRounds: number;
  contextTtlHours: number;
  globalSystemPrompt: string;
  delegationMaxRounds: number;
  /** 存储为毫秒（UI 按分钟展示，×60000 转换）。 */
  delegationTimeout: number;
  /** 留空 = 后端内置默认模板。 */
  delegationSystemPrompt: string;
  /** 留空 = 后端内置默认模板。 */
  delegationHeartbeatPrompt: string;
}

export const GLOBAL_DEFAULTS: GlobalConfig = {
  maxHistoryRounds: 7,
  contextTtlHours: 24,
  globalSystemPrompt: '',
  delegationMaxRounds: 15,
  delegationTimeout: 300000,
  delegationSystemPrompt: '',
  delegationHeartbeatPrompt: '',
};

export function normalizeGlobalConfig(raw: unknown): GlobalConfig {
  const record = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {};
  return {
    maxHistoryRounds: asNumber(record.maxHistoryRounds, GLOBAL_DEFAULTS.maxHistoryRounds),
    contextTtlHours: asNumber(record.contextTtlHours, GLOBAL_DEFAULTS.contextTtlHours),
    globalSystemPrompt: String(record.globalSystemPrompt ?? ''),
    delegationMaxRounds: asNumber(record.delegationMaxRounds, GLOBAL_DEFAULTS.delegationMaxRounds),
    delegationTimeout: asNumber(record.delegationTimeout, GLOBAL_DEFAULTS.delegationTimeout),
    delegationSystemPrompt: String(record.delegationSystemPrompt ?? ''),
    delegationHeartbeatPrompt: String(record.delegationHeartbeatPrompt ?? ''),
  };
}

// ---------- 编辑草稿 ----------

export interface AgentDraft {
  /** 编辑时的原始 chineseName（新建为 null）。改名检测与引用扫描依赖它。 */
  originalName: string | null;
  chineseName: string;
  baseName: string;
  modelId: string;
  description: string;
  systemPrompt: string;
  maxOutputTokens: number;
  temperature: number;
  extras: Record<string, unknown>;
}

export function emptyAgentDraft(): AgentDraft {
  return {
    originalName: null,
    chineseName: '',
    baseName: '',
    modelId: '',
    description: '',
    systemPrompt: '',
    maxOutputTokens: AGENT_DEFAULTS.maxOutputTokens,
    temperature: AGENT_DEFAULTS.temperature,
    extras: {},
  };
}

export function draftFromAgent(entry: AgentEntry): AgentDraft {
  return {
    originalName: entry.chineseName,
    chineseName: entry.chineseName,
    baseName: entry.baseName,
    modelId: entry.modelId,
    description: entry.description,
    systemPrompt: entry.systemPrompt,
    maxOutputTokens: entry.maxOutputTokens,
    temperature: entry.temperature,
    extras: { ...entry.extras },
  };
}

/** 草稿 → 提交对象：浅拷贝 extras 铺底，已知键覆盖（不重建丢字段）。 */
export function draftToAgentObject(draft: AgentDraft): Record<string, unknown> {
  return {
    ...draft.extras,
    chineseName: draft.chineseName.trim(),
    baseName: draft.baseName.trim(),
    modelId: draft.modelId.trim(),
    description: draft.description,
    systemPrompt: draft.systemPrompt,
    maxOutputTokens: draft.maxOutputTokens,
    temperature: draft.temperature,
  };
}

/**
 * 草稿校验（与 Rust 侧 validate_agents 同规则 + 数值边界）。
 * @param otherNames 其他 agent 的 chineseName（查重用，不含自身原始名）。
 */
export function validateAgentDraft(draft: AgentDraft, otherNames: string[]): string | null {
  const name = draft.chineseName.trim();
  if (!name) return '中文名（chineseName）不能为空——它是派发的唯一标识';
  if (otherNames.includes(name)) return `中文名「${name}」与已有 Agent 重复`;
  if (!draft.modelId.trim()) return '必须指定模型（modelId）';
  if (!Number.isFinite(draft.maxOutputTokens) || draft.maxOutputTokens <= 0) {
    return 'maxOutputTokens 必须是正整数';
  }
  if (!Number.isFinite(draft.temperature) || draft.temperature < 0 || draft.temperature > 2) {
    return 'temperature 需在 0 ~ 2 之间';
  }
  return null;
}

// ---------- 任务引用扫描（引用完整性：改名/删除前的受影响任务清单） ----------

/**
 * 在 task-assistant config 的 tasks[].targets.agents 中查找对指定 chineseName 的
 * 精确引用（randomN 魔法标签不会误匹配），返回任务名称列表。
 */
export function collectTaskReferences(taskConfigRaw: unknown, chineseName: string): string[] {
  if (!taskConfigRaw || typeof taskConfigRaw !== 'object') return [];
  const config = (taskConfigRaw as Record<string, unknown>).config ?? taskConfigRaw;
  const tasks = (config as Record<string, unknown>)?.tasks;
  if (!Array.isArray(tasks)) return [];

  const hits: string[] = [];
  for (const rawTask of tasks) {
    if (!rawTask || typeof rawTask !== 'object') continue;
    const task = rawTask as Record<string, unknown>;
    const targets = task.targets as Record<string, unknown> | undefined;
    const agents = targets?.agents;
    if (!Array.isArray(agents)) continue;
    if (agents.some((agent) => typeof agent === 'string' && agent.trim() === chineseName)) {
      hits.push(String(task.name ?? task.id ?? '未命名任务'));
    }
  }
  return hits;
}
