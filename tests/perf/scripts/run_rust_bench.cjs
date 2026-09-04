const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { ROOT, timestamp, ensureDir } = require('../../e2e-android/scripts/adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/run_rust_bench.cjs [--out-dir <dir>] [--no-run] [--collect-only]

Runs cargo bench --profile perf and stores stdout/stderr, then scans
src-tauri/target/criterion for per-benchmark estimates and writes
criterion-estimates.{json,md}.
--collect-only skips the bench run and archives the existing target/criterion data.
`);
}

function parseArgs(argv) {
  const args = { outDir: '', noRun: false, collectOnly: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--out-dir') {
      args.outDir = argv[++i];
    } else if (arg === '--no-run') {
      args.noRun = true;
    } else if (arg === '--collect-only') {
      args.collectOnly = true;
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return args;
}

// 扫描 src-tauri/target/criterion/<group>/<tier>/new/estimates.json，
// 汇总为 { group: { tier: mean_ns } } 并附一份可读的 markdown 表。
function collectCriterionEstimates(outDir) {
  const criterionDir = path.join(ROOT, 'src-tauri', 'target', 'criterion');
  const groups = {};
  if (!fs.existsSync(criterionDir)) return null;
  for (const group of fs.readdirSync(criterionDir)) {
    const groupDir = path.join(criterionDir, group);
    if (!fs.statSync(groupDir).isDirectory() || group === 'report') continue;
    for (const tier of fs.readdirSync(groupDir)) {
      const estPath = path.join(groupDir, tier, 'new', 'estimates.json');
      if (!fs.existsSync(estPath)) continue;
      const est = JSON.parse(fs.readFileSync(estPath, 'utf8'));
      if (!groups[group]) groups[group] = {};
      groups[group][tier] = est.mean.point_estimate;
    }
  }
  const names = Object.keys(groups).sort();
  if (names.length === 0) return null;

  const fmt = (ns) => (ns >= 1e6 ? `${(ns / 1e6).toFixed(3)}ms` : `${(ns / 1e3).toFixed(1)}µs`);
  const lines = ['# Criterion estimates (mean)', ''];
  for (const name of names) {
    lines.push(`## ${name}`, '');
    const tiers = Object.keys(groups[name]).sort((a, b) => Number(a) - Number(b));
    for (const tier of tiers) lines.push(`- ${tier}: ${fmt(groups[name][tier])}`);
    if (groups[name]['2048'] && groups[name]['40960']) {
      lines.push(`- ratio F(40960)/F(2048) = ${(groups[name]['40960'] / groups[name]['2048']).toFixed(2)}x`);
    }
    lines.push('');
  }

  fs.writeFileSync(path.join(outDir, 'criterion-estimates.json'), JSON.stringify(groups, null, 2), 'utf8');
  fs.writeFileSync(path.join(outDir, 'criterion-estimates.md'), lines.join('\n'), 'utf8');
  return names.length;
}

const args = parseArgs(process.argv.slice(2));
const outDir = ensureDir(path.resolve(ROOT, args.outDir || path.join('tests', 'perf', 'reports', timestamp())));
const cargoArgs = ['bench', '--locked', '--profile', 'perf'];
if (args.noRun) {
  cargoArgs.push('--no-run');
}

let status = 0;
let stdout = '';
let stderr = '';
if (!args.collectOnly) {
  const result = spawnSync('cargo', cargoArgs, {
    cwd: path.join(ROOT, 'src-tauri'),
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  status = result.status;
  stdout = result.stdout || '';
  stderr = result.stderr || '';
}

fs.writeFileSync(path.join(outDir, 'cargo-bench.stdout.txt'), stdout, 'utf8');
fs.writeFileSync(path.join(outDir, 'cargo-bench.stderr.txt'), stderr, 'utf8');
fs.writeFileSync(path.join(outDir, 'rust-bench-summary.json'), JSON.stringify({
  generated_at: new Date().toISOString(),
  command: args.collectOnly ? '(collect-only, no bench run)' : `cargo ${cargoArgs.join(' ')}`,
  status,
  out_dir: outDir,
  note: 'Criterion raw report remains under src-tauri/target/criterion when benchmarks are executed.',
}, null, 2), 'utf8');

const collected = collectCriterionEstimates(outDir);
if (collected === null) {
  console.warn('[run_rust_bench] no criterion estimates found under src-tauri/target/criterion');
} else {
  console.log(`[run_rust_bench] collected estimates for ${collected} benchmark group(s)`);
}

if (status !== 0) {
  console.error(stdout);
  console.error(stderr);
  process.exit(status || 1);
}

console.log(`[run_rust_bench] wrote ${outDir}`);
