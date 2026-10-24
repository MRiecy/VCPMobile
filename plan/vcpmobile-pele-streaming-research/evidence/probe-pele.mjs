// Isolated research probe. Run from repository root:
// node plan/vcpmobile-pele-streaming-research/evidence/probe-pele.mjs
// Downloads the pinned npm artifact to OS temp; never installs project dependencies.
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir, cpus } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';

const version = '1.9.0';
const metadataUrl = `https://registry.npmjs.org/pele/${version}`;
const metadataResponse = await fetch(metadataUrl);
if (!metadataResponse.ok) throw new Error(`Registry HTTP ${metadataResponse.status}`);
const metadata = await metadataResponse.json();
const tarResponse = await fetch(metadata.dist.tarball);
if (!tarResponse.ok) throw new Error(`Tarball HTTP ${tarResponse.status}`);
const tarball = Buffer.from(await tarResponse.arrayBuffer());
const integrity = `sha512-${createHash('sha512').update(tarball).digest('base64')}`;
if (integrity !== metadata.dist.integrity) throw new Error('npm integrity mismatch');
const scratch = await mkdtemp(join(tmpdir(), 'vcp-pele-research-'));
const archive = join(scratch, 'pele.tgz');
await writeFile(archive, tarball, { flag: 'wx' });
const extracted = spawnSync('tar', ['-xzf', archive, '-C', scratch], { encoding: 'utf8' });
if (extracted.status !== 0) throw new Error(extracted.stderr || 'tar extraction failed');
const packageDir = join(scratch, 'package');
const pkg = JSON.parse(await readFile(join(packageDir, 'package.json'), 'utf8'));
const { render, parse, supports, detectType } = await import(pathToFileURL(join(packageDir, 'dist/pele.js')).href);

function attempt(source, options = {}) {
  const start = performance.now();
  try {
    const result = render(source, options);
    return { ok: true, type: result.type, width: result.width, height: result.height,
      svgChars: result.svg.length, svgHash: createHash('sha256').update(result.svg).digest('hex'),
      ms: performance.now() - start };
  } catch (error) {
    return { ok: false, code: error.code ?? null, message: error.message, ms: performance.now() - start };
  }
}
const fixtures = [
  ['empty', ''],
  ['partial-keyword', 'flowch'],
  ['header-only', 'flowchart LR'],
  ['node-open', 'flowchart LR\nA['],
  ['node-label-open', 'flowchart LR\nA[hello'],
  ['node-complete', 'flowchart LR\nA[hello]'],
  ['edge-open', 'flowchart LR\nA[hello] -->'],
  ['edge-target-open', 'flowchart LR\nA[hello] --> B[wor'],
  ['edge-complete-no-newline', 'flowchart LR\nA[hello] --> B[world]'],
  ['edge-complete-newline', 'flowchart LR\nA[hello] --> B[world]\n'],
  ['subgraph-open', 'flowchart TD\nsubgraph group\nA --> B'],
  ['subgraph-complete', 'flowchart TD\nsubgraph group\nA --> B\nend'],
  ['sequence-message-open', 'sequenceDiagram\nAlice->>Bob: hel'],
  ['sequence-loop-open', 'sequenceDiagram\nloop retry\nAlice->>Bob: hello'],
  ['sequence-loop-complete', 'sequenceDiagram\nloop retry\nAlice->>Bob: hello\nend'],
  ['chinese-flow', 'flowchart LR\nA[开始] --> B[完成]'],
  ['math-without-resolver', 'flowchart LR\nA["$$x^2$$"] --> B[done]'],
];
const cases = fixtures.map(([name, source]) => ({ name, source, supported: supports(source),
  detected: detectType(source), result: attempt(source) }));
const streamSource = 'flowchart LR\n' + Array.from({ length: 12 }, (_, i) => `N${i}[Step ${i}] --> N${i + 1}[Step ${i + 1}]`).join('\n');
const streams = [1, 8, 32, 128].map(chunkChars => {
  let successes = 0, failures = 0, lastGoodAt = 0;
  const codes = {};
  const start = performance.now();
  for (let end = chunkChars; end < streamSource.length + chunkChars; end += chunkChars) {
    const length = Math.min(end, streamSource.length);
    const result = attempt(streamSource.slice(0, length));
    if (result.ok) { successes++; lastGoodAt = length; }
    else { failures++; codes[result.code] = (codes[result.code] ?? 0) + 1; }
  }
  return { chunkChars, sourceChars: streamSource.length, successes, failures, lastGoodAt,
    errorCodes: codes, totalMs: performance.now() - start };
});
function percentile(values, fraction) { return values[Math.min(values.length - 1, Math.ceil(values.length * fraction) - 1)]; }
const warmRender = [20, 100, 300].map(nodes => {
  const source = 'flowchart LR\n' + Array.from({ length: nodes - 1 }, (_, i) => `N${i}[Node ${i}] --> N${i + 1}[Node ${i + 1}]`).join('\n');
  for (let i = 0; i < 3; i++) render(source);
  const samples = Array.from({ length: 20 }, () => attempt(source));
  const times = samples.map(x => x.ms).sort((a, b) => a - b);
  return { nodes, sourceChars: source.length, samples: samples.length, successes: samples.filter(x => x.ok).length,
    p50Ms: percentile(times, 0.5), p95Ms: percentile(times, 0.95), maxMs: times.at(-1), svgChars: samples[0].svgChars };
});
const limits = [
  { name: 'source-default-50001', result: attempt('flowchart LR\n%%' + 'x'.repeat(50001 - 15)) },
  { name: 'edge-limit-2', result: attempt('flowchart LR\nA-->B\nB-->C\nC-->D', { maxEdges: 2 }) },
  { name: 'output-limit-100', result: attempt('flowchart LR\nA-->B', { outputLimit: 100 }) },
];
const source = 'flowchart LR\nA[Hello] --> B[World]';
const model = parse(source);
const parsedResult = render(model);
const directResult = render(source);
const output = {
  runAt: new Date().toISOString(), researchDate: '2026-10-24',
  runtime: { node: process.version, platform: process.platform, arch: process.arch, cpu: cpus()[0]?.model },
  artifact: { version: pkg.version, metadataUrl, tarball: metadata.dist.tarball, integrity,
    sha256: createHash('sha256').update(tarball).digest('hex'), license: pkg.license, dependencies: pkg.dependencies ?? {}, scratch },
  scope: 'Host Node SVG string render only. No browser DOM, layout, font loading, sanitizer, IPC, Android/iOS, or Mermaid comparison.',
  cases, streams, warmRender, limits,
  parsedModel: { type: model.type, svgEqualsDirect: parsedResult.svg === directResult.svg },
};
const outputUrl = new URL('./host-probe-1.9.0.json', import.meta.url);
await writeFile(outputUrl, JSON.stringify(output, null, 2) + '\n', { flag: 'wx' });
console.log(JSON.stringify(output, null, 2));
