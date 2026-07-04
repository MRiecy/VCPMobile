const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');
const { ROOT, timestamp, ensureDir } = require('../../e2e-android/scripts/adb-env.cjs');

function usage() {
  console.log(`Usage:
  node tests/perf/scripts/measure_apk_size.cjs --apk <path> [--out <json>]

Outputs a JSON size report. Internal APK section sizes are best-effort and use
available system tools (unzip/jar) when present; apk_bytes is always reported.
`);
}

function parseArgs(argv) {
  const args = { apk: '', out: '' };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--apk') {
      args.apk = argv[++i];
    } else if (arg === '--out') {
      args.out = argv[++i];
    } else if (arg === '--help' || arg === '-h') {
      usage();
      process.exit(0);
    } else {
      throw new Error(`未知参数: ${arg}`);
    }
  }
  if (!args.apk) {
    throw new Error('缺少 --apk <path>');
  }
  return args;
}

function runTool(command, args) {
  const result = spawnSync(command, args, { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
  if (result.status !== 0) {
    return '';
  }
  return result.stdout || '';
}

function listZip(apkPath) {
  const unzip = runTool('unzip', ['-l', apkPath]);
  if (unzip) return unzip;
  const jar = runTool('jar', ['tfv', apkPath]);
  return jar;
}

function addByCategory(report, entryPath, size) {
  if (entryPath.startsWith('lib/arm64-v8a/') || entryPath.startsWith('lib/aarch64-linux-android/')) {
    report.lib_aarch64_bytes += size;
  } else if (entryPath.endsWith('.dex')) {
    report.dex_bytes += size;
  } else if (entryPath.startsWith('assets/')) {
    report.assets_bytes += size;
  } else if (entryPath.startsWith('res/') || entryPath === 'resources.arsc') {
    report.res_bytes += size;
  }
}

function parseListing(listing, report) {
  for (const line of listing.split(/\r?\n/)) {
    const trimmed = line.trim();
    // unzip -l: "  1234  date time  path"
    let match = trimmed.match(/^(\d+)\s+\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}\s+(.+)$/);
    if (!match) {
      // jar tfv: "  1234 date time path"
      match = trimmed.match(/^(\d+)\s+\w{3}\s+\w{3}\s+\d+\s+\d{2}:\d{2}:\d{2}\s+\w+\s+\d{4}\s+(.+)$/);
    }
    if (!match) continue;
    addByCategory(report, match[2].replace(/\\/g, '/'), Number(match[1]));
  }
}

const args = parseArgs(process.argv.slice(2));
const apkPath = path.resolve(ROOT, args.apk);
if (!fs.existsSync(apkPath)) {
  throw new Error(`APK 不存在: ${apkPath}`);
}

const stat = fs.statSync(apkPath);
const report = {
  generated_at: new Date().toISOString(),
  apk_path: apkPath,
  apk_bytes: stat.size,
  apk_mb: Number((stat.size / 1024 / 1024).toFixed(2)),
  lib_aarch64_bytes: 0,
  dex_bytes: 0,
  assets_bytes: 0,
  res_bytes: 0,
};

const listing = listZip(apkPath);
if (listing) {
  parseListing(listing, report);
} else {
  report.note = 'No unzip/jar command available; only apk_bytes was reported.';
}

const output = JSON.stringify(report, null, 2);
console.log(output);
if (args.out) {
  const outPath = path.resolve(ROOT, args.out);
  ensureDir(path.dirname(outPath));
  fs.writeFileSync(outPath, output, 'utf8');
} else {
  const outDir = ensureDir(path.join(ROOT, 'tests', 'perf', 'reports', timestamp()));
  fs.writeFileSync(path.join(outDir, 'apk-size.json'), output, 'utf8');
}
