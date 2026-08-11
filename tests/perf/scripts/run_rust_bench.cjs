const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { ROOT, timestamp, ensureDir } = require('../../e2e-android/scripts/adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/run_rust_bench.cjs [--out-dir <dir>] [--no-run]

Runs cargo bench --profile perf and stores stdout/stderr. JSON parsing of Criterion
results is intentionally future work; this script establishes artifact capture now.
`);
}

function parseArgs(argv) {
  const args = { outDir: '', noRun: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--out-dir') {
      args.outDir = argv[++i];
    } else if (arg === '--no-run') {
      args.noRun = true;
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return args;
}

const args = parseArgs(process.argv.slice(2));
const outDir = ensureDir(path.resolve(ROOT, args.outDir || path.join('tests', 'perf', 'reports', timestamp())));
const cargoArgs = ['bench', '--locked', '--profile', 'perf'];
if (args.noRun) {
  cargoArgs.push('--no-run');
}

const result = spawnSync('cargo', cargoArgs, {
  cwd: path.join(ROOT, 'src-tauri'),
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
});

fs.writeFileSync(path.join(outDir, 'cargo-bench.stdout.txt'), result.stdout || '', 'utf8');
fs.writeFileSync(path.join(outDir, 'cargo-bench.stderr.txt'), result.stderr || '', 'utf8');
fs.writeFileSync(path.join(outDir, 'rust-bench-summary.json'), JSON.stringify({
  generated_at: new Date().toISOString(),
  command: `cargo ${cargoArgs.join(' ')}`,
  status: result.status,
  out_dir: outDir,
  note: 'Criterion raw report remains under src-tauri/target/criterion when benchmarks are executed.',
}, null, 2), 'utf8');

if (result.status !== 0) {
  console.error(result.stdout);
  console.error(result.stderr);
  process.exit(result.status || 1);
}

console.log(`[run_rust_bench] wrote ${outDir}`);
