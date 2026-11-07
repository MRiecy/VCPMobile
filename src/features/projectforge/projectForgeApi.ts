/**
 * V工程 / 桌面桥接（MobilePluginBridge）HTTP 客户端。
 *
 * 通道：`POST {base}/invoke`，body `{ toolName, args }`，鉴权 `Authorization: Bearer {token}`。
 * base/token 统一来自 `src/core/bridge/bridgeConfig.ts`（与「远程桌面 / 文坊」共用一份配置）。
 *
 * 成功响应（实测）：`{ ok:true, toolName, requestId, elapsedMs, result:{ content:[...], details:{...} } }`
 * 失败响应：HTTP 4xx/5xx + `{ ok:false, error:{ code, message } }`（不是 200+ok:false）。
 *
 * 本模块负责：
 * 1. `pfInvoke`     —— ProjectForge 24 命令调用，**details 优先**；details 为空时再把 content[0].text 当 JSON 解析。
 * 2. `pfInvokeEnvelope` —— 需要 content 多段正文时用（典型：ReadCode 正文在 content[1..]）。
 * 3. `pluginInvoke` —— 任意桌面插件（FileOperator / PTYShellExecutor …），返回语义同 pfInvoke。
 * 4. `ptyExec`      —— 走 PTYShellExecutor 执行 shell/git 命令，归一化 `{ text, ok, exitCode }`。
 * 5. 错误分级 `ProjectForgeError{kind: network|auth|bridge|protocol|validation}` + 超时（AbortController）。
 */
import { bridgeBaseUrl, bridgeToken } from '../../core/bridge/bridgeConfig';
import type { PfErrorKind } from './projectForgeTypes';

/** 桥接默认超时（毫秒）。ReadCode/GetNodeDiff 大文件较慢，读取类可通过 options.timeoutMs 单独放宽。 */
export const DEFAULT_BRIDGE_TIMEOUT_MS = 30_000;
/** PTY / Git 命令默认超时（毫秒）。 */
export const DEFAULT_PTY_TIMEOUT_MS = 120_000;

/** 桥接 content 段落。 */
export interface PfBridgeContentPart {
  type: string;
  text: string;
}

/** /invoke 成功响应 envelop。 */
export interface PfBridgeEnvelope {
  ok: boolean;
  result?: {
    content?: PfBridgeContentPart[];
    details?: unknown;
  };
  toolName?: string;
  requestId?: string;
  elapsedMs?: number;
  error?: { code?: string; message?: string };
}

/** 单次调用选项。 */
export interface PfInvokeOptions {
  /** 覆盖默认超时（毫秒）；<=0 表示不设超时。 */
  timeoutMs?: number;
  /** 外部取消信号（与超时合并）。 */
  signal?: AbortSignal;
}

/** PTY 执行额外选项。 */
export interface PtyExecOptions extends PfInvokeOptions {
  /**
   * 原样透传给 PTYShellExecutor 的额外参数（如桌面插件支持的 shell/cwd/后台模式等）。
   * 默认命令参数为 `{ command }`；这里可覆盖/补充，不会剥离 `command`。
   */
  args?: Record<string, unknown>;
}

/** PTY 执行归一化结果。 */
export interface PtyExecResult {
  /** 命令是否成功：能拿到退出码时 `exitCode === 0`；拿不到时退回桥接 details 的 ok/success（未知则 true）。 */
  ok: boolean;
  /** 合并后的输出文本（stdout/stderr 由桌面插件决定，通常 PTY 会合并；失败时同样会包含 stderr/错误信息）。 */
  text: string;
  /** 退出码；桌面插件未返回时为 null。 */
  exitCode: number | null;
  /** 桌面插件原始返回，供排查。 */
  raw: unknown;
}

/** 桥接调用错误分级。 */
export class ProjectForgeError extends Error {
  kind: PfErrorKind;
  status?: number;
  code?: string;

  constructor(kind: PfErrorKind, message: string, extra?: { status?: number; code?: string }) {
    super(message);
    this.name = 'ProjectForgeError';
    this.kind = kind;
    this.status = extra?.status;
    this.code = extra?.code;
  }
}

/** 当前桥接 Base（无末尾斜杠）。 */
export function getBridgeBase(): string {
  return String(bridgeBaseUrl.value ?? '').trim().replace(/\/+$/, '');
}

/** 当前桥接 Token。 */
export function getBridgeToken(): string {
  return String(bridgeToken.value ?? '').trim();
}

function errorMessageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error ?? '');
}

/** 尝试把文本解析为 JSON（容忍 ```json 围栏）。 */
function tryParseJsonLoose(text: string): { found: boolean; value?: unknown } {
  const raw = String(text ?? '').trim();
  if (!raw) return { found: false };
  const candidates = [raw];
  const fenced = /^```[a-zA-Z0-9_-]*[ \t]*\r?\n([\s\S]*?)\r?\n```$/.exec(raw);
  if (fenced) candidates.push(fenced[1].trim());
  for (const candidate of candidates) {
    if (!candidate) continue;
    const first = candidate[0];
    if (first !== '{' && first !== '[') continue;
    try {
      return { found: true, value: JSON.parse(candidate) };
    } catch {
      // 继续尝试下一个候选
    }
  }
  return { found: false };
}

/** details 是否为空（null / undefined / '' / 空数组 / 空对象）。 */
function isEmptyDetails(details: unknown): boolean {
  if (details === null || details === undefined || details === '') return true;
  if (Array.isArray(details)) return details.length === 0;
  if (typeof details === 'object') return Object.keys(details as Record<string, unknown>).length === 0;
  return false;
}

/** 取桥接响应的全部 content 段落。 */
export function bridgeContentParts(envelope: PfBridgeEnvelope): PfBridgeContentPart[] {
  const content = envelope.result?.content;
  if (!Array.isArray(content)) return [];
  return content.map((part) => ({
    type: String((part as PfBridgeContentPart)?.type ?? 'text'),
    text: typeof (part as PfBridgeContentPart)?.text === 'string' ? String((part as PfBridgeContentPart).text) : '',
  }));
}

/** 取桥接响应的 content 文本（多段用空行连接，跳过空段）。 */
export function bridgeContentText(envelope: PfBridgeEnvelope): string {
  return bridgeContentParts(envelope)
    .map((part) => part.text)
    .filter((text) => text.trim() !== '')
    .join('\n\n');
}

/**
 * 从 envelope 中解出可用结果：details 优先；details 为空时解析 content[0].text 的 JSON；
 * 仍解析不出则退回纯文本（给只返回文本的插件用）。两者都空则抛 protocol。
 */
export function unwrapBridgeResult<T = Record<string, unknown>>(command: string, envelope: PfBridgeEnvelope): T {
  const details = envelope.result?.details;
  if (!isEmptyDetails(details)) return details as T;

  const text = bridgeContentText(envelope);
  if (text) {
    const parsed = tryParseJsonLoose(text);
    if (parsed.found) return parsed.value as T;
    return text as unknown as T;
  }
  throw new ProjectForgeError(
    'protocol',
    `桌面端命令 ${command || '(空)'} 返回了空结果（既没有 details 也没有 content）`,
  );
}

/** 读响应体文本；读取失败按空串处理（错误分类交给状态码/JSON 检查）。 */
async function readResponseText(response: Response): Promise<string> {
  try {
    return await response.text();
  } catch {
    return '';
  }
}

/**
 * 统一发起一次 /invoke 请求并做协议校验，返回 envelope（不拆 details）。
 * 错误分级：network（网络不可达/超时/取消）、auth（401/403）、bridge（HTTP 4xx/5xx 或 ok:false）、
 * protocol（非 JSON / 缺 ok:true / 缺 result）、validation（本地参数）。
 */
async function invokeBridge(
  toolName: string,
  args: Record<string, unknown>,
  options: PfInvokeOptions = {},
): Promise<PfBridgeEnvelope> {
  const name = String(toolName ?? '').trim();
  if (!name) throw new ProjectForgeError('validation', '缺少 toolName，无法调用桌面桥接');
  const base = getBridgeBase();
  if (!base) throw new ProjectForgeError('validation', '桥接 Base 地址为空，请先在设置中填写桌面地址');

  const timeoutMs = options.timeoutMs === undefined ? DEFAULT_BRIDGE_TIMEOUT_MS : Number(options.timeoutMs);
  const controller = new AbortController();
  let timedOut = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const abortFromCaller = (): void => controller.abort();
  if (options.signal) {
    if (options.signal.aborted) controller.abort();
    else options.signal.addEventListener('abort', abortFromCaller, { once: true });
  }
  if (Number.isFinite(timeoutMs) && timeoutMs > 0) {
    timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);
  }

  let response: Response;
  try {
    response = await fetch(`${base}/invoke`, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${getBridgeToken()}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ toolName: name, args: args ?? {} }),
      signal: controller.signal,
    });
  } catch (error) {
    if (timedOut) {
      throw new ProjectForgeError(
        'network',
        `请求超时（${timeoutMs}ms）：桌面端未在限定时间内返回（${base}）。网络抖动或命令耗时较长时可重试`,
      );
    }
    if ((error as { name?: string })?.name === 'AbortError' || controller.signal.aborted) {
      throw new ProjectForgeError('network', '请求已取消');
    }
    throw new ProjectForgeError(
      'network',
      `无法连接桌面桥接（${base}）：网络不可达或地址错误，请确认手机与桌面端在同一网络并检查设置`,
    );
  } finally {
    if (timer !== null) clearTimeout(timer);
    if (options.signal) options.signal.removeEventListener('abort', abortFromCaller);
  }

  const status = Number(response.status ?? 0);
  const text = await readResponseText(response);

  // 401/403 优先按鉴权失败处理（反向代理也可能返回 HTML 错误页，此时提示检查 Token 更有用）
  if (status === 401 || status === 403) {
    let code: string | undefined;
    const parsedAuth = tryParseJsonLoose(text);
    if (parsedAuth.found && parsedAuth.value && typeof parsedAuth.value === 'object') {
      code = (parsedAuth.value as { error?: { code?: string } })?.error?.code;
    }
    throw new ProjectForgeError('auth', `桥接鉴权失败（HTTP ${status}）：请检查设置中的 Token`, { status, code });
  }

  let payload: Record<string, unknown> | null = null;
  if (text.trim()) {
    try {
      payload = JSON.parse(text) as Record<string, unknown>;
    } catch {
      throw new ProjectForgeError(
        'protocol',
        `桥接返回了非 JSON 响应（HTTP ${status}）：请确认 Base 地址指向 MobilePluginBridge`,
        { status },
      );
    }
  }

  const payloadError = (payload?.error ?? null) as { code?: string; message?: string } | null;
  if (!response.ok || payload?.ok === false) {
    const message =
      payloadError?.message ||
      (typeof payload?.message === 'string' ? payload.message : '') ||
      `桌面端命令执行失败（HTTP ${status}）`;
    throw new ProjectForgeError('bridge', String(message), { status, code: payloadError?.code });
  }

  if (!payload) {
    throw new ProjectForgeError('protocol', `桥接响应为空（HTTP ${status}），无法读取工程数据`, { status });
  }
  if (payload.ok !== true) {
    throw new ProjectForgeError('protocol', '桥接响应不是预期的 { ok:true } 结构，请确认 Base 地址指向 MobilePluginBridge', {
      status,
    });
  }
  if (!payload.result || typeof payload.result !== 'object') {
    throw new ProjectForgeError('protocol', '桥接响应缺少 result 字段，无法读取工程数据', { status });
  }

  const result = payload.result as { content?: PfBridgeContentPart[]; details?: unknown };
  return {
    ok: true,
    result: {
      content: Array.isArray(result.content) ? result.content : [],
      details: result.details,
    },
    toolName: typeof payload.toolName === 'string' ? payload.toolName : name,
    requestId: typeof payload.requestId === 'string' ? payload.requestId : undefined,
    elapsedMs: typeof payload.elapsedMs === 'number' ? payload.elapsedMs : undefined,
  };
}

/**
 * 调用 ProjectForge 命令（toolName 固定 ProjectForge）。
 * @param command 命令名，如 `ListProjects` / `GetProject` / `SearchHistory`（见 .lead/turn4/projectforge-commands.md）
 * @param args 命令参数（projectId、limit 等；command 会自动并入）
 * @returns details（优先）或 content[0].text 解析出的 JSON；只返回文本的插件则返回文本
 */
export async function pfInvoke<T = Record<string, unknown>>(
  command: string,
  args: Record<string, unknown> = {},
  options: PfInvokeOptions = {},
): Promise<T> {
  const name = String(command ?? '').trim();
  if (!name) throw new ProjectForgeError('validation', '缺少 command，无法调用 ProjectForge');
  const envelope = await invokeBridge('ProjectForge', { command: name, ...(args ?? {}) }, options);
  return unwrapBridgeResult<T>(name, envelope);
}

/** 调用 ProjectForge 命令并保留完整 envelope（需要 content 多段正文时用，如 ReadCode / GetNodeDiff）。 */
export async function pfInvokeEnvelope(
  command: string,
  args: Record<string, unknown> = {},
  options: PfInvokeOptions = {},
): Promise<PfBridgeEnvelope> {
  const name = String(command ?? '').trim();
  if (!name) throw new ProjectForgeError('validation', '缺少 command，无法调用 ProjectForge');
  return invokeBridge('ProjectForge', { command: name, ...(args ?? {}) }, options);
}

/**
 * 调用任意桌面插件（FileOperator / PTYShellExecutor / 其它经 MobilePluginBridge 暴露的工具）。
 * 返回值语义同 pfInvoke：details 优先；无 details 时把 content[0].text 当 JSON 解析，否则返回纯文本。
 */
export async function pluginInvoke<T = Record<string, unknown>>(
  toolName: string,
  args: Record<string, unknown> = {},
  options: PfInvokeOptions = {},
): Promise<T> {
  const name = String(toolName ?? '').trim();
  if (!name) throw new ProjectForgeError('validation', '缺少 toolName，无法调用桌面插件');
  const envelope = await invokeBridge(name, args ?? {}, options);
  return unwrapBridgeResult<T>(name, envelope);
}

function pickPrimaryString(value: unknown): string | undefined {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) {
    const joined = value
      .map((item) => {
        if (typeof item === 'string') return item;
        if (item && typeof item === 'object' && typeof (item as { text?: unknown }).text === 'string') {
          return String((item as { text: string }).text);
        }
        return '';
      })
      .filter(Boolean)
      .join('\n');
    return joined || undefined;
  }
  return undefined;
}

/** 从 PTYShellExecutor 的任意返回形态里提取输出文本。 */
export function extractPtyText(raw: unknown, depth = 0): string {
  if (depth > 3) return '';
  const direct = pickPrimaryString(raw);
  if (direct !== undefined) return direct;
  if (!raw || typeof raw !== 'object') return '';
  const obj = raw as Record<string, unknown>;
  for (const key of ['text', 'output', 'stdout', 'stderr', 'result', 'content', 'data', 'message']) {
    const value = obj[key];
    const found = pickPrimaryString(value);
    if (found !== undefined) return found;
    if (value && typeof value === 'object') {
      const nested = extractPtyText(value, depth + 1);
      if (nested) return nested;
    }
  }
  return '';
}

/** 从 PTYShellExecutor 的任意返回形态里提取退出码。 */
export function extractPtyExitCode(raw: unknown, depth = 0): number | null {
  if (depth > 3 || !raw || typeof raw !== 'object') return null;
  const obj = raw as Record<string, unknown>;
  for (const key of ['exitCode', 'exit_code', 'code', 'status', 'returncode', 'returnCode']) {
    const value = obj[key];
    if (typeof value === 'number' && Number.isFinite(value)) return Math.trunc(value);
    if (typeof value === 'string' && /^-?\d+$/.test(value.trim())) return Number(value.trim());
  }
  for (const key of ['result', 'details', 'data']) {
    const value = obj[key];
    if (value && typeof value === 'object') {
      const nested = extractPtyExitCode(value, depth + 1);
      if (nested !== null) return nested;
    }
  }
  return null;
}

/**
 * 通过桌面插件 PTYShellExecutor 执行 shell / git 命令。
 *
 * 推荐用法（Git 面板）：命令里自行加 `GIT_TERMINAL_PROMPT=0`，并 `cd /d/<workspace>` 后执行
 * `git status` 等；PTY 会回显命令与提示符，调用方可自行用起止标记裁剪。
 *
 * @returns `{ ok, text, exitCode, raw }`；`ok` 为「退出码 === 0」（拿不到退出码时退回桥接成功标志）。
 */
export async function ptyExec(command: string, options: PtyExecOptions = {}): Promise<PtyExecResult> {
  const cmd = String(command ?? '').trim();
  if (!cmd) throw new ProjectForgeError('validation', 'PTY 命令不能为空');
  const mergedOptions: PfInvokeOptions = {
    timeoutMs: options.timeoutMs ?? DEFAULT_PTY_TIMEOUT_MS,
    signal: options.signal,
  };
  const args: Record<string, unknown> = { ...(options.args ?? {}), command: cmd };
  const raw = await pluginInvoke<unknown>('PTYShellExecutor', args, mergedOptions);
  const text = extractPtyText(raw);
  const exitCode = extractPtyExitCode(raw);
  let ok = exitCode === null ? true : exitCode === 0;
  if (raw && typeof raw === 'object') {
    const obj = raw as Record<string, unknown>;
    if (typeof obj.ok === 'boolean' && exitCode === null) ok = obj.ok;
    if (typeof obj.success === 'boolean' && exitCode === null) ok = obj.success;
    if (obj.timedOut === true || obj.timeout === true) ok = false;
  }
  return { ok, text, exitCode, raw };
}

/** 仅用于排查：把错误转成可读文案（分类型前缀）。 */
export function describeProjectForgeError(error: unknown): string {
  if (error instanceof ProjectForgeError) {
    const prefix: Record<PfErrorKind, string> = {
      network: '网络',
      auth: '鉴权',
      bridge: '桌面端',
      protocol: '协议',
      validation: '参数',
    };
    return `[${prefix[error.kind]}] ${error.message}`;
  }
  return errorMessageOf(error);
}
