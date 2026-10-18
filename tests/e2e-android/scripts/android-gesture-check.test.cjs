'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { selectedPage, checkedPid, checkedSocket, installObservation, boundedTargetList, runGestureCheck } = require('./android-gesture-check.cjs');
const { main } = require('./android-debug-agent.cjs');

const page = (overrides = {}) => ({
  type: 'page', url: 'http://127.0.0.1:1420/#/chat',
  webSocketDebuggerUrl: 'ws://127.0.0.1:12345/devtools/page/ABC-123', ...overrides,
});

test('PID and socket discovery reject neighboring processes and ambiguous matches', () => {
  assert.equal(checkedPid('29406\n'), '29406');
  for (const value of ['', '1 2', '0', '29406; echo bad']) assert.throws(() => checkedPid(value));
  assert.equal(checkedSocket('000: 0 0 0 0 0 @webview_devtools_remote_29406', '29406'), 'webview_devtools_remote_29406');
  for (const value of ['@webview_devtools_remote_294060', '@webview_devtools_remote_1', '@webview_devtools_remote_29406\n@webview_devtools_remote_29406']) {
    assert.throws(() => checkedSocket(value, '29406'));
  }
});

test('target must be one allowed page with a websocket on the owned forward', () => {
  assert.equal(selectedPage([page()], 12345), page().webSocketDebuggerUrl);
  assert.equal(selectedPage([page({ url: 'http://tauri.localhost/' })], 12345), page().webSocketDebuggerUrl);
  for (const pages of [[], [page(), page()], [page({ url: 'http://127.0.0.1:1421/' })], [page({ type: 'iframe' })],
    [page({ webSocketDebuggerUrl: 'ws://example.com:12345/devtools/page/A' })],
    [page({ webSocketDebuggerUrl: 'ws://localhost:12346/devtools/page/A' })],
    [page({ webSocketDebuggerUrl: 'ws://localhost:12345/devtools/browser/A' })],
    [page({ webSocketDebuggerUrl: 'ws://localhost:12345/devtools/page/A?key=secret' })]]) {
    assert.throws(() => selectedPage(pages, 12345));
  }
});

test('discovery bounds response bytes and does not include response text in failures', async () => {
  assert.deepEqual(await boundedTargetList(new Response('[1]')), [1]);
  await assert.rejects(boundedTargetList(new Response('sensitive invalid JSON')), /Invalid Debug WebView target JSON/);
  await assert.rejects(boundedTargetList(new Response('x'.repeat(65537))), /size budget/);
});

test('CLI requires explicit USB serial and does not offer arbitrary selectors or expressions', async () => {
  await assert.rejects(runGestureCheck({ serial: '' }), /explicit USB/);
  await assert.rejects(runGestureCheck({ serial: '192.168.0.1:5555' }), /explicit USB/);
  await assert.rejects(main(['gesture-check', '--eval', 'anything']), /accepts only/);
  await assert.rejects(main(['gesture-check', '--package', 'com.vcp.avatar']), /accepts only/);
});

async function observationFixture() {
  const { Window } = await import('happy-dom');
  const window = new Window({ url: 'http://127.0.0.1:1420/' });
  Object.defineProperty(window.document, 'visibilityState', { value: 'visible' });
  window.document.body.innerHTML = '<button aria-label="切换深浅模式，长按切换消息呈现" aria-expanded="false" style="display:block;visibility:visible;opacity:1"><svg></svg></button><div class="vcp-message-item"><div data-message-content><p style="display:block;visibility:visible;opacity:1">PRIVATE_MESSAGE_FIXTURE</p></div></div>';
  const theme = window.document.querySelector('button');
  const paragraph = window.document.querySelector('p');
  theme.getBoundingClientRect = () => ({ left: 0, right: 40, top: 0, bottom: 40, width: 40, height: 40 });
  paragraph.getBoundingClientRect = () => ({ left: 0, right: 200, top: 100, bottom: 140, width: 200, height: 40 });
  window.document.elementFromPoint = (_x, y) => y < 50 ? theme.querySelector('svg') : paragraph;
  const install = () => window.eval(`(${installObservation.toString()})('fixture-token',48)`);
  return { window, theme, paragraph, install };
}

test('fixed DOM targets include theme SVG and visible paragraph without returning message content', async () => {
  const { window, paragraph, install } = await observationFixture();
  try {
    assert.equal(install().error, undefined);
    const state = window.__VCP_GESTURE_CHECK__;
    assert.equal(state.begin('theme').hit, 'theme');
    assert.equal(state.begin('message').hit, 'paragraph');
    for (let i = 0; i < 80; i++) paragraph.dispatchEvent(new window.Event('touchstart', { bubbles: true }));
    const snapshot = state.snapshot();
    assert.equal(snapshot.events.length, 48);
    assert.equal(snapshot.dropped, 32);
    assert.ok(!JSON.stringify(snapshot).includes('PRIVATE_MESSAGE_FIXTURE'));
    assert.equal(state.cleanup(), true);
    assert.equal(window.__VCP_GESTURE_CHECK__, undefined);
    paragraph.dispatchEvent(new window.Event('touchstart', { bubbles: true }));
    assert.equal(state.snapshot().dropped, 32);
  } finally { await window.happyDOM.close(); }
});

test('covered content, protected paragraphs and pre-existing modals fail closed', async () => {
  const { window, paragraph, install } = await observationFixture();
  try {
    install();
    paragraph.setAttribute('data-longpress-native', '');
    assert.equal(window.__VCP_GESTURE_CHECK__.begin('message').error, 'paragraph_not_visible');
    paragraph.removeAttribute('data-longpress-native');
    window.document.elementFromPoint = () => window.document.body;
    assert.equal(window.__VCP_GESTURE_CHECK__.begin('theme').error, 'theme_not_visible');
    window.__VCP_GESTURE_CHECK__.cleanup();
    const modal = window.document.createElement('div');
    modal.setAttribute('role', 'dialog'); modal.setAttribute('aria-modal', 'true');
    modal.style.cssText = 'display:block;visibility:visible;opacity:1';
    modal.getBoundingClientRect = () => ({ width: 100, height: 100 });
    window.document.body.append(modal);
    assert.equal(install().error, 'existing_modal');
    assert.equal(window.__VCP_GESTURE_CHECK__, undefined);
  } finally { await window.happyDOM.close(); }
});

test('observed Android sequence replay is fixed and exposes both compatibility events as synthetic', async () => {
  const { window, install } = await observationFixture();
  try {
    install();
    const state = window.__VCP_GESTURE_CHECK__;
    state.begin('theme');
    assert.equal((await state.replayObservedAndroidEvents()).syntheticCompatibilityEvents, true);
    const events = state.snapshot().events;
    assert.deepEqual(Array.from(events, event => event.type), ['mousemove', 'contextmenu']);
    assert.ok(events.every(event => !event.trusted));
    assert.equal(events[1].pointer, 'touch');
    state.cleanup();
  } finally { await window.happyDOM.close(); }
});

test('failed CDP discovery removes only its allocated forward on the explicit serial', async () => {
  const childProcess = require('child_process');
  const adbEnv = require('./adb-env.cjs');
  const helperPath = require.resolve('./android-gesture-check.cjs');
  const originalSpawn = childProcess.spawnSync, originalFindAdb = adbEnv.findAdb, originalFetch = global.fetch;
  const calls = [];
  try {
    adbEnv.findAdb = () => 'test-adb';
    childProcess.spawnSync = (_program, args) => {
      calls.push(args);
      if (args[0] === 'devices') return { status: 0, stdout: 'List of devices attached\nSERIAL-1\tdevice\n' };
      assert.deepEqual(args.slice(0, 2), ['-s', 'SERIAL-1']);
      if (args.includes('pidof')) return { status: 0, stdout: '29406\n' };
      if (args.some(value => value.startsWith('grep -E '))) return { status: 0, stdout: '0: 0 0 0 0 0 @webview_devtools_remote_29406\n' };
      if (args.includes('tcp:0')) return { status: 0, stdout: '12345\n' };
      if (args.includes('--remove')) return { status: 0, stdout: '' };
      throw new Error('Unexpected fake ADB command');
    };
    global.fetch = async (url) => {
      assert.equal(url, 'http://127.0.0.1:12345/json/list');
      return new Response(JSON.stringify([page(), page()]));
    };
    delete require.cache[helperPath];
    const result = await require(helperPath).runGestureCheck({ serial: 'SERIAL-1' });
    assert.equal(result.status, 'error');
    assert.equal(result.cleanup.forward, true);
    assert.deepEqual(calls.at(-1), ['-s', 'SERIAL-1', 'forward', '--remove', 'tcp:12345']);
    assert.ok(calls.filter(args => args.includes('pidof')).every(args => args.at(-1) === 'com.vcp.avatar.debug'));
    assert.ok(!JSON.stringify(calls).includes('--remove-all'));
  } finally {
    childProcess.spawnSync = originalSpawn; adbEnv.findAdb = originalFindAdb; global.fetch = originalFetch;
    delete require.cache[helperPath];
  }
});
