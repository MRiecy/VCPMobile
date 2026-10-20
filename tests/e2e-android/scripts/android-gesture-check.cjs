'use strict';

// These names are used only by functions serialized into the Debug WebView.
/* global location, document, window, Element, getComputedStyle, innerWidth, innerHeight, MouseEvent, PointerEvent */

// This deliberately has no arbitrary selector, expression, URL, package or input API.
// CDP supplies browser-engine touch events, not evidence of physical finger motion.
const { spawnSync } = require('child_process');
const { randomUUID, createHash } = require('crypto');
const { DEBUG_PACKAGE, findAdb } = require('./adb-env.cjs');

const MAX_TRACE = 48;
const HOLD_MS = 900;
const ALLOWED_ORIGINS = new Set(['http://tauri.localhost', 'http://127.0.0.1:1420']);

function selectedPage(pages, port) {
  if (!Array.isArray(pages) || pages.length > 32) throw new Error('Invalid Debug WebView target list');
  const matches = pages.filter((page) => {
    try { return page.type === 'page' && ALLOWED_ORIGINS.has(new URL(page.url).origin); }
    catch { return false; }
  });
  if (matches.length !== 1) throw new Error('Expected exactly one allowed Debug WebView page');
  let socket;
  try { socket = new URL(matches[0].webSocketDebuggerUrl); }
  catch { throw new Error('Invalid Debug WebView debugger endpoint'); }
  if (socket.protocol !== 'ws:' || !['127.0.0.1', 'localhost'].includes(socket.hostname)
    || socket.port !== String(port) || socket.username || socket.password || socket.search || socket.hash
    || !/^\/devtools\/page\/[a-zA-Z0-9_-]+$/.test(socket.pathname)) {
    throw new Error('Debugger endpoint escaped the owned local forward');
  }
  socket.hostname = '127.0.0.1';
  return socket.href;
}

function checkedPid(value) {
  const pid = value.trim();
  if (!/^[1-9]\d{0,9}$/.test(pid)) throw new Error('Debug main process is absent or ambiguous');
  return pid;
}

function checkedSocket(value, pid) {
  const expected = `@webview_devtools_remote_${checkedPid(pid)}`;
  const lines = value.trim().split(/\r?\n/).filter(Boolean);
  if (lines.length !== 1 || lines[0].trim().split(/\s+/).at(-1) !== expected) {
    throw new Error('The exact Debug PID WebView socket was not found');
  }
  return expected.slice(1);
}

async function boundedTargetList(response) {
  if (!response.ok || !response.body) throw new Error('Debug WebView target discovery failed');
  const reader = response.body.getReader();
  const chunks = [];
  let bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > 64 * 1024) throw new Error('Debug WebView target discovery exceeded its size budget');
      chunks.push(Buffer.from(value));
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { throw new Error('Invalid Debug WebView target JSON'); }
}

// Serialized only by this tool; arguments below are internally generated, never CLI input.
function installObservation(token, limit) {
  if (!['http://tauri.localhost', 'http://127.0.0.1:1420'].includes(location.origin)) return { error: 'origin_changed' };
  if (document.visibilityState !== 'visible') return { error: 'document_not_visible' };
  if (window.__VCP_GESTURE_CHECK__) return { error: 'observation_already_active' };
  const themeSelector = 'button[aria-label="切换深浅模式，长按切换消息呈现"]';
  const protectedSelector = 'input,textarea,select,a,button,summary,[role="button"],[role="link"],pre,code,[contenteditable]:not([contenteditable="false"]),[data-longpress-native],[data-vcp-button],[onclick],[oncontextmenu],[ontouchstart],[onpointerdown],video,audio,iframe,img,svg,canvas';
  const allowedTitles = ['消息呈现', 'User', 'Assistant'];
  const visible = (element) => {
    if (!(element instanceof Element) || element.closest('[hidden],[inert],[aria-hidden="true"]')) return false;
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.display !== 'none' && style.visibility === 'visible' && Number(style.opacity) > 0
      && rect.width > 1 && rect.height > 1;
  };
  const menus = () => Array.from(document.querySelectorAll('[role="dialog"][aria-modal="true"]'))
    .filter(visible).slice(0, 8).map(element => {
      const title = element.getAttribute('aria-label');
      return allowedTitles.includes(title) ? title : 'other';
    });
  if (menus().length) return { error: 'existing_modal' };
  const controller = new AbortController();
  let target = null;
  let kind = null;
  let events = [];
  let dropped = 0;
  let timeout;
  const label = (node) => {
    if (node === document) return 'document';
    if (node === window) return 'window';
    if (!(node instanceof Element)) return 'other';
    if (node.closest(themeSelector)) return 'theme';
    if (node.closest('.vcp-message-item [data-message-content] p')) return 'paragraph';
    if (node.closest('.vcp-message-item')) return 'message';
    if (node.closest('[role="dialog"]')) return 'dialog';
    return 'other';
  };
  const snapshot = () => ({
    menus: menus(),
    themeExpanded: document.querySelector(themeSelector)?.getAttribute('aria-expanded') === 'true',
    selectionCollapsed: window.getSelection()?.isCollapsed ?? null,
    events: events.slice(0, limit), dropped,
  });
  const findTarget = (wanted) => {
    const candidates = wanted === 'theme'
      ? Array.from(document.querySelectorAll(themeSelector))
      : Array.from(document.querySelectorAll('.vcp-message-item [data-message-content] p')).slice(0, 400);
    if (wanted === 'theme' && candidates.length !== 1) return null;
    for (const candidate of candidates) {
      if (!visible(candidate) || (wanted === 'message' && (candidate.closest(protectedSelector) || candidate.querySelector(protectedSelector)))) continue;
      const rect = candidate.getBoundingClientRect();
      const left = Math.max(1, rect.left), right = Math.min(innerWidth - 1, rect.right);
      const top = Math.max(1, rect.top), bottom = Math.min(innerHeight - 1, rect.bottom);
      if (right - left < 4 || bottom - top < 4) continue;
      const x = (left + right) / 2, y = (top + bottom) / 2;
      const hit = document.elementFromPoint(x, y);
      if (!hit || !candidate.contains(hit) || (wanted === 'message' && hit.closest(protectedSelector))) continue;
      return { element: candidate, hitElement: hit, x, y, hit: label(hit) };
    }
    return null;
  };
  const record = (event) => {
    if (!kind) return;
    if (events.length >= limit) { dropped += 1; return; }
    const point = event.touches?.[0] ?? event.changedTouches?.[0] ?? event;
    const entry = {
      type: event.type, atMs: Math.round(performance.now()), target: label(event.target),
      trusted: event.isTrusted, x: Number.isFinite(point.clientX) ? Math.round(point.clientX) : null,
      y: Number.isFinite(point.clientY) ? Math.round(point.clientY) : null,
      touches: event.touches?.length ?? null,
      pointer: ['touch', 'mouse', 'pen'].includes(event.pointerType) ? event.pointerType : null,
      prevented: event.defaultPrevented,
    };
    events.push(entry);
    queueMicrotask(() => { entry.prevented = event.defaultPrevented; });
  };
  for (const type of ['pointerdown', 'pointerup', 'pointercancel', 'touchstart', 'touchmove', 'touchend', 'touchcancel', 'mousedown', 'mousemove', 'mouseup', 'mouseleave', 'contextmenu', 'selectstart', 'scroll', 'click']) {
    document.addEventListener(type, record, { capture: true, passive: true, signal: controller.signal });
  }
  const cleanup = () => {
    controller.abort();
    clearTimeout(timeout);
    if (window.__VCP_GESTURE_CHECK__?.token === token) delete window.__VCP_GESTURE_CHECK__;
    return true;
  };
  window.__VCP_GESTURE_CHECK__ = {
    token, snapshot, cleanup,
    begin(wanted) {
      if (!['theme', 'message'].includes(wanted)) return { error: 'invalid_builtin_target' };
      if (menus().length) return { error: 'modal_not_closed' };
      target = findTarget(wanted);
      if (!target) return { error: wanted === 'theme' ? 'theme_not_visible' : 'paragraph_not_visible' };
      kind = wanted; events = []; dropped = 0;
      const surface = wanted === 'theme' ? target.element : target.element.closest('.vcp-message-item');
      return { x: target.x, y: target.y, hit: target.hit, legacyDirectiveHook: typeof surface?._longpressCleanup === 'function' };
    },
    async replayObservedAndroidEvents() {
      if (!target?.hitElement.isConnected) return { error: 'target_changed' };
      // A physical OPPO trace established this sequence at +492/+494 ms.
      // These two events are explicitly synthetic; CDP touch alone omits them.
      const init = { bubbles: true, cancelable: true, view: window, clientX: target.x, clientY: target.y };
      target.hitElement.dispatchEvent(new MouseEvent('mousemove', { ...init, button: 0, buttons: 0 }));
      await new Promise(resolve => setTimeout(resolve, 2));
      if (!target.hitElement.isConnected) return { error: 'target_changed' };
      target.hitElement.dispatchEvent(new PointerEvent('contextmenu', {
        ...init, button: 2, buttons: 0, pointerType: 'touch', pointerId: 1, isPrimary: true,
      }));
      return { syntheticCompatibilityEvents: true };
    },
  };
  timeout = setTimeout(cleanup, 26000);
  const documentId = window.__vcpWebViewResume?.documentId;
  return { timeOrigin: performance.timeOrigin, documentId: typeof documentId === 'string' && /^[a-f0-9-]{36}$/i.test(documentId) ? documentId : null };
}

class CdpConnection {
  constructor(socket, remaining) {
    this.socket = socket;
    this.remaining = remaining;
    this.nextId = 0;
    this.pending = new Map();
    this.scripts = new Map();
    socket.addEventListener('message', (event) => {
      if (typeof event.data !== 'string' || event.data.length > 128 * 1024) return;
      let data;
      try { data = JSON.parse(event.data); } catch { return; }
      if (data.method === 'Debugger.scriptParsed' && this.scripts.size < 8) {
        try {
          const url = new URL(data.params.url);
          if (ALLOWED_ORIGINS.has(url.origin) && /^\/src\/core\/directives\/[a-zA-Z0-9_-]+\.ts$/.test(url.pathname)) {
            this.scripts.set(data.params.scriptId, { path: url.pathname, timestamp: /^\d+$/.test(url.searchParams.get('t') || '') ? url.searchParams.get('t') : null });
          }
        } catch { /* Unrelated scripts are never retained or printed. */ }
      }
      const request = this.pending.get(data.id);
      if (!request) return;
      this.pending.delete(data.id);
      clearTimeout(request.timer);
      if (data.error) request.reject(new Error(`CDP ${request.method} failed`));
      else request.resolve(data.result);
    });
    const lost = () => {
      for (const request of this.pending.values()) {
        clearTimeout(request.timer);
        request.reject(new Error('Debug WebView connection closed'));
      }
      this.pending.clear();
    };
    socket.addEventListener('close', lost);
    socket.addEventListener('error', lost);
  }

  static async connect(url, remaining) {
    const socket = new WebSocket(url);
    const connection = new CdpConnection(socket, remaining);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { socket.close(); reject(new Error('Debug WebView connection timed out')); }, remaining(1500));
      socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); socket.close(); reject(new Error('Debug WebView connection failed')); }, { once: true });
    });
    return connection;
  }

  send(method, params = {}, cleanup = false) {
    if (this.socket.readyState !== WebSocket.OPEN) return Promise.reject(new Error('Debug WebView is disconnected'));
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`CDP ${method} timed out`)); }, cleanup ? 400 : this.remaining(1500));
      this.pending.set(id, { resolve, reject, timer, method });
      try { this.socket.send(JSON.stringify({ id, method, params })); }
      catch { clearTimeout(timer); this.pending.delete(id); reject(new Error(`CDP ${method} could not be sent`)); }
    });
  }

  async evaluate(expression, cleanup = false) {
    const result = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, cleanup);
    if (result.exceptionDetails || !result.result || !('value' in result.result)) throw new Error('Built-in WebView observation failed');
    const value = result.result.value;
    if (value?.error) throw new Error(`WebView check: ${String(value.error).slice(0, 80)}`);
    return value;
  }

  close() { this.socket.close(); }
}

async function runGestureCheck(options) {
  if (!options.serial || options.serial.includes(':') || !/^[a-zA-Z0-9._-]+$/.test(options.serial)) {
    throw new Error('gesture-check requires an explicit USB --serial');
  }
  if (typeof WebSocket !== 'function') throw new Error('gesture-check requires Node with built-in WebSocket');
  const started = Date.now(), deadline = started + 27000;
  const remaining = (cap) => {
    const value = Math.min(cap, deadline - Date.now());
    if (value <= 0) throw new Error('gesture-check exceeded its time budget');
    return value;
  };
  const adb = findAdb();
  const executeAdb = (args, cleanup = false, global = false) => {
    const run = spawnSync(adb, [...(global ? [] : ['-s', options.serial]), ...args], {
      encoding: 'utf8', windowsHide: true, stdio: 'pipe', maxBuffer: 64 * 1024,
      timeout: cleanup ? 750 : remaining(1500),
    });
    if (run.error || run.status !== 0) throw new Error(`Debug gesture ADB ${args[0]} failed`);
    return run.stdout || '';
  };
  const devices = executeAdb(['devices'], false, true).split(/\r?\n/)
    .map(line => line.trim().split(/\s+/)).filter(parts => parts[0] === options.serial && parts[1] === 'device');
  if (devices.length !== 1) throw new Error('Selected USB serial is not uniquely connected');
  const pid = checkedPid(executeAdb(['shell', 'pidof', '-s', DEBUG_PACKAGE]));
  const socketName = checkedSocket(executeAdb(['shell', `grep -E '[[:space:]]@?webview_devtools_remote_${pid}$' /proc/net/unix`]), pid);
  const result = {
    schema: 'vcp.android-debug.gesture-check.v1', package: DEBUG_PACKAGE, serial: options.serial, pid,
    source: 'CDP browser-engine touch; not physical finger input', holdMs: HOLD_MS,
    releasedWith: 'touchCancel', status: 'error', checks: [], cleanup: {},
  };
  let port = null, cdp = null, observing = false, touchActive = false;
  const token = randomUUID();
  const access = `window.__VCP_GESTURE_CHECK__?.token === ${JSON.stringify(token)} ? window.__VCP_GESTURE_CHECK__`;
  const call = (method, arg = '') => `(${access}.${method}(${arg}) : ({error:'document_changed'}))`;
  const assertPid = () => {
    if (checkedPid(executeAdb(['shell', 'pidof', '-s', DEBUG_PACKAGE])) !== pid) throw new Error('Debug PID changed during gesture-check');
  };
  const delay = (ms) => ms <= 0 ? Promise.resolve() : new Promise(resolve => setTimeout(resolve, remaining(ms)));
  const escape = async (cleanup = false) => {
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 }, cleanup);
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 }, cleanup);
  };
  try {
    const allocated = executeAdb(['forward', '--no-rebind', 'tcp:0', `localabstract:${socketName}`]).trim();
    if (!/^\d{1,5}$/.test(allocated) || Number(allocated) < 1024 || Number(allocated) > 65535) throw new Error('ADB did not return an owned ephemeral port');
    port = Number(allocated);
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { redirect: 'error', signal: AbortSignal.timeout(remaining(1500)) });
    const pages = await boundedTargetList(response);
    cdp = await CdpConnection.connect(selectedPage(pages, port), remaining);
    assertPid();
    await cdp.send('Debugger.enable');
    result.loadedDirectiveSources = [];
    for (const [scriptId, source] of cdp.scripts) {
      const { scriptSource } = await cdp.send('Debugger.getScriptSource', { scriptId });
      if (typeof scriptSource !== 'string' || scriptSource.length > 100000) throw new Error('Directive source exceeded its fingerprint budget');
      result.loadedDirectiveSources.push({ ...source, sha256: createHash('sha256').update(scriptSource).digest('hex') });
    }
    if (!result.loadedDirectiveSources.some(source => source.path === '/src/core/directives/longpress.ts')) {
      throw new Error('The loaded longpress source could not be fingerprinted');
    }
    await cdp.send('Debugger.disable');
    result.document = await cdp.evaluate(`(${installObservation.toString()})(${JSON.stringify(token)},${MAX_TRACE})`);
    observing = true;
    for (const [kind, mode] of ['theme', 'message'].flatMap(kind => ['browser-hold', 'android-observed-sequence'].map(mode => [kind, mode]))) {
      assertPid();
      const target = await cdp.evaluate(call('begin', JSON.stringify(kind)));
      if (![target.x, target.y].every(value => Number.isFinite(value) && value >= 0 && value < 10000)) throw new Error('Invalid visible gesture target');
      touchActive = true;
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: target.x, y: target.y, radiusX: 1, radiusY: 1, force: 1, id: 0 }] });
      const pressedAt = Date.now();
      if (mode === 'android-observed-sequence') {
        await delay(492);
        await cdp.evaluate(call('replayObservedAndroidEvents'));
      }
      await delay(HOLD_MS - (Date.now() - pressedAt));
      const held = await cdp.evaluate(call('snapshot'));
      // Cancel rather than synthesize a short click when long-press fails, or click a newly opened menu.
      await cdp.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] });
      touchActive = false;
      await delay(100);
      const released = await cdp.evaluate(call('snapshot'));
      const expected = kind === 'theme' ? ['消息呈现'] : ['User', 'Assistant'];
      const passed = held.menus.length === 1 && expected.includes(held.menus[0]) && (kind !== 'theme' || held.themeExpanded);
      const check = {
        target: kind, mode, point: target, passed, held, released,
        evidence: mode === 'browser-hold' ? 'CDP touch only' : 'Replay of observed physical sequence; compatibility mouse/contextmenu events are synthetic',
        syntheticCompatibilityEvents: mode === 'android-observed-sequence',
      };
      result.checks.push(check);
      if (released.menus.length) {
        if (released.menus.length !== 1 || !expected.includes(released.menus[0])) throw new Error('Unexpected modal; refusing to dismiss it');
        await escape();
        await delay(350);
        const closed = await cdp.evaluate(call('snapshot'));
        check.closedWithEscape = closed.menus.length === 0 && (kind !== 'theme' || !closed.themeExpanded);
        if (!check.closedWithEscape) throw new Error('Gesture menu did not close with Escape');
      }
    }
    result.status = result.checks.every(check => check.passed) ? 'passed' : 'failed';
  } catch (error) {
    result.error = error instanceof Error ? error.message : 'Gesture check failed';
  } finally {
    if (cdp) {
      if (touchActive) {
        try { await cdp.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] }, true); result.cleanup.touch = true; }
        catch { result.cleanup.touch = false; }
      }
      if (observing) {
        try {
          const state = await cdp.evaluate(call('snapshot'), true);
          if (state.menus.length === 1 && ['消息呈现', 'User', 'Assistant'].includes(state.menus[0])) await escape(true);
          await cdp.evaluate(call('cleanup'), true);
          result.cleanup.observation = true;
        } catch { result.cleanup.observation = false; }
      }
      cdp.close();
    }
    if (port !== null) {
      try { executeAdb(['forward', '--remove', `tcp:${port}`], true); result.cleanup.forward = true; }
      catch { result.cleanup.forward = false; }
    }
    result.elapsedMs = Date.now() - started;
    if (Object.values(result.cleanup).some(value => value === false)) result.status = 'error';
  }
  return result;
}

module.exports = { runGestureCheck, selectedPage, checkedPid, checkedSocket, installObservation, boundedTargetList };
