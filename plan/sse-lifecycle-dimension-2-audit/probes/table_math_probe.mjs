// Run from the repository root:
// node plan/sse-lifecycle-dimension-2-audit/probes/table_math_probe.mjs
// Imports production code. happy-dom provides DOM semantics, not browser layout/GPU evidence.
import { Window } from 'happy-dom';
import { createServer } from 'vite';
import assert from 'node:assert/strict';

const browser = new Window();
const { document } = browser;
for (const key of ['window', 'document', 'Node', 'Element', 'HTMLElement', 'HTMLImageElement', 'HTMLMediaElement', 'HTMLInputElement', 'Text', 'DocumentFragment']) {
  globalThis[key] = key === 'window' ? browser : key === 'document' ? browser.document : browser[key];
}
const server = await createServer({
  configFile: false,
  server: { middlewareMode: true, watch: null },
  appType: 'custom',
});

try {
  const { rebuildSnapshot, applyFrame, cleanupRegistry } = await server.ssrLoadModule('/src/core/utils/astExecutor.ts');
  const root = document.createElement('div');
  document.body.appendChild(root);
  const cell = value => [{ type: 'text', value }];
  const table = rows => ({
    type: 'table',
    header: [cell('A'), cell('B')],
    rows: rows.map(row => row.map(cell)),
    wrapper_class: 'vcp-scrollable no-swipe',
  });
  rebuildSnapshot([table([['one', 'two']])], 'audit-table', root);
  const selectors = ['div', 'table', 'thead', 'tbody', 'th', 'tbody tr', 'td', 'td:nth-child(2)'];
  const baseline = selectors.map(selector => root.querySelector(selector));
  const frames = [];
  for (const [label, rows] of [
    ['change existing cell', [['one', 'two changed']]],
    ['append row', [['one', 'two changed'], ['three', 'four']]],
  ]) {
    let createdElements = 0;
    const createElement = document.createElement.bind(document);
    document.createElement = (...args) => { createdElements++; return createElement(...args); };
    try {
      assert.equal(applyFrame([{ op: 'replace', id: 't0', node: table(rows) }], 'audit-table', root).ok, true);
    } finally {
      document.createElement = createElement;
    }
    const retained = selectors.map((selector, index) => ({ selector, retained: root.querySelector(selector) === baseline[index] }));
    for (const item of retained) assert.equal(item.retained, true, `${label}: ${item.selector}`);
    assert.equal(root.querySelectorAll('tbody tr').length, rows.length);
    assert.equal(root.querySelector('td:nth-child(2)').textContent, 'two changed');
    frames.push({ label, createdElements, retained, liveRows: rows.length });
  }
  cleanupRegistry('audit-table');

  const mathRoot = document.createElement('div');
  document.body.appendChild(mathRoot);
  const mathNode = content => ({ type: 'paragraph', children: [{ type: 'inline_math', content, display_mode: true }] });
  rebuildSnapshot([mathNode('x')], 'audit-math', mathRoot);
  const mathSpan = mathRoot.querySelector('.vcp-math-block');
  assert.equal(applyFrame([{ op: 'replace_inline', id: 't0.i0', node: mathNode('x+y').children[0] }], 'audit-math', mathRoot).ok, true);
  assert.equal(mathRoot.querySelector('.vcp-math-block'), mathSpan);
  assert.equal(mathSpan.dataset.latex, 'x+y');
  assert.equal(mathSpan.textContent, 'x+y');
  cleanupRegistry('audit-math');
  console.log(JSON.stringify({
    assertions: 'passed',
    frames,
    mathSpanRetained: true,
    environment: 'happy-dom; production astExecutor imported by Vite SSR; no layout, paint, frame-time or GPU evidence',
  }, null, 2));
} finally {
  await server.close();
  await browser.happyDOM.close();
}
