// Run from repository root: node plan/vcpmobile-pele-streaming-research/evidence/contract-probe.mjs
/* global document, HTMLElement */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Window } from 'happy-dom';

const baseline = JSON.parse(readFileSync(new URL('./host-probe-1.9.0.json', import.meta.url), 'utf8'));
const pele = await import(pathToFileURL(join(baseline.artifact.scratch, 'package/dist/pele.js')).href);
const window = new Window({ url: 'http://localhost:1420' });
for (const name of ['window', 'document', 'Node', 'Element', 'HTMLElement', 'SVGElement', 'Text', 'DocumentFragment']) {
  globalThis[name] = name === 'window' ? window : window[name];
}
const { default: morphdom } = await import('morphdom');
const { createApp, h, ref, watch, nextTick } = await import('vue');
const selector = '[data-mermaid-host="1"][data-content-kind="mermaid"]';
const states = new Map();
let promotions = [];
let renders = 0;
const records = [];
const valid = 'flowchart LR\nA[one] --> B[two]';
const invalid = 'flowchart LR\nA[one] --> B[';

function host(source) {
  const node = document.createElement('div');
  node.dataset.mermaidHost = '1';
  node.dataset.contentKind = 'mermaid';
  node.dataset.mermaidSource = source;
  node.innerHTML = '<div data-view></div><pre data-source hidden></pre><div data-status></div>';
  node.querySelector('[data-source]').textContent = source;
  return node;
}
function isHost(node) { return node instanceof HTMLElement && node.matches(selector); }
function preserve(from, to) {
  if (!isHost(from) || !isHost(to)) return false;
  from.dataset.mermaidSource = to.dataset.mermaidSource;
  from.querySelector('[data-source]').textContent = to.dataset.mermaidSource;
  return true;
}
function sweep(root) {
  for (const node of states.keys()) if (!root.contains(node) || !isHost(node)) states.delete(node);
}
function sync(node, finalized) {
  const source = node.dataset.mermaidSource;
  const state = states.get(node) ?? { lastGood: undefined };
  const view = node.querySelector('[data-view]');
  try {
    let template = promotions.find(item => item.source === source);
    if (!template) {
      renders++;
      const result = pele.render(source, {
        autoDirection: false, idPrefix: 'probe',
        measurer: { width: (text, size) => text.length * size * 0.6 },
      });
      template = { source, svg: result.svg };
    }
    view.innerHTML = template.svg;
    state.lastGood = template;
    node.dataset.mermaidRenderedSource = source;
    node.dataset.mermaidState = finalized ? 'ready' : 'streaming';
    view.hidden = false;
    node.querySelector('[data-source]').hidden = true;
  } catch {
    node.dataset.mermaidState = finalized ? 'error' : 'streaming';
    view.hidden = finalized || !state.lastGood;
    node.querySelector('[data-source]').hidden = !finalized;
  }
  states.set(node, state);
}

const root = document.createElement('div');
document.body.append(root);
const initial = host(valid);
root.append(initial);
sync(initial, false);
const originalSvg = initial.querySelector('svg');
morphdom(initial, host(invalid), {
  onBeforeElUpdated: (from, to) => !preserve(from, to),
});
assert.equal(root.firstElementChild, initial);
assert.equal(initial.querySelector('svg'), originalSvg);
assert.equal(initial.dataset.mermaidSource, invalid);
records.push('morph-keeps-host-and-svg-updates-source');
sync(initial, false);
assert.equal(initial.querySelector('svg'), originalSvg);
assert.equal(initial.dataset.mermaidRenderedSource, valid);
records.push('invalid-prefix-keeps-last-good-pair');
sync(initial, true);
assert.equal(initial.dataset.mermaidState, 'error');
assert.equal(initial.querySelector('[data-view]').hidden, true);
assert.equal(initial.querySelector('[data-source]').hidden, false);
records.push('final-invalid-shows-error-and-source');
initial.dataset.mermaidSource = valid;
sync(initial, false);
assert.equal(initial.dataset.mermaidState, 'streaming');
assert.equal(initial.querySelector('[data-view]').hidden, false);
records.push('corrected-source-recovers');
morphdom(initial, document.createElement('div'));
sweep(root);
assert.equal(states.size, 0);
records.push('same-tag-type-change-is-swept');
root.remove();

async function promotionCase(finalSource, expectedExtraRenders) {
  const mountPoint = document.createElement('div');
  document.body.append(mountPoint);
  const stable = ref(false);
  const html = source => host(source).outerHTML;
  const app = createApp({
    setup() {
      watch(stable, () => {
        promotions = [...states.values()].flatMap(state => state.lastGood ? [state.lastGood] : []);
      }, { flush: 'pre' });
      watch(stable, () => {
        // Deliberately remove the old entry before stable hydration.
        sweep(mountPoint);
        sync(mountPoint.querySelector(selector), true);
        promotions = [];
      }, { flush: 'post' });
      return () => h('div', { key: stable.value ? 'stable' : 'tail', innerHTML: html(stable.value ? finalSource : valid) });
    },
  });
  app.mount(mountPoint);
  sync(mountPoint.querySelector(selector), false);
  const count = renders;
  stable.value = true;
  await nextTick();
  const newHost = mountPoint.querySelector(selector);
  assert.ok(newHost.querySelector('svg'));
  assert.equal(newHost.dataset.mermaidState, 'ready');
  assert.equal(newHost.dataset.mermaidRenderedSource, finalSource);
  assert.equal(newHost.querySelector('[data-source]').hidden, true);
  assert.equal(renders - count, expectedExtraRenders);
  assert.equal(promotions.length, 0);
  app.unmount();
  sweep(mountPoint);
  mountPoint.remove();
}
await promotionCase(valid, 0);
records.push('vue-pre-capture-post-cache-hit-after-old-entry-sweep');
await promotionCase(valid + '\n', 1);
records.push('vue-post-cache-miss-renders-synchronously');
console.log(JSON.stringify({ passed: records.length, cases: records, renderCalls: renders,
  runtime: process.version, pele: '1.9.0', dom: 'happy-dom',
  measurement: 'deterministic text widths; assertions cover DOM and Vue scheduling',
}, null, 2));
await window.happyDOM.close();
