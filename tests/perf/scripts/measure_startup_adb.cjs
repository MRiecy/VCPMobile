const fs = require('fs');
const path = require('path');
const { runAdb, getDeviceInfo, DEBUG_PACKAGE, ROOT, timestamp, ensureDir } = require('../../e2e-android/scripts/adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/measure_startup_adb.cjs [--samples 10] [--out <json>]

Uses adb am start -W to measure cold launch. Boot-ready metrics require future app
instrumentation; this script resolves the explicit Launcher activity and records
Android Activity launch timings now.
`);
}

function parseArgs(argv) {
  const args = { samples: 10, out: '' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--samples') {
      args.samples = Number(argv[++i]);
    } else if (arg === '--out') {
      args.out = argv[++i];
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  return args;
}

function parseAmStart(output) {
  const result = {};
  for (const line of output.split(/\r?\n/)) {
    const match = line.match(/^\s*(ThisTime|TotalTime|WaitTime):\s*(\d+)/);
    if (match) {
      result[match[1]] = Number(match[2]);
    }
  }
  return {
    this_time_ms: result.ThisTime ?? null,
    total_time_ms: result.TotalTime ?? null,
    wait_time_ms: result.WaitTime ?? null,
    raw: output.trim(),
  };
}

function resolveLauncherComponent(pkg) {
  const output = runAdb([
    'shell',
    'cmd',
    'package',
    'resolve-activity',
    '--brief',
    '-a',
    'android.intent.action.MAIN',
    '-c',
    'android.intent.category.LAUNCHER',
    '--user',
    '0',
    pkg,
  ], { allowFailure: true });
  const component = output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .findLast((line) => line.includes('/'));
  if (!component) {
    throw new Error(`无法解析 ${pkg} 的 Launcher Activity：\n${output}`);
  }
  return component;
}

function stats(values) {
  const clean = values.filter((value) => Number.isFinite(value)).sort((a, b) => a - b);
  if (clean.length === 0) return null;
  const percentile = (p) => clean[Math.min(clean.length - 1, Math.floor((clean.length - 1) * p))];
  const mean = clean.reduce((sum, value) => sum + value, 0) / clean.length;
  return {
    min: clean[0],
    median: percentile(0.5),
    p90: percentile(0.9),
    p95: percentile(0.95),
    max: clean[clean.length - 1],
    mean: Number(mean.toFixed(2)),
    sample_count: clean.length,
  };
}

const args = parseArgs(process.argv.slice(2));
const device = getDeviceInfo();
const pkg = DEBUG_PACKAGE;
const component = resolveLauncherComponent(pkg);
console.log(`[startup] device=${device.serial} ${device.manufacturer} ${device.model} sdk=${device.sdk} package=${pkg} component=${component}`);

const samples = [];
for (let i = 0; i < args.samples; i += 1) {
  runAdb(['shell', 'am', 'force-stop', pkg], { allowFailure: true });
  const output = runAdb(['shell', 'am', 'start', '-W', '-n', component]);
  const parsed = parseAmStart(output);
  samples.push({ index: i + 1, ...parsed });
  console.log(`[startup] #${i + 1} total=${parsed.total_time_ms} wait=${parsed.wait_time_ms} this=${parsed.this_time_ms}`);
}

const report = {
  generated_at: new Date().toISOString(),
  mode: 'debug',
  package: pkg,
  component,
  device,
  samples,
  total_time_ms: stats(samples.map((sample) => sample.total_time_ms)),
  wait_time_ms: stats(samples.map((sample) => sample.wait_time_ms)),
  this_time_ms: stats(samples.map((sample) => sample.this_time_ms)),
  note: 'Boot-ready app-level timing is not yet instrumented; this report uses adb am start -W.',
};

const output = JSON.stringify(report, null, 2);
if (args.out) {
  const outPath = path.resolve(ROOT, args.out);
  ensureDir(path.dirname(outPath));
  fs.writeFileSync(outPath, output, 'utf8');
} else {
  const outDir = ensureDir(path.join(ROOT, 'tests', 'perf', 'reports', timestamp()));
  fs.writeFileSync(path.join(outDir, 'startup-report.json'), output, 'utf8');
}
console.log(output);
